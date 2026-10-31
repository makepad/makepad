//! GStreamer decode/control from linux_video_playback.rs, detached from Cx and GL.
//! The session's worker owns every pipeline; decoded frames carry their native sample lease
//! into the Vulkan texture table, which retires it after the last GPU submission completes.
use super::{
    gstreamer_sys::*,
    video::{DmaPlane, Frame, Plane, Prepared, Storage},
};
use crate::types::format;
use std::{
    collections::VecDeque,
    ffi::{c_void, CStr, CString},
    os::{
        fd::BorrowedFd,
        raw::c_uint,
    },
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoCapsProfile {
    DmaBuf,
    SystemI420,
    SystemRgba,
}
impl VideoCapsProfile {
    fn is_dmabuf(self) -> bool {
        self == Self::DmaBuf
    }
    fn caps_text(self) -> &'static str {
        match self {
            Self::DmaBuf => "video/x-raw(memory:DMABuf)",
            Self::SystemI420 => "video/x-raw,format=I420",
            Self::SystemRgba => "video/x-raw,format=RGBA",
        }
    }
    fn next_fallback(self) -> Option<Self> {
        match self {
            Self::DmaBuf => Some(Self::SystemI420),
            Self::SystemI420 => Some(Self::SystemRgba),
            Self::SystemRgba => None,
        }
    }
}
pub enum Source {
    Network(String),
    File(String),
    Memory(Arc<[u8]>),
}
pub enum GstRuntimeEvent {
    Error(String),
    Eos,
    TracksChanged {
        video_tracks: Vec<String>,
        audio_tracks: Vec<String>,
    },
}
pub struct Player {
    library: Arc<LibGStreamer>,
    gst: *const LibGStreamer,
    pipeline: *mut GstElement,
    video_sink: *mut GstElement,
    audio_sink: *mut GstElement,
    audio_volume: *mut GstElement,
    bus: *mut GstBus,
    video_id: u64,
    is_prepared: bool,
    prepare_notified: bool,
    eos_notified: bool,
    is_looping: bool,
    audio_only: bool,
    video_width: u32,
    video_height: u32,
    duration_ns: i64,
    source_uri: String,
    temp_file_path: Option<PathBuf>,
    caps_profile: VideoCapsProfile,
    playbin3: bool,
    uses_playbin: bool,
    yuv_matrix: f32,
    yuv_full_range: bool,
    last_buffered: Vec<(f64, f64)>,
    pause_muted: bool,
    user_muted: bool,
    playback_volume: f64,
    playback_rate: f64,
    restore: Option<(u64, f64, usize, usize)>,
    resume_position_ns: i64,
    user_paused: bool,
    pending_state_target: u32,
    pending_state_since: Option<Instant>,
    prepare_started: Instant,
    stream_video_ids: Vec<String>,
    stream_audio_ids: Vec<String>,
    stream_video_labels: Vec<String>,
    stream_audio_labels: Vec<String>,
    selected_video_idx: usize,
    selected_audio_idx: usize,
    last_track_labels: Option<(Vec<String>, Vec<String>)>,
    preroll: Option<Arc<Sample>>,
    pending_frame: Option<Frame>,
    messages: VecDeque<(u32, *mut GstMessage)>,
}
struct BuiltPipeline {
    pipeline: *mut GstElement,
    video_sink: *mut GstElement,
    audio_sink: *mut GstElement,
    audio_volume: *mut GstElement,
    bus: *mut GstBus,
    playbin3: bool,
    uses_playbin: bool,
}
struct Sample {
    library: Arc<LibGStreamer>,
    sample: *mut GstSample,
}
// GstSample/GstBuffer refs are thread-safe; neither the decoder nor HAL mutates a leased sample.
unsafe impl Send for Sample {}
unsafe impl Sync for Sample {}
impl Drop for Sample {
    fn drop(&mut self) {
        unsafe {
            (self.library.gst_mini_object_unref)(self.sample.cast());
        }
    }
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
/// `flags(u32)+pad + info* + buffer* + flags/format/id + w/h/n_planes + offset[4] + stride[4]`.
/// `n_planes` ends at byte 48 on x86_64, already aligned for the gsize offsets.
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
    offset: usize,
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
    let y_end = layout
        .y_off
        .checked_add(layout.y_stride as usize * height)?;
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

impl Player {
    pub fn new(
        library: Arc<LibGStreamer>,
        session: u32,
        video_id: u64,
        source: Source,
        autoplay: bool,
        is_looping: bool,
        audio_only: bool,
    ) -> Result<Self, String> {
        let (uri, temp_file_path) = match source {
            Source::Network(url) => (url, None),
            Source::File(path) => {
                let uri = if path.starts_with("file:") {
                    path
                } else {
                    let absolute = std::path::absolute(&path).map_err(|e| e.to_string())?;
                    Self::path_to_file_uri(&absolute.to_string_lossy())
                };
                (uri, None)
            }
            Source::Memory(data) => {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "makepad-hal-{}-{session}-{video_id}-{serial}.{}",
                    std::process::id(),
                    Self::sniff_container_extension(&data)
                ));
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map_err(|e| e.to_string())?;
                if let Err(error) = file.write_all(&data) {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(error.to_string());
                }
                (Self::path_to_file_uri(&path.to_string_lossy()), Some(path))
            }
        };
        let adaptive = uri
            .split(['?', '#'])
            .next()
            .is_some_and(|s| s.ends_with(".m3u8") || s.ends_with(".mpd"));
        let profile = if !audio_only
            && !adaptive
            && std::env::var_os("MAKEPAD_GST_NO_DMABUF").is_none()
            && library.has_dmabuf_support()
        {
            VideoCapsProfile::DmaBuf
        } else {
            VideoCapsProfile::SystemI420
        };
        let mut this = Self {
            gst: Arc::as_ptr(&library),
            library,
            pipeline: std::ptr::null_mut(),
            video_sink: std::ptr::null_mut(),
            audio_sink: std::ptr::null_mut(),
            audio_volume: std::ptr::null_mut(),
            bus: std::ptr::null_mut(),
            video_id,
            is_prepared: false,
            prepare_notified: false,
            eos_notified: false,
            is_looping,
            audio_only,
            video_width: 0,
            video_height: 0,
            duration_ns: 0,
            source_uri: uri,
            temp_file_path,
            caps_profile: profile,
            playbin3: false,
            uses_playbin: false,
            yuv_matrix: 0.0,
            yuv_full_range: false,
            last_buffered: Vec::new(),
            pause_muted: false,
            user_muted: false,
            playback_volume: 1.0,
            playback_rate: 1.0,
            restore: None,
            resume_position_ns: -1,
            user_paused: !autoplay,
            pending_state_target: 0,
            pending_state_since: None,
            prepare_started: Instant::now(),
            stream_video_ids: Vec::new(),
            stream_audio_ids: Vec::new(),
            stream_video_labels: Vec::new(),
            stream_audio_labels: Vec::new(),
            selected_video_idx: 0,
            selected_audio_idx: 0,
            last_track_labels: None,
            preroll: None,
            pending_frame: None,
            messages: VecDeque::new(),
        };
        loop {
            if let Some(built) = Self::build_pipeline(
                &this.library,
                video_id,
                &this.source_uri,
                audio_only,
                this.caps_profile,
            ) {
                this.install(built);
                break;
            }
            let Some(next) = this.caps_profile.next_fallback() else {
                return Err("GStreamer playbin/appsink unavailable".into());
            };
            eprintln!(
                "[hal-video] {:?} pipeline unavailable; trying {next:?}",
                this.caps_profile
            );
            this.caps_profile = next;
        }
        Ok(this)
    }
    fn install(&mut self, b: BuiltPipeline) {
        self.pipeline = b.pipeline;
        self.video_sink = b.video_sink;
        self.audio_sink = b.audio_sink;
        self.audio_volume = b.audio_volume;
        self.bus = b.bus;
        self.playbin3 = b.playbin3;
        self.uses_playbin = b.uses_playbin;
    }
    pub fn fallback(&mut self) -> bool {
        let gst = unsafe { &*self.gst };
        self.try_rebuild_with_caps_fallback(gst, "Vulkan DMA-BUF import failed")
    }
    fn try_rebuild_with_caps_fallback(&mut self, gst: &LibGStreamer, reason: &str) -> bool {
        let Some(next) = self.caps_profile.next_fallback() else {
            return false;
        };
        let position = self.current_position_ms();
        eprintln!(
            "[hal-video] {:?}: {reason}; trying {next:?}",
            self.caps_profile
        );
        let restore = (
            position as u64,
            self.playback_rate,
            self.selected_video_idx,
            self.selected_audio_idx,
        );
        self.destroy_pipeline();
        self.caps_profile = next;
        let Some(b) =
            Self::build_pipeline(gst, self.video_id, &self.source_uri, self.audio_only, next)
        else {
            return false;
        };
        self.install(b);
        self.is_prepared = false;
        self.prepare_notified = false;
        self.prepare_started = Instant::now();
        self.video_width = 0;
        self.video_height = 0;
        self.restore = Some(restore);
        self.apply_mute_state();
        true
    }
    fn destroy_pipeline(&mut self) {
        self.preroll = None;
        self.pending_frame = None;
        unsafe {
            if self.pipeline.is_null() {
                return;
            }
            let gst = &*self.gst;
            for (_, message) in self.messages.drain(..) {
                (gst.gst_mini_object_unref)(message.cast());
            }
            // This is the worker, never the HAL/UI thread. GstSample leases retain pool storage.
            (gst.gst_element_set_state)(self.pipeline, GST_STATE_NULL);
            if !self.video_sink.is_null() {
                (gst.gst_object_unref)(self.video_sink.cast());
            }
            if !self.audio_volume.is_null() {
                (gst.gst_object_unref)(self.audio_volume.cast());
            }
            if !self.audio_sink.is_null() {
                (gst.gst_object_unref)(self.audio_sink.cast());
            }
            if !self.bus.is_null() {
                (gst.gst_object_unref)(self.bus.cast());
            }
            (gst.gst_object_unref)(self.pipeline.cast());
            self.pipeline = std::ptr::null_mut();
            self.video_sink = std::ptr::null_mut();
            self.audio_sink = std::ptr::null_mut();
            self.audio_volume = std::ptr::null_mut();
            self.bus = std::ptr::null_mut();
        }
    }
    fn build_pipeline(
        gst: &LibGStreamer,
        video_id: u64,
        uri: &str,
        audio_only: bool,
        caps_profile: VideoCapsProfile,
    ) -> Option<BuiltPipeline> {
        let uri_cstr = CString::new(uri).ok()?;
        unsafe {
            let (pipeline, playbin3) = Self::make_playbin(gst, caps_profile, uri);
            if pipeline.is_null() {
                eprintln!(
                    "Failed to create GStreamer playbin/playbin3 for video {:?}",
                    video_id
                );
                return None;
            }

            let uri_prop = CString::new("uri").unwrap();
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

            // Keep the pitch when the playback rate is not 1: playbin plays a
            // rate change by resampling, which shifts the pitch with the speed.
            let scaletempo_type = CString::new("scaletempo").unwrap();
            let scaletempo =
                (gst.gst_element_factory_make)(scaletempo_type.as_ptr(), std::ptr::null());
            if scaletempo.is_null() {
                eprintln!("VIDEO: scaletempo unavailable; a playback rate change shifts the pitch");
            } else {
                let audio_filter_prop = CString::new("audio-filter").unwrap();
                (gst.g_object_set_ptr)(
                    pipeline,
                    audio_filter_prop.as_ptr(),
                    scaletempo as *mut c_void,
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
                let (video_sink_bin, appsink) = if caps_profile.is_dmabuf() {
                    match Self::make_dmabuf_video_sink_bin(gst, video_id, caps_profile) {
                        Some(pair) => pair,
                        None => {
                            if !audio_sink_element.is_null() {
                                (gst.gst_object_unref)(audio_sink_element.cast());
                            }
                            if !audio_volume.is_null() {
                                (gst.gst_object_unref)(audio_volume.cast());
                            }
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
                        eprintln!(
                            "Failed to create GStreamer appsink for video {:?}",
                            video_id
                        );
                        if !audio_sink_element.is_null() {
                            (gst.gst_object_unref)(audio_sink_element.cast());
                        }
                        if !audio_volume.is_null() {
                            (gst.gst_object_unref)(audio_volume.cast());
                        }
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
                    (gst.g_object_set_int)(appsink, max_buffers_prop.as_ptr(), 2, std::ptr::null());
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

            if !video_sink.is_null() {
                (gst.gst_object_ref)(video_sink.cast());
            }
            let bus = (gst.gst_element_get_bus)(pipeline);
            (gst.gst_element_set_state)(pipeline, GST_STATE_PAUSED);
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
    fn make_playbin(
        gst: &LibGStreamer,
        caps_profile: VideoCapsProfile,
        uri: &str,
    ) -> (*mut GstElement, bool) {
        let local = uri.starts_with("file://") || uri.starts_with("file:");
        let prefer_classic = local && (caps_profile.is_dmabuf());
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
                        eprintln!("VIDEO: using playbin for local {:?} playback", caps_profile);
                    } else if name == "playbin3" {
                        eprintln!("VIDEO: using playbin3");
                    }
                    return (pipeline, is_playbin3);
                }
            }
            (std::ptr::null_mut(), false)
        }
    }

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
                    eprintln!(
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
                eprintln!("VIDEO: audio sink = pulsesink (no bin support; server-side volume)");
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
                eprintln!("VIDEO: audio sink = pulsesink (no volume bin; server-side volume)");
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
                eprintln!("VIDEO: volume bin wiring failed; using bare pulsesink");
                return (make_tuned_pulsesink(), std::ptr::null_mut());
            }
            add_pad(bin, ghost);

            eprintln!(
                "VIDEO: audio sink = bin(volume ! pulsesink) buffer-time=20ms latency-time=10ms"
            );
            (bin, volume)
        }
    }

    fn make_dmabuf_video_sink_bin(
        gst: &LibGStreamer,
        video_id: u64,
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
                eprintln!("Failed to create appsink for DMA-Buf sink {:?}", video_id);
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
                eprintln!(
                    "VIDEO: video sink = appsink caps={} (no vapostproc)",
                    caps_text
                );
                return Some((appsink, appsink));
            }

            if link(postproc, appsink) == 0 {
                (gst.gst_object_unref)(postproc as *mut c_void);
                (gst.gst_object_unref)(appsink as *mut c_void);
                eprintln!(
                    "Failed to link DMA-Buf video sink bin for video {:?}",
                    video_id
                );
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
                (gst.gst_object_unref)(bin as *mut c_void);
                eprintln!(
                    "Failed to ghost-pad DMA-Buf video sink bin for video {:?}",
                    video_id
                );
                return None;
            }
            add_pad(bin, ghost);

            eprintln!(
                "VIDEO: video sink = bin({} ! appsink) caps={}",
                postproc_name, caps_text
            );
            Some((bin, appsink))
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
        if data.len() >= 4
            && (data.starts_with(b"\x00\x00\x00\x14") || data.starts_with(b"\x00\x00\x00\x18"))
            && data.len() >= 8
            && &data[4..8] == b"ftyp"
        {
            return "mp4";
        }
        // Generic name; GStreamer typefind still inspects contents.
        "bin"
    }

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
            let matrix = if s.contains("bt601") || s.contains("smpte240m") || s.contains("jpeg") {
                1.0
            } else if s.contains("bt2020") || s.contains("bt2100") {
                2.0
            } else {
                0.0 // bt709 / default
            };
            ColorMeta { matrix, full_range }
        }
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
                && (profile.is_dmabuf())
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

    /// Pop ANY and sort locally: gst_bus_pop_filtered discards messages of other types,
    /// which otherwise loses EOS and stream collections while looking for an error.
    unsafe fn pop_message(&mut self, gst: &LibGStreamer, kind: u32) -> *mut GstMessage {
        loop {
            let message = (gst.gst_bus_pop_filtered)(self.bus, u32::MAX);
            if message.is_null() {
                break;
            }
            let ty = message_type(message);
            if matches!(
                ty,
                GST_MESSAGE_ERROR
                    | GST_MESSAGE_WARNING
                    | GST_MESSAGE_EOS
                    | GST_MESSAGE_STREAM_COLLECTION
                    | GST_MESSAGE_BUFFERING
            ) {
                self.messages.push_back((ty, message));
            } else {
                (gst.gst_mini_object_unref)(message.cast());
            }
        }
        self.messages
            .iter()
            .position(|m| m.0 == kind)
            .and_then(|i| self.messages.remove(i))
            .map_or(std::ptr::null_mut(), |m| m.1)
    }

    pub fn check_prepared(&mut self) -> Option<Result<Prepared, String>> {
        if self.prepare_notified || self.pipeline.is_null() {
            return None;
        }

        let gst = unsafe { &*self.gst };

        gst.pump_default_main_context();

        unsafe {
            // Check for errors on the bus
            let msg = self.pop_message(gst, GST_MESSAGE_ERROR);
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

                let negotiation_error =
                    Self::is_caps_fallback_error(&err_str, &debug_str, self.caps_profile);
                if negotiation_error && self.try_rebuild_with_caps_fallback(gst, &err_str) {
                    return None;
                }

                if debug_str.is_empty() {
                    eprintln!("GStreamer error id={} msg={}", self.video_id, err_str);
                } else {
                    eprintln!(
                        "GStreamer error id={} msg={} debug={}",
                        self.video_id, err_str, debug_str
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
            let state_wait_ns: u64 = if self.caps_profile.is_dmabuf() {
                50_000_000
            } else {
                0
            };
            let ret =
                (gst.gst_element_get_state)(self.pipeline, &mut state, &mut pending, state_wait_ns);

            // Live sources have no preroll in PAUSED. Run just far enough to obtain their
            // first sample; the requested paused state is restored once prepared.
            if ret == GST_STATE_CHANGE_NO_PREROLL && state == GST_STATE_PAUSED && !self.audio_only {
                (gst.gst_element_set_state)(self.pipeline, GST_STATE_PLAYING);
                return None;
            }

            if ret == GST_STATE_CHANGE_FAILURE {
                if self.caps_profile.is_dmabuf()
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

            // A hardware sink can stall before PAUSED without posting ERROR. Fall back.
            let zero_copy_prepare_timeout = if self.caps_profile.is_dmabuf() {
                Duration::from_secs(5)
            } else {
                Duration::from_secs(2)
            };
            if ret == GST_STATE_CHANGE_ASYNC
                && state < GST_STATE_PAUSED
                && self.caps_profile.is_dmabuf()
                && self.prepare_started.elapsed() >= zero_copy_prepare_timeout
                && self
                    .try_rebuild_with_caps_fallback(gst, "timed out waiting for zero-copy PAUSED")
            {
                return None;
            }

            // Need at least PAUSED for preroll to be done
            if state < GST_STATE_PAUSED || self.is_prepared {
                return None;
            }

            // Pull the preroll sample to get video dimensions.
            // try_pull_preroll works in PAUSED state (try_pull_sample does NOT).
            if !self.video_sink.is_null() {
                let mut sample = (gst.gst_app_sink_try_pull_preroll)(self.video_sink, 0);
                if sample.is_null() && state == GST_STATE_PLAYING {
                    sample = (gst.gst_app_sink_try_pull_sample)(self.video_sink, 0);
                }
                if !sample.is_null() {
                    self.extract_dims_from_sample(gst, sample);
                    self.preroll = Some(Arc::new(Sample {
                        library: self.library.clone(),
                        sample,
                    }));
                }
            }

            // Wait for real dimensions instead of inventing 1920x1080.
            // Zero-copy probes can hang in ASYNC without ERROR (black screen);
            // fail them over to the next caps profile quickly.
            if !self.audio_only && (self.video_width == 0 || self.video_height == 0) {
                let timeout = if self.caps_profile.is_dmabuf() {
                    Duration::from_secs(5)
                } else {
                    Duration::from_secs(15)
                };
                if self.prepare_started.elapsed() >= timeout {
                    if self.caps_profile.is_dmabuf()
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

            if let Some((position, rate, video, audio)) = self.restore.take() {
                self.seek_to(position);
                self.set_playback_rate(rate);
                self.select_video_track(video);
                self.select_audio_track(audio);
            }
            // Honour controls received while preparation was still in progress.
            if !self.user_paused {
                (gst.gst_element_set_state)(self.pipeline, GST_STATE_PLAYING);
            } else if state == GST_STATE_PLAYING {
                (gst.gst_element_set_state)(self.pipeline, GST_STATE_PAUSED);
            }

            let (video_tracks, audio_tracks) = self.query_track_labels(gst);
            self.last_buffered = self.buffered_ranges();
            self.last_track_labels = Some((video_tracks.clone(), audio_tracks.clone()));

            Some(Ok(Prepared::new(
                self.video_width,
                self.video_height,
                duration_ms,
                is_seekable,
                video_tracks,
                audio_tracks,
            )))
        }
    }

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
                let msg = self.pop_message(gst, GST_MESSAGE_STREAM_COLLECTION);
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

    fn ingest_stream_collection(
        &mut self,
        gst: &LibGStreamer,
        collection: *mut GstStreamCollection,
    ) {
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

    pub fn select_video_track(&mut self, index: usize) -> bool {
        if self.pipeline.is_null() {
            return false;
        }
        let gst = unsafe { &*self.gst };
        self.drain_stream_collection_messages(gst);
        if self.playbin3 {
            if self.stream_video_ids.is_empty() {
                eprintln!(
                    "VIDEO: select_video_track({}) ignored — no playbin3 video stream ids yet",
                    index
                );
                return false;
            }
            if index >= self.stream_video_ids.len() {
                eprintln!(
                    "VIDEO: select_video_track({}) out of range (n={})",
                    index,
                    self.stream_video_ids.len()
                );
                return false;
            }
            self.selected_video_idx = index;
            let ok = self.send_select_streams(gst);
            if !ok {
                eprintln!("VIDEO: select_video_track({}) SELECT_STREAMS failed", index);
            }
            ok
        } else if self.uses_playbin {
            unsafe {
                let prop = CString::new("current-video").unwrap();
                (gst.g_object_set_int)(
                    self.pipeline,
                    prop.as_ptr(),
                    index as i32,
                    std::ptr::null(),
                );
            }
            self.selected_video_idx = index;
            true
        } else {
            false
        }
    }

    pub fn select_audio_track(&mut self, index: usize) -> bool {
        if self.pipeline.is_null() {
            return false;
        }
        let gst = unsafe { &*self.gst };
        self.drain_stream_collection_messages(gst);
        if self.playbin3 {
            if self.stream_audio_ids.is_empty() {
                eprintln!(
                    "VIDEO: select_audio_track({}) ignored — no playbin3 audio stream ids yet",
                    index
                );
                return false;
            }
            if index >= self.stream_audio_ids.len() {
                eprintln!(
                    "VIDEO: select_audio_track({}) out of range (n={})",
                    index,
                    self.stream_audio_ids.len()
                );
                return false;
            }
            self.selected_audio_idx = index;
            let ok = self.send_select_streams(gst);
            if !ok {
                eprintln!("VIDEO: select_audio_track({}) SELECT_STREAMS failed", index);
            }
            ok
        } else if self.uses_playbin {
            unsafe {
                let prop = CString::new("current-audio").unwrap();
                (gst.g_object_set_int)(
                    self.pipeline,
                    prop.as_ptr(),
                    index as i32,
                    std::ptr::null(),
                );
            }
            self.selected_audio_idx = index;
            true
        } else {
            false
        }
    }

    fn send_select_streams(&self, gst: &LibGStreamer) -> bool {
        let (Some(new_event), Some(append)) = (gst.gst_event_new_select_streams, gst.g_list_append)
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
                let idx = self.selected_video_idx.min(self.stream_video_ids.len() - 1);
                owned.push(CString::new(self.stream_video_ids[idx].as_str()).unwrap());
            }
            if !self.stream_audio_ids.is_empty() {
                let idx = self.selected_audio_idx.min(self.stream_audio_ids.len() - 1);
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

    fn nv12_plane_layouts(meta: &VideoMetaLayout) -> Option<(DmaPlaneLayout, DmaPlaneLayout)> {
        if meta.n_planes < 2
            || meta.width == 0
            || meta.height == 0
            || meta.stride[0] <= 0
            || meta.stride[1] <= 0
        {
            return None;
        }
        Some((
            DmaPlaneLayout {
                offset: meta.offset[0],
                pitch: meta.stride[0] as u32,
                width: meta.width,
                height: meta.height,
                fourcc: drm_fourcc(b"R8  "),
            },
            DmaPlaneLayout {
                offset: meta.offset[1],
                pitch: meta.stride[1] as u32,
                width: meta.width.div_ceil(2),
                height: meta.height.div_ceil(2),
                fourcc: drm_fourcc(b"RG88"),
            },
        ))
    }

    fn rgba_plane_layout(
        fourcc: u32,
        name: &str,
        meta: &VideoMetaLayout,
    ) -> Option<DmaPlaneLayout> {
        if meta.n_planes != 1 || meta.width == 0 || meta.height == 0 || meta.stride[0] <= 0 {
            return None;
        }
        let fourcc = if fourcc != 0 {
            fourcc
        } else {
            match name {
                "RGBA" => drm_fourcc(b"AB24"),
                "RGBx" => drm_fourcc(b"XB24"),
                "BGRA" => drm_fourcc(b"AR24"),
                "BGRx" => drm_fourcc(b"XR24"),
                _ => return None,
            }
        };
        Some(DmaPlaneLayout {
            offset: meta.offset[0],
            pitch: meta.stride[0] as u32,
            width: meta.width,
            height: meta.height,
            fourcc,
        })
    }

    fn dmabuf_format_from_caps(gst: &LibGStreamer, caps: *mut GstCaps) -> (u32, u64, String) {
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

    pub fn poll_runtime(&mut self) -> Vec<GstRuntimeEvent> {
        let mut out = Vec::new();
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
        if self.is_prepared
            && (!self.stream_video_ids.is_empty() || !self.stream_audio_ids.is_empty())
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
                let msg = self.pop_message(gst, GST_MESSAGE_ERROR);
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
                let msg = self.pop_message(gst, GST_MESSAGE_WARNING);
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
                let msg = self.pop_message(gst, GST_MESSAGE_EOS);
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

    fn drain_buffering_messages(&mut self, gst: &LibGStreamer) {
        let Some(parse) = gst.gst_message_parse_buffering else {
            // Still pop so the bus cannot back up if parse is missing.
            unsafe {
                loop {
                    let msg = self.pop_message(gst, GST_MESSAGE_BUFFERING);
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
                let msg = self.pop_message(gst, GST_MESSAGE_BUFFERING);
                if msg.is_null() {
                    break;
                }
                let mut percent: i32 = 100;
                parse(msg, &mut percent);
                (gst.gst_mini_object_unref)(msg as *mut GstMiniObject);
            }
        }
    }

    pub fn take_buffered_ranges_if_changed(&mut self) -> Option<Vec<(f64, f64)>> {
        let ranges = self.buffered_ranges();
        if ranges == self.last_buffered {
            None
        } else {
            self.last_buffered = ranges.clone();
            Some(ranges)
        }
    }

    fn query_position_ns(&self) -> i64 {
        if self.pipeline.is_null() {
            return -1;
        }
        unsafe {
            let gst = &*self.gst;
            let mut position_ns: i64 = 0;
            if (gst.gst_element_query_position)(self.pipeline, GST_FORMAT_TIME, &mut position_ns)
                != 0
                && position_ns >= 0
            {
                position_ns
            } else {
                -1
            }
        }
    }

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
            eprintln!(
                "VIDEO: reached {} in {:.1}ms",
                if target == GST_STATE_PLAYING {
                    "PLAYING"
                } else {
                    "PAUSED"
                },
                elapsed.as_secs_f64() * 1000.0
            );
            self.pending_state_since = None;
            self.pending_state_target = 0;
        } else if elapsed > std::time::Duration::from_millis(1000) {
            eprintln!(
                "VIDEO: state change to {} still pending after 1s (current={} pending={})",
                if target == GST_STATE_PLAYING {
                    "PLAYING"
                } else {
                    "PAUSED"
                },
                current,
                pending
            );
            self.pending_state_since = None;
            self.pending_state_target = 0;
        }
    }

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
            let heading_playing = current == GST_STATE_PLAYING || pending == GST_STATE_PLAYING;
            if ret != GST_STATE_CHANGE_FAILURE && heading_playing {
                return;
            }
            eprintln!(
                "VIDEO: PLAYING recover after pause (ret={} current={} pending={})",
                ret, current, pending
            );
            // Unpin keep-alives so the buffer pool can preroll PLAYING.
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
        self.preroll = None;
        self.pending_frame = None;
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

    pub fn set_playback_rate(&mut self, rate: f64) {
        if self.pipeline.is_null() {
            return;
        }
        let rate = if rate == 0.0 { 1.0 } else { rate };
        if !rate.is_finite() {
            return;
        }
        self.playback_rate = rate;
        self.preroll = None;
        self.pending_frame = None;
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
}
impl Player {
    pub fn prepared_info(&self, video_tracks: Vec<String>, audio_tracks: Vec<String>) -> Prepared {
        Prepared::new(
            self.video_width,
            self.video_height,
            (self.duration_ns.max(0) / 1_000_000) as u128,
            !self.seekable_ranges().is_empty(),
            video_tracks,
            audio_tracks,
        )
    }
    pub fn poll_frame(&mut self) -> Option<Frame> {
        if self.pending_frame.is_none() {
            self.pending_frame = self.decode_frame();
        }
        match self.pending_frame.as_ref()?.ready() {
            Ok(true) => self.pending_frame.take(),
            Ok(false) => None,
            Err(error) => {
                self.pending_frame = None;
                let gst = unsafe { &*self.gst };
                self.try_rebuild_with_caps_fallback(gst, &error);
                None
            }
        }
    }
    fn decode_frame(&mut self) -> Option<Frame> {
        if self.pipeline.is_null() || self.video_sink.is_null() || !self.is_prepared {
            return None;
        }
        let gst = unsafe { &*self.gst };
        unsafe {
            let lease = if let Some(preroll) = self.preroll.take() {
                preroll
            } else {
                let sample = if self.user_paused {
                    (gst.gst_app_sink_try_pull_preroll)(self.video_sink, 0)
                } else {
                    (gst.gst_app_sink_try_pull_sample)(self.video_sink, 0)
                };
                if sample.is_null() {
                    return None;
                }
                Arc::new(Sample {
                    library: self.library.clone(),
                    sample,
                })
            };
            let sample = lease.sample;
            let buffer = (gst.gst_sample_get_buffer)(sample);
            if buffer.is_null() {
                return None;
            }
            self.extract_dims_from_sample(gst, sample);
            if self.caps_profile.is_dmabuf() {
                let frame = self.dmabuf_frame(gst, buffer, lease);
                if frame.is_none() {
                    self.try_rebuild_with_caps_fallback(gst, "unsupported DMA-BUF layout");
                }
                return frame;
            }
            let mut map = GstMapInfo::default();
            if (gst.gst_buffer_map)(buffer, &mut map, GST_MAP_READ) == 0 {
                return None;
            }
            let frame = (|| {
                if map.data.is_null() {
                    return None;
                }
                let bytes = std::slice::from_raw_parts(map.data, map.size);
                let (w, h) = (self.video_width, self.video_height);
                let mut planes = Vec::new();
                let mut bits = 0;
                if self.caps_profile == VideoCapsProfile::SystemI420 {
                    let layout = Self::read_video_meta(gst, buffer)
                        .and_then(|m| {
                            i420_layout_from_video_meta(&m, w as usize, h as usize, map.size)
                        })
                        .or_else(|| infer_i420_layout(w as usize, h as usize, map.size))?;
                    for (width, height, offset, stride) in [
                        (w, h, layout.y_off, layout.y_stride),
                        (w.div_ceil(2), h.div_ceil(2), layout.u_off, layout.u_stride),
                        (w.div_ceil(2), h.div_ceil(2), layout.v_off, layout.v_stride),
                    ] {
                        planes.push(Plane::packed(
                            width,
                            height,
                            format::R8,
                            bytes.get(offset..)?,
                            stride as usize,
                        )?);
                    }
                    bits = crate::media::yuv::ENABLED;
                } else {
                    let (offset, stride) = Self::read_video_meta(gst, buffer)
                        .map(|m| (m.offset[0], m.stride[0] as usize))
                        .unwrap_or((0, w as usize * 4));
                    planes.push(Plane::packed(
                        w,
                        h,
                        format::RGBA8,
                        bytes.get(offset..)?,
                        stride,
                    )?);
                }
                if self.yuv_full_range {
                    bits |= crate::media::yuv::FULL_RANGE;
                }
                Some(Frame {
                    planes,
                    yuv: bits,
                    matrix: self.yuv_matrix,
                })
            })();
            (gst.gst_buffer_unmap)(buffer, &mut map);
            frame
        }
    }

    unsafe fn dmabuf_frame(
        &self,
        gst: &LibGStreamer,
        buffer: *mut GstBuffer,
        lease: Arc<Sample>,
    ) -> Option<Frame> {
        let (is_dma, get_fd) = (gst.gst_is_dmabuf_memory?, gst.gst_dmabuf_memory_get_fd?);
        let n = gst.gst_buffer_n_memory?(buffer);
        let mut memories = Vec::new();
        let mut base = 0usize;
        for i in 0..n {
            let memory = (gst.gst_buffer_peek_memory)(buffer, i);
            if memory.is_null() || is_dma(memory) == 0 {
                return None;
            }
            let fd = get_fd(memory);
            if fd < 0 {
                return None;
            }
            let (mut offset, mut maxsize) = (0, 0);
            let size = (gst.gst_memory_get_sizes)(memory, &mut offset, &mut maxsize);
            memories.push((fd, base, size, offset, maxsize));
            base = base.checked_add(size)?;
        }
        let caps = (gst.gst_sample_get_caps)(lease.sample);
        let (fourcc, mut modifier, name) = Self::dmabuf_format_from_caps(gst, caps);
        let meta = Self::read_video_meta(gst, buffer)?;
        let (w, h) = (self.video_width as usize, self.video_height as usize);
        let nv12 = fourcc == drm_fourcc(b"NV12") || name.eq_ignore_ascii_case("NV12");
        let layouts = if nv12 {
            if modifier == 0 {
                modifier =
                    super::va_dmabuf_modifier::probe_nv12_modifier(w as u32, h as u32).unwrap_or(0);
            }
            let (y, uv) = Self::nv12_plane_layouts(&meta)?;
            vec![(y, format::R8), (uv, format::RG8)]
        } else {
            let layout = Self::rgba_plane_layout(fourcc, &name, &meta)?;
            let format = if layout.fourcc == drm_fourcc(b"AB24")
                || layout.fourcc == drm_fourcc(b"XB24")
            {
                format::RGBA8
            } else if layout.fourcc == drm_fourcc(b"AR24") || layout.fourcc == drm_fourcc(b"XR24") {
                format::BGRA8
            } else {
                return None;
            };
            vec![(layout, format)]
        };
        let mut planes = Vec::new();
        for (layout, format) in layouts {
            // VideoMeta offsets address the concatenated GstBuffer. Translate into the
            // individual GstMemory allocation, retaining its original suballocation offset.
            let absolute = layout.offset as usize;
            let &(fd, base, _, offset, maxsize) = memories
                .iter()
                .find(|(_, base, size, _, _)| absolute >= *base && absolute < base + size)?;
            let fd = BorrowedFd::borrow_raw(fd).try_clone_to_owned().ok()?;
            let storage = DmaPlane {
                fd,
                modifier,
                offset: (absolute - base + offset) as u64,
                pitch: layout.pitch as u64,
                size: maxsize as u64,
                _sample: lease.clone(),
            };
            planes.push(Plane {
                width: layout.width,
                height: layout.height,
                format,
                storage: Storage::Dma(Arc::new(storage)),
            });
        }
        let mut bits = if nv12 {
            planes.push(Plane::dummy());
            crate::media::yuv::ENABLED | crate::media::yuv::BIPLANAR
        } else {
            0
        };
        if self.yuv_full_range {
            bits |= crate::media::yuv::FULL_RANGE;
        }
        Some(Frame {
            planes,
            yuv: bits,
            matrix: self.yuv_matrix,
        })
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        self.destroy_pipeline();
        if let Some(path) = self.temp_file_path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub fn can_play_type(mime: &str) -> &'static str {
    let base = mime.split(';').next().unwrap_or("").trim();
    match base {
        // Containers + codecs GStreamer handles well with base/good/bad plugins
        "video/mp4" | "video/x-m4v" => "probably",
        "video/webm" => "probably",
        "video/ogg" => "probably",
        "video/x-matroska" | "video/x-msvideo" | "video/quicktime" => "maybe",
        // Adaptive streaming manifests (playbin3 + adaptivedemux2 / dashdemux)
        "application/vnd.apple.mpegurl"
        | "application/x-mpegURL"
        | "audio/mpegurl"
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
