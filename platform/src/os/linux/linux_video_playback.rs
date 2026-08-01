//! GStreamer-based video player for Linux desktop (X11/Wayland).
//!
//! Uses `playbin3`/`playbin` + `appsink` to decode video. Presentation paths:
//! 1. **GLMemory** (preferred when Makepad EGL context is shared with GStreamer)
//! 2. **DMA-Buf** (VA-API / DRM export → EGLImage → GL texture)
//! 3. **System I420 / RGBA** CPU upload fallback

use {
    super::egl_sys,
    super::gl_sys,
    super::gl_sys::LibGl,
    super::gl_video_upload::upload_i420_planes_to_gl,
    super::gst_gl_share::GstGlShare,
    super::gstreamer_sys::*,
    super::opengl_cx::OpenglCx,
    crate::{
        event::video_playback::VideoSource,
        makepad_error_log::*,
        makepad_live_id::LiveId,
        media_plugin::PlaybackPrepared,
        texture::{CxTexturePool, TextureAlloc, TextureCategory, TextureId, TexturePixel},
    },
    std::{
        ffi::{c_void, CStr, CString},
        os::fd::RawFd,
        path::PathBuf,
        sync::Mutex,
        time::{Duration, Instant},
    },
};

/// Pipelines whose `NULL` transition was requested but has not finished yet.
/// `destroy_pipeline` never blocks on `get_state`; instead it parks the element
/// here and [`poll_pending_pipeline_drops`] finishes the unrefs on a later tick.
struct PendingPipelineDrop {
    gst: *const LibGStreamer,
    pipeline: *mut GstElement,
    bus: *mut GstBus,
    audio_sink: *mut GstElement,
    audio_volume: *mut GstElement,
    started: Instant,
}

// SAFETY: only touched from the UI/platform thread that owns all video players.
unsafe impl Send for PendingPipelineDrop {}

static PENDING_PIPELINE_DROPS: Mutex<Vec<PendingPipelineDrop>> = Mutex::new(Vec::new());

fn poll_pending_pipeline_drops() {
    let Ok(mut pending) = PENDING_PIPELINE_DROPS.lock() else {
        return;
    };
    if pending.is_empty() {
        return;
    }
    let mut i = 0;
    while i < pending.len() {
        let item = &pending[i];
        let gst = unsafe { &*item.gst };
        let mut state: u32 = 0;
        let mut pending_state: u32 = 0;
        let ret = unsafe {
            (gst.gst_element_get_state)(item.pipeline, &mut state, &mut pending_state, 0)
        };
        let timed_out = item.started.elapsed() > Duration::from_secs(10);
        let done = state == GST_STATE_NULL || ret == GST_STATE_CHANGE_FAILURE || timed_out;
        if done {
            if timed_out && state != GST_STATE_NULL {
                crate::log!(
                    "VIDEO: forcing unref of pipeline still leaving NULL (state={} pending={})",
                    state,
                    pending_state
                );
            }
            let item = pending.remove(i);
            unsafe {
                if !item.bus.is_null() {
                    (gst.gst_object_unref)(item.bus as *mut c_void);
                }
                if !item.audio_volume.is_null() {
                    (gst.gst_object_unref)(item.audio_volume as *mut c_void);
                }
                if !item.audio_sink.is_null() {
                    (gst.gst_object_unref)(item.audio_sink as *mut c_void);
                }
                (gst.gst_object_unref)(item.pipeline as *mut c_void);
            }
        } else {
            i += 1;
        }
    }
}

/// Finish async pipeline teardowns. Safe to call every timer tick even with no
/// active players — needed so the last closed video still drains PENDING.
pub fn poll_pending_gstreamer_teardowns() {
    poll_pending_pipeline_drops();
}

pub fn has_pending_gstreamer_teardowns() -> bool {
    PENDING_PIPELINE_DROPS
        .lock()
        .map(|p| !p.is_empty())
        .unwrap_or(false)
}

/// Returns the canPlayType string for the given MIME type on Linux (GStreamer backend).
/// Uses a hardcoded table covering common formats supported by typical GStreamer installs.
pub fn can_play_type(mime: &str) -> &'static str {
    let base = mime.split(';').next().unwrap_or("").trim();
    match base {
        // Containers + codecs GStreamer handles well with base/good/bad plugins
        "video/mp4" | "video/x-m4v" => "probably",
        "video/webm" => "probably",
        "video/ogg" => "probably",
        "video/x-matroska" | "video/x-msvideo" | "video/quicktime" => "maybe",
        // Adaptive streaming manifests (playbin3 + adaptivedemux2 / dashdemux)
        "application/vnd.apple.mpegurl" | "application/x-mpegURL" | "audio/mpegurl"
        | "audio/x-mpegurl" => "probably",
        "application/dash+xml" => "probably",
        "audio/mp4" | "audio/x-m4a" => "probably",
        "audio/mpeg" => "probably",
        "audio/ogg" | "audio/vorbis" => "probably",
        "audio/webm" => "probably",
        "audio/wav" | "audio/x-wav" => "probably",
        "audio/flac" | "audio/x-flac" => "probably",
        "audio/opus" | "audio/ogg; codecs=opus" => "probably",
        _ if base.starts_with("video/") || base.starts_with("audio/") => "maybe",
        _ => "",
    }
}

#[derive(Clone, Copy, Debug)]
pub struct YuvTextureIds {
    pub tex_y_id: TextureId,
    pub tex_u_id: TextureId,
    pub tex_v_id: TextureId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoCapsProfile {
    /// Requires a shared Makepad EGL context ([`GstGlShare`]).
    GlMemoryRgba2D,
    GlMemoryRgba,
    /// DMA-Buf frames (often from VA-API / vapostproc) imported via EGLImage.
    DmaBuf,
    SystemI420,
    SystemRgba,
}

impl VideoCapsProfile {
    fn caps_text(self) -> &'static str {
        match self {
            Self::GlMemoryRgba2D => "video/x-raw(memory:GLMemory),format=RGBA,texture-target=2D",
            Self::GlMemoryRgba => "video/x-raw(memory:GLMemory),format=RGBA",
            Self::DmaBuf => "video/x-raw(memory:DMABuf)",
            Self::SystemI420 => "video/x-raw,format=I420",
            Self::SystemRgba => "video/x-raw,format=RGBA",
        }
    }

    fn is_gl_memory(self) -> bool {
        matches!(self, Self::GlMemoryRgba2D | Self::GlMemoryRgba)
    }

    fn is_dmabuf(self) -> bool {
        matches!(self, Self::DmaBuf)
    }

    fn next_fallback(self) -> Option<Self> {
        match self {
            Self::GlMemoryRgba2D => Some(Self::GlMemoryRgba),
            Self::GlMemoryRgba => Some(Self::DmaBuf),
            Self::DmaBuf => Some(Self::SystemI420),
            Self::SystemI420 => Some(Self::SystemRgba),
            Self::SystemRgba => None,
        }
    }

    fn initial(
        audio_only: bool,
        gst: &LibGStreamer,
        has_gl_share: bool,
        prefer_system_memory: bool,
    ) -> Self {
        if audio_only {
            return Self::SystemRgba;
        }
        // Disable experimental paths with MAKEPAD_GST_NO_GLMEMORY / MAKEPAD_GST_NO_DMABUF.
        let force_gl = std::env::var_os("MAKEPAD_GST_GLMEMORY").is_some();
        let no_gl = std::env::var_os("MAKEPAD_GST_NO_GLMEMORY").is_some();
        let no_dmabuf = std::env::var_os("MAKEPAD_GST_NO_DMABUF").is_some();

        // Network / HLS / DASH manifests: start on system memory. GLMemory and
        // DMABuf almost never negotiate for adaptive demuxers, and each failed
        // attempt re-downloads the playlist + first segments (often 2–4s each).
        if prefer_system_memory && !force_gl {
            crate::log!(
                "VIDEO: adaptive stream — starting with system I420 (skip GL/DMA probes)"
            );
            return Self::SystemI420;
        }

        let can_gl = !no_gl && has_gl_share && gst.has_gl_share_support();
        if can_gl {
            return Self::GlMemoryRgba2D;
        }
        // MAKEPAD_GST_GLMEMORY alone is not enough — sharing is required for a
        // valid texture namespace. Log once via force flag if share is missing.
        if force_gl && !has_gl_share {
            crate::log!(
                "VIDEO: MAKEPAD_GST_GLMEMORY set but Makepad EGL context is not shared; using system memory"
            );
        }
        if !no_dmabuf && gst.has_dmabuf_support() {
            return Self::DmaBuf;
        }
        Self::SystemI420
    }
}

/// Runtime notifications discovered while polling the GStreamer bus after prepare.
#[derive(Clone, Debug)]
pub enum GstRuntimeEvent {
    Error(String),
    Eos,
    TracksChanged {
        video_tracks: Vec<String>,
        audio_tracks: Vec<String>,
    },
}

pub struct GStreamerVideoPlayer {
    gst: *const LibGStreamer,
    pipeline: *mut GstElement,
    video_sink: *mut GstElement,
    /// Custom `pulsesink` (extra ref) passed to playbin as `audio-sink`.
    audio_sink: *mut GstElement,
    /// Client-side `volume` element inside the audio sink bin (may be null).
    audio_volume: *mut GstElement,
    bus: *mut GstBus,
    pub(crate) video_id: LiveId,
    texture_id: TextureId,
    yuv_ids: Option<YuvTextureIds>,
    is_prepared: bool,
    prepare_notified: bool,
    eos_notified: bool,
    autoplay: bool,
    is_looping: bool,
    /// Audio-only mode: no appsink, no texture updates.
    audio_only: bool,
    video_width: u32,
    video_height: u32,
    duration_ns: i64,
    source_uri: String,
    temp_file_path: Option<PathBuf>,
    /// Scratch buffer used only when direct upload is not possible.
    /// Reused to avoid per-frame heap allocation in fallback row-pack path.
    pixel_buf: Vec<u8>,
    /// Dimensions of the currently allocated GL texture (0x0 = not yet allocated).
    /// Used to choose between glTexImage2D (realloc) and glTexSubImage2D (update).
    tex_width: usize,
    tex_height: usize,
    /// Log first successful upload once per player.
    logged_first_upload: bool,
    /// Current caps profile used to build the GStreamer pipeline.
    caps_profile: VideoCapsProfile,
    /// True when the pipeline was created as `playbin3` (streams-aware).
    playbin3: bool,
    /// Current YUV matrix selector for shader path (0.0 = BT.709).
    yuv_matrix: f32,
    /// Last retained GLMemory sample. Retaining it keeps the texture alive.
    retained_gl_sample: *mut GstSample,
    /// Last buffered ranges we reported (for change detection).
    last_buffered: Vec<(f64, f64)>,
    /// Soft mute applied while paused so residual sink audio is silenced.
    pause_muted: bool,
    /// User-requested mute (independent of pause_muted).
    user_muted: bool,
    /// Last user-facing playbin volume (restored after pause soft-silence).
    playback_volume: f64,
    /// Last playhead (ns) captured when pausing; used to recover a wedged PAUSED pipeline.
    resume_position_ns: i64,
    /// UI requested pause — do not pull decoded frames from appsink.
    user_paused: bool,
    /// Pending async state change we are timing (0 = none) and when it started.
    pending_state_target: u32,
    pending_state_since: Option<Instant>,
    /// When prepare started waiting for dimensions.
    prepare_started: Instant,
    /// Shared Makepad↔GStreamer GL context (owns GstGLDisplay / wrapped context).
    gl_share: Option<GstGlShare>,
    /// playbin3 stream collection (video + audio stream ids / labels).
    stream_video_ids: Vec<String>,
    stream_audio_ids: Vec<String>,
    stream_video_labels: Vec<String>,
    stream_audio_labels: Vec<String>,
    selected_video_idx: usize,
    selected_audio_idx: usize,
    /// True when the last presented frame used DMA-Buf → YUV plane textures.
    dmabuf_yuv_mode: bool,
    /// True when YUV is NV12-style biplanar (UV in one RG8 texture).
    yuv_biplanar: bool,
    /// True when YUV is full/PC range (JPEG colorimetry).
    yuv_full_range: bool,
    /// Last track labels we reported (for change detection after STREAM_COLLECTION).
    last_track_labels: Option<(Vec<String>, Vec<String>)>,
    /// EGLImages that must stay alive while the GL texture samples them.
    retained_egl_images: Vec<*mut c_void>,
    egl_display_for_images: *mut c_void,
    egl_destroy_image:
        Option<unsafe extern "C" fn(egl_sys::EGLDisplay, egl_sys::EGLImageKHR) -> egl_sys::EGLBoolean>,
}

struct BuiltPipeline {
    pipeline: *mut GstElement,
    video_sink: *mut GstElement,
    audio_sink: *mut GstElement,
    audio_volume: *mut GstElement,
    bus: *mut GstBus,
    playbin3: bool,
}

struct I420Layout {
    y_stride: u32,
    u_stride: u32,
    v_stride: u32,
    y_off: usize,
    u_off: usize,
    v_off: usize,
}

fn drm_fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}

/// Minimal view of GStreamer `GstVideoMeta` for stride/offset reads.
/// Layout matches 64-bit: GstMeta(16) + buffer + flags/format/id + width/height/
/// n_planes + pad to align `gsize` + offset[4] + stride[4].
#[repr(C)]
struct GstVideoMetaView {
    _meta_flags: u32,
    _pad0: u32,
    _info: *mut c_void,
    _buffer: *mut c_void,
    _frame_flags: i32,
    _format: i32,
    _id: i32,
    width: u32,
    height: u32,
    n_planes: u32,
    _pad_align_gsize: u32,
    offset: [usize; 4],
    stride: [i32; 4],
}

#[derive(Clone, Copy, Debug)]
struct DmaPlaneLayout {
    fd_index: usize,
    offset: u32,
    pitch: u32,
    width: u32,
    height: u32,
    fourcc: u32,
}

#[derive(Clone, Copy, Debug)]
struct ColorMeta {
    matrix: f32,
    full_range: bool,
}

fn infer_i420_layout(width: usize, height: usize, size: usize) -> Option<I420Layout> {
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);
    let tight = width * height + cw * ch * 2;
    if size < tight {
        return None;
    }
    if size == tight {
        return Some(I420Layout {
            y_stride: width as u32,
            u_stride: cw as u32,
            v_stride: cw as u32,
            y_off: 0,
            u_off: width * height,
            v_off: width * height + cw * ch,
        });
    }
    for align in [1usize, 2, 4, 8, 16, 32, 64] {
        let ys = width.div_ceil(align) * align;
        let us = cw.div_ceil(align) * align;
        let vs = us;
        let need = ys * height + us * ch + vs * ch;
        if need == size {
            return Some(I420Layout {
                y_stride: ys as u32,
                u_stride: us as u32,
                v_stride: vs as u32,
                y_off: 0,
                u_off: ys * height,
                v_off: ys * height + us * ch,
            });
        }
    }
    Some(I420Layout {
        y_stride: width as u32,
        u_stride: cw as u32,
        v_stride: cw as u32,
        y_off: 0,
        u_off: width * height,
        v_off: width * height + cw * ch,
    })
}

fn i420_layout_from_video_meta(
    meta: &GstVideoMetaView,
    width: usize,
    height: usize,
    size: usize,
) -> Option<I420Layout> {
    if meta.n_planes < 3 || meta.stride[0] <= 0 || meta.stride[1] <= 0 || meta.stride[2] <= 0 {
        return None;
    }
    let layout = I420Layout {
        y_stride: meta.stride[0] as u32,
        u_stride: meta.stride[1] as u32,
        v_stride: meta.stride[2] as u32,
        y_off: meta.offset[0],
        u_off: meta.offset[1],
        v_off: meta.offset[2],
    };
    let ch = height.div_ceil(2);
    let y_end = layout.y_off.checked_add(layout.y_stride as usize * height)?;
    let u_end = layout.u_off.checked_add(layout.u_stride as usize * ch)?;
    let v_end = layout.v_off.checked_add(layout.v_stride as usize * ch)?;
    if y_end > size || u_end > size || v_end > size {
        return None;
    }
    if meta.width as usize != width || meta.height as usize != height {
        // Meta dims can disagree briefly during renegotiation; still usable if
        // the plane ranges fit the mapped buffer.
    }
    Some(layout)
}

impl GStreamerVideoPlayer {
    pub fn new(
        gst: &LibGStreamer,
        video_id: LiveId,
        texture_id: TextureId,
        yuv_ids: Option<YuvTextureIds>,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
        opengl_cx: Option<&OpenglCx>,
    ) -> Self {
        Self::new_impl(
            gst,
            video_id,
            texture_id,
            yuv_ids,
            source,
            autoplay,
            is_looping,
            false,
            opengl_cx,
        )
    }

    pub fn new_audio_only(
        gst: &LibGStreamer,
        video_id: LiveId,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Self {
        // Use a placeholder texture_id — audio-only never uploads frames
        Self::new_impl(
            gst,
            video_id,
            TextureId::default(),
            None,
            source,
            autoplay,
            is_looping,
            true,
            None,
        )
    }

    fn new_impl(
        gst: &LibGStreamer,
        video_id: LiveId,
        texture_id: TextureId,
        yuv_ids: Option<YuvTextureIds>,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
        audio_only: bool,
        opengl_cx: Option<&OpenglCx>,
    ) -> Self {
        let gst_ptr = gst as *const LibGStreamer;

        // Resolve the URI from the source
        let (uri, temp_file_path) = Self::uri_from_source(video_id, &source);

        let mut gl_share = None;
        if !audio_only {
            if let Some(cx) = opengl_cx {
                gl_share = GstGlShare::try_new(gst, cx.egl_display, cx.egl_context);
                if gl_share.is_some() {
                    crate::log!("VIDEO: shared Makepad EGL context with GStreamer");
                }
            }
        }
        let has_gl_share = gl_share.is_some();
        // Adaptive manifests (remote or local .m3u8/.mpd) should skip GL/DMA
        // probes — failed zero-copy caps force a full pipeline rebuild and re-fetch.
        // Progressive network MP4/WebM can still try GL/DMA first.
        let prefer_system_memory = source.is_adaptive_manifest();
        let caps_profile =
            VideoCapsProfile::initial(audio_only, gst, has_gl_share, prefer_system_memory);
        crate::log!(
            "VIDEO: starting caps profile {:?} uri={}",
            caps_profile,
            uri
        );

        let Some(built) =
            Self::build_pipeline(gst, video_id, &uri, audio_only, caps_profile, gl_share.as_ref())
        else {
            if let Some(mut share) = gl_share.take() {
                share.release(gst);
            }
            return Self::null_player(
                gst_ptr,
                video_id,
                texture_id,
                yuv_ids,
                autoplay,
                is_looping,
                audio_only,
                temp_file_path,
            );
        };

        let (egl_display_for_images, egl_destroy_image) = opengl_cx
            .map(|cx| (cx.egl_display, cx.libegl.eglDestroyImageKHR))
            .unwrap_or((std::ptr::null_mut(), None));

        Self {
            gst: gst_ptr,
            pipeline: built.pipeline,
            video_sink: built.video_sink,
            audio_sink: built.audio_sink,
            audio_volume: built.audio_volume,
            bus: built.bus,
            video_id,
            texture_id,
            yuv_ids,
            is_prepared: false,
            prepare_notified: false,
            eos_notified: false,
            autoplay,
            is_looping,
            audio_only,
            video_width: 0,
            video_height: 0,
            duration_ns: 0,
            source_uri: uri,
            temp_file_path,
            pixel_buf: Vec::new(),
            tex_width: 0,
            tex_height: 0,
            logged_first_upload: false,
            caps_profile,
            playbin3: built.playbin3,
            yuv_matrix: 0.0,
            retained_gl_sample: std::ptr::null_mut(),
            last_buffered: Vec::new(),
            pause_muted: false,
            user_muted: false,
            playback_volume: 1.0,
            user_paused: false,
            pending_state_target: 0,
            pending_state_since: None,
            resume_position_ns: -1,
            prepare_started: Instant::now(),
            gl_share,
            stream_video_ids: Vec::new(),
            stream_audio_ids: Vec::new(),
            stream_video_labels: Vec::new(),
            stream_audio_labels: Vec::new(),
            selected_video_idx: 0,
            selected_audio_idx: 0,
            dmabuf_yuv_mode: false,
            yuv_biplanar: false,
            yuv_full_range: false,
            last_track_labels: None,
            retained_egl_images: Vec::new(),
            egl_display_for_images,
            egl_destroy_image,
        }
    }

    /// Prefer `playbin3`: `hlsdemux2` / adaptive demuxers require a streams-aware
    /// context that classic `playbin` + `decodebin` do not provide.
    fn make_playbin(gst: &LibGStreamer) -> (*mut GstElement, bool) {
        unsafe {
            for (name, is_playbin3) in [("playbin3", true), ("playbin", false)] {
                let playbin_name = CString::new(name).unwrap();
                let pipeline =
                    (gst.gst_element_factory_make)(playbin_name.as_ptr(), std::ptr::null());
                if !pipeline.is_null() {
                    return (pipeline, is_playbin3);
                }
            }
            (std::ptr::null_mut(), false)
        }
    }

    /// Build a low-latency audio sink bin: `volume ! pulsesink`.
    ///
    /// The `volume` element is the important part. `pulsesink` implements
    /// `GstStreamVolume`, and playbin/playsink only inserts its own client-side
    /// `volume` element when the sink does *not*. So with a bare `pulsesink`,
    /// both `playbin.volume` and `playbin.mute` are forwarded to the
    /// PulseAudio/pipewire-pulse **server**: the change is asynchronous and does
    /// not touch audio already committed to the server. That is what makes a
    /// soft-mute-on-pause leave an audible tail, and makes the first moments
    /// after resume silent. Wrapping the sink in a plain `GstBin` hides
    /// `GstStreamVolume` from playsink, so muting happens on the samples
    /// themselves before they ever reach the device.
    ///
    /// Do **not** use `pipewiresink`: it is not a `GstAudioBaseSink` and has no
    /// `buffer-time`/`latency-time` (setting them only logs GLib-CRITICAL and
    /// leaves the default PipeWire quantum).
    ///
    /// Returns `(sink_for_playbin, volume_element_or_null)`.
    fn make_low_latency_audio_sink(gst: &LibGStreamer) -> (*mut GstElement, *mut GstElement) {
        unsafe {
            // GstAudioBaseSink times are microseconds; the 200 ms default
            // buffer-time is what bounds the residual tail after muting.
            let make_tuned_pulsesink = || {
                let sink_type = CString::new("pulsesink").unwrap();
                let sink = (gst.gst_element_factory_make)(sink_type.as_ptr(), std::ptr::null());
                if !sink.is_null() {
                    let buffer_time = CString::new("buffer-time").unwrap();
                    let latency_time = CString::new("latency-time").unwrap();
                    (gst.g_object_set_int64)(sink, buffer_time.as_ptr(), 20_000, std::ptr::null());
                    (gst.g_object_set_int64)(sink, latency_time.as_ptr(), 10_000, std::ptr::null());
                    let sync = CString::new("sync").unwrap();
                    (gst.g_object_set_int)(sink, sync.as_ptr(), 1, std::ptr::null());
                }
                sink
            };

            let sink = make_tuned_pulsesink();
            if sink.is_null() {
                let auto_type = CString::new("autoaudiosink").unwrap();
                let sink = (gst.gst_element_factory_make)(auto_type.as_ptr(), std::ptr::null());
                if !sink.is_null() {
                    crate::log!(
                        "VIDEO: pulsesink unavailable; using autoaudiosink (pause tail may be longer)"
                    );
                }
                return (sink, std::ptr::null_mut());
            }

            let (
                Some(bin_new),
                Some(bin_add),
                Some(link),
                Some(static_pad),
                Some(ghost_new),
                Some(add_pad),
            ) = (
                gst.gst_bin_new,
                gst.gst_bin_add,
                gst.gst_element_link,
                gst.gst_element_get_static_pad,
                gst.gst_ghost_pad_new,
                gst.gst_element_add_pad,
            )
            else {
                crate::log!("VIDEO: audio sink = pulsesink (no bin support; server-side volume)");
                return (sink, std::ptr::null_mut());
            };

            let volume_type = CString::new("volume").unwrap();
            let volume = (gst.gst_element_factory_make)(volume_type.as_ptr(), std::ptr::null());
            let bin_name = CString::new("makepad-audio-sink").unwrap();
            let bin = if volume.is_null() {
                std::ptr::null_mut()
            } else {
                bin_new(bin_name.as_ptr())
            };
            if bin.is_null() {
                if !volume.is_null() {
                    (gst.gst_object_unref)(volume as *mut c_void);
                }
                crate::log!("VIDEO: audio sink = pulsesink (no volume bin; server-side volume)");
                return (sink, std::ptr::null_mut());
            }

            // gst_bin_add consumes the floating ref, so take our own afterwards.
            bin_add(bin, volume);
            bin_add(bin, sink);
            (gst.gst_object_ref)(volume as *mut c_void);

            let sink_pad_name = CString::new("sink").unwrap();
            let target = if link(volume, sink) != 0 {
                static_pad(volume, sink_pad_name.as_ptr())
            } else {
                std::ptr::null_mut()
            };
            let ghost = if target.is_null() {
                std::ptr::null_mut()
            } else {
                let ghost = ghost_new(sink_pad_name.as_ptr(), target);
                (gst.gst_object_unref)(target as *mut c_void);
                ghost
            };
            if ghost.is_null() {
                // Unref-ing the bin also drops `volume` and `sink`.
                (gst.gst_object_unref)(volume as *mut c_void);
                (gst.gst_object_unref)(bin as *mut c_void);
                crate::log!("VIDEO: volume bin wiring failed; using bare pulsesink");
                return (make_tuned_pulsesink(), std::ptr::null_mut());
            }
            add_pad(bin, ghost);

            crate::log!(
                "VIDEO: audio sink = bin(volume ! pulsesink) buffer-time=20ms latency-time=10ms"
            );
            (bin, volume)
        }
    }

    fn build_pipeline(
        gst: &LibGStreamer,
        video_id: LiveId,
        uri: &str,
        audio_only: bool,
        caps_profile: VideoCapsProfile,
        gl_share: Option<&GstGlShare>,
    ) -> Option<BuiltPipeline> {
        unsafe {
            let (pipeline, playbin3) = Self::make_playbin(gst);
            if pipeline.is_null() {
                error!(
                    "Failed to create GStreamer playbin/playbin3 for video {:?}",
                    video_id
                );
                return None;
            }

            if let Some(share) = gl_share {
                share.apply_to_element(gst, pipeline);
            }

            let uri_prop = CString::new("uri").unwrap();
            let uri_cstr = CString::new(uri).unwrap();
            (gst.g_object_set_string)(
                pipeline,
                uri_prop.as_ptr(),
                uri_cstr.as_ptr(),
                std::ptr::null(),
            );

            // Network streams: shrink playbin's download buffer so preroll does
            // not wait on multi-second / multi-megabyte fill before ASYNC_DONE.
            // Local files keep GStreamer defaults.
            if uri.starts_with("http://") || uri.starts_with("https://") {
                // Values are nanoseconds / bytes. Defaults are effectively large
                // (~2s / ~10MB); for HLS that delays VideoPlaybackPrepared a lot.
                let buffer_duration = CString::new("buffer-duration").unwrap();
                let buffer_size = CString::new("buffer-size").unwrap();
                (gst.g_object_set_int64)(
                    pipeline,
                    buffer_duration.as_ptr(),
                    500_000_000, // 500 ms
                    std::ptr::null(),
                );
                (gst.g_object_set_int64)(
                    pipeline,
                    buffer_size.as_ptr(),
                    1_048_576, // 1 MiB
                    std::ptr::null(),
                );
            }

            // Keep audio latency short so pause/seek feel responsive.
            let mut audio_sink_element: *mut GstElement = std::ptr::null_mut();
            let (audio_sink, audio_volume) = Self::make_low_latency_audio_sink(gst);
            if !audio_sink.is_null() {
                (gst.gst_object_ref)(audio_sink as *mut c_void);
                audio_sink_element = audio_sink;
                let audio_sink_prop = CString::new("audio-sink").unwrap();
                (gst.g_object_set_ptr)(
                    pipeline,
                    audio_sink_prop.as_ptr(),
                    audio_sink as *mut c_void,
                    std::ptr::null(),
                );
            }

            let video_sink = if audio_only {
                let fakesink_type = CString::new("fakesink").unwrap();
                let fakesink =
                    (gst.gst_element_factory_make)(fakesink_type.as_ptr(), std::ptr::null());
                if !fakesink.is_null() {
                    let video_sink_prop = CString::new("video-sink").unwrap();
                    (gst.g_object_set_ptr)(
                        pipeline,
                        video_sink_prop.as_ptr(),
                        fakesink as *mut c_void,
                        std::ptr::null(),
                    );
                }
                std::ptr::null_mut()
            } else {
                let appsink_type = CString::new("appsink").unwrap();
                let appsink_name = CString::new("videosink").unwrap();
                let video_sink =
                    (gst.gst_element_factory_make)(appsink_type.as_ptr(), appsink_name.as_ptr());
                if video_sink.is_null() {
                    error!(
                        "Failed to create GStreamer appsink for video {:?}",
                        video_id
                    );
                    (gst.gst_object_unref)(pipeline as *mut c_void);
                    return None;
                }

                let caps_text = caps_profile.caps_text();
                let caps_str = CString::new(caps_text).unwrap();
                let caps = (gst.gst_caps_from_string)(caps_str.as_ptr());
                if !caps.is_null() {
                    (gst.gst_app_sink_set_caps)(video_sink, caps);
                    (gst.gst_caps_unref)(caps);
                }

                // Present on Makepad's timer (~8ms), not by blocking appsink on the
                // pipeline clock. Measured on HLS: sync=true reaches PAUSED instantly
                // on pause; sync=false leaves PLAYING→PAUSED ASYNC for seconds.
                // Audio (pulsesink sync=true) remains the clock provider for A/V sync;
                // appsink still timestamps frames, we just do not wait on them here.
                let max_buffers_prop = CString::new("max-buffers").unwrap();
                (gst.g_object_set_int)(video_sink, max_buffers_prop.as_ptr(), 2, std::ptr::null());
                let drop_prop = CString::new("drop").unwrap();
                (gst.g_object_set_int)(video_sink, drop_prop.as_ptr(), 1, std::ptr::null());
                let sync_prop = CString::new("sync").unwrap();
                (gst.g_object_set_int)(video_sink, sync_prop.as_ptr(), 1, std::ptr::null());
                let qos_prop = CString::new("qos").unwrap();
                (gst.g_object_set_int)(video_sink, qos_prop.as_ptr(), 1, std::ptr::null());

                let video_sink_prop = CString::new("video-sink").unwrap();
                (gst.g_object_set_ptr)(
                    pipeline,
                    video_sink_prop.as_ptr(),
                    video_sink as *mut c_void,
                    std::ptr::null(),
                );
                video_sink
            };

            let bus = (gst.gst_element_get_bus)(pipeline);
            (gst.gst_element_set_state)(pipeline, GST_STATE_PAUSED);
            Some(BuiltPipeline {
                pipeline,
                video_sink,
                audio_sink: audio_sink_element,
                audio_volume,
                bus,
                playbin3,
            })
        }
    }

    fn destroy_pipeline(&mut self) {
        // Drop present-side keep-alives before tearing down the pipeline so we
        // never sample a GL/DMA texture whose GstBuffer is already gone.
        if !self.retained_gl_sample.is_null() {
            unsafe {
                let gst = &*self.gst;
                (gst.gst_mini_object_unref)(self.retained_gl_sample as *mut GstMiniObject);
            }
            self.retained_gl_sample = std::ptr::null_mut();
        }
        self.release_egl_images();

        if self.pipeline.is_null() {
            poll_pending_pipeline_drops();
            return;
        }
        unsafe {
            let gst = &*self.gst;
            // Request NULL without blocking. Streaming threads / HTTP cancel can
            // take hundreds of ms; parking the element in PENDING keeps the UI
            // responsive (caps fallback and cleanup both hit this path).
            (gst.gst_element_set_state)(self.pipeline, GST_STATE_NULL);
            if let Ok(mut pending) = PENDING_PIPELINE_DROPS.lock() {
                pending.push(PendingPipelineDrop {
                    gst: self.gst,
                    pipeline: self.pipeline,
                    bus: self.bus,
                    audio_sink: self.audio_sink,
                    audio_volume: self.audio_volume,
                    started: Instant::now(),
                });
            } else {
                // Mutex poisoned — fall back to best-effort sync unref.
                if !self.bus.is_null() {
                    (gst.gst_object_unref)(self.bus as *mut c_void);
                }
                if !self.audio_volume.is_null() {
                    (gst.gst_object_unref)(self.audio_volume as *mut c_void);
                }
                if !self.audio_sink.is_null() {
                    (gst.gst_object_unref)(self.audio_sink as *mut c_void);
                }
                (gst.gst_object_unref)(self.pipeline as *mut c_void);
            }
            self.pipeline = std::ptr::null_mut();
            self.bus = std::ptr::null_mut();
            self.video_sink = std::ptr::null_mut();
            self.audio_sink = std::ptr::null_mut();
            self.audio_volume = std::ptr::null_mut();
        }
        poll_pending_pipeline_drops();
    }

    fn release_egl_images(&mut self) {
        if let Some(destroy) = self.egl_destroy_image {
            if !self.egl_display_for_images.is_null() {
                for image in self.retained_egl_images.drain(..) {
                    if !image.is_null() {
                        unsafe {
                            destroy(self.egl_display_for_images, image);
                        }
                    }
                }
                return;
            }
        }
        self.retained_egl_images.clear();
    }

    fn null_player(
        gst: *const LibGStreamer,
        video_id: LiveId,
        texture_id: TextureId,
        yuv_ids: Option<YuvTextureIds>,
        autoplay: bool,
        is_looping: bool,
        audio_only: bool,
        temp_file_path: Option<PathBuf>,
    ) -> Self {
        Self {
            gst,
            pipeline: std::ptr::null_mut(),
            video_sink: std::ptr::null_mut(),
            audio_sink: std::ptr::null_mut(),
            audio_volume: std::ptr::null_mut(),
            bus: std::ptr::null_mut(),
            video_id,
            texture_id,
            yuv_ids,
            is_prepared: false,
            prepare_notified: false,
            eos_notified: false,
            autoplay,
            is_looping,
            audio_only,
            video_width: 0,
            video_height: 0,
            duration_ns: 0,
            source_uri: String::new(),
            temp_file_path,
            pixel_buf: Vec::new(),
            tex_width: 0,
            tex_height: 0,
            logged_first_upload: false,
            caps_profile: VideoCapsProfile::SystemI420,
            playbin3: false,
            yuv_matrix: 0.0,
            retained_gl_sample: std::ptr::null_mut(),
            last_buffered: Vec::new(),
            pause_muted: false,
            user_muted: false,
            playback_volume: 1.0,
            user_paused: false,
            pending_state_target: 0,
            pending_state_since: None,
            resume_position_ns: -1,
            prepare_started: Instant::now(),
            gl_share: None,
            stream_video_ids: Vec::new(),
            stream_audio_ids: Vec::new(),
            stream_video_labels: Vec::new(),
            stream_audio_labels: Vec::new(),
            selected_video_idx: 0,
            selected_audio_idx: 0,
            dmabuf_yuv_mode: false,
            yuv_biplanar: false,
            yuv_full_range: false,
            last_track_labels: None,
            retained_egl_images: Vec::new(),
            egl_display_for_images: std::ptr::null_mut(),
            egl_destroy_image: None,
        }
    }

    fn uri_from_source(video_id: LiveId, source: &VideoSource) -> (String, Option<PathBuf>) {
        match source {
            VideoSource::Network(url) => (url.clone(), None),
            VideoSource::Filesystem(path) => {
                if path.starts_with("file://") {
                    (path.clone(), None)
                } else {
                    (Self::path_to_file_uri(path), None)
                }
            }
            VideoSource::InMemory(data) => {
                let ext = Self::sniff_container_extension(data.as_ref());
                let tmp_path = std::env::temp_dir()
                    .join(format!("makepad_video_{}.{}", video_id.0, ext));
                if let Err(e) = std::fs::write(&tmp_path, data.as_ref()) {
                    error!("Failed to write video to temp file: {}", e);
                }
                let uri = Self::path_to_file_uri(&tmp_path.to_string_lossy());
                (uri, Some(tmp_path))
            }
            VideoSource::Camera(..) => {
                // Camera sources are handled by V4l2CameraPlayer, not GStreamer.
                ("".to_string(), None)
            }
            VideoSource::PlaybackSession(..) | VideoSource::Session(..) => {
                crate::error!("VIDEO: session sources are handled by the software video player");
                ("".to_string(), None)
            }
        }
    }

    fn path_to_file_uri(path: &str) -> String {
        // Percent-encode so spaces / non-ASCII paths work with GStreamer URI handlers.
        let mut out = String::from("file://");
        for &b in path.as_bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char);
                }
                _ => {
                    use std::fmt::Write;
                    let _ = write!(&mut out, "%{:02X}", b);
                }
            }
        }
        out
    }

    fn sniff_container_extension(data: &[u8]) -> &'static str {
        if data.len() >= 12 && &data[4..8] == b"ftyp" {
            return "mp4";
        }
        if data.len() >= 4 && data.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
            return "webm";
        }
        if data.len() >= 4 && data.starts_with(b"OggS") {
            return "ogg";
        }
        if data.len() >= 4 && data.starts_with(b"RIFF") {
            return "avi";
        }
        if data.len() >= 4 && (data.starts_with(b"\x00\x00\x00\x14") || data.starts_with(b"\x00\x00\x00\x18"))
            && data.len() >= 8
            && &data[4..8] == b"ftyp"
        {
            return "mp4";
        }
        // Generic name; GStreamer typefind still inspects contents.
        "bin"
    }

    /// Extract video dimensions / colorimetry from a GstSample's caps.
    fn extract_dims_from_sample(&mut self, gst: &LibGStreamer, sample: *mut GstSample) {
        unsafe {
            let caps = (gst.gst_sample_get_caps)(sample);
            if caps.is_null() {
                return;
            }
            let structure = (gst.gst_caps_get_structure)(caps, 0);
            if structure.is_null() {
                return;
            }
            let width_key = CString::new("width").unwrap();
            let height_key = CString::new("height").unwrap();
            let mut w: i32 = 0;
            let mut h: i32 = 0;
            (gst.gst_structure_get_int)(structure, width_key.as_ptr(), &mut w);
            (gst.gst_structure_get_int)(structure, height_key.as_ptr(), &mut h);
            if w > 0 && h > 0 {
                self.video_width = w as u32;
                self.video_height = h as u32;
            }
            let color = Self::color_from_caps_structure(gst, structure);
            self.yuv_matrix = color.matrix;
            self.yuv_full_range = color.full_range;
        }
    }

    fn color_from_caps_structure(gst: &LibGStreamer, structure: *mut GstStructure) -> ColorMeta {
        unsafe {
            let key = CString::new("colorimetry").unwrap();
            let ptr = (gst.gst_structure_get_string)(structure, key.as_ptr());
            if ptr.is_null() {
                return ColorMeta {
                    matrix: 0.0,
                    full_range: false,
                };
            }
            let s = CStr::from_ptr(ptr).to_string_lossy().to_ascii_lowercase();
            // GStreamer colorimetry tokens: bt709 / bt601 / bt2020 / jpeg / smpte240m / ...
            // "jpeg" and "1:x:x:x" with range=1 indicate full/PC range.
            let full_range = s.contains("jpeg")
                || s.contains(":1:")
                || s.starts_with("1:")
                || s.contains("full");
            let matrix = if s.contains("bt601") || s.contains("smpte240m") || s.contains("jpeg")
            {
                1.0
            } else if s.contains("bt2020") || s.contains("bt2100") {
                2.0
            } else {
                0.0 // bt709 / default
            };
            ColorMeta { matrix, full_range }
        }
    }

    /// Check if the player has finished prerolling and is ready to play.
    /// Returns `Ok(...)` with metadata when ready, `Err(msg)` on failure, `None` if still loading.
    pub fn check_prepared(&mut self) -> Option<Result<PlaybackPrepared, String>> {
        poll_pending_pipeline_drops();
        if self.prepare_notified || self.pipeline.is_null() {
            return None;
        }

        let gst = unsafe { &*self.gst };

        unsafe {
            // Check for errors on the bus
            let msg = (gst.gst_bus_pop_filtered)(self.bus, GST_MESSAGE_ERROR);
            if !msg.is_null() {
                let mut error: *mut GError = std::ptr::null_mut();
                let mut debug: *mut std::os::raw::c_char = std::ptr::null_mut();
                (gst.gst_message_parse_error)(msg, &mut error, &mut debug);
                let err_str = if !error.is_null() {
                    let msg_ptr = (*error).message;
                    let s = if !msg_ptr.is_null() {
                        CStr::from_ptr(msg_ptr).to_string_lossy().to_string()
                    } else {
                        "Unknown GStreamer error".to_string()
                    };
                    (gst.g_error_free)(error);
                    s
                } else {
                    "Unknown GStreamer error".to_string()
                };
                let debug_str = if !debug.is_null() {
                    CStr::from_ptr(debug).to_string_lossy().to_string()
                } else {
                    String::new()
                };
                if !debug.is_null() {
                    (gst.g_free)(debug as *mut c_void);
                }
                (gst.gst_mini_object_unref)(msg as *mut GstMiniObject);

                // Only retry caps profiles for negotiation / format mismatches.
                // Missing plugins and pure network failures should fail immediately.
                // HLS/GLMemory failures often surface as "Internal data stream error"
                // with "not-negotiated" only in the debug string — treat those as
                // caps fallbacks too, otherwise we burn 2–4s per doomed profile
                // and then hard-fail without trying system memory.
                let negotiation_error = err_str.contains("not-negotiated")
                    || err_str.contains("negotiation")
                    || debug_str.contains("not-negotiated")
                    || debug_str.contains("not negotiated")
                    || (debug_str.contains("caps")
                        && (debug_str.contains("not") || debug_str.contains("fail")))
                    || (err_str.contains("Internal data stream error")
                        && (self.caps_profile.is_gl_memory() || self.caps_profile.is_dmabuf()));
                if negotiation_error {
                    if let Some(mut next_profile) = self.caps_profile.next_fallback() {
                        crate::log!(
                            "VIDEO: caps {:?} failed ({}); falling back to {:?}",
                            self.caps_profile,
                            err_str,
                            next_profile
                        );
                        self.destroy_pipeline();
                        // Avoid sharing Makepad's EGL context with a pipeline that
                        // is still asynchronously leaving NULL.
                        if has_pending_gstreamer_teardowns() {
                            while next_profile.is_gl_memory() || next_profile.is_dmabuf() {
                                match next_profile.next_fallback() {
                                    Some(n) => {
                                        crate::log!(
                                            "VIDEO: pending teardown — skipping {:?} until NULL completes",
                                            next_profile
                                        );
                                        next_profile = n;
                                    }
                                    None => break,
                                }
                            }
                        }
                        let gl_for_rebuild = if next_profile.is_gl_memory() {
                            self.gl_share.as_ref()
                        } else {
                            None
                        };
                        if let Some(built) = Self::build_pipeline(
                            gst,
                            self.video_id,
                            &self.source_uri,
                            self.audio_only,
                            next_profile,
                            gl_for_rebuild,
                        ) {
                            self.pipeline = built.pipeline;
                            self.video_sink = built.video_sink;
                            self.audio_sink = built.audio_sink;
                            self.audio_volume = built.audio_volume;
                            self.bus = built.bus;
                            self.caps_profile = next_profile;
                            self.playbin3 = built.playbin3;
                            self.prepare_started = Instant::now();
                            self.dmabuf_yuv_mode = false;
                            self.yuv_biplanar = false;
                            return None;
                        }
                    }
                }

                if debug_str.is_empty() {
                    error!("GStreamer error id={} msg={}", self.video_id.0, err_str);
                } else {
                    error!(
                        "GStreamer error id={} msg={} debug={}",
                        self.video_id.0, err_str, debug_str
                    );
                }

                self.prepare_notified = true;
                let hint = if err_str.contains("missing a plug-in")
                    || debug_str.contains("Missing element")
                    || debug_str.contains("No suitable plugins")
                {
                    " Install GStreamer plugins: gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav (see tools/linux_deps.sh)."
                } else if debug_str.contains("streams-aware") {
                    " Adaptive streams need playbin3; update GStreamer >= 1.22 or reinstall gst-plugins-base."
                } else {
                    ""
                };
                return Some(Err(format!("{err_str}{hint}")));
            }

            // Non-blocking state check
            let mut state: u32 = 0;
            let mut pending: u32 = 0;
            let ret = (gst.gst_element_get_state)(self.pipeline, &mut state, &mut pending, 0);

            if ret == GST_STATE_CHANGE_FAILURE {
                self.prepare_notified = true;
                return Some(Err(
                    "GStreamer pipeline failed to reach PAUSED (state-change failure)".into(),
                ));
            }

            // Also answer GL NEED_CONTEXT during prepare so GLMemory can negotiate.
            if let Some(share) = self.gl_share.as_ref() {
                loop {
                    let msg = (gst.gst_bus_pop_filtered)(self.bus, GST_MESSAGE_NEED_CONTEXT);
                    if msg.is_null() {
                        break;
                    }
                    if share.is_gl_need_context_message(gst, msg) {
                        share.apply_to_element(gst, self.pipeline);
                    }
                    (gst.gst_mini_object_unref)(msg as *mut GstMiniObject);
                }
            }

            // Need at least PAUSED for preroll to be done
            if state < GST_STATE_PAUSED || self.is_prepared {
                return None;
            }

            // Pull the preroll sample to get video dimensions.
            // try_pull_preroll works in PAUSED state (try_pull_sample does NOT).
            if !self.video_sink.is_null() {
                let sample = (gst.gst_app_sink_try_pull_preroll)(self.video_sink, 0);
                if !sample.is_null() {
                    self.extract_dims_from_sample(gst, sample);
                    (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                }
            }

            // Wait for real dimensions instead of inventing 1920x1080.
            if !self.audio_only && (self.video_width == 0 || self.video_height == 0) {
                if self.prepare_started.elapsed().as_secs() >= 15 {
                    self.prepare_notified = true;
                    return Some(Err(
                        "Timed out waiting for video dimensions from GStreamer preroll".into(),
                    ));
                }
                return None;
            }

            self.is_prepared = true;
            self.prepare_notified = true;

            // Query duration
            let mut duration_ns: i64 = 0;
            (gst.gst_element_query_duration)(self.pipeline, GST_FORMAT_TIME, &mut duration_ns);
            self.duration_ns = duration_ns;

            let duration_ms = if duration_ns > 0 {
                (duration_ns / 1_000_000) as u128
            } else {
                0u128
            };

            // Query seekable range
            let is_seekable = self.query_is_seekable(gst);

            // Start playback immediately if autoplay
            if self.autoplay {
                (gst.gst_element_set_state)(self.pipeline, GST_STATE_PLAYING);
            }

            let (video_tracks, audio_tracks) = self.query_track_labels(gst);
            self.last_buffered = self.buffered_ranges();
            self.last_track_labels = Some((video_tracks.clone(), audio_tracks.clone()));

            Some(Ok(PlaybackPrepared::new(
                self.video_width,
                self.video_height,
                duration_ms,
                is_seekable,
                video_tracks,
                audio_tracks,
            )))
        }
    }

    /// Prefer playbin3 stream-collection labels; else classic playbin
    /// `n-video` / `n-audio`; else coarse fallbacks.
    fn query_track_labels(&mut self, gst: &LibGStreamer) -> (Vec<String>, Vec<String>) {
        self.drain_stream_collection_messages(gst);

        if self.playbin3
            && (!self.stream_video_labels.is_empty() || !self.stream_audio_labels.is_empty())
        {
            return (
                self.stream_video_labels.clone(),
                self.stream_audio_labels.clone(),
            );
        }

        let video_fallback = if self.audio_only || (self.video_width == 0 && self.video_height == 0)
        {
            vec![]
        } else {
            vec!["video".to_string()]
        };
        let audio_fallback = vec!["audio".to_string()];

        if self.playbin3 || self.pipeline.is_null() {
            return (video_fallback, audio_fallback);
        }

        unsafe {
            let mut n_video: i32 = -1;
            let mut n_audio: i32 = -1;
            let video_prop = CString::new("n-video").unwrap();
            let audio_prop = CString::new("n-audio").unwrap();
            (gst.g_object_get_int)(
                self.pipeline,
                video_prop.as_ptr(),
                &mut n_video,
                std::ptr::null(),
            );
            (gst.g_object_get_int)(
                self.pipeline,
                audio_prop.as_ptr(),
                &mut n_audio,
                std::ptr::null(),
            );

            let video_tracks = if video_fallback.is_empty() {
                vec![]
            } else if n_video > 0 {
                (0..n_video)
                    .map(|i| format!("Video track {}", i + 1))
                    .collect()
            } else {
                video_fallback
            };

            let audio_tracks = if n_audio > 0 {
                (0..n_audio)
                    .map(|i| format!("Audio track {}", i + 1))
                    .collect()
            } else if n_audio == 0 {
                vec![]
            } else {
                audio_fallback
            };

            (video_tracks, audio_tracks)
        }
    }

    fn drain_stream_collection_messages(&mut self, gst: &LibGStreamer) {
        if self.bus.is_null() {
            return;
        }
        let parse = match gst.gst_message_parse_stream_collection {
            Some(f) => f,
            None => return,
        };
        unsafe {
            loop {
                let msg = (gst.gst_bus_pop_filtered)(self.bus, GST_MESSAGE_STREAM_COLLECTION);
                if msg.is_null() {
                    break;
                }
                let mut collection: *mut GstStreamCollection = std::ptr::null_mut();
                parse(msg, &mut collection);
                if !collection.is_null() {
                    self.ingest_stream_collection(gst, collection);
                    (gst.gst_object_unref)(collection as *mut c_void);
                }
                (gst.gst_mini_object_unref)(msg as *mut GstMiniObject);
            }
        }
    }

    fn ingest_stream_collection(&mut self, gst: &LibGStreamer, collection: *mut GstStreamCollection) {
        let (Some(get_size), Some(get_stream), Some(get_type), Some(get_id)) = (
            gst.gst_stream_collection_get_size,
            gst.gst_stream_collection_get_stream,
            gst.gst_stream_get_stream_type,
            gst.gst_stream_get_stream_id,
        ) else {
            return;
        };
        self.stream_video_ids.clear();
        self.stream_audio_ids.clear();
        self.stream_video_labels.clear();
        self.stream_audio_labels.clear();
        unsafe {
            let size = get_size(collection);
            for i in 0..size {
                let stream = get_stream(collection, i);
                if stream.is_null() {
                    continue;
                }
                let ty = get_type(stream);
                let id_ptr = get_id(stream);
                if id_ptr.is_null() {
                    continue;
                }
                let id = CStr::from_ptr(id_ptr).to_string_lossy().to_string();
                if ty & GST_STREAM_TYPE_VIDEO != 0 {
                    let label = self.stream_label(gst, stream, ty, self.stream_video_ids.len());
                    self.stream_video_labels.push(label);
                    self.stream_video_ids.push(id);
                } else if ty & GST_STREAM_TYPE_AUDIO != 0 {
                    let label = self.stream_label(gst, stream, ty, self.stream_audio_ids.len());
                    self.stream_audio_labels.push(label);
                    self.stream_audio_ids.push(id);
                }
            }
        }
        if self.selected_video_idx >= self.stream_video_ids.len() {
            self.selected_video_idx = 0;
        }
        if self.selected_audio_idx >= self.stream_audio_ids.len() {
            self.selected_audio_idx = 0;
        }
    }

    fn stream_label(
        &self,
        gst: &LibGStreamer,
        stream: *mut GstStream,
        ty: u32,
        index: usize,
    ) -> String {
        unsafe {
            if let (Some(get_tags), Some(get_string)) =
                (gst.gst_stream_get_tags, gst.gst_tag_list_get_string)
            {
                let tags = get_tags(stream);
                if !tags.is_null() {
                    for key in ["title", "language-code", "language-name", "codec"] {
                        let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();
                        let ckey = CString::new(key).unwrap();
                        if get_string(tags, ckey.as_ptr(), &mut out) != 0 && !out.is_null() {
                            let s = CStr::from_ptr(out).to_string_lossy().to_string();
                            (gst.g_free)(out as *mut c_void);
                            (gst.gst_mini_object_unref)(tags as *mut GstMiniObject);
                            return s;
                        }
                    }
                    (gst.gst_mini_object_unref)(tags as *mut GstMiniObject);
                }
            }
        }
        if ty & GST_STREAM_TYPE_VIDEO != 0 {
            format!("Video track {}", index + 1)
        } else if ty & GST_STREAM_TYPE_AUDIO != 0 {
            format!("Audio track {}", index + 1)
        } else {
            format!("Track {}", index + 1)
        }
    }

    /// Select a video elementary stream by index into the prepared track list.
    pub fn select_video_track(&mut self, index: usize) -> bool {
        if self.pipeline.is_null() {
            return false;
        }
        let gst = unsafe { &*self.gst };
        self.drain_stream_collection_messages(gst);
        if self.playbin3 {
            if self.stream_video_ids.is_empty() {
                crate::log!(
                    "VIDEO: select_video_track({}) ignored — no playbin3 video stream ids yet",
                    index
                );
                return false;
            }
            if index >= self.stream_video_ids.len() {
                crate::log!(
                    "VIDEO: select_video_track({}) out of range (n={})",
                    index,
                    self.stream_video_ids.len()
                );
                return false;
            }
            self.selected_video_idx = index;
            let ok = self.send_select_streams(gst);
            if !ok {
                crate::log!("VIDEO: select_video_track({}) SELECT_STREAMS failed", index);
            }
            ok
        } else {
            unsafe {
                let prop = CString::new("current-video").unwrap();
                (gst.g_object_set_int)(self.pipeline, prop.as_ptr(), index as i32, std::ptr::null());
            }
            self.selected_video_idx = index;
            true
        }
    }

    /// Select an audio elementary stream by index into the prepared track list.
    pub fn select_audio_track(&mut self, index: usize) -> bool {
        if self.pipeline.is_null() {
            return false;
        }
        let gst = unsafe { &*self.gst };
        self.drain_stream_collection_messages(gst);
        if self.playbin3 {
            if self.stream_audio_ids.is_empty() {
                crate::log!(
                    "VIDEO: select_audio_track({}) ignored — no playbin3 audio stream ids yet",
                    index
                );
                return false;
            }
            if index >= self.stream_audio_ids.len() {
                crate::log!(
                    "VIDEO: select_audio_track({}) out of range (n={})",
                    index,
                    self.stream_audio_ids.len()
                );
                return false;
            }
            self.selected_audio_idx = index;
            let ok = self.send_select_streams(gst);
            if !ok {
                crate::log!("VIDEO: select_audio_track({}) SELECT_STREAMS failed", index);
            }
            ok
        } else {
            unsafe {
                let prop = CString::new("current-audio").unwrap();
                (gst.g_object_set_int)(self.pipeline, prop.as_ptr(), index as i32, std::ptr::null());
            }
            self.selected_audio_idx = index;
            true
        }
    }

    fn send_select_streams(&self, gst: &LibGStreamer) -> bool {
        let (Some(new_event), Some(append)) =
            (gst.gst_event_new_select_streams, gst.g_list_append)
        else {
            return false;
        };
        unsafe {
            // SELECT_STREAMS replaces the *entire* selection. Always include both
            // the current video and audio stream when present, otherwise choosing
            // a video track can silently drop audio (and vice versa).
            let mut owned: Vec<CString> = Vec::new();
            let mut list: *mut GList = std::ptr::null_mut();
            if !self.stream_video_ids.is_empty() {
                let idx = self
                    .selected_video_idx
                    .min(self.stream_video_ids.len() - 1);
                owned.push(CString::new(self.stream_video_ids[idx].as_str()).unwrap());
            }
            if !self.stream_audio_ids.is_empty() {
                let idx = self
                    .selected_audio_idx
                    .min(self.stream_audio_ids.len() - 1);
                owned.push(CString::new(self.stream_audio_ids[idx].as_str()).unwrap());
            }
            for s in &owned {
                list = append(list, s.as_ptr() as *mut c_void);
            }
            if list.is_null() {
                return false;
            }
            let event = new_event(list);
            if event.is_null() {
                // Event did not take ownership — free the GList nodes only.
                if let Some(free_list) = gst.g_list_free {
                    free_list(list);
                }
                return false;
            }
            // Event owns list + gchar* contents; forget CStrings so Drop won't free them.
            for s in owned {
                std::mem::forget(s);
            }
            (gst.gst_element_send_event)(self.pipeline, event) != 0
        }
    }

    /// Query GStreamer for whether the current source is seekable.
    unsafe fn query_is_seekable(&self, gst: &LibGStreamer) -> bool {
        if self.pipeline.is_null() {
            return false;
        }
        let query = (gst.gst_query_new_seeking)(GST_FORMAT_TIME);
        if query.is_null() {
            return false;
        }
        let res = (gst.gst_element_query)(self.pipeline, query);
        if res == 0 {
            (gst.gst_mini_object_unref)(query as *mut GstMiniObject);
            return false;
        }
        let mut format: std::os::raw::c_int = 0;
        let mut seekable: std::os::raw::c_int = 0;
        let mut start: i64 = 0;
        let mut stop: i64 = 0;
        (gst.gst_query_parse_seeking)(query, &mut format, &mut seekable, &mut start, &mut stop);
        (gst.gst_mini_object_unref)(query as *mut GstMiniObject);
        seekable != 0
    }

    /// Pull a frame from appsink and upload/adopt it into GL textures.
    /// Returns true if a new frame was presented.
    pub fn poll_frame(
        &mut self,
        gl: &LibGl,
        textures: &mut CxTexturePool,
        opengl_cx: Option<&OpenglCx>,
    ) -> bool {
        if self.user_paused || self.pipeline.is_null() || self.video_sink.is_null() {
            return false;
        }

        let gst = unsafe { &*self.gst };

        // Satisfy GL NEED_CONTEXT while the pipeline is running.
        if let Some(share) = self.gl_share.as_ref() {
            unsafe {
                loop {
                    let msg = (gst.gst_bus_pop_filtered)(self.bus, GST_MESSAGE_NEED_CONTEXT);
                    if msg.is_null() {
                        break;
                    }
                    if share.is_gl_need_context_message(gst, msg) {
                        share.apply_to_element(gst, self.pipeline);
                    }
                    (gst.gst_mini_object_unref)(msg as *mut GstMiniObject);
                }
            }
        }

        // Do not gate frame pulls on gst_element_get_state(timeout=0):
        // PLAYING transitions are asynchronous and can stay in PAUSED/ASYNC while
        // decoded samples are already available on appsink (common on desktop setups
        // without a stable audio sink). Non-blocking try_pull_sample below naturally
        // returns null when no new frame is ready.

        // Check for EOS and loop if needed
        if self.is_looping {
            unsafe {
                if (gst.gst_app_sink_is_eos)(self.video_sink) != 0 {
                    (gst.gst_element_seek_simple)(
                        self.pipeline,
                        GST_FORMAT_TIME,
                        GST_SEEK_FLAG_FLUSH | GST_SEEK_FLAG_KEY_UNIT,
                        0,
                    );
                }
            }
        }

        unsafe {
            // Pull next decoded frame — non-blocking (timeout=0).
            let sample = (gst.gst_app_sink_try_pull_sample)(self.video_sink, 0);
            if sample.is_null() {
                return false;
            }

            let buffer = (gst.gst_sample_get_buffer)(sample);
            if buffer.is_null() {
                (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                return false;
            }

            // Extract dimensions from the sample's caps
            self.extract_dims_from_sample(gst, sample);

            // Zero-copy paths: on failure, drop the sample and wait — do NOT
            // reinterpret GLMemory/DMABuf as system RGBA (that produces garbage).
            if self.caps_profile.is_gl_memory() {
                if let (Some(is_gl_memory), Some(get_gl_texture_id)) =
                    (gst.gst_is_gl_memory, gst.gst_gl_memory_get_texture_id)
                {
                    let memory = (gst.gst_buffer_peek_memory)(buffer, 0);
                    if !memory.is_null() && is_gl_memory(memory) != 0 {
                        let gl_texture = get_gl_texture_id(memory);
                        if gl_texture != 0 {
                            let cxtexture = &mut textures[self.texture_id];

                            if let Some(old) = cxtexture.os.gl_texture {
                                if old != gl_texture && cxtexture.os.gl_texture_owned {
                                    (gl.glDeleteTextures)(1, &old);
                                }
                            }

                            cxtexture.os.gl_texture = Some(gl_texture);
                            cxtexture.os.gl_texture_owned = false;
                            cxtexture.format = crate::texture::TextureFormat::VideoExternal;
                            cxtexture.alloc = Some(TextureAlloc {
                                width: 0,
                                height: 0,
                                pixel: TexturePixel::VideoExternal,
                                category: TextureCategory::Video,
                            });

                            if !self.retained_gl_sample.is_null() {
                                (gst.gst_mini_object_unref)(
                                    self.retained_gl_sample as *mut GstMiniObject,
                                );
                            }
                            self.retained_gl_sample = sample;
                            self.dmabuf_yuv_mode = false;
                            self.yuv_biplanar = false;

                            if !self.logged_first_upload {
                                self.logged_first_upload = true;
                                crate::log!("VIDEO: presenting via GLMemory zero-copy");
                            }

                            return true;
                        }
                    }
                }
                (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                return false;
            }

            if self.caps_profile.is_dmabuf() {
                if let Some(cx) = opengl_cx {
                    if self.try_present_dmabuf(gst, gl, textures, cx, sample, buffer) {
                        return true;
                    }
                }
                (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                return false;
            }

            // System-memory fallback path: map buffer and upload I420 or RGBA.
            if !self.retained_gl_sample.is_null() {
                (gst.gst_mini_object_unref)(self.retained_gl_sample as *mut GstMiniObject);
                self.retained_gl_sample = std::ptr::null_mut();
            }
            self.release_egl_images();
            let mut map_info = GstMapInfo::default();
            if (gst.gst_buffer_map)(buffer, &mut map_info, GST_MAP_READ) == 0 {
                (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                return false;
            }

            let width = self.video_width as usize;
            let height = self.video_height as usize;

            if map_info.data.is_null() || width == 0 || height == 0 {
                (gst.gst_buffer_unmap)(buffer, &mut map_info);
                (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                return false;
            }

            if self.caps_profile == VideoCapsProfile::SystemI420 {
                let Some(yuv_ids) = self.yuv_ids else {
                    (gst.gst_buffer_unmap)(buffer, &mut map_info);
                    (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                    return false;
                };

                let meta = Self::read_video_meta(gst, buffer);
                let Some(layout) = meta
                    .and_then(|m| i420_layout_from_video_meta(m, width, height, map_info.size))
                    .or_else(|| infer_i420_layout(width, height, map_info.size))
                else {
                    (gst.gst_buffer_unmap)(buffer, &mut map_info);
                    (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                    return false;
                };

                let data = std::slice::from_raw_parts(map_info.data, map_info.size);
                let y_end = layout.y_off + layout.y_stride as usize * height;
                let u_end = layout.u_off + layout.u_stride as usize * height.div_ceil(2);
                let v_end = layout.v_off + layout.v_stride as usize * height.div_ceil(2);
                if y_end > data.len() || u_end > data.len() || v_end > data.len() {
                    (gst.gst_buffer_unmap)(buffer, &mut map_info);
                    (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                    return false;
                }
                let y = &data[layout.y_off..y_end];
                let u = &data[layout.u_off..u_end];
                let v = &data[layout.v_off..v_end];
                upload_i420_planes_to_gl(
                    gl,
                    textures,
                    yuv_ids.tex_y_id,
                    yuv_ids.tex_u_id,
                    yuv_ids.tex_v_id,
                    y,
                    u,
                    v,
                    width as u32,
                    height as u32,
                    layout.y_stride,
                    layout.u_stride,
                    layout.v_stride,
                );

                (gst.gst_buffer_unmap)(buffer, &mut map_info);
                (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                self.dmabuf_yuv_mode = false;
                self.yuv_biplanar = false;

                if !self.logged_first_upload {
                    self.logged_first_upload = true;
                    crate::log!("VIDEO: presenting via system I420 upload");
                }
                return true;
            }

            let row_bytes = width * 4; // RGBA = 4 bytes per pixel
            let packed_size = row_bytes * height;

            let stride = if height > 1 {
                map_info.size / height
            } else {
                row_bytes
            };
            let min_size = stride
                .saturating_mul(height.saturating_sub(1))
                .saturating_add(row_bytes);
            if stride < row_bytes || map_info.size < min_size {
                (gst.gst_buffer_unmap)(buffer, &mut map_info);
                (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                return false;
            }

            let cxtexture = &mut textures[self.texture_id];
            if cxtexture.os.gl_texture.is_some() && !cxtexture.os.gl_texture_owned {
                cxtexture.os.gl_texture = None;
                cxtexture.os.gl_texture_owned = true;
            }
            let needs_alloc = if cxtexture.os.gl_texture.is_none() {
                let mut gl_texture = std::mem::MaybeUninit::uninit();
                (gl.glGenTextures)(1, gl_texture.as_mut_ptr());
                let gl_texture = gl_texture.assume_init();
                cxtexture.os.gl_texture = Some(gl_texture);
                cxtexture.os.gl_texture_owned = true;

                (gl.glBindTexture)(gl_sys::TEXTURE_2D, gl_texture);
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_2D,
                    gl_sys::TEXTURE_WRAP_S,
                    gl_sys::CLAMP_TO_EDGE as i32,
                );
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_2D,
                    gl_sys::TEXTURE_WRAP_T,
                    gl_sys::CLAMP_TO_EDGE as i32,
                );
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_2D,
                    gl_sys::TEXTURE_MIN_FILTER,
                    gl_sys::LINEAR as i32,
                );
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_2D,
                    gl_sys::TEXTURE_MAG_FILTER,
                    gl_sys::LINEAR as i32,
                );
                true
            } else {
                self.tex_width != width || self.tex_height != height
            };

            let gl_texture = cxtexture.os.gl_texture.unwrap();
            (gl.glBindTexture)(gl_sys::TEXTURE_2D, gl_texture);
            (gl.glPixelStorei)(gl_sys::UNPACK_ALIGNMENT, 4);
            (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_PIXELS, 0);
            (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_ROWS, 0);

            let can_direct_upload = stride % 4 == 0;
            let upload_ptr: *const c_void = if can_direct_upload {
                (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, (stride / 4) as i32);
                map_info.data as *const c_void
            } else {
                (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, 0);
                self.pixel_buf.clear();
                self.pixel_buf.resize(packed_size, 0);
                let src = std::slice::from_raw_parts(map_info.data, map_info.size);
                for y in 0..height {
                    let row_start = y * stride;
                    let src_row = &src[row_start..row_start + row_bytes];
                    let dst_row_start = y * row_bytes;
                    self.pixel_buf[dst_row_start..dst_row_start + row_bytes]
                        .copy_from_slice(src_row);
                }
                self.pixel_buf.as_ptr() as *const c_void
            };

            if needs_alloc {
                (gl.glTexImage2D)(
                    gl_sys::TEXTURE_2D,
                    0,
                    gl_sys::RGBA as i32,
                    width as i32,
                    height as i32,
                    0,
                    gl_sys::RGBA,
                    gl_sys::UNSIGNED_BYTE,
                    upload_ptr,
                );
                self.tex_width = width;
                self.tex_height = height;
            } else {
                (gl.glTexSubImage2D)(
                    gl_sys::TEXTURE_2D,
                    0,
                    0,
                    0,
                    width as i32,
                    height as i32,
                    gl_sys::RGBA,
                    gl_sys::UNSIGNED_BYTE,
                    upload_ptr,
                );
            }

            (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, 0);
            (gl.glBindTexture)(gl_sys::TEXTURE_2D, 0);
            (gst.gst_buffer_unmap)(buffer, &mut map_info);
            (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);

            cxtexture.format = crate::texture::TextureFormat::VideoExternal;
            cxtexture.alloc = Some(TextureAlloc {
                width,
                height,
                pixel: TexturePixel::VideoExternal,
                category: TextureCategory::Video,
            });
            self.dmabuf_yuv_mode = false;
            self.yuv_biplanar = false;

            if !self.logged_first_upload {
                self.logged_first_upload = true;
                crate::log!("VIDEO: presenting via system RGBA upload");
            }

            true
        }
    }

    /// Import a DMA-Buf sample via EGLImage. Returns true on success (and retains
    /// `sample`). On failure the caller should drop the sample (no CPU reinterpret).
    unsafe fn try_present_dmabuf(
        &mut self,
        gst: &LibGStreamer,
        gl: &LibGl,
        textures: &mut CxTexturePool,
        opengl_cx: &OpenglCx,
        sample: *mut GstSample,
        buffer: *mut GstBuffer,
    ) -> bool {
        let (Some(is_dmabuf), Some(get_fd)) =
            (gst.gst_is_dmabuf_memory, gst.gst_dmabuf_memory_get_fd)
        else {
            return false;
        };
        let Some(create_image) = opengl_cx.libegl.eglCreateImageKHR else {
            return false;
        };
        let Some(target_tex) = opengl_cx.libegl.glEGLImageTargetTexture2DOES else {
            return false;
        };

        let n_mem = gst.gst_buffer_n_memory.map(|f| f(buffer)).unwrap_or(1);
        if n_mem == 0 {
            return false;
        }

        let mut fds: Vec<RawFd> = Vec::new();
        for i in 0..n_mem {
            let memory = (gst.gst_buffer_peek_memory)(buffer, i);
            if memory.is_null() || is_dmabuf(memory) == 0 {
                return false;
            }
            let fd = get_fd(memory);
            if fd < 0 {
                return false;
            }
            fds.push(fd);
        }

        let width = self.video_width as usize;
        let height = self.video_height as usize;
        if width == 0 || height == 0 {
            return false;
        }

        let caps = (gst.gst_sample_get_caps)(sample);
        let (fourcc, modifier, format_name) = Self::dmabuf_format_from_caps(gst, caps);
        let meta = Self::read_video_meta(gst, buffer);

        opengl_cx.make_current();

        let is_nv12 = format_name.to_ascii_uppercase().contains("NV12")
            || fourcc == drm_fourcc(b"NV12")
            || (fds.len() >= 2
                && fourcc == 0
                && meta
                    .as_ref()
                    .map(|m| m.n_planes >= 2)
                    .unwrap_or(false));

        if is_nv12 {
            let Some(yuv_ids) = self.yuv_ids else {
                return false;
            };
            let (y_plane, uv_plane) =
                Self::nv12_plane_layouts(width, height, fds.len(), fourcc, modifier, meta);

            let y_image = Self::egl_image_from_dmabuf(
                opengl_cx,
                create_image,
                fds[y_plane.fd_index.min(fds.len() - 1)],
                y_plane.offset,
                y_plane.pitch,
                y_plane.width,
                y_plane.height,
                y_plane.fourcc,
                modifier,
            );
            let uv_image = Self::egl_image_from_dmabuf(
                opengl_cx,
                create_image,
                fds[uv_plane.fd_index.min(fds.len() - 1)],
                uv_plane.offset,
                uv_plane.pitch,
                uv_plane.width,
                uv_plane.height,
                uv_plane.fourcc,
                modifier,
            );
            if y_image.is_null() || uv_image.is_null() {
                if !y_image.is_null() {
                    if let Some(destroy) = opengl_cx.libegl.eglDestroyImageKHR {
                        destroy(opengl_cx.egl_display, y_image);
                    }
                }
                if !uv_image.is_null() {
                    if let Some(destroy) = opengl_cx.libegl.eglDestroyImageKHR {
                        destroy(opengl_cx.egl_display, uv_image);
                    }
                }
                return false;
            }

            self.bind_egl_image_to_texture(
                gl,
                textures,
                yuv_ids.tex_y_id,
                y_image,
                target_tex,
                y_plane.width as usize,
                y_plane.height as usize,
                TexturePixel::Ru8,
            );
            self.bind_egl_image_to_texture(
                gl,
                textures,
                yuv_ids.tex_u_id,
                uv_image,
                target_tex,
                uv_plane.width as usize,
                uv_plane.height as usize,
                TexturePixel::RGu8,
            );
            // Biplanar shaders sample UV from tex_u (RG); mirror into V for
            // shaders that still bind three slots.
            {
                let u_tex = textures[yuv_ids.tex_u_id].os.gl_texture;
                let u_alloc = textures[yuv_ids.tex_u_id].alloc.clone();
                let tex_v = &mut textures[yuv_ids.tex_v_id];
                tex_v.format = crate::texture::TextureFormat::VideoYuvPlane;
                tex_v.os.gl_texture = u_tex;
                tex_v.os.gl_texture_owned = false;
                tex_v.alloc = u_alloc;
            }

            self.release_egl_images();
            self.retained_egl_images = vec![y_image, uv_image];
            self.egl_display_for_images = opengl_cx.egl_display;
            self.egl_destroy_image = opengl_cx.libegl.eglDestroyImageKHR;

            if !self.retained_gl_sample.is_null() {
                (gst.gst_mini_object_unref)(self.retained_gl_sample as *mut GstMiniObject);
            }
            self.retained_gl_sample = sample;
            self.dmabuf_yuv_mode = true;
            self.yuv_biplanar = true;
            if !self.logged_first_upload {
                self.logged_first_upload = true;
                crate::log!("VIDEO: presenting via DMA-Buf NV12 import");
            }
            return true;
        }

        // Single-plane RGBA/BGRA style DRM fourcc → VideoExternal.
        let plane = Self::rgba_plane_layout(width, height, fourcc, meta);
        let image = Self::egl_image_from_dmabuf(
            opengl_cx,
            create_image,
            fds[0],
            plane.offset,
            plane.pitch,
            plane.width,
            plane.height,
            plane.fourcc,
            modifier,
        );
        if image.is_null() {
            return false;
        }

        self.bind_egl_image_to_texture(
            gl,
            textures,
            self.texture_id,
            image,
            target_tex,
            width,
            height,
            TexturePixel::VideoExternal,
        );
        textures[self.texture_id].format = crate::texture::TextureFormat::VideoExternal;

        self.release_egl_images();
        self.retained_egl_images = vec![image];
        self.egl_display_for_images = opengl_cx.egl_display;
        self.egl_destroy_image = opengl_cx.libegl.eglDestroyImageKHR;

        if !self.retained_gl_sample.is_null() {
            (gst.gst_mini_object_unref)(self.retained_gl_sample as *mut GstMiniObject);
        }
        self.retained_gl_sample = sample;
        self.dmabuf_yuv_mode = false;
        self.yuv_biplanar = false;
        if !self.logged_first_upload {
            self.logged_first_upload = true;
            crate::log!("VIDEO: presenting via DMA-Buf RGBA import");
        }
        true
    }

    unsafe fn read_video_meta(
        gst: &LibGStreamer,
        buffer: *mut GstBuffer,
    ) -> Option<&'static GstVideoMetaView> {
        let get_meta = gst.gst_buffer_get_video_meta?;
        let meta = get_meta(buffer);
        if meta.is_null() {
            return None;
        }
        let view = &*(meta as *const GstVideoMetaView);
        // Cross-check the hand-rolled ABI view against libgstvideo helpers when
        // available — reject clearly-corrupt layouts instead of uploading garbage.
        if let Some(plane_height) = gst.gst_video_meta_get_plane_height {
            if view.n_planes == 0 || view.n_planes > 4 {
                return None;
            }
            for plane in 0..view.n_planes {
                let mut h: u32 = 0;
                if plane_height(meta, plane, &mut h) == 0 || h == 0 {
                    return None;
                }
                if view.stride[plane as usize] <= 0 {
                    return None;
                }
            }
        } else if view.n_planes == 0 || view.n_planes > 4 || view.stride[0] <= 0 {
            return None;
        }
        Some(view)
    }

    fn nv12_plane_layouts(
        width: usize,
        height: usize,
        n_fds: usize,
        _fourcc: u32,
        _modifier: u64,
        meta: Option<&GstVideoMetaView>,
    ) -> (DmaPlaneLayout, DmaPlaneLayout) {
        let cw = width.div_ceil(2) as u32;
        let ch = height.div_ceil(2) as u32;
        if let Some(m) = meta {
            if m.n_planes >= 2 && m.stride[0] > 0 && m.stride[1] > 0 {
                let y = DmaPlaneLayout {
                    fd_index: 0,
                    offset: m.offset[0] as u32,
                    pitch: m.stride[0] as u32,
                    width: width as u32,
                    height: height as u32,
                    fourcc: drm_fourcc(b"R8  "),
                };
                let uv = DmaPlaneLayout {
                    fd_index: if n_fds >= 2 { 1 } else { 0 },
                    offset: m.offset[1] as u32,
                    pitch: m.stride[1] as u32,
                    width: cw,
                    height: ch,
                    fourcc: drm_fourcc(b"GR88"),
                };
                return (y, uv);
            }
        }
        let pitch_y = width as u32;
        (
            DmaPlaneLayout {
                fd_index: 0,
                offset: 0,
                pitch: pitch_y,
                width: width as u32,
                height: height as u32,
                fourcc: drm_fourcc(b"R8  "),
            },
            DmaPlaneLayout {
                fd_index: if n_fds >= 2 { 1 } else { 0 },
                offset: if n_fds >= 2 {
                    0
                } else {
                    pitch_y.saturating_mul(height as u32)
                },
                pitch: pitch_y,
                width: cw,
                height: ch,
                fourcc: drm_fourcc(b"GR88"),
            },
        )
    }

    fn rgba_plane_layout(
        width: usize,
        height: usize,
        fourcc: u32,
        meta: Option<&GstVideoMetaView>,
    ) -> DmaPlaneLayout {
        let fourcc = if fourcc != 0 {
            fourcc
        } else {
            drm_fourcc(b"AB24")
        };
        if let Some(m) = meta {
            if m.n_planes >= 1 && m.stride[0] > 0 {
                return DmaPlaneLayout {
                    fd_index: 0,
                    offset: m.offset[0] as u32,
                    pitch: m.stride[0] as u32,
                    width: width as u32,
                    height: height as u32,
                    fourcc,
                };
            }
        }
        DmaPlaneLayout {
            fd_index: 0,
            offset: 0,
            pitch: (width * 4) as u32,
            width: width as u32,
            height: height as u32,
            fourcc,
        }
    }

    unsafe fn egl_image_from_dmabuf(
        opengl_cx: &OpenglCx,
        create_image: unsafe extern "C" fn(
            egl_sys::EGLDisplay,
            egl_sys::EGLContext,
            egl_sys::EGLenum,
            egl_sys::EGLClientBuffer,
            *const egl_sys::EGLint,
        ) -> egl_sys::EGLImageKHR,
        fd: RawFd,
        offset: u32,
        pitch: u32,
        width: u32,
        height: u32,
        fourcc: u32,
        modifier: u64,
    ) -> *mut c_void {
        let attribs = [
            egl_sys::EGL_LINUX_DRM_FOURCC_EXT,
            fourcc,
            egl_sys::EGL_WIDTH,
            width,
            egl_sys::EGL_HEIGHT,
            height,
            egl_sys::EGL_DMA_BUF_PLANE0_FD_EXT,
            fd as u32,
            egl_sys::EGL_DMA_BUF_PLANE0_OFFSET_EXT,
            offset,
            egl_sys::EGL_DMA_BUF_PLANE0_PITCH_EXT,
            pitch,
            egl_sys::EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT,
            modifier as u32,
            egl_sys::EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT,
            (modifier >> 32) as u32,
            egl_sys::EGL_NONE,
        ];
        create_image(
            opengl_cx.egl_display,
            std::ptr::null_mut(),
            egl_sys::EGL_LINUX_DMA_BUF_EXT,
            std::ptr::null_mut(),
            attribs.as_ptr() as _,
        )
    }

    unsafe fn bind_egl_image_to_texture(
        &self,
        gl: &LibGl,
        textures: &mut CxTexturePool,
        texture_id: TextureId,
        egl_image: *mut c_void,
        target_tex: unsafe extern "C" fn(super::gl_sys::GLenum, egl_sys::EGLImageKHR),
        width: usize,
        height: usize,
        pixel: TexturePixel,
    ) {
        let cxtexture = &mut textures[texture_id];
        if cxtexture.os.gl_texture.is_some() && cxtexture.os.gl_texture_owned {
            if let Some(old) = cxtexture.os.gl_texture.take() {
                (gl.glDeleteTextures)(1, &old);
            }
        }
        let gl_texture = cxtexture.os.gl_texture.get_or_insert_with(|| {
            let mut t = std::mem::MaybeUninit::uninit();
            (gl.glGenTextures)(1, t.as_mut_ptr());
            t.assume_init()
        });
        cxtexture.os.gl_texture_owned = true; // we own the GL name; EGLImage is separate
        (gl.glBindTexture)(gl_sys::TEXTURE_2D, *gl_texture);
        (gl.glTexParameteri)(
            gl_sys::TEXTURE_2D,
            gl_sys::TEXTURE_WRAP_S,
            gl_sys::CLAMP_TO_EDGE as i32,
        );
        (gl.glTexParameteri)(
            gl_sys::TEXTURE_2D,
            gl_sys::TEXTURE_WRAP_T,
            gl_sys::CLAMP_TO_EDGE as i32,
        );
        (gl.glTexParameteri)(
            gl_sys::TEXTURE_2D,
            gl_sys::TEXTURE_MIN_FILTER,
            gl_sys::LINEAR as i32,
        );
        (gl.glTexParameteri)(
            gl_sys::TEXTURE_2D,
            gl_sys::TEXTURE_MAG_FILTER,
            gl_sys::LINEAR as i32,
        );
        target_tex(gl_sys::TEXTURE_2D, egl_image);
        (gl.glBindTexture)(gl_sys::TEXTURE_2D, 0);

        let format = if matches!(pixel, TexturePixel::VideoExternal) {
            crate::texture::TextureFormat::VideoExternal
        } else {
            crate::texture::TextureFormat::VideoYuvPlane
        };
        cxtexture.format = format;
        cxtexture.alloc = Some(TextureAlloc {
            width,
            height,
            pixel,
            category: TextureCategory::Video,
        });
    }

    fn dmabuf_format_from_caps(
        gst: &LibGStreamer,
        caps: *mut GstCaps,
    ) -> (u32, u64, String) {
        if caps.is_null() {
            return (0, 0, String::new());
        }
        unsafe {
            let structure = (gst.gst_caps_get_structure)(caps, 0);
            if structure.is_null() {
                return (0, 0, String::new());
            }
            let mut format_name = String::new();
            let format_key = CString::new("format").unwrap();
            let fptr = (gst.gst_structure_get_string)(structure, format_key.as_ptr());
            if !fptr.is_null() {
                format_name = CStr::from_ptr(fptr).to_string_lossy().to_string();
            }
            let mut fourcc = 0u32;
            let mut modifier = 0u64;
            let drm_key = CString::new("drm-format").unwrap();
            let dptr = (gst.gst_structure_get_string)(structure, drm_key.as_ptr());
            if !dptr.is_null() {
                if let Some(parse) = gst.gst_video_dma_drm_fourcc_from_string {
                    fourcc = parse(dptr, &mut modifier);
                }
                if format_name.is_empty() {
                    format_name = CStr::from_ptr(dptr).to_string_lossy().to_string();
                }
            }
            (fourcc, modifier, format_name)
        }
    }

    /// Check if this player has reached end of stream (non-looping only).
    /// Returns true once per EOS event.
    pub fn check_eos(&mut self) -> bool {
        if self.eos_notified || self.is_looping {
            return false;
        }
        // Video path: appsink EOS. Audio-only: bus EOS (handled in poll_runtime).
        if !self.video_sink.is_null() {
            let is_eos = unsafe { ((*self.gst).gst_app_sink_is_eos)(self.video_sink) != 0 };
            if is_eos {
                self.eos_notified = true;
                return true;
            }
        }
        false
    }

    /// Poll the bus for runtime errors / EOS / buffering after prepare completed.
    pub fn poll_runtime(&mut self) -> Vec<GstRuntimeEvent> {
        let mut out = Vec::new();
        poll_pending_pipeline_drops();
        if self.pipeline.is_null() || self.bus.is_null() {
            return out;
        }
        let gst = unsafe { &*self.gst };

        self.report_pending_state_latency(gst);
        self.drain_buffering_messages(gst);

        // Refresh playbin3 stream ids/labels before other bus drains so track
        // selection always sees the latest STREAM_COLLECTION.
        self.drain_stream_collection_messages(gst);
        let current_labels = (
            self.stream_video_labels.clone(),
            self.stream_audio_labels.clone(),
        );
        if (!self.stream_video_ids.is_empty() || !self.stream_audio_ids.is_empty())
            && self.last_track_labels.as_ref() != Some(&current_labels)
        {
            self.last_track_labels = Some(current_labels.clone());
            out.push(GstRuntimeEvent::TracksChanged {
                video_tracks: current_labels.0,
                audio_tracks: current_labels.1,
            });
        }

        unsafe {
            // Drain ERROR messages.
            loop {
                let msg = (gst.gst_bus_pop_filtered)(self.bus, GST_MESSAGE_ERROR);
                if msg.is_null() {
                    break;
                }
                let mut error: *mut GError = std::ptr::null_mut();
                let mut debug: *mut std::os::raw::c_char = std::ptr::null_mut();
                (gst.gst_message_parse_error)(msg, &mut error, &mut debug);
                let s = if !error.is_null() {
                    let msg_ptr = (*error).message;
                    let text = if !msg_ptr.is_null() {
                        CStr::from_ptr(msg_ptr).to_string_lossy().to_string()
                    } else {
                        "Unknown GStreamer error".to_string()
                    };
                    (gst.g_error_free)(error);
                    text
                } else {
                    "Unknown GStreamer error".to_string()
                };
                if !debug.is_null() {
                    (gst.g_free)(debug as *mut c_void);
                }
                (gst.gst_mini_object_unref)(msg as *mut GstMiniObject);
                out.push(GstRuntimeEvent::Error(s));
            }
            // Drain WARNING messages (only escalate plugin/caps issues).
            loop {
                let msg = (gst.gst_bus_pop_filtered)(self.bus, GST_MESSAGE_WARNING);
                if msg.is_null() {
                    break;
                }
                let mut error: *mut GError = std::ptr::null_mut();
                let mut debug: *mut std::os::raw::c_char = std::ptr::null_mut();
                (gst.gst_message_parse_warning)(msg, &mut error, &mut debug);
                let s = if !error.is_null() {
                    let msg_ptr = (*error).message;
                    let text = if !msg_ptr.is_null() {
                        CStr::from_ptr(msg_ptr).to_string_lossy().to_string()
                    } else {
                        String::new()
                    };
                    (gst.g_error_free)(error);
                    text
                } else {
                    String::new()
                };
                if !debug.is_null() {
                    (gst.g_free)(debug as *mut c_void);
                }
                (gst.gst_mini_object_unref)(msg as *mut GstMiniObject);
                if s.contains("missing") || s.contains("not-negotiated") {
                    out.push(GstRuntimeEvent::Error(s));
                }
            }
            // Drain EOS.
            loop {
                let msg = (gst.gst_bus_pop_filtered)(self.bus, GST_MESSAGE_EOS);
                if msg.is_null() {
                    break;
                }
                (gst.gst_mini_object_unref)(msg as *mut GstMiniObject);
                if self.is_looping {
                    (gst.gst_element_seek_simple)(
                        self.pipeline,
                        GST_FORMAT_TIME,
                        GST_SEEK_FLAG_FLUSH | GST_SEEK_FLAG_KEY_UNIT,
                        0,
                    );
                } else if !self.eos_notified {
                    self.eos_notified = true;
                    out.push(GstRuntimeEvent::Eos);
                }
            }
        }
        out
    }

    /// Drain `GST_MESSAGE_BUFFERING` so the bus does not fill up.
    ///
    /// Do **not** pause/play the pipeline from these messages. For HLS/DASH the
    /// adaptive demux needs PLAYING (or at least an active streaming thread) to
    /// keep fetching segments; pausing on buffering < 100 deadlocks resume
    /// (buffer never reaches 100% because download stopped). Progressive
    /// download is already covered by playbin's own queue + our
    /// `QUERY_BUFFERING` ranges for the UI.
    fn drain_buffering_messages(&mut self, gst: &LibGStreamer) {
        let Some(parse) = gst.gst_message_parse_buffering else {
            // Still pop so the bus cannot back up if parse is missing.
            unsafe {
                loop {
                    let msg = (gst.gst_bus_pop_filtered)(self.bus, GST_MESSAGE_BUFFERING);
                    if msg.is_null() {
                        break;
                    }
                    (gst.gst_mini_object_unref)(msg as *mut GstMiniObject);
                }
            }
            return;
        };
        unsafe {
            loop {
                let msg = (gst.gst_bus_pop_filtered)(self.bus, GST_MESSAGE_BUFFERING);
                if msg.is_null() {
                    break;
                }
                let mut percent: i32 = 100;
                parse(msg, &mut percent);
                (gst.gst_mini_object_unref)(msg as *mut GstMiniObject);
            }
        }
    }

    /// Returns updated buffered ranges when they change.
    pub fn take_buffered_ranges_if_changed(&mut self) -> Option<Vec<(f64, f64)>> {
        let ranges = self.buffered_ranges();
        if ranges == self.last_buffered {
            None
        } else {
            self.last_buffered = ranges.clone();
            Some(ranges)
        }
    }

    /// Whether this player has an active (non-null) pipeline.
    pub fn is_active(&self) -> bool {
        !self.pipeline.is_null()
    }

    pub fn is_yuv_mode(&self) -> bool {
        self.caps_profile == VideoCapsProfile::SystemI420 || self.dmabuf_yuv_mode
    }

    pub fn yuv_biplanar(&self) -> bool {
        self.yuv_biplanar
    }

    pub fn yuv_full_range(&self) -> bool {
        self.yuv_full_range
    }

    pub fn yuv_metadata(&self) -> crate::event::video_playback::VideoYuvMetadata {
        crate::event::video_playback::VideoYuvMetadata {
            enabled: self.is_yuv_mode(),
            matrix: self.yuv_matrix,
            biplanar: self.yuv_biplanar,
            full_range: self.yuv_full_range,
            rotation_steps: 0.0,
        }
    }

    pub fn yuv_matrix(&self) -> f32 {
        self.yuv_matrix
    }

    fn query_position_ns(&self) -> i64 {
        if self.pipeline.is_null() {
            return -1;
        }
        unsafe {
            let gst = &*self.gst;
            let mut position_ns: i64 = 0;
            if (gst.gst_element_query_position)(self.pipeline, GST_FORMAT_TIME, &mut position_ns) != 0
                && position_ns >= 0
            {
                position_ns
            } else {
                -1
            }
        }
    }

    /// Non-blocking probe of the last requested state change. Logs how long the
    /// pipeline actually took to reach PAUSED/PLAYING, which is the number that
    /// tells us whether an audible pause tail comes from GStreamer or from the
    /// audio device buffer.
    fn report_pending_state_latency(&mut self, gst: &LibGStreamer) {
        let (Some(since), target) = (self.pending_state_since, self.pending_state_target) else {
            return;
        };
        let mut current: std::os::raw::c_uint = 0;
        let mut pending: std::os::raw::c_uint = 0;
        unsafe {
            (gst.gst_element_get_state)(self.pipeline, &mut current, &mut pending, 0);
        }
        let elapsed = since.elapsed();
        if current == target {
            crate::log!(
                "VIDEO: reached {} in {:.1}ms",
                if target == GST_STATE_PLAYING { "PLAYING" } else { "PAUSED" },
                elapsed.as_secs_f64() * 1000.0
            );
            self.pending_state_since = None;
            self.pending_state_target = 0;
        } else if elapsed > std::time::Duration::from_millis(1000) {
            crate::log!(
                "VIDEO: state change to {} still pending after 1s (current={} pending={})",
                if target == GST_STATE_PLAYING { "PLAYING" } else { "PAUSED" },
                current,
                pending
            );
            self.pending_state_since = None;
            self.pending_state_target = 0;
        }
    }

    /// Kick playbin back to PLAYING after pause. Never block on `get_state` here:
    /// we are called from the UI thread (paint / input) and blocking waits can
    /// deadlock GStreamer's async state machine so resume appears "stuck".
    fn request_playing(&mut self) {
        if self.pipeline.is_null() {
            return;
        }
        self.pending_state_target = GST_STATE_PLAYING;
        self.pending_state_since = Some(Instant::now());
        unsafe {
            let gst = &*self.gst;
            let ret = (gst.gst_element_set_state)(self.pipeline, GST_STATE_PLAYING);
            if ret != GST_STATE_CHANGE_FAILURE {
                return;
            }
            crate::log!("VIDEO: PLAYING failed after pause — recovering with in-place seek");
            let pos = if self.resume_position_ns >= 0 {
                self.resume_position_ns
            } else {
                self.query_position_ns()
            };
            if pos >= 0 {
                (gst.gst_element_seek_simple)(
                    self.pipeline,
                    GST_FORMAT_TIME,
                    GST_SEEK_FLAG_FLUSH | GST_SEEK_FLAG_KEY_UNIT,
                    pos,
                );
            }
            (gst.gst_element_set_state)(self.pipeline, GST_STATE_PLAYING);
        }
    }

    fn apply_mute_state(&self) {
        if self.pipeline.is_null() {
            return;
        }
        let mute = self.user_muted || self.pause_muted;
        let volume = if self.pause_muted {
            0.0
        } else {
            self.playback_volume
        };
        unsafe {
            let gst = &*self.gst;
            let vol_prop = CString::new("volume").unwrap();
            let mute_prop = CString::new("mute").unwrap();

            // Client-side volume element inside our audio sink bin: this is the
            // one that silences samples before they reach the device, so it is
            // what actually bounds the pause tail to the sink's buffer-time.
            if !self.audio_volume.is_null() {
                (gst.g_object_set_double)(
                    self.audio_volume,
                    vol_prop.as_ptr(),
                    volume,
                    std::ptr::null(),
                );
                (gst.g_object_set_int)(
                    self.audio_volume,
                    mute_prop.as_ptr(),
                    if mute { 1 } else { 0 },
                    std::ptr::null(),
                );
            }

            // Keep playbin in sync so app-visible volume/mute stay correct.
            (gst.g_object_set_int)(
                self.pipeline,
                mute_prop.as_ptr(),
                if mute { 1 } else { 0 },
                std::ptr::null(),
            );
            (gst.g_object_set_double)(
                self.pipeline,
                vol_prop.as_ptr(),
                volume,
                std::ptr::null(),
            );
        }
    }

    pub fn play(&mut self) {
        if self.pipeline.is_null() {
            return;
        }
        self.eos_notified = false;
        self.user_paused = false;
        self.pause_muted = false;
        self.apply_mute_state();
        self.request_playing();
    }

    pub fn play_mut(&mut self) {
        self.play();
    }

    pub fn pause(&mut self) {
        if self.pipeline.is_null() {
            return;
        }
        self.resume_position_ns = self.query_position_ns();
        self.user_paused = true;
        self.pause_muted = true;
        self.apply_mute_state();
        self.pending_state_target = GST_STATE_PAUSED;
        self.pending_state_since = Some(Instant::now());
        unsafe {
            ((*self.gst).gst_element_set_state)(self.pipeline, GST_STATE_PAUSED);
        }
    }

    pub fn resume(&mut self) {
        self.play();
    }

    pub fn mute(&mut self) {
        self.user_muted = true;
        self.apply_mute_state();
    }

    pub fn unmute(&mut self) {
        self.user_muted = false;
        self.apply_mute_state();
    }

    pub fn seek_to(&mut self, position_ms: u64) {
        if self.pipeline.is_null() {
            return;
        }
        // Allow a subsequent EOS notification after replay / scrub.
        self.eos_notified = false;
        unsafe {
            let position_ns = position_ms as i64 * 1_000_000;
            ((*self.gst).gst_element_seek_simple)(
                self.pipeline,
                GST_FORMAT_TIME,
                GST_SEEK_FLAG_FLUSH | GST_SEEK_FLAG_ACCURATE,
                position_ns,
            );
        }
    }

    pub fn set_volume(&mut self, volume: f64) {
        if self.pipeline.is_null() {
            return;
        }
        self.playback_volume = volume.clamp(0.0, 10.0);
        self.apply_mute_state();
    }

    pub fn set_playback_rate(&self, rate: f64) {
        if self.pipeline.is_null() {
            return;
        }
        let rate = if rate == 0.0 { 1.0 } else { rate };
        unsafe {
            let gst = &*self.gst;
            // Query current position to seek in-place with new rate
            let mut pos_ns: i64 = 0;
            (gst.gst_element_query_position)(self.pipeline, GST_FORMAT_TIME, &mut pos_ns);
            if pos_ns < 0 {
                pos_ns = 0;
            }
            (gst.gst_element_seek)(
                self.pipeline,
                rate,
                GST_FORMAT_TIME,
                GST_SEEK_FLAG_FLUSH | GST_SEEK_FLAG_ACCURATE,
                GST_SEEK_TYPE_SET,
                pos_ns,
                GST_SEEK_TYPE_NONE,
                -1,
            );
        }
    }

    /// Returns seekable time ranges as (start_secs, end_secs) pairs.
    pub fn seekable_ranges(&self) -> Vec<(f64, f64)> {
        if self.pipeline.is_null() || self.duration_ns <= 0 {
            return vec![];
        }
        let gst = unsafe { &*self.gst };
        let is_seekable = unsafe { self.query_is_seekable(gst) };
        if is_seekable {
            let end = self.duration_ns as f64 / 1_000_000_000.0;
            vec![(0.0, end)]
        } else {
            vec![]
        }
    }

    /// Returns buffered time ranges as (start_secs, end_secs) pairs.
    pub fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        if self.pipeline.is_null() {
            return vec![];
        }
        let gst = unsafe { &*self.gst };
        unsafe {
            let query = (gst.gst_query_new_buffering)(GST_FORMAT_TIME);
            if query.is_null() {
                return vec![];
            }
            let ok = (gst.gst_element_query)(self.pipeline, query);
            if ok == 0 {
                (gst.gst_mini_object_unref)(query as *mut GstMiniObject);
                // Fallback: assume buffered to current position
                let pos = self.current_position_ms() as f64 / 1000.0;
                return if pos > 0.0 { vec![(0.0, pos)] } else { vec![] };
            }
            let n = (gst.gst_query_get_n_buffering_ranges)(query);
            let mut ranges = Vec::with_capacity(n as usize);
            for i in 0..n {
                let mut start: i64 = 0;
                let mut stop: i64 = 0;
                let ok = (gst.gst_query_parse_nth_buffering_range)(query, i, &mut start, &mut stop);
                if ok != 0 && start >= 0 && stop > start {
                    let start_s = start as f64 / 1_000_000_000.0;
                    let stop_s = stop as f64 / 1_000_000_000.0;
                    ranges.push((start_s, stop_s));
                }
            }
            (gst.gst_mini_object_unref)(query as *mut GstMiniObject);
            ranges
        }
    }

    pub fn current_position_ms(&self) -> u128 {
        if self.pipeline.is_null() {
            return 0;
        }
        unsafe {
            let mut position_ns: i64 = 0;
            if ((*self.gst).gst_element_query_position)(
                self.pipeline,
                GST_FORMAT_TIME,
                &mut position_ns,
            ) != 0
                && position_ns >= 0
            {
                (position_ns / 1_000_000) as u128
            } else {
                0
            }
        }
    }

    pub fn cleanup(&mut self) {
        // destroy_pipeline clears retained GL/DMA samples and EGL images first.
        self.destroy_pipeline();
        if let Some(mut share) = self.gl_share.take() {
            unsafe {
                share.release(&*self.gst);
            }
        }
        if let Some(path) = self.temp_file_path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Drop for GStreamerVideoPlayer {
    fn drop(&mut self) {
        self.cleanup();
    }
}
