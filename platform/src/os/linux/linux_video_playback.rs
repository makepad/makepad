//! GStreamer-based video player for Linux desktop (X11/Wayland).
//!
//! Uses `playbin3`/`playbin` + `appsink` to decode video. Presentation paths:
//! 1. **DMA-Buf** — Intel: `vapostproc` → RGBA EGLImage; NVIDIA: `vah*dec` → NV12 DMA-Buf
//! 2. **System I420 / RGBA** CPU upload fallback
//! 3. **GLMemory** (`glupload ! glcolorconvert`) only when `MAKEPAD_GST_GLMEMORY` is set —
//!    shared-EGL glupload often hangs or blacks out on desktop Intel/NVIDIA stacks

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
        os::{
            fd::RawFd,
            raw::{c_int, c_uint},
        },
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
    /// Optional EXTERNAL_OES plane textures for DMA-Buf NV12 zero-copy.
    pub tex_y_oes_id: Option<TextureId>,
    pub tex_u_oes_id: Option<TextureId>,
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
            Self::GlMemoryRgba => {
                if std::env::var_os("MAKEPAD_GST_NO_DMABUF").is_some() {
                    Some(Self::SystemI420)
                } else {
                    Some(Self::DmaBuf)
                }
            }
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
        // Opt into experimental GLMemory with MAKEPAD_GST_GLMEMORY.
        // Disable DMA with MAKEPAD_GST_NO_DMABUF.
        let force_gl = std::env::var_os("MAKEPAD_GST_GLMEMORY").is_some();
        let no_gl = std::env::var_os("MAKEPAD_GST_NO_GLMEMORY").is_some();
        let no_dmabuf = std::env::var_os("MAKEPAD_GST_NO_DMABUF").is_some();

        // Network / HLS / DASH manifests: start on system memory. Failed zero-copy
        // probes force a full pipeline rebuild and re-fetch (often 2–4s each).
        if prefer_system_memory && !force_gl {
            crate::log!(
                "VIDEO: adaptive stream — starting with system I420 (skip GL/DMA probes)"
            );
            return Self::SystemI420;
        }

        // Prefer DMA-Buf (VA-API) over GLMemory: shared-EGL glupload frequently
        // reaches PAUSED with black frames / stuck PLAYING on desktop Intel GLES.
        if !no_dmabuf && gst.has_dmabuf_support() {
            return Self::DmaBuf;
        }

        let can_gl = force_gl && !no_gl && has_gl_share && gst.has_gl_share_support();
        if can_gl {
            crate::log!("VIDEO: MAKEPAD_GST_GLMEMORY set — trying GLMemory zero-copy first");
            return Self::GlMemoryRgba2D;
        }
        if force_gl && !has_gl_share {
            crate::log!(
                "VIDEO: MAKEPAD_GST_GLMEMORY set but Makepad EGL context is not shared; using system memory"
            );
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
    /// Frames presented on GLMemory path (for delayed pixel probe).
    gl_memory_frame: u32,
    /// True when YUV planes are sampled as EXTERNAL_OES (DMA-Buf NV12 zero-copy).
    yuv_external_oes: bool,
    /// After plane EXTERNAL bind fails, skip DMA-Buf present and fall back.
    nv12_skip_egl_tex2d: bool,
    /// Current caps profile used to build the GStreamer pipeline.
    caps_profile: VideoCapsProfile,
    /// True when the pipeline was created as `playbin3` (streams-aware).
    playbin3: bool,
    /// True when volume/mute/track GObject properties live on `playbin`.
    uses_playbin: bool,
    /// Current YUV matrix selector for shader path (0.0 = BT.709).
    yuv_matrix: f32,
    /// Last retained present sample. Retaining it keeps the texture / DMA-Buf alive.
    retained_gl_sample: *mut GstSample,
    /// Previous present sample (one frame behind). Keeps the on-screen texture out
    /// of the GStreamer pool for an extra frame so 4K reuse cannot tear/ghost.
    retained_gl_sample_prev: *mut GstSample,
    /// Last GLMemory frame was `TEXTURE_2D` (vs `EXTERNAL_OES`).
    gl_memory_tex_2d: bool,
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
    /// True when the top-level element is `playbin`/`playbin3` (not a custom parse pipeline).
    uses_playbin: bool,
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

/// Minimal view of GStreamer `GstVideoMeta` for stride/offset reads (64-bit).
///
/// Layout matches `GstMeta` + `GstVideoMeta` fields up through `stride[]`:
/// `flags(u32)+pad + info* + buffer* + flags/format/id + w/h/n_planes + pad + offset[4] + stride[4]`.
#[repr(C)]
struct GstVideoMetaView {
    meta_flags: u32,
    _pad0: u32,
    info: *mut c_void,
    buffer: *mut c_void,
    flags: i32,
    format: i32,
    id: i32,
    width: u32,
    height: u32,
    n_planes: u32,
    _pad_align_gsize: u32,
    offset: [usize; 4],
    stride: [i32; 4],
}

#[derive(Clone, Copy, Debug)]
struct VideoMetaLayout {
    width: u32,
    height: u32,
    n_planes: u32,
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
    meta: &VideoMetaLayout,
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

        // Adaptive manifests (remote or local .m3u8/.mpd) should skip GL/DMA
        // probes — failed zero-copy caps force a full pipeline rebuild and re-fetch.
        let prefer_system_memory = source.is_adaptive_manifest();
        let can_share_egl = opengl_cx.is_some();
        let caps_profile = VideoCapsProfile::initial(
            audio_only,
            gst,
            can_share_egl,
            prefer_system_memory,
        );
        crate::log!(
            "VIDEO: starting caps profile {:?} uri={}",
            caps_profile,
            uri
        );

        // Only share Makepad EGL with GStreamer for GLMemory. Creating a GstGL
        // context for DMA-Buf / VA-API paths can stall playbin preroll on Wayland.
        let mut gl_share = None;
        if !audio_only && caps_profile.is_gl_memory() {
            if let Some(cx) = opengl_cx {
                gl_share = GstGlShare::try_new(gst, cx);
                if gl_share.is_some() {
                    crate::log!("VIDEO: shared Makepad EGL context with GStreamer");
                }
            }
        }

        let mut caps_profile = caps_profile;
        let built = match Self::build_pipeline(
            gst,
            video_id,
            &uri,
            audio_only,
            caps_profile,
            if caps_profile.is_gl_memory() {
                gl_share.as_ref()
            } else {
                None
            },
        ) {
            Some(built) => built,
            None => {
                // Zero-copy probe can fail to construct; retry once on system memory
                // so local playback still works (HW decode may still win inside playbin).
                if !audio_only && (caps_profile.is_dmabuf() || caps_profile.is_gl_memory()) {
                    crate::log!(
                        "VIDEO: {:?} pipeline build failed; retrying SystemI420",
                        caps_profile
                    );
                    caps_profile = VideoCapsProfile::SystemI420;
                    if let Some(mut share) = gl_share.take() {
                        share.release(gst);
                    }
                    match Self::build_pipeline(
                        gst,
                        video_id,
                        &uri,
                        audio_only,
                        caps_profile,
                        None,
                    ) {
                        Some(built) => built,
                        None => {
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
                        }
                    }
                } else {
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
                }
            }
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
            gl_memory_frame: 0,
            nv12_skip_egl_tex2d: false,
            yuv_external_oes: false,
            caps_profile,
            playbin3: built.playbin3,
            uses_playbin: built.uses_playbin,
            yuv_matrix: 0.0,
            retained_gl_sample: std::ptr::null_mut(),
            retained_gl_sample_prev: std::ptr::null_mut(),
            gl_memory_tex_2d: false,
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

    /// Prefer `playbin3` for adaptive streams; classic `playbin` + `decodebin`
    /// negotiates VA DMA-Buf / GLMemory sinks more reliably for local progressive files.
    fn make_playbin(gst: &LibGStreamer, caps_profile: VideoCapsProfile, uri: &str) -> (*mut GstElement, bool) {
        let local = uri.starts_with("file://") || uri.starts_with("file:");
        let prefer_classic =
            local && (caps_profile.is_dmabuf() || caps_profile.is_gl_memory());
        unsafe {
            let order: [(&str, bool); 2] = if prefer_classic {
                [("playbin", false), ("playbin3", true)]
            } else {
                [("playbin3", true), ("playbin", false)]
            };
            for (name, is_playbin3) in order {
                let playbin_name = CString::new(name).unwrap();
                let pipeline =
                    (gst.gst_element_factory_make)(playbin_name.as_ptr(), std::ptr::null());
                if !pipeline.is_null() {
                    if prefer_classic && name == "playbin" {
                        crate::log!(
                            "VIDEO: using playbin for local {:?} playback",
                            caps_profile
                        );
                    } else if name == "playbin3" {
                        crate::log!("VIDEO: using playbin3");
                    }
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

    /// Build GLMemory video sink via `gst_parse_bin_from_description`.
    ///
    /// Matches the working gst-launch/Python shape. Manual ghost-pad bins were
    /// less reliable with playbin's playsink wrapping.
    ///
    /// `videoconvert` downloads decoder output to system memory before
    /// `glupload`. Direct DMA-Buf→GL import against a wrapped Makepad EGL
    /// share group hangs on NVIDIA (`DirectDmabufExternal`).
    fn make_gl_video_sink_bin(
        gst: &LibGStreamer,
        video_id: LiveId,
        caps_profile: VideoCapsProfile,
        gl_share: Option<&GstGlShare>,
    ) -> Option<(*mut GstElement, *mut GstElement)> {
        let Some(parse_bin) = gst.gst_parse_bin_from_description else {
            error!(
                "gst_parse_bin_from_description unavailable for GLMemory video {:?}",
                video_id
            );
            return None;
        };
        let Some(get_by_name) = gst.gst_bin_get_by_name else {
            return None;
        };
        let caps_text = caps_profile.caps_text();
        // ghost=true: parse_bin creates a sink ghost pad on the first element.
        //
        // `videoconvert ! RGBA ! glupload` (no glcolorconvert): on NVIDIA with a
        // shared EGL share-group, `glcolorconvert`'s FBO path often leaves textures
        // stuck at clear-green (0,255,0,255) even after sync. Uploading system RGBA
        // via glupload still yields a share-group TEXTURE_2D we can sample zero-copy.
        // appsink sync=false for negotiate; enable clock sync after prepare.
        // max-buffers=4: we retain current+previous samples for tear-free present.
        let desc = format!(
            "videoconvert ! video/x-raw,format=RGBA ! glupload name=glupload ! \
             appsink name=videosink caps=\"{caps}\" max-buffers=4 drop=true sync=false qos=true",
            caps = caps_text
        );
        let desc_c = CString::new(desc).unwrap();
        unsafe {
            let mut error: *mut GError = std::ptr::null_mut();
            let bin = parse_bin(desc_c.as_ptr(), 1, &mut error);
            if !error.is_null() {
                let msg = if !(*error).message.is_null() {
                    CStr::from_ptr((*error).message)
                        .to_string_lossy()
                        .into_owned()
                } else {
                    "unknown".into()
                };
                crate::log!("VIDEO: parse GL sink bin failed: {}", msg);
                (gst.g_error_free)(error);
            }
            if bin.is_null() {
                return None;
            }
            let appsink_name = CString::new("videosink").unwrap();
            let appsink = get_by_name(bin, appsink_name.as_ptr());
            if appsink.is_null() {
                (gst.gst_object_unref)(bin as *mut c_void);
                error!("GL sink bin missing appsink for video {:?}", video_id);
                return None;
            }
            // get_by_name returns a ref; keep it for the player.
            if let Some(share) = gl_share {
                share.apply_to_element(gst, bin);
                let glupload_name = CString::new("glupload").unwrap();
                let glupload = get_by_name(bin, glupload_name.as_ptr());
                if !glupload.is_null() {
                    share.apply_to_element(gst, glupload);
                    (gst.gst_object_unref)(glupload as *mut c_void);
                }
            }
            crate::log!(
                "VIDEO: video sink = bin(videoconvert ! RGBA ! glupload ! appsink) caps={}",
                caps_text
            );
            Some((bin, appsink))
        }
    }

    /// Build `vapostproc ! appsink` (or `vaapipostproc`) so playbin can negotiate
    /// DMA-Buf. Bare appsink + DMABuf caps fails: decodebin will not insert the
    /// VA postproc (rank `none`) on its own.
    fn make_dmabuf_video_sink_bin(
        gst: &LibGStreamer,
        video_id: LiveId,
        _caps_profile: VideoCapsProfile,
    ) -> Option<(*mut GstElement, *mut GstElement)> {
        unsafe {
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
                return None;
            };

            let appsink_type = CString::new("appsink").unwrap();
            let appsink_name = CString::new("videosink").unwrap();
            let appsink =
                (gst.gst_element_factory_make)(appsink_type.as_ptr(), appsink_name.as_ptr());
            if appsink.is_null() {
                error!("Failed to create appsink for DMA-Buf sink {:?}", video_id);
                return None;
            }

            let mut postproc: *mut GstElement = std::ptr::null_mut();
            let mut postproc_name = "";
            // Only modern `vapostproc` is trusted for DMA-Buf export. Legacy
            // `vaapipostproc` (nvidia-vaapi) cannot negotiate `memory:DMABuf`.
            if gst.has_modern_va_postproc() {
                let ty = CString::new("vapostproc").unwrap();
                let el = (gst.gst_element_factory_make)(ty.as_ptr(), std::ptr::null());
                if !el.is_null() {
                    postproc = el;
                    postproc_name = "vapostproc";
                }
            }

            // Caps: RGBA when postproc can convert; otherwise NV12 from decoder-direct DMA-Buf.
            let caps_text = if postproc.is_null() {
                "video/x-raw(memory:DMABuf),format=NV12"
            } else {
                "video/x-raw(memory:DMABuf),format=RGBA"
            };
            let caps_str = CString::new(caps_text).unwrap();
            let caps = (gst.gst_caps_from_string)(caps_str.as_ptr());
            if !caps.is_null() {
                (gst.gst_app_sink_set_caps)(appsink, caps);
                (gst.gst_caps_unref)(caps);
            }
            let max_buffers_prop = CString::new("max-buffers").unwrap();
            (gst.g_object_set_int)(appsink, max_buffers_prop.as_ptr(), 4, std::ptr::null());
            let drop_prop = CString::new("drop").unwrap();
            (gst.g_object_set_int)(appsink, drop_prop.as_ptr(), 1, std::ptr::null());
            let sync_prop = CString::new("sync").unwrap();
            // Clock-sync so DMA-Buf NV12 zero-copy stays locked to audio (see GL path).
            (gst.g_object_set_int)(appsink, sync_prop.as_ptr(), 1, std::ptr::null());
            let qos_prop = CString::new("qos").unwrap();
            (gst.g_object_set_int)(appsink, qos_prop.as_ptr(), 1, std::ptr::null());

            if postproc.is_null() {
                // Give playbin the appsink directly — wrapping in a ghost-pad bin is
                // unnecessary and has been a source of DMA-Buf negotiation flakes.
                crate::log!(
                    "VIDEO: video sink = appsink caps={} (no vapostproc)",
                    caps_text
                );
                return Some((appsink, appsink));
            }

            if link(postproc, appsink) == 0 {
                (gst.gst_object_unref)(postproc as *mut c_void);
                (gst.gst_object_unref)(appsink as *mut c_void);
                error!("Failed to link DMA-Buf video sink bin for video {:?}", video_id);
                return None;
            }

            let bin_name = CString::new("makepad-dmabuf-video-sink").unwrap();
            let bin = bin_new(bin_name.as_ptr());
            if bin.is_null() {
                (gst.gst_object_unref)(postproc as *mut c_void);
                (gst.gst_object_unref)(appsink as *mut c_void);
                return None;
            }

            bin_add(bin, postproc);
            bin_add(bin, appsink);
            (gst.gst_object_ref)(appsink as *mut c_void);

            let sink_pad_name = CString::new("sink").unwrap();
            let target = static_pad(postproc, sink_pad_name.as_ptr());
            let ghost = if target.is_null() {
                std::ptr::null_mut()
            } else {
                let ghost = ghost_new(sink_pad_name.as_ptr(), target);
                (gst.gst_object_unref)(target as *mut c_void);
                ghost
            };
            if ghost.is_null() {
                (gst.gst_object_unref)(appsink as *mut c_void);
                (gst.gst_object_unref)(bin as *mut c_void);
                error!(
                    "Failed to ghost-pad DMA-Buf video sink bin for video {:?}",
                    video_id
                );
                return None;
            }
            add_pad(bin, ghost);

            crate::log!(
                "VIDEO: video sink = bin({} ! appsink) caps={}",
                postproc_name,
                caps_text
            );
            Some((bin, appsink))
        }
    }

    fn drain_gl_need_context(&self, gst: &LibGStreamer) {
        if self.bus.is_null() {
            return;
        }
        if let Some(share) = self.gl_share.as_ref() {
            unsafe {
                loop {
                    let msg =
                        (gst.gst_bus_pop_filtered)(self.bus, GST_MESSAGE_NEED_CONTEXT);
                    if msg.is_null() {
                        break;
                    }
                    share.handle_need_context_message(gst, msg, self.pipeline);
                    (gst.gst_mini_object_unref)(msg as *mut GstMiniObject);
                }
            }
        }
    }

    /// Block until playbin reaches PAUSED or the deadline passes. Without this,
    /// `gst_element_get_state(..., 0)` polling from the UI thread never completes
    /// async preroll when no GLib main loop is running.
    fn wait_for_zero_copy_preroll(
        gst: &LibGStreamer,
        pipeline: *mut GstElement,
        bus: *mut GstBus,
        gl_share: Option<&GstGlShare>,
        caps_profile: VideoCapsProfile,
    ) {
        Self::wait_for_zero_copy_preroll_deadline(
            gst,
            pipeline,
            bus,
            gl_share,
            caps_profile,
            Duration::from_secs(8),
        );
    }

    fn wait_for_zero_copy_preroll_deadline(
        gst: &LibGStreamer,
        pipeline: *mut GstElement,
        bus: *mut GstBus,
        gl_share: Option<&GstGlShare>,
        caps_profile: VideoCapsProfile,
        max_wait: Duration,
    ) {
        if !caps_profile.is_dmabuf() && !caps_profile.is_gl_memory() {
            return;
        }
        let deadline = Instant::now() + max_wait;
        if caps_profile.is_gl_memory() {
            if let Some(share) = gl_share {
                share.release_makepad_current();
            }
        }
        unsafe {
            // One long wait (matches gst-launch / Python get_state). Short
            // 100ms polls + main-context pumping can stall GstGL's thread while
            // it tries to activate a shared EGL context on NVIDIA.
            let timeout_ns = max_wait.as_nanos().min(u64::MAX as u128) as u64;
            let mut state: u32 = 0;
            let mut pending: u32 = 0;
            let mut ret = (gst.gst_element_get_state)(
                pipeline,
                &mut state,
                &mut pending,
                timeout_ns,
            );
            // Still answer NEED_CONTEXT if we timed out mid-negotiate.
            let mut spins = 0;
            while (state < GST_STATE_PAUSED || pending != 0)
                && ret != GST_STATE_CHANGE_FAILURE
                && Instant::now() < deadline
                && spins < 50
            {
                spins += 1;
                gst.pump_default_main_context();
                Self::drain_gl_need_context_on_bus(gst, bus, pipeline, gl_share);
                ret = (gst.gst_element_get_state)(
                    pipeline,
                    &mut state,
                    &mut pending,
                    200_000_000,
                );
            }
            if state < GST_STATE_PAUSED || pending != 0 {
                crate::log!(
                    "VIDEO: zero-copy preroll wait ended state={} pending={} ret={}",
                    state,
                    pending,
                    ret
                );
            }
        }
        if caps_profile.is_gl_memory() {
            if let Some(share) = gl_share {
                share.restore_makepad_current();
            }
        }
    }

    fn drain_gl_need_context_on_bus(
        gst: &LibGStreamer,
        bus: *mut GstBus,
        pipeline: *mut GstElement,
        gl_share: Option<&GstGlShare>,
    ) {
        if bus.is_null() {
            return;
        }
        if let Some(share) = gl_share {
            unsafe {
                loop {
                    let msg =
                        (gst.gst_bus_pop_filtered)(bus, GST_MESSAGE_NEED_CONTEXT);
                    if msg.is_null() {
                        break;
                    }
                    share.handle_need_context_message(gst, msg, pipeline);
                    (gst.gst_mini_object_unref)(msg as *mut GstMiniObject);
                }
            }
        }
    }

    fn build_dmabuf_uri_parse_pipeline(
        gst: &LibGStreamer,
        video_id: LiveId,
        uri: &str,
    ) -> Option<BuiltPipeline> {
        if gst.gst_parse_launch.is_none() || gst.gst_bin_get_by_name.is_none() {
            crate::log!("VIDEO: gst_parse_launch/gst_bin_get_by_name unavailable");
            return None;
        }

        // Intel/Mesa: vapostproc can convert to DMA-Buf RGBA for single-plane OES.
        if gst.has_modern_va_postproc() {
            if let Some(built) = Self::build_dmabuf_parse_desc(
                gst,
                video_id,
                &format!(
                    "uridecodebin uri=\"{uri}\" name=dec \
                     dec. ! queue max-size-buffers=2 leaky=downstream ! vapostproc ! \
                     video/x-raw(memory:DMABuf),format=RGBA ! appsink name=videosink sync=true drop=true max-buffers=2 qos=true \
                     dec. ! queue ! audioconvert ! audioresample ! volume name=makepad-volume ! \
                     pulsesink buffer-time=20000000 latency-time=10000000"
                ),
                "uridecodebin ! vapostproc ! DMA-Buf RGBA",
            ) {
                return Some(built);
            }
            crate::log!("VIDEO: vapostproc DMA-Buf RGBA path failed; trying decoder-direct NV12");
        }

        // NVIDIA / no modern vapostproc: decoder-direct qtdemux parse is flaky with
        // pulsesink (often sticks at READY). Classic playbin + DMA-Buf NV12 appsink
        // negotiates `vah*dec` reliably once ranks are bumped — defer to that path.
        if let Some(path) = Self::file_uri_to_path(uri) {
            crate::log!(
                "VIDEO: no vapostproc — deferring DMA-Buf to playbin (vah*dec NV12) for {}",
                path
            );
        } else {
            crate::log!("VIDEO: no vapostproc — deferring DMA-Buf to playbin (could not decode file URI)");
        }
        None
    }

    fn build_dmabuf_parse_desc(
        gst: &LibGStreamer,
        video_id: LiveId,
        desc: &str,
        log_label: &str,
    ) -> Option<BuiltPipeline> {
        let (Some(parse_launch), Some(get_by_name)) =
            (gst.gst_parse_launch, gst.gst_bin_get_by_name)
        else {
            return None;
        };
        unsafe {
            let desc_c = match CString::new(desc) {
                Ok(c) => c,
                Err(_) => {
                    crate::log!(
                        "VIDEO: DMA-Buf parse desc has interior NUL ({}) for {:?}",
                        log_label,
                        video_id
                    );
                    return None;
                }
            };
            let mut error: *mut GError = std::ptr::null_mut();
            let pipeline = parse_launch(desc_c.as_ptr(), &mut error);
            if pipeline.is_null() {
                if !error.is_null() {
                    let msg_ptr = (*error).message;
                    let err_str = if !msg_ptr.is_null() {
                        CStr::from_ptr(msg_ptr).to_string_lossy().to_string()
                    } else {
                        "gst_parse_launch failed".to_string()
                    };
                    (gst.g_error_free)(error);
                    crate::log!(
                        "VIDEO: DMA-Buf parse failed ({}) for {:?}: {}",
                        log_label,
                        video_id,
                        err_str
                    );
                }
                return None;
            }
            if !error.is_null() {
                (gst.g_error_free)(error);
            }

            let appsink_name = CString::new("videosink").unwrap();
            let video_sink = get_by_name(pipeline, appsink_name.as_ptr());
            if video_sink.is_null() {
                (gst.gst_object_unref)(pipeline as *mut c_void);
                crate::log!(
                    "VIDEO: DMA-Buf parse missing videosink ({}) for {:?}",
                    log_label,
                    video_id
                );
                return None;
            }
            (gst.gst_object_ref)(video_sink as *mut c_void);

            let vol_name = CString::new("makepad-volume").unwrap();
            let audio_volume = get_by_name(pipeline, vol_name.as_ptr());
            let audio_volume = if audio_volume.is_null() {
                std::ptr::null_mut()
            } else {
                (gst.gst_object_ref)(audio_volume as *mut c_void);
                audio_volume
            };

            crate::log!("VIDEO: pipeline = {} (local file)", log_label);

            let bus = (gst.gst_element_get_bus)(pipeline);
            (gst.gst_element_set_state)(pipeline, GST_STATE_PAUSED);
            // Probe attempts should fail fast; a stuck READY must not burn 8s × N codecs.
            Self::wait_for_zero_copy_preroll_deadline(
                gst,
                pipeline,
                bus,
                None,
                VideoCapsProfile::DmaBuf,
                Duration::from_millis(2500),
            );

            let mut state: u32 = 0;
            let mut pending: u32 = 0;
            let ret = (gst.gst_element_get_state)(pipeline, &mut state, &mut pending, 0);
            if state < GST_STATE_PAUSED || ret == GST_STATE_CHANGE_FAILURE {
                crate::log!(
                    "VIDEO: DMA-Buf preroll incomplete ({}) state={} pending={} ret={} — discarding",
                    log_label,
                    state,
                    pending,
                    ret
                );
                (gst.gst_element_set_state)(pipeline, GST_STATE_NULL);
                if !audio_volume.is_null() {
                    (gst.gst_object_unref)(audio_volume as *mut c_void);
                }
                (gst.gst_object_unref)(video_sink as *mut c_void);
                if !bus.is_null() {
                    (gst.gst_object_unref)(bus as *mut c_void);
                }
                (gst.gst_object_unref)(pipeline as *mut c_void);
                return None;
            }

            Some(BuiltPipeline {
                pipeline,
                video_sink,
                audio_sink: std::ptr::null_mut(),
                audio_volume,
                bus,
                playbin3: false,
                uses_playbin: false,
            })
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
        super::gst_gl_share::bump_va_decoder_ranks(gst);
        if caps_profile.is_dmabuf()
            && !audio_only
            && (uri.starts_with("file://") || uri.starts_with("file:"))
        {
            if let Some(built) = Self::build_dmabuf_uri_parse_pipeline(gst, video_id, uri) {
                return Some(built);
            }
            crate::log!("VIDEO: parse DMA-Buf path unavailable; trying playbin");
        }
        unsafe {
            let (pipeline, playbin3) = Self::make_playbin(gst, caps_profile, uri);
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
                let (video_sink_bin, appsink) = if caps_profile.is_gl_memory() {
                    match Self::make_gl_video_sink_bin(gst, video_id, caps_profile, gl_share) {
                        Some(pair) => pair,
                        None => {
                            (gst.gst_object_unref)(pipeline as *mut c_void);
                            return None;
                        }
                    }
                } else if caps_profile.is_dmabuf() {
                    match Self::make_dmabuf_video_sink_bin(gst, video_id, caps_profile) {
                        Some(pair) => pair,
                        None => {
                            (gst.gst_object_unref)(pipeline as *mut c_void);
                            return None;
                        }
                    }
                } else {
                    let appsink_type = CString::new("appsink").unwrap();
                    let appsink_name = CString::new("videosink").unwrap();
                    let appsink = (gst.gst_element_factory_make)(
                        appsink_type.as_ptr(),
                        appsink_name.as_ptr(),
                    );
                    if appsink.is_null() {
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
                        (gst.gst_app_sink_set_caps)(appsink, caps);
                        (gst.gst_caps_unref)(caps);
                    }

                    let max_buffers_prop = CString::new("max-buffers").unwrap();
                    (gst.g_object_set_int)(
                        appsink,
                        max_buffers_prop.as_ptr(),
                        2,
                        std::ptr::null(),
                    );
                    let drop_prop = CString::new("drop").unwrap();
                    (gst.g_object_set_int)(appsink, drop_prop.as_ptr(), 1, std::ptr::null());
                    let sync_prop = CString::new("sync").unwrap();
                    (gst.g_object_set_int)(appsink, sync_prop.as_ptr(), 1, std::ptr::null());
                    let qos_prop = CString::new("qos").unwrap();
                    (gst.g_object_set_int)(appsink, qos_prop.as_ptr(), 1, std::ptr::null());

                    (appsink, appsink)
                };

                let video_sink_prop = CString::new("video-sink").unwrap();
                (gst.g_object_set_ptr)(
                    pipeline,
                    video_sink_prop.as_ptr(),
                    video_sink_bin as *mut c_void,
                    std::ptr::null(),
                );
                appsink
            };

            let bus = (gst.gst_element_get_bus)(pipeline);
            if caps_profile.is_gl_memory() {
                if let Some(share) = gl_share {
                    share.release_makepad_current();
                }
            }
            (gst.gst_element_set_state)(pipeline, GST_STATE_PAUSED);
            Self::drain_gl_need_context_on_bus(gst, bus, pipeline, gl_share);
            Self::wait_for_zero_copy_preroll(gst, pipeline, bus, gl_share, caps_profile);
            if caps_profile.is_gl_memory() {
                if let Some(share) = gl_share {
                    share.restore_makepad_current();
                }
            }

            if caps_profile.is_dmabuf() || caps_profile.is_gl_memory() {
                let mut state: u32 = 0;
                let mut pending: u32 = 0;
                let ret = (gst.gst_element_get_state)(pipeline, &mut state, &mut pending, 0);
                if state < GST_STATE_PAUSED || ret == GST_STATE_CHANGE_FAILURE {
                    crate::log!(
                        "VIDEO: playbin zero-copy preroll incomplete state={} pending={} ret={}",
                        state,
                        pending,
                        ret
                    );
                    (gst.gst_element_set_state)(pipeline, GST_STATE_NULL);
                    if !audio_volume.is_null() {
                        (gst.gst_object_unref)(audio_volume as *mut c_void);
                    }
                    if !audio_sink_element.is_null() {
                        (gst.gst_object_unref)(audio_sink_element as *mut c_void);
                    }
                    if !bus.is_null() {
                        (gst.gst_object_unref)(bus as *mut c_void);
                    }
                    (gst.gst_object_unref)(pipeline as *mut c_void);
                    return None;
                }
                if caps_profile.is_gl_memory() {
                    crate::log!(
                        "VIDEO: GLMemory build reached PAUSED state={} ret={}",
                        state,
                        ret
                    );
                }
            }

            Some(BuiltPipeline {
                pipeline,
                video_sink,
                audio_sink: audio_sink_element,
                audio_volume,
                bus,
                playbin3,
                uses_playbin: true,
            })
        }
    }

    fn destroy_pipeline(&mut self) {
        // Drop present-side keep-alives before tearing down the pipeline so we
        // never sample a GL/DMA texture whose GstBuffer is already gone.
        if !self.retained_gl_sample.is_null() || !self.retained_gl_sample_prev.is_null() {
            unsafe {
                let gst = &*self.gst;
                if !self.retained_gl_sample_prev.is_null() {
                    (gst.gst_mini_object_unref)(self.retained_gl_sample_prev as *mut GstMiniObject);
                }
                if !self.retained_gl_sample.is_null() {
                    (gst.gst_mini_object_unref)(self.retained_gl_sample as *mut GstMiniObject);
                }
            }
            self.retained_gl_sample_prev = std::ptr::null_mut();
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

    /// Keep current + previous present samples so the on-screen texture cannot be
    /// recycled by GStreamer's pool while the compositor still samples it (4K tear/ghost).
    unsafe fn retain_present_sample(&mut self, gst: &LibGStreamer, sample: *mut GstSample) {
        if !self.retained_gl_sample_prev.is_null() {
            (gst.gst_mini_object_unref)(self.retained_gl_sample_prev as *mut GstMiniObject);
        }
        self.retained_gl_sample_prev = self.retained_gl_sample;
        self.retained_gl_sample = sample;
    }

    unsafe fn clear_retained_present_samples(&mut self, gst: &LibGStreamer) {
        if !self.retained_gl_sample_prev.is_null() {
            (gst.gst_mini_object_unref)(self.retained_gl_sample_prev as *mut GstMiniObject);
            self.retained_gl_sample_prev = std::ptr::null_mut();
        }
        if !self.retained_gl_sample.is_null() {
            (gst.gst_mini_object_unref)(self.retained_gl_sample as *mut GstMiniObject);
            self.retained_gl_sample = std::ptr::null_mut();
        }
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
            gl_memory_frame: 0,
            nv12_skip_egl_tex2d: false,
            yuv_external_oes: false,
            caps_profile: VideoCapsProfile::SystemI420,
            playbin3: false,
            uses_playbin: false,
            yuv_matrix: 0.0,
            retained_gl_sample: std::ptr::null_mut(),
            retained_gl_sample_prev: std::ptr::null_mut(),
            gl_memory_tex_2d: false,
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

    fn file_uri_to_path(uri: &str) -> Option<String> {
        let rest = uri.strip_prefix("file://").or_else(|| uri.strip_prefix("file:"))?;
        // Skip optional authority empty host (`file:///path` → `/path`).
        let path_enc = if rest.starts_with('/') {
            rest
        } else if let Some(idx) = rest.find('/') {
            &rest[idx..]
        } else {
            return None;
        };
        let bytes = path_enc.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'%' if i + 2 < bytes.len() => {
                    let h = |c: u8| -> Option<u8> {
                        match c {
                            b'0'..=b'9' => Some(c - b'0'),
                            b'a'..=b'f' => Some(c - b'a' + 10),
                            b'A'..=b'F' => Some(c - b'A' + 10),
                            _ => None,
                        }
                    };
                    if let (Some(hi), Some(lo)) = (h(bytes[i + 1]), h(bytes[i + 2])) {
                        out.push((hi << 4) | lo);
                        i += 3;
                        continue;
                    }
                    out.push(bytes[i]);
                    i += 1;
                }
                b'+' => {
                    // Prefer literal '+' in paths; space is %20 in our encoder.
                    out.push(b'+');
                    i += 1;
                }
                c => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        String::from_utf8(out).ok()
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

    fn try_rebuild_with_caps_fallback(&mut self, gst: &LibGStreamer, reason: &str) -> bool {
        let Some(mut next_profile) = self.caps_profile.next_fallback() else {
            return false;
        };
        crate::log!(
            "VIDEO: caps {:?} failed ({}); falling back to {:?}",
            self.caps_profile,
            reason,
            next_profile
        );
        if self.caps_profile.is_gl_memory() && next_profile == VideoCapsProfile::SystemI420 {
            crate::log!(
                "VIDEO: GL zero-copy unavailable on this GPU/driver; using CPU I420."
            );
        }
        self.destroy_pipeline();
        while next_profile.is_dmabuf() && !gst.has_hardware_dmabuf_decoder() {
            let Some(skipped) = next_profile.next_fallback() else {
                return false;
            };
            crate::log!(
                "VIDEO: skipping {:?} — no VA-API/NVDEC decoder registered (try: gst-inspect-1.0 vaapih264dec). \
                 gstreamer1.0-vaapi alone is not enough; install a VA driver (e.g. mesa-va-drivers) and verify with vainfo.",
                next_profile
            );
            next_profile = skipped;
        }
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
                    None => return false,
                }
            }
        }
        let gl_for_rebuild = if next_profile.is_gl_memory() {
            self.gl_share.as_ref()
        } else {
            None
        };
        let Some(built) = Self::build_pipeline(
            gst,
            self.video_id,
            &self.source_uri,
            self.audio_only,
            next_profile,
            gl_for_rebuild,
        ) else {
            return false;
        };
        self.pipeline = built.pipeline;
        self.video_sink = built.video_sink;
        self.audio_sink = built.audio_sink;
        self.audio_volume = built.audio_volume;
        self.bus = built.bus;
        self.caps_profile = next_profile;
        self.playbin3 = built.playbin3;
        self.uses_playbin = built.uses_playbin;
        self.prepare_started = Instant::now();
        self.dmabuf_yuv_mode = false;
        self.yuv_biplanar = false;
        self.nv12_skip_egl_tex2d = false;
        self.yuv_external_oes = false;
        self.logged_first_upload = false;
        true
    }

    fn is_caps_fallback_error(err_str: &str, debug_str: &str, profile: VideoCapsProfile) -> bool {
        err_str.contains("not-negotiated")
            || err_str.contains("negotiation")
            || err_str.contains("Failed to upload buffer")
            || debug_str.contains("not-negotiated")
            || debug_str.contains("not negotiated")
            || debug_str.contains("Failed to upload buffer")
            || (debug_str.contains("caps")
                && (debug_str.contains("not") || debug_str.contains("fail")))
            || ((err_str.contains("Internal data stream error")
                || err_str.contains("streaming stopped, reason error"))
                && (profile.is_gl_memory() || profile.is_dmabuf())
                // qtdemux posts this for many root causes; only treat as caps
                // fallback when debug points at negotiation / DMA-Buf, or when
                // there is no debug (probe clearly dead).
                && (debug_str.is_empty()
                    || debug_str.contains("not-negotiated")
                    || debug_str.contains("not negotiated")
                    || debug_str.contains("DMABuf")
                    || debug_str.contains("dmabuf")
                    || debug_str.contains("caps")))
    }

    /// Check if the player has finished prerolling and is ready to play.
    /// Returns `Ok(...)` with metadata when ready, `Err(msg)` on failure, `None` if still loading.
    pub fn check_prepared(&mut self) -> Option<Result<PlaybackPrepared, String>> {
        poll_pending_pipeline_drops();
        if self.prepare_notified || self.pipeline.is_null() {
            return None;
        }

        let gst = unsafe { &*self.gst };

        gst.pump_default_main_context();
        self.drain_gl_need_context(gst);

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

                let negotiation_error = Self::is_caps_fallback_error(
                    &err_str,
                    &debug_str,
                    self.caps_profile,
                );
                if negotiation_error && self.try_rebuild_with_caps_fallback(gst, &err_str) {
                    return None;
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

            // Short wait so async playbin/decodebin state changes can complete
            // without a GLib main loop (timeout 0 returns immediately and can
            // leave zero-copy pipelines stuck in ASYNC forever).
            let mut state: u32 = 0;
            let mut pending: u32 = 0;
            let state_wait_ns: u64 = if self.caps_profile.is_gl_memory()
                || self.caps_profile.is_dmabuf()
            {
                50_000_000
            } else {
                0
            };
            let ret = (gst.gst_element_get_state)(
                self.pipeline,
                &mut state,
                &mut pending,
                state_wait_ns,
            );

            if ret == GST_STATE_CHANGE_FAILURE {
                if (self.caps_profile.is_gl_memory() || self.caps_profile.is_dmabuf())
                    && self.try_rebuild_with_caps_fallback(
                        gst,
                        "GStreamer pipeline failed to reach PAUSED (state-change failure)",
                    )
                {
                    return None;
                }
                self.prepare_notified = true;
                return Some(Err(
                    "GStreamer pipeline failed to reach PAUSED (state-change failure)".into(),
                ));
            }

            // GST_STATE_CHANGE_ASYNC while still below PAUSED: zero-copy paths can
            // stall here without posting ERROR (shared-EGL glupload). Fall back.
            let zero_copy_prepare_timeout = if self.caps_profile.is_dmabuf() {
                Duration::from_secs(5)
            } else {
                Duration::from_secs(2)
            };
            if ret == GST_STATE_CHANGE_ASYNC
                && state < GST_STATE_PAUSED
                && (self.caps_profile.is_gl_memory() || self.caps_profile.is_dmabuf())
                && self.prepare_started.elapsed() >= zero_copy_prepare_timeout
                && self.try_rebuild_with_caps_fallback(
                    gst,
                    "timed out waiting for zero-copy PAUSED",
                )
            {
                return None;
            }

            // Also answer GL NEED_CONTEXT during prepare so GLMemory can negotiate.
            self.drain_gl_need_context(gst);

            // Need at least PAUSED for preroll to be done
            if state < GST_STATE_PAUSED || self.is_prepared {
                return None;
            }

            // Pull the preroll sample to get video dimensions.
            // try_pull_preroll works in PAUSED state (try_pull_sample does NOT).
            if !self.video_sink.is_null() {
                // GLMemory upload may still be finishing on the GstGL thread after
                // the pipeline reports PAUSED — wait briefly for the preroll buffer.
                let pull_ns: u64 = if self.caps_profile.is_gl_memory() {
                    if let Some(share) = self.gl_share.as_ref() {
                        share.release_makepad_current();
                    }
                    200_000_000 // 200ms
                } else {
                    0
                };
                let sample = (gst.gst_app_sink_try_pull_preroll)(self.video_sink, pull_ns);
                if self.caps_profile.is_gl_memory() {
                    if let Some(share) = self.gl_share.as_ref() {
                        share.restore_makepad_current();
                    }
                }
                if !sample.is_null() {
                    self.extract_dims_from_sample(gst, sample);
                    (gst.gst_mini_object_unref)(sample as *mut GstMiniObject);
                }
            }

            // Wait for real dimensions instead of inventing 1920x1080.
            // Zero-copy probes can hang in ASYNC without ERROR (black screen);
            // fail them over to the next caps profile quickly.
            if !self.audio_only && (self.video_width == 0 || self.video_height == 0) {
                let timeout = if self.caps_profile.is_dmabuf() {
                    Duration::from_secs(5)
                } else if self.caps_profile.is_gl_memory() {
                    Duration::from_secs(8)
                } else {
                    Duration::from_secs(15)
                };
                if self.prepare_started.elapsed() >= timeout {
                    if (self.caps_profile.is_gl_memory() || self.caps_profile.is_dmabuf())
                        && self.try_rebuild_with_caps_fallback(
                            gst,
                            "timed out waiting for zero-copy preroll",
                        )
                    {
                        return None;
                    }
                    self.prepare_notified = true;
                    return Some(Err(
                        "Timed out waiting for video dimensions from GStreamer preroll".into(),
                    ));
                }
                return None;
            }

            self.is_prepared = true;
            self.prepare_notified = true;

            // GLMemory negotiate uses appsink sync=false; turn sync on for PLAYING
            // so video tracks the pipeline clock with audio.
            if self.caps_profile.is_gl_memory() && !self.video_sink.is_null() {
                let sync_prop = CString::new("sync").unwrap();
                (gst.g_object_set_int)(self.video_sink, sync_prop.as_ptr(), 1, std::ptr::null());
            }

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

        // `playbin3` has no `n-video` / `n-audio`; use stream-collection labels or fallbacks.
        if !self.uses_playbin || self.playbin3 || self.pipeline.is_null() {
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
        } else if self.uses_playbin {
            unsafe {
                let prop = CString::new("current-video").unwrap();
                (gst.g_object_set_int)(self.pipeline, prop.as_ptr(), index as i32, std::ptr::null());
            }
            self.selected_video_idx = index;
            true
        } else {
            false
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
        } else if self.uses_playbin {
            unsafe {
                let prop = CString::new("current-audio").unwrap();
                (gst.g_object_set_int)(self.pipeline, prop.as_ptr(), index as i32, std::ptr::null());
            }
            self.selected_audio_idx = index;
            true
        } else {
            false
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
        self.drain_gl_need_context(gst);

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
                if self.try_present_gl_memory(gst, gl, textures, opengl_cx, sample, buffer) {
                    return true;
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
                // NVIDIA often rejects DMA-Buf → TEXTURE_2D and cannot CPU-map the
                // buffer either. Drop to SystemI420 once so playback is not stuck
                // with black/garbage frames and per-frame present failures.
                if self.nv12_skip_egl_tex2d {
                    let _ = self.try_rebuild_with_caps_fallback(
                        gst,
                        "DMA-Buf NV12 EGL import failed (EXTERNAL_OES and plane-split)",
                    );
                }
                return false;
            }

            // System-memory fallback path: map buffer and upload I420 or RGBA.
            self.clear_retained_present_samples(gst);
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
                    .as_ref()
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

    /// Adopt a GStreamer GLMemory texture into Makepad's share group (true zero-copy —
    /// no blit, no CPU download of the frame into Makepad).
    ///
    /// `glupload` leaves `NEED_UPLOAD` until the buffer is mapped with `GST_MAP_GL` on
    /// **GStreamer's** GL thread. Mapping on the UI thread asserts; we marshal via
    /// `gst_gl_context_thread_add` on the memory's own context. After map we
    /// `glFinish` (+ optional `GstGLSyncMeta` fence) so Makepad's share-group context
    /// cannot sample a 4K upload mid-write (tear / portrait ghosting).
    unsafe fn try_present_gl_memory(
        &mut self,
        gst: &LibGStreamer,
        gl: &LibGl,
        textures: &mut CxTexturePool,
        opengl_cx: Option<&OpenglCx>,
        sample: *mut GstSample,
        buffer: *mut GstBuffer,
    ) -> bool {
        let (Some(is_gl_memory), Some(get_gl_texture_id)) =
            (gst.gst_is_gl_memory, gst.gst_gl_memory_get_texture_id)
        else {
            return false;
        };
        let Some(thread_add) = gst.gst_gl_context_thread_add else {
            if !self.logged_first_upload {
                crate::log!("VIDEO: gst_gl_context_thread_add unavailable — cannot MAP_GL");
            }
            return false;
        };
        let memory = (gst.gst_buffer_peek_memory)(buffer, 0);
        if memory.is_null() || is_gl_memory(memory) == 0 {
            return false;
        }

        // GstGLBaseMemory.context sits immediately after GstMemory (size 112 here).
        let gst_gl_ctx = {
            const CONTEXT_OFF: usize = 112;
            *((memory as *const u8).add(CONTEXT_OFF) as *const *mut GstGLContext)
        };
        if gst_gl_ctx.is_null() {
            return false;
        }

        struct GlMapJob {
            map_fn: unsafe extern "C" fn(*mut GstBuffer, *mut GstMapInfo, c_uint) -> c_int,
            unmap_fn: unsafe extern "C" fn(*mut GstBuffer, *mut GstMapInfo),
            buffer: *mut GstBuffer,
            info: GstMapInfo,
            mapped: bool,
            gl_finish: Option<unsafe extern "C" fn()>,
            set_sync_point: Option<unsafe extern "C" fn(*mut GstGLSyncMeta, *mut GstGLContext)>,
            get_meta: Option<unsafe extern "C" fn(*mut GstBuffer, GType) -> *mut c_void>,
            sync_api: Option<unsafe extern "C" fn() -> GType>,
            gst_gl_ctx: *mut GstGLContext,
            sync_meta: *mut GstGLSyncMeta,
        }
        unsafe extern "C" fn map_on_gst_gl(_ctx: *mut GstGLContext, data: *mut c_void) {
            let job = &mut *(data as *mut GlMapJob);
            job.mapped =
                (job.map_fn)(job.buffer, &mut job.info, GST_MAP_READ | GST_MAP_GL) != 0;
            if !job.mapped {
                return;
            }
            // Ensure the upload GPU commands complete before any share-group context
            // samples this texture. Without this, 4K frames commonly tear/ghost.
            if let Some(finish) = job.gl_finish {
                finish();
            }
            if let (Some(get_meta), Some(api_type), Some(set_sync)) =
                (job.get_meta, job.sync_api, job.set_sync_point)
            {
                let sync = get_meta(job.buffer, api_type()) as *mut GstGLSyncMeta;
                if !sync.is_null() {
                    set_sync(sync, job.gst_gl_ctx);
                    job.sync_meta = sync;
                }
            }
        }
        unsafe extern "C" fn unmap_on_gst_gl(_ctx: *mut GstGLContext, data: *mut c_void) {
            let job = &mut *(data as *mut GlMapJob);
            if job.mapped {
                (job.unmap_fn)(job.buffer, &mut job.info);
                job.mapped = false;
            }
        }

        let mut job = GlMapJob {
            map_fn: gst.gst_buffer_map,
            unmap_fn: gst.gst_buffer_unmap,
            buffer,
            info: GstMapInfo::default(),
            mapped: false,
            gl_finish: Some(gl.glFinish),
            set_sync_point: gst.gst_gl_sync_meta_set_sync_point,
            get_meta: gst.gst_buffer_get_meta,
            sync_api: gst.gst_gl_sync_meta_api_get_type,
            gst_gl_ctx,
            sync_meta: std::ptr::null_mut(),
        };
        // Release Makepad current so the Gst GL thread can makeCurrent its context.
        if let Some(share) = self.gl_share.as_ref() {
            share.release_makepad_current();
        } else if let Some(cx) = opengl_cx {
            cx.clear_current();
        }
        thread_add(gst_gl_ctx, Some(map_on_gst_gl), &mut job as *mut _ as *mut c_void);
        if !job.mapped {
            if let Some(cx) = opengl_cx {
                cx.make_current();
            }
            if !self.logged_first_upload {
                crate::log!("VIDEO: GLMemory GST_MAP_GL on Gst GL thread failed");
            }
            return false;
        }

        let gl_texture = get_gl_texture_id(memory);
        let tex_target = gst
            .gst_gl_memory_get_texture_target
            .map(|f| f(memory))
            .unwrap_or(GST_GL_TEXTURE_TARGET_2D);
        let tex_format = gst
            .gst_gl_memory_get_texture_format
            .map(|f| f(memory))
            .unwrap_or(0);

        thread_add(gst_gl_ctx, Some(unmap_on_gst_gl), &mut job as *mut _ as *mut c_void);
        if let Some(cx) = opengl_cx {
            cx.make_current();
        }

        // Consumer-side wait: prefer GstGLSyncMeta, then raw glWaitSync if present.
        if !job.sync_meta.is_null() {
            if let Some(wait_cpu) = gst.gst_gl_sync_meta_wait_cpu {
                wait_cpu(job.sync_meta, gst_gl_ctx);
            } else if let Some(wait) = gst.gst_gl_sync_meta_wait {
                wait(job.sync_meta, gst_gl_ctx);
            }
        } else {
            let fence = if let (Some(get_meta), Some(api_type)) =
                (gst.gst_buffer_get_meta, gst.gst_gl_sync_meta_api_get_type)
            {
                let sync = get_meta(buffer, api_type());
                if sync.is_null() {
                    std::ptr::null_mut()
                } else {
                    #[repr(C)]
                    struct GstGLSyncMetaHead {
                        flags: u32,
                        _pad: u32,
                        info: *const c_void,
                        context: *mut c_void,
                        data: *mut c_void,
                    }
                    (*(sync as *const GstGLSyncMetaHead)).data
                }
            } else {
                std::ptr::null_mut()
            };
            if !fence.is_null() && fence as usize > 0x1000 {
                if let Some(wait_sync) = gl.glWaitSync {
                    wait_sync(fence, 0, u64::MAX);
                }
            } else if !self.logged_first_upload {
                crate::log!(
                    "VIDEO: GLMemory sync via glFinish (no GstGLSyncMeta fence={:?})",
                    fence
                );
            }
        }

        if gl_texture == 0 {
            return false;
        }

        let use_tex_2d = tex_target == GST_GL_TEXTURE_TARGET_2D;
        let use_oes = tex_target == GST_GL_TEXTURE_TARGET_EXTERNAL_OES;
        if !use_tex_2d && !use_oes {
            if !self.logged_first_upload {
                crate::log!(
                    "VIDEO: GLMemory unsupported texture-target={} format=0x{:x}",
                    tex_target,
                    tex_format
                );
            }
            return false;
        }

        let cxtexture = &mut textures[self.texture_id];
        if let Some(old) = cxtexture.os.gl_texture {
            if old != gl_texture && cxtexture.os.gl_texture_owned {
                (gl.glDeleteTextures)(1, &old);
            }
        }

        let bind_target = if use_oes {
            gl_sys::TEXTURE_EXTERNAL_OES
        } else {
            gl_sys::TEXTURE_2D
        };
        (gl.glBindTexture)(bind_target, gl_texture);
        // Probe whether the Gst texture is visible in Makepad's share group.
        if !self.logged_first_upload {
            let is_tex = gl
                .glIsTexture
                .map(|f| f(gl_texture) != 0)
                .unwrap_or(false);
            let mut tw: i32 = -1;
            let mut th: i32 = -1;
            if use_tex_2d {
                (gl.glGetTexLevelParameteriv)(bind_target, 0, gl_sys::TEXTURE_WIDTH, &mut tw);
                (gl.glGetTexLevelParameteriv)(bind_target, 0, gl_sys::TEXTURE_HEIGHT, &mut th);
            }
            let err = (gl.glGetError)();
            crate::log!(
                "VIDEO: GLMemory share probe tex={} is_texture={} size={}x{} video={}x{} err=0x{:x}",
                gl_texture,
                is_tex,
                tw,
                th,
                self.video_width,
                self.video_height,
                err
            );
            if use_tex_2d && tw > 0 && th > 0 {
                let mut fbo = 0u32;
                (gl.glGenFramebuffers)(1, &mut fbo);
                (gl.glBindFramebuffer)(gl_sys::FRAMEBUFFER, fbo);
                (gl.glFramebufferTexture2D)(
                    gl_sys::FRAMEBUFFER,
                    gl_sys::COLOR_ATTACHMENT0,
                    bind_target,
                    gl_texture,
                    0,
                );
                let mut px = [0u8; 4];
                (gl.glReadPixels)(
                    tw / 2,
                    th / 2,
                    1,
                    1,
                    gl_sys::RGBA,
                    gl_sys::UNSIGNED_BYTE,
                    px.as_mut_ptr() as *mut _,
                );
                let read_err = (gl.glGetError)();
                crate::log!(
                    "VIDEO: GLMemory center pixel RGBA=({},{},{},{}) read_err=0x{:x}",
                    px[0],
                    px[1],
                    px[2],
                    px[3],
                    read_err
                );
                (gl.glBindFramebuffer)(gl_sys::FRAMEBUFFER, 0);
                (gl.glDeleteFramebuffers)(1, &fbo);
            }
        }
        (gl.glTexParameteri)(
            bind_target,
            gl_sys::TEXTURE_MIN_FILTER,
            gl_sys::LINEAR as i32,
        );
        (gl.glTexParameteri)(
            bind_target,
            gl_sys::TEXTURE_MAG_FILTER,
            gl_sys::LINEAR as i32,
        );
        (gl.glTexParameteri)(
            bind_target,
            gl_sys::TEXTURE_WRAP_S,
            gl_sys::CLAMP_TO_EDGE as i32,
        );
        (gl.glTexParameteri)(
            bind_target,
            gl_sys::TEXTURE_WRAP_T,
            gl_sys::CLAMP_TO_EDGE as i32,
        );
        (gl.glBindTexture)(bind_target, 0);

        cxtexture.os.gl_texture = Some(gl_texture);
        cxtexture.os.gl_texture_owned = false;
        if use_tex_2d {
            cxtexture.format = crate::texture::TextureFormat::VideoGlMemoryRgba;
            cxtexture.alloc = Some(TextureAlloc {
                width: self.video_width as usize,
                height: self.video_height as usize,
                pixel: TexturePixel::VideoGlMemoryRgba,
                category: TextureCategory::Video,
            });
        } else {
            cxtexture.format = crate::texture::TextureFormat::VideoExternal;
            cxtexture.alloc = Some(TextureAlloc {
                width: self.video_width as usize,
                height: self.video_height as usize,
                pixel: TexturePixel::VideoExternal,
                category: TextureCategory::Video,
            });
        }

        self.retain_present_sample(gst, sample);
        self.gl_memory_tex_2d = use_tex_2d;
        self.dmabuf_yuv_mode = false;
        self.yuv_biplanar = false;
        self.yuv_external_oes = false;
        self.gl_memory_frame = self.gl_memory_frame.saturating_add(1);

        if self.gl_memory_frame == 45 && use_tex_2d {
            (gl.glBindTexture)(bind_target, gl_texture);
            let mut tw = 0i32;
            let mut th = 0i32;
            (gl.glGetTexLevelParameteriv)(bind_target, 0, gl_sys::TEXTURE_WIDTH, &mut tw);
            (gl.glGetTexLevelParameteriv)(bind_target, 0, gl_sys::TEXTURE_HEIGHT, &mut th);
            if tw > 0 && th > 0 {
                let mut fbo = 0u32;
                (gl.glGenFramebuffers)(1, &mut fbo);
                (gl.glBindFramebuffer)(gl_sys::FRAMEBUFFER, fbo);
                (gl.glFramebufferTexture2D)(
                    gl_sys::FRAMEBUFFER,
                    gl_sys::COLOR_ATTACHMENT0,
                    bind_target,
                    gl_texture,
                    0,
                );
                let mut px = [0u8; 4];
                (gl.glReadPixels)(
                    tw / 2,
                    th / 2,
                    1,
                    1,
                    gl_sys::RGBA,
                    gl_sys::UNSIGNED_BYTE,
                    px.as_mut_ptr() as *mut _,
                );
                crate::log!(
                    "VIDEO: GLMemory frame45 center pixel RGBA=({},{},{},{})",
                    px[0],
                    px[1],
                    px[2],
                    px[3]
                );
                (gl.glBindFramebuffer)(gl_sys::FRAMEBUFFER, 0);
                (gl.glDeleteFramebuffers)(1, &fbo);
            }
            (gl.glBindTexture)(bind_target, 0);
        }

        if !self.logged_first_upload {
            self.logged_first_upload = true;
            let caps_fmt = {
                let caps = (gst.gst_sample_get_caps)(sample);
                if caps.is_null() {
                    "unknown".into()
                } else {
                    let st = (gst.gst_caps_get_structure)(caps, 0);
                    let format_key = CString::new("format").unwrap();
                    let target_key = CString::new("texture-target").unwrap();
                    let fmt = if st.is_null() {
                        None
                    } else {
                        let p = (gst.gst_structure_get_string)(st, format_key.as_ptr());
                        if p.is_null() {
                            None
                        } else {
                            Some(CStr::from_ptr(p).to_string_lossy().into_owned())
                        }
                    };
                    let tgt = if st.is_null() {
                        None
                    } else {
                        let p = (gst.gst_structure_get_string)(st, target_key.as_ptr());
                        if p.is_null() {
                            None
                        } else {
                            Some(CStr::from_ptr(p).to_string_lossy().into_owned())
                        }
                    };
                    format!(
                        "format={} texture-target={} gl_target={} gl_format=0x{:x}",
                        fmt.as_deref().unwrap_or("?"),
                        tgt.as_deref().unwrap_or("?"),
                        if use_tex_2d { "2D" } else { "EXTERNAL_OES" },
                        tex_format
                    )
                }
            };
            crate::log!(
                "VIDEO: presenting via GLMemory zero-copy ({}) {}",
                if use_tex_2d {
                    "TEXTURE_2D RGBA"
                } else {
                    "EXTERNAL_OES"
                },
                caps_fmt
            );
        }

        true
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
            let (y_plane, uv_plane) =
                Self::nv12_plane_layouts(width, height, fds.len(), fourcc, modifier, meta.as_ref());

            if self.nv12_skip_egl_tex2d {
                return false;
            }

            // Caps often omit drm-format (modifier=0 / LINEAR). NVIDIA NVDEC
            // surfaces are block-linear — probe VA for the real modifier.
            let mut modifier = modifier;
            if modifier == 0 {
                if let Some(probed) = super::va_dmabuf_modifier::probe_nv12_modifier(
                    self.video_width,
                    self.video_height,
                ) {
                    modifier = probed;
                }
            }

            // True zero-copy: Y/UV planes as TEXTURE_EXTERNAL_OES (NVIDIA rejects
            // TEXTURE_2D for these). Video shader samples tex_y_oes/tex_u_oes and
            // converts BT.709 — no GPU blit, no CPU upload.
            let (Some(y_oes_id), Some(uv_oes_id)) = (
                self.yuv_ids.and_then(|y| y.tex_y_oes_id),
                self.yuv_ids.and_then(|y| y.tex_u_oes_id),
            ) else {
                if !self.logged_first_upload {
                    crate::log!("VIDEO: DMA-Buf NV12 OES plane textures not allocated");
                }
                self.nv12_skip_egl_tex2d = true;
                return false;
            };

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
                if !self.logged_first_upload {
                    let err = opengl_cx
                        .libegl
                        .eglGetError
                        .map(|f| unsafe { f() })
                        .unwrap_or(0);
                    crate::log!(
                        "VIDEO: DMA-Buf NV12 plane EGLImages failed (y_null={} uv_null={} egl=0x{:x})",
                        y_image.is_null(),
                        uv_image.is_null(),
                        err,
                    );
                }
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
                self.nv12_skip_egl_tex2d = true;
                return false;
            }

            let y_ok = self.bind_egl_image_to_texture(
                gl,
                textures,
                y_oes_id,
                y_image,
                target_tex,
                self.video_width as usize,
                self.video_height as usize,
                TexturePixel::VideoExternal,
            );
            let uv_ok = y_ok
                && self.bind_egl_image_to_texture(
                    gl,
                    textures,
                    uv_oes_id,
                    uv_image,
                    target_tex,
                    self.video_width.div_ceil(2) as usize,
                    self.video_height.div_ceil(2) as usize,
                    TexturePixel::VideoExternal,
                );
            if !uv_ok {
                if let Some(destroy) = opengl_cx.libegl.eglDestroyImageKHR {
                    destroy(opengl_cx.egl_display, y_image);
                    destroy(opengl_cx.egl_display, uv_image);
                }
                self.nv12_skip_egl_tex2d = true;
                if !self.logged_first_upload {
                    crate::log!(
                        "VIDEO: DMA-Buf NV12 → TEXTURE_EXTERNAL_OES plane bind failed; \
                         falling back to SystemI420"
                    );
                }
                return false;
            }

            self.release_egl_images();
            self.retained_egl_images = vec![y_image, uv_image];
            self.egl_display_for_images = opengl_cx.egl_display;
            self.egl_destroy_image = opengl_cx.libegl.eglDestroyImageKHR;
            self.retain_present_sample(gst, sample);
            self.dmabuf_yuv_mode = true;
            self.yuv_biplanar = true;
            self.yuv_external_oes = true;
            if !self.logged_first_upload {
                self.logged_first_upload = true;
                crate::log!(
                    "VIDEO: presenting via DMA-Buf NV12 zero-copy (EXTERNAL_OES planes) \
                     {}x{} pitch_y={} pitch_uv={} mod=0x{:x}",
                    width,
                    height,
                    y_plane.pitch,
                    uv_plane.pitch,
                    modifier
                );
            }
            return true;
        }

        // Single-plane RGBA/BGRA style DRM fourcc → VideoExternal.
        let plane = Self::rgba_plane_layout(width, height, fourcc, meta.as_ref());
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

        if !self.bind_egl_image_to_texture(
            gl,
            textures,
            self.texture_id,
            image,
            target_tex,
            width,
            height,
            TexturePixel::VideoExternal,
        ) {
            if let Some(destroy) = opengl_cx.libegl.eglDestroyImageKHR {
                destroy(opengl_cx.egl_display, image);
            }
            return false;
        }
        textures[self.texture_id].format = crate::texture::TextureFormat::VideoExternal;

        self.release_egl_images();
        self.retained_egl_images = vec![image];
        self.egl_display_for_images = opengl_cx.egl_display;
        self.egl_destroy_image = opengl_cx.libegl.eglDestroyImageKHR;

        self.retain_present_sample(gst, sample);
        self.dmabuf_yuv_mode = false;
        self.yuv_biplanar = false;
        if !self.logged_first_upload {
            self.logged_first_upload = true;
            crate::log!(
                "VIDEO: presenting via DMA-Buf RGBA import fmt={} fourcc=0x{:08x} {}x{} pitch={} mod=0x{:x}",
                format_name,
                plane.fourcc,
                plane.width,
                plane.height,
                plane.pitch,
                modifier
            );
        }
        true
    }

    unsafe fn read_video_meta(
        gst: &LibGStreamer,
        buffer: *mut GstBuffer,
    ) -> Option<VideoMetaLayout> {
        let get_meta = gst.gst_buffer_get_video_meta?;
        let meta = get_meta(buffer);
        if meta.is_null() {
            return None;
        }
        let view = &*(meta as *const GstVideoMetaView);
        if view.n_planes == 0 || view.n_planes > 4 || view.stride[0] <= 0 {
            return None;
        }
        let mut plane_height = [0u32; 4];
        if let Some(get_plane_height) = gst.gst_video_meta_get_plane_height {
            if get_plane_height(meta, plane_height.as_mut_ptr()) == 0 {
                return None;
            }
            for plane in 0..view.n_planes as usize {
                if plane_height[plane] == 0 || view.stride[plane] <= 0 {
                    return None;
                }
            }
        } else {
            for plane in 0..view.n_planes as usize {
                if view.stride[plane] <= 0 {
                    return None;
                }
            }
        }
        Some(VideoMetaLayout {
            width: view.width,
            height: view.height,
            n_planes: view.n_planes,
            offset: view.offset,
            stride: view.stride,
        })
    }

    fn nv12_plane_layouts(
        width: usize,
        height: usize,
        n_fds: usize,
        _fourcc: u32,
        _modifier: u64,
        meta: Option<&VideoMetaLayout>,
    ) -> (DmaPlaneLayout, DmaPlaneLayout) {
        let cw = width.div_ceil(2) as u32;
        let ch = height.div_ceil(2) as u32;
        if let Some(m) = meta {
            if m.n_planes >= 2 && m.stride[0] > 0 && m.stride[1] > 0 {
                let y_w = if m.width > 0 { m.width } else { width as u32 };
                let y_h = if m.height > 0 { m.height } else { height as u32 };
                let uv_w = y_w.div_ceil(2);
                let uv_h = y_h.div_ceil(2);
                let y = DmaPlaneLayout {
                    fd_index: 0,
                    offset: m.offset[0] as u32,
                    pitch: m.stride[0] as u32,
                    width: y_w,
                    height: y_h,
                    fourcc: drm_fourcc(b"R8  "),
                };
                let uv = DmaPlaneLayout {
                    fd_index: if n_fds >= 2 { 1 } else { 0 },
                    offset: m.offset[1] as u32,
                    pitch: m.stride[1] as u32,
                    width: uv_w,
                    height: uv_h,
                    fourcc: drm_fourcc(b"RG88"),
                };
                return (y, uv);
            }
        }
        // Intel VA often pads rows to 64; guessing width-as-pitch fails EGL import.
        let pitch_y = ((width + 63) & !63) as u32;
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
                fourcc: drm_fourcc(b"RG88"),
            },
        )
    }

    fn rgba_plane_layout(
        width: usize,
        height: usize,
        fourcc: u32,
        meta: Option<&VideoMetaLayout>,
    ) -> DmaPlaneLayout {
        let fourcc = if fourcc != 0 {
            fourcc
        } else {
            drm_fourcc(b"AB24")
        };
        if let Some(m) = meta {
            if m.n_planes >= 1 && m.stride[0] > 0 {
                let w = if m.width > 0 { m.width } else { width as u32 };
                let h = if m.height > 0 { m.height } else { height as u32 };
                return DmaPlaneLayout {
                    fd_index: 0,
                    offset: m.offset[0] as u32,
                    pitch: m.stride[0] as u32,
                    width: w,
                    height: h,
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
        let use_modifier = Self::dmabuf_should_pass_modifier(modifier);
        let mut attribs: Vec<i32> = vec![
            egl_sys::EGL_LINUX_DRM_FOURCC_EXT as i32,
            fourcc as i32,
            egl_sys::EGL_WIDTH as i32,
            width as i32,
            egl_sys::EGL_HEIGHT as i32,
            height as i32,
            egl_sys::EGL_DMA_BUF_PLANE0_FD_EXT as i32,
            fd as i32,
            egl_sys::EGL_DMA_BUF_PLANE0_OFFSET_EXT as i32,
            offset as i32,
            egl_sys::EGL_DMA_BUF_PLANE0_PITCH_EXT as i32,
            pitch as i32,
        ];
        if use_modifier {
            attribs.push(egl_sys::EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT as i32);
            attribs.push(modifier as i32);
            attribs.push(egl_sys::EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT as i32);
            attribs.push((modifier >> 32) as i32);
        }
        attribs.push(egl_sys::EGL_NONE as i32);
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
    ) -> bool {
        let cxtexture = &mut textures[texture_id];
        // Reuse the existing GL texture name. Deleting/recreating every frame breaks
        // TextureHandleReady and can detach EGLImage bindings on NVIDIA.
        let gl_texture = cxtexture.os.gl_texture.get_or_insert_with(|| {
            let mut t = std::mem::MaybeUninit::uninit();
            (gl.glGenTextures)(1, t.as_mut_ptr());
            t.assume_init()
        });
        cxtexture.os.gl_texture_owned = true; // we own the GL name; EGLImage is separate
        // VideoExternal is sampled as samplerExternalOES on GLES (including desktop
        // Mesa). Binding the EGLImage to TEXTURE_2D while the shader samples
        // EXTERNAL_OES produces garbage / 花屏.
        //
        // YUV plane slots (`tex_y`/`tex_u`) are `texture_2d` in the Video widget, so
        // they must stay on TEXTURE_2D. NVIDIA often rejects DMA-Buf → TEXTURE_2D;
        // prefer a single DRM_FORMAT_NV12 image on TEXTURE_EXTERNAL_OES instead.
        let target = if matches!(pixel, TexturePixel::VideoExternal) {
            gl_sys::TEXTURE_EXTERNAL_OES
        } else {
            gl_sys::TEXTURE_2D
        };
        while (gl.glGetError)() != 0 {}
        (gl.glBindTexture)(target, *gl_texture);
        (gl.glTexParameteri)(
            target,
            gl_sys::TEXTURE_WRAP_S,
            gl_sys::CLAMP_TO_EDGE as i32,
        );
        (gl.glTexParameteri)(
            target,
            gl_sys::TEXTURE_WRAP_T,
            gl_sys::CLAMP_TO_EDGE as i32,
        );
        (gl.glTexParameteri)(
            target,
            gl_sys::TEXTURE_MIN_FILTER,
            gl_sys::LINEAR as i32,
        );
        (gl.glTexParameteri)(
            target,
            gl_sys::TEXTURE_MAG_FILTER,
            gl_sys::LINEAR as i32,
        );
        target_tex(target, egl_image);
        let err = (gl.glGetError)();
        (gl.glBindTexture)(target, 0);
        if err != 0 {
            return false;
        }

        let format = if matches!(pixel, TexturePixel::VideoExternal) {
            crate::texture::TextureFormat::VideoExternal
        } else {
            crate::texture::TextureFormat::VideoYuvPlane
        };
        cxtexture.format = format;
        // Keep pixel-type identity for alloc_video(); dimensions are informational.
        if matches!(pixel, TexturePixel::VideoExternal) {
            cxtexture.alloc = Some(TextureAlloc {
                width: 0,
                height: 0,
                pixel,
                category: TextureCategory::Video,
            });
        } else {
            cxtexture.alloc = Some(TextureAlloc {
                width,
                height,
                pixel,
                category: TextureCategory::Video,
            });
        }
        true
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
            if fourcc == 0 {
                // Map common GStreamer pixel formats to DRM fourccs when
                // `drm-format` is absent (typical for vapostproc DMABuf RGBA).
                fourcc = match format_name.to_ascii_uppercase().as_str() {
                    "RGBA" | "RGBx" => drm_fourcc(b"AB24"),
                    "BGRA" | "BGRx" => drm_fourcc(b"AR24"),
                    "ARGB" => drm_fourcc(b"BA24"),
                    "NV12" => drm_fourcc(b"NV12"),
                    _ => 0,
                };
            }
            // vah*dec NV12 DMABuf caps usually omit drm-format. Leave modifier=0
            // here; the NV12 present path probes VA for the real tiling modifier.
            (fourcc, modifier, format_name)
        }
    }

    /// Pass modifiers to `eglCreateImage` unless the buffer is explicitly linear.
    fn dmabuf_should_pass_modifier(modifier: u64) -> bool {
        const DRM_FORMAT_MOD_LINEAR: u64 = 0;
        modifier != DRM_FORMAT_MOD_LINEAR
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

    /// True while presenting GStreamer GLMemory RGBA as TEXTURE_2D (not OES).
    pub fn is_gl_memory_rgba(&self) -> bool {
        self.caps_profile.is_gl_memory()
            && !self.retained_gl_sample.is_null()
            && self.gl_memory_tex_2d
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
            external: self.yuv_external_oes,
        array: false,
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
    /// we are called from the UI thread and blocking waits can deadlock GStreamer's
    /// async state machine so resume appears "stuck".
    fn request_playing(&mut self) {
        if self.pipeline.is_null() {
            return;
        }
        let interrupted_pause = self.pending_state_target == GST_STATE_PAUSED;
        self.pending_state_target = GST_STATE_PLAYING;
        self.pending_state_since = Some(Instant::now());
        unsafe {
            let gst = &*self.gst;
            let ret = (gst.gst_element_set_state)(self.pipeline, GST_STATE_PLAYING);
            if ret != GST_STATE_CHANGE_FAILURE && !interrupted_pause {
                return;
            }
            // Only recover when PLAYING failed or we interrupted an in-flight
            // PAUSED transition. Use ACCURATE (not KEY_UNIT) so resume does not
            // visibly jump backward to the previous keyframe.
            let mut current: c_uint = 0;
            let mut pending: c_uint = 0;
            (gst.gst_element_get_state)(self.pipeline, &mut current, &mut pending, 0);
            let heading_playing =
                current == GST_STATE_PLAYING || pending == GST_STATE_PLAYING;
            if ret != GST_STATE_CHANGE_FAILURE && heading_playing {
                return;
            }
            crate::log!(
                "VIDEO: PLAYING recover after pause (ret={} current={} pending={})",
                ret,
                current,
                pending
            );
            // Unpin keep-alives so the buffer pool can preroll PLAYING.
            self.release_present_keepalives(gst);
            let pos = if self.resume_position_ns >= 0 {
                self.resume_position_ns
            } else {
                self.query_position_ns()
            };
            if pos >= 0 {
                (gst.gst_element_seek_simple)(
                    self.pipeline,
                    GST_FORMAT_TIME,
                    GST_SEEK_FLAG_FLUSH | GST_SEEK_FLAG_ACCURATE,
                    pos,
                );
            }
            (gst.gst_element_set_state)(self.pipeline, GST_STATE_PLAYING);
        }
    }

    /// Unpin present keep-alives so DMA/GL buffer pools can allocate again.
    unsafe fn release_present_keepalives(&mut self, gst: &LibGStreamer) {
        self.clear_retained_present_samples(gst);
        self.release_egl_images();
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
            if self.uses_playbin {
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
    }

    pub fn play(&mut self) {
        if self.pipeline.is_null() {
            return;
        }
        self.eos_notified = false;
        self.user_paused = false;
        self.pause_muted = false;
        self.apply_mute_state();
        // Do not seek on normal resume — KEY_UNIT/soft-pause seek caused a visible
        // rewind. Pipeline PAUSED→PLAYING continues from the freeze point.
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
        // Drop the extra present pin so only one sample is held across PAUSED.
        // Holding current+prev across PAUSED→PLAYING can starve DMA/GL pools and
        // leave rapid resume unable to preroll.
        unsafe {
            let gst = &*self.gst;
            if !self.retained_gl_sample_prev.is_null() {
                (gst.gst_mini_object_unref)(self.retained_gl_sample_prev as *mut GstMiniObject);
                self.retained_gl_sample_prev = std::ptr::null_mut();
            }
        }
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
