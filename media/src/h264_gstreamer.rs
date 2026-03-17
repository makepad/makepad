//! GStreamer-based H.264 support for Linux.
//!
//! Decoder:
//! - `appsrc ! h264parse ! avdec_h264 ! videoconvert ! appsink`
//!
//! Encoder:
//! - `appsrc ! x264enc ! h264parse ! appsink`
//!
//! The encoder accepts CPU camera frames, converts them to tightly packed I420,
//! and emits the same packet shape used by the Apple/Android backends:
//! - Annex B config packets containing SPS/PPS
//! - Annex B access-unit packets for video samples

#![allow(non_camel_case_types, non_snake_case, dead_code)]

use {
    crate::h264_packets,
    makepad_platform::{
        video_decode::yuv::{YuvColorMatrix, YuvLayout, YuvPlaneData},
        CameraFrameOwned, CameraFrameRef, EncodedVideoPacketRef, MediaVideoEncoder,
        MseDecodedFrame, VideoBitstreamFormat, VideoCodec, VideoEncodeError,
        VideoEncoderConfig, VideoFrameDecoder, VideoOutputFn, VideoQueuePolicy,
    },
    std::{
        collections::VecDeque,
        ffi::{c_void, CString},
        os::raw::{c_char, c_int, c_uint},
        ptr,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Condvar, Mutex, OnceLock,
        },
        time::Duration,
    },
};

// dlopen/dlsym/dlclose
extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

const RTLD_NOW: c_int = 0x2;
const RTLD_GLOBAL: c_int = 0x100;

// ---------------------------------------------------------------------------
// Minimal GStreamer FFI — just what we need for appsrc/appsink pipelines
// ---------------------------------------------------------------------------

type GstElement = c_void;
type GstBus = c_void;
type GstSample = c_void;
type GstBuffer = c_void;
type GstCaps = c_void;
type GstStructure = c_void;

const GST_STATE_NULL: c_uint = 1;
const GST_STATE_PLAYING: c_uint = 4;
const GST_MAP_READ: c_uint = 1;
const GST_FLOW_OK: c_int = 0;

#[repr(C)]
struct GstMapInfo {
    memory: *mut c_void,
    flags: c_uint,
    data: *mut u8,
    size: usize,
    maxsize: usize,
    _padding: [*mut c_void; 8],
}

impl Default for GstMapInfo {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

struct GstLib {
    // libgstreamer-1.0
    gst_init: unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char),
    gst_parse_launch: unsafe extern "C" fn(*const c_char, *mut *mut c_void) -> *mut GstElement,
    gst_element_set_state: unsafe extern "C" fn(*mut GstElement, c_uint) -> c_int,
    gst_element_get_bus: unsafe extern "C" fn(*mut GstElement) -> *mut GstBus,
    gst_object_unref: unsafe extern "C" fn(*mut c_void),
    gst_sample_get_buffer: unsafe extern "C" fn(*mut GstSample) -> *mut GstBuffer,
    gst_sample_get_caps: unsafe extern "C" fn(*mut GstSample) -> *mut GstCaps,
    gst_buffer_map: unsafe extern "C" fn(*mut GstBuffer, *mut GstMapInfo, c_uint) -> c_int,
    gst_buffer_unmap: unsafe extern "C" fn(*mut GstBuffer, *mut GstMapInfo),
    gst_caps_get_structure: unsafe extern "C" fn(*mut GstCaps, c_uint) -> *mut GstStructure,
    gst_structure_get_int:
        unsafe extern "C" fn(*mut GstStructure, *const c_char, *mut c_int) -> c_int,
    gst_mini_object_unref: unsafe extern "C" fn(*mut c_void),
    gst_caps_from_string: unsafe extern "C" fn(*const c_char) -> *mut GstCaps,
    gst_caps_unref: unsafe extern "C" fn(*mut GstCaps),
    gst_buffer_new_allocate:
        unsafe extern "C" fn(*mut c_void, usize, *mut c_void) -> *mut GstBuffer,
    gst_buffer_fill: unsafe extern "C" fn(*mut GstBuffer, usize, *const u8, usize) -> usize,
    gst_bin_get_by_name: unsafe extern "C" fn(*mut GstElement, *const c_char) -> *mut GstElement,

    // libgstapp-1.0
    gst_app_src_push_buffer: unsafe extern "C" fn(*mut GstElement, *mut GstBuffer) -> c_int,
    gst_app_src_set_caps: unsafe extern "C" fn(*mut GstElement, *const GstCaps),
    gst_app_src_end_of_stream: unsafe extern "C" fn(*mut GstElement) -> c_int,
    gst_app_sink_try_pull_sample: unsafe extern "C" fn(*mut GstElement, u64) -> *mut GstSample,
    gst_app_sink_is_eos: unsafe extern "C" fn(*mut GstElement) -> c_int,

    _gst_handle: *mut c_void,
    _gstapp_handle: *mut c_void,
}

unsafe impl Send for GstLib {}

impl GstLib {
    unsafe fn try_load() -> Option<Self> {
        let gst_handle = dlopen(
            b"libgstreamer-1.0.so.0\0".as_ptr() as *const c_char,
            RTLD_NOW | RTLD_GLOBAL,
        );
        if gst_handle.is_null() {
            return None;
        }

        let gstapp_handle = dlopen(
            b"libgstapp-1.0.so.0\0".as_ptr() as *const c_char,
            RTLD_NOW,
        );
        if gstapp_handle.is_null() {
            dlclose(gst_handle);
            return None;
        }

        macro_rules! sym {
            ($handle:expr, $name:literal) => {{
                let s = dlsym($handle, concat!($name, "\0").as_ptr() as *const c_char);
                if s.is_null() {
                    dlclose(gstapp_handle);
                    dlclose(gst_handle);
                    return None;
                }
                std::mem::transmute(s)
            }};
        }

        let lib = GstLib {
            gst_init: sym!(gst_handle, "gst_init"),
            gst_parse_launch: sym!(gst_handle, "gst_parse_launch"),
            gst_element_set_state: sym!(gst_handle, "gst_element_set_state"),
            gst_element_get_bus: sym!(gst_handle, "gst_element_get_bus"),
            gst_object_unref: sym!(gst_handle, "gst_object_unref"),
            gst_sample_get_buffer: sym!(gst_handle, "gst_sample_get_buffer"),
            gst_sample_get_caps: sym!(gst_handle, "gst_sample_get_caps"),
            gst_buffer_map: sym!(gst_handle, "gst_buffer_map"),
            gst_buffer_unmap: sym!(gst_handle, "gst_buffer_unmap"),
            gst_caps_get_structure: sym!(gst_handle, "gst_caps_get_structure"),
            gst_structure_get_int: sym!(gst_handle, "gst_structure_get_int"),
            gst_mini_object_unref: sym!(gst_handle, "gst_mini_object_unref"),
            gst_caps_from_string: sym!(gst_handle, "gst_caps_from_string"),
            gst_caps_unref: sym!(gst_handle, "gst_caps_unref"),
            gst_buffer_new_allocate: sym!(gst_handle, "gst_buffer_new_allocate"),
            gst_buffer_fill: sym!(gst_handle, "gst_buffer_fill"),
            gst_bin_get_by_name: sym!(gst_handle, "gst_bin_get_by_name"),

            gst_app_src_push_buffer: sym!(gstapp_handle, "gst_app_src_push_buffer"),
            gst_app_src_set_caps: sym!(gstapp_handle, "gst_app_src_set_caps"),
            gst_app_src_end_of_stream: sym!(gstapp_handle, "gst_app_src_end_of_stream"),
            gst_app_sink_try_pull_sample: sym!(gstapp_handle, "gst_app_sink_try_pull_sample"),
            gst_app_sink_is_eos: sym!(gstapp_handle, "gst_app_sink_is_eos"),

            _gst_handle: gst_handle,
            _gstapp_handle: gstapp_handle,
        };

        (lib.gst_init)(ptr::null_mut(), ptr::null_mut());
        Some(lib)
    }
}

// ---------------------------------------------------------------------------
// Capability probes
// ---------------------------------------------------------------------------

static H264_GSTREAMER_SUPPORT: OnceLock<(bool, bool)> = OnceLock::new();

pub fn has_gstreamer_h264_encoder() -> bool {
    H264_GSTREAMER_SUPPORT
        .get_or_init(probe_h264_gstreamer_support)
        .0
}

pub fn has_gstreamer_h264_decoder() -> bool {
    H264_GSTREAMER_SUPPORT
        .get_or_init(probe_h264_gstreamer_support)
        .1
}

fn probe_h264_gstreamer_support() -> (bool, bool) {
    unsafe {
        let Some(lib) = GstLib::try_load() else {
            return (false, false);
        };

        let encoder = probe_pipeline(
            &lib,
            "appsrc name=src is-live=true format=3 block=true ! \
             x264enc tune=zerolatency speed-preset=ultrafast byte-stream=true key-int-max=30 bframes=0 aud=false bitrate=512 ! \
             h264parse config-interval=-1 ! \
             video/x-h264,stream-format=byte-stream,alignment=au ! \
             appsink name=sink emit-signals=false sync=false",
            Some("video/x-raw,format=I420,width=64,height=64,framerate=30/1"),
        );

        let decoder = probe_pipeline(
            &lib,
            "appsrc name=src is-live=true format=3 ! \
             h264parse ! avdec_h264 ! videoconvert ! \
             video/x-raw,format=I420 ! \
             appsink name=sink emit-signals=false sync=false",
            Some("video/x-h264,stream-format=byte-stream,alignment=au,width=64,height=64"),
        );

        (encoder, decoder)
    }
}

unsafe fn probe_pipeline(lib: &GstLib, pipeline_desc: &str, caps_str: Option<&str>) -> bool {
    let pipeline_desc = match CString::new(pipeline_desc) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let mut error: *mut c_void = ptr::null_mut();
    let pipeline = (lib.gst_parse_launch)(pipeline_desc.as_ptr(), &mut error);
    if pipeline.is_null() {
        return false;
    }

    let src_name = CString::new("src").unwrap();
    let sink_name = CString::new("sink").unwrap();
    let appsrc = (lib.gst_bin_get_by_name)(pipeline, src_name.as_ptr());
    let appsink = (lib.gst_bin_get_by_name)(pipeline, sink_name.as_ptr());

    let mut ok = !appsrc.is_null() && !appsink.is_null();

    if ok {
        if let Some(caps_str) = caps_str {
            if let Ok(caps_str) = CString::new(caps_str) {
                let caps = (lib.gst_caps_from_string)(caps_str.as_ptr());
                if !caps.is_null() {
                    (lib.gst_app_src_set_caps)(appsrc, caps);
                    (lib.gst_caps_unref)(caps);
                }
            }
        }

        ok = (lib.gst_element_set_state)(pipeline, GST_STATE_PLAYING) != 0;
        (lib.gst_element_set_state)(pipeline, GST_STATE_NULL);
    }

    if !appsrc.is_null() {
        (lib.gst_object_unref)(appsrc);
    }
    if !appsink.is_null() {
        (lib.gst_object_unref)(appsink);
    }
    (lib.gst_object_unref)(pipeline);
    ok
}

// ---------------------------------------------------------------------------
// GStreamer H.264 decoder
// ---------------------------------------------------------------------------

pub struct GstreamerH264Decoder {
    lib: GstLib,
    pipeline: *mut GstElement,
    appsrc: *mut GstElement,
    appsink: *mut GstElement,
    _bus: *mut GstBus,
    width: u32,
    height: u32,
}

unsafe impl Send for GstreamerH264Decoder {}

impl GstreamerH264Decoder {
    pub fn new(sps_pps_annexb: &[u8], width: u32, height: u32) -> Result<Self, String> {
        let lib = unsafe { GstLib::try_load() }
            .ok_or_else(|| "GStreamer not available".to_string())?;

        unsafe {
            let pipeline_desc = CString::new(
                "appsrc name=src is-live=true format=3 ! \
                 h264parse ! avdec_h264 ! \
                 videoconvert ! \
                 video/x-raw,format=I420 ! \
                 appsink name=sink emit-signals=false sync=false",
            )
            .unwrap();

            let mut error: *mut c_void = ptr::null_mut();
            let pipeline = (lib.gst_parse_launch)(pipeline_desc.as_ptr(), &mut error);
            if pipeline.is_null() {
                return Err("failed to create GStreamer pipeline".into());
            }

            let src_name = CString::new("src").unwrap();
            let sink_name = CString::new("sink").unwrap();
            let appsrc = (lib.gst_bin_get_by_name)(pipeline, src_name.as_ptr());
            let appsink = (lib.gst_bin_get_by_name)(pipeline, sink_name.as_ptr());
            if appsrc.is_null() || appsink.is_null() {
                if !appsrc.is_null() {
                    (lib.gst_object_unref)(appsrc);
                }
                if !appsink.is_null() {
                    (lib.gst_object_unref)(appsink);
                }
                (lib.gst_object_unref)(pipeline);
                return Err("failed to get appsrc/appsink from pipeline".into());
            }

            let caps_str = CString::new(format!(
                "video/x-h264,stream-format=byte-stream,alignment=au,width={},height={}",
                width, height,
            ))
            .unwrap();
            let caps = (lib.gst_caps_from_string)(caps_str.as_ptr());
            if !caps.is_null() {
                (lib.gst_app_src_set_caps)(appsrc, caps);
                (lib.gst_caps_unref)(caps);
            }

            if !sps_pps_annexb.is_empty() {
                let buf =
                    (lib.gst_buffer_new_allocate)(ptr::null_mut(), sps_pps_annexb.len(), ptr::null_mut());
                if !buf.is_null() {
                    (lib.gst_buffer_fill)(buf, 0, sps_pps_annexb.as_ptr(), sps_pps_annexb.len());
                    (lib.gst_app_src_push_buffer)(appsrc, buf);
                }
            }

            let bus = (lib.gst_element_get_bus)(pipeline);
            if (lib.gst_element_set_state)(pipeline, GST_STATE_PLAYING) == 0 {
                if !bus.is_null() {
                    (lib.gst_object_unref)(bus);
                }
                (lib.gst_object_unref)(appsrc);
                (lib.gst_object_unref)(appsink);
                (lib.gst_object_unref)(pipeline);
                return Err("failed to start GStreamer pipeline".into());
            }

            Ok(Self {
                lib,
                pipeline,
                appsrc,
                appsink,
                _bus: bus,
                width,
                height,
            })
        }
    }
}

impl VideoFrameDecoder for GstreamerH264Decoder {
    fn push_data(&mut self, data: &[u8], _pts_ms: u64) -> Result<(), String> {
        unsafe {
            let buf = (self.lib.gst_buffer_new_allocate)(ptr::null_mut(), data.len(), ptr::null_mut());
            if buf.is_null() {
                return Err("failed to allocate GStreamer buffer".into());
            }
            (self.lib.gst_buffer_fill)(buf, 0, data.as_ptr(), data.len());

            let ret = (self.lib.gst_app_src_push_buffer)(self.appsrc, buf);
            if ret != GST_FLOW_OK {
                return Err(format!("gst_app_src_push_buffer failed: {}", ret));
            }
            Ok(())
        }
    }

    fn pull_frame(&mut self) -> Result<Option<MseDecodedFrame>, String> {
        unsafe {
            let sample = (self.lib.gst_app_sink_try_pull_sample)(self.appsink, 5_000_000);
            if sample.is_null() {
                return Ok(None);
            }

            let buffer = (self.lib.gst_sample_get_buffer)(sample);
            if buffer.is_null() {
                (self.lib.gst_mini_object_unref)(sample);
                return Ok(None);
            }

            let caps = (self.lib.gst_sample_get_caps)(sample);
            let (w, h) = if !caps.is_null() {
                let structure = (self.lib.gst_caps_get_structure)(caps, 0);
                let mut w: c_int = self.width as c_int;
                let mut h: c_int = self.height as c_int;
                if !structure.is_null() {
                    let width_key = CString::new("width").unwrap();
                    let height_key = CString::new("height").unwrap();
                    (self.lib.gst_structure_get_int)(structure, width_key.as_ptr(), &mut w);
                    (self.lib.gst_structure_get_int)(structure, height_key.as_ptr(), &mut h);
                }
                (w as u32, h as u32)
            } else {
                (self.width, self.height)
            };

            let mut map = GstMapInfo::default();
            if (self.lib.gst_buffer_map)(buffer, &mut map, GST_MAP_READ) == 0 {
                (self.lib.gst_mini_object_unref)(sample);
                return Ok(None);
            }

            let y_size = (w * h) as usize;
            let uv_size = ((w / 2) * (h / 2)) as usize;
            let expected = y_size + 2 * uv_size;

            let frame = if map.size >= expected {
                let data = std::slice::from_raw_parts(map.data, map.size);
                let y = data[..y_size].to_vec();
                let u = data[y_size..y_size + uv_size].to_vec();
                let v = data[y_size + uv_size..y_size + 2 * uv_size].to_vec();

                Some(MseDecodedFrame {
                    track_id: 0,
                    pts_ms: 0,
                    yuv: YuvPlaneData {
                        y,
                        u,
                        v,
                        width: w,
                        height: h,
                        layout: YuvLayout::I420,
                        matrix: YuvColorMatrix::BT709,
                    },
                })
            } else {
                None
            };

            (self.lib.gst_buffer_unmap)(buffer, &mut map);
            (self.lib.gst_mini_object_unref)(sample);
            Ok(frame)
        }
    }

    fn flush(&mut self) {
        unsafe {
            (self.lib.gst_element_set_state)(self.pipeline, GST_STATE_NULL);
            (self.lib.gst_element_set_state)(self.pipeline, GST_STATE_PLAYING);
        }
    }
}

impl Drop for GstreamerH264Decoder {
    fn drop(&mut self) {
        unsafe {
            (self.lib.gst_element_set_state)(self.pipeline, GST_STATE_NULL);
            if !self._bus.is_null() {
                (self.lib.gst_object_unref)(self._bus);
            }
            (self.lib.gst_object_unref)(self.appsrc);
            (self.lib.gst_object_unref)(self.appsink);
            (self.lib.gst_object_unref)(self.pipeline);
        }
    }
}

// ---------------------------------------------------------------------------
// GStreamer H.264 encoder
// ---------------------------------------------------------------------------

struct EncoderSharedQueue {
    queue: Mutex<VecDeque<CameraFrameOwned>>,
    condvar: Condvar,
}

struct GstreamerH264OutputState {
    output: VideoOutputFn,
    config_id: u32,
    last_emitted_config_id: Option<u32>,
    active_config_annexb: Vec<u8>,
}

struct GstreamerEncoderPipeline {
    lib: GstLib,
    pipeline: *mut GstElement,
    appsrc: *mut GstElement,
    appsink: *mut GstElement,
    bus: *mut GstBus,
}

unsafe impl Send for GstreamerEncoderPipeline {}

impl GstreamerEncoderPipeline {
    unsafe fn create(config: &VideoEncoderConfig) -> Option<Self> {
        let lib = GstLib::try_load()?;
        let bitrate_kbps = ((config.target_bitrate.max(1) + 999) / 1000).max(1);
        let keyint = config.keyint.max(1);

        let pipeline_desc = CString::new(format!(
            "appsrc name=src is-live=true format=3 block=true do-timestamp=false ! \
             x264enc tune=zerolatency speed-preset=ultrafast byte-stream=true key-int-max={} bframes=0 aud=false bitrate={} ! \
             h264parse config-interval=-1 ! \
             video/x-h264,stream-format=byte-stream,alignment=au ! \
             appsink name=sink emit-signals=false sync=false max-buffers=8 drop=true",
            keyint, bitrate_kbps,
        ))
        .ok()?;

        let mut error: *mut c_void = ptr::null_mut();
        let pipeline = (lib.gst_parse_launch)(pipeline_desc.as_ptr(), &mut error);
        if pipeline.is_null() {
            return None;
        }

        let src_name = CString::new("src").unwrap();
        let sink_name = CString::new("sink").unwrap();
        let appsrc = (lib.gst_bin_get_by_name)(pipeline, src_name.as_ptr());
        let appsink = (lib.gst_bin_get_by_name)(pipeline, sink_name.as_ptr());
        if appsrc.is_null() || appsink.is_null() {
            if !appsrc.is_null() {
                (lib.gst_object_unref)(appsrc);
            }
            if !appsink.is_null() {
                (lib.gst_object_unref)(appsink);
            }
            (lib.gst_object_unref)(pipeline);
            return None;
        }

        let caps_str = CString::new(format!(
            "video/x-raw,format=I420,width={},height={},framerate={}/{}",
            config.width,
            config.height,
            config.fps_num,
            config.fps_den.max(1),
        ))
        .ok()?;
        let caps = (lib.gst_caps_from_string)(caps_str.as_ptr());
        if caps.is_null() {
            (lib.gst_object_unref)(appsrc);
            (lib.gst_object_unref)(appsink);
            (lib.gst_object_unref)(pipeline);
            return None;
        }
        (lib.gst_app_src_set_caps)(appsrc, caps);
        (lib.gst_caps_unref)(caps);

        let bus = (lib.gst_element_get_bus)(pipeline);
        if (lib.gst_element_set_state)(pipeline, GST_STATE_PLAYING) == 0 {
            if !bus.is_null() {
                (lib.gst_object_unref)(bus);
            }
            (lib.gst_object_unref)(appsrc);
            (lib.gst_object_unref)(appsink);
            (lib.gst_object_unref)(pipeline);
            return None;
        }

        Some(Self {
            lib,
            pipeline,
            appsrc,
            appsink,
            bus,
        })
    }

    unsafe fn destroy(&mut self) {
        (self.lib.gst_element_set_state)(self.pipeline, GST_STATE_NULL);
        if !self.bus.is_null() {
            (self.lib.gst_object_unref)(self.bus);
            self.bus = ptr::null_mut();
        }
        if !self.appsrc.is_null() {
            (self.lib.gst_object_unref)(self.appsrc);
            self.appsrc = ptr::null_mut();
        }
        if !self.appsink.is_null() {
            (self.lib.gst_object_unref)(self.appsink);
            self.appsink = ptr::null_mut();
        }
        if !self.pipeline.is_null() {
            (self.lib.gst_object_unref)(self.pipeline);
            self.pipeline = ptr::null_mut();
        }
    }
}

pub struct GstreamerH264Encoder {
    running: Arc<AtomicBool>,
    queue: Arc<EncoderSharedQueue>,
    queue_policy: VideoQueuePolicy,
    queue_capacity: usize,
    output_state: Arc<Mutex<GstreamerH264OutputState>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl GstreamerH264Encoder {
    pub fn start(config: VideoEncoderConfig, output: VideoOutputFn) -> Option<Self> {
        if config.codec != VideoCodec::H264 {
            return None;
        }
        if config.width == 0 || config.height == 0 || config.fps_num == 0 {
            return None;
        }

        let pipeline = unsafe { GstreamerEncoderPipeline::create(&config) }?;
        let running = Arc::new(AtomicBool::new(true));
        let queue = Arc::new(EncoderSharedQueue {
            queue: Mutex::new(VecDeque::new()),
            condvar: Condvar::new(),
        });
        let output_state = Arc::new(Mutex::new(GstreamerH264OutputState {
            output,
            config_id: 0,
            last_emitted_config_id: None,
            active_config_annexb: Vec::new(),
        }));

        let worker = std::thread::Builder::new()
            .name("gstreamer-h264-encoder".to_string())
            .spawn({
                let running = running.clone();
                let queue = queue.clone();
                let output_state = output_state.clone();
                move || worker_loop(config, running, queue, output_state, pipeline)
            })
            .ok()?;

        Some(Self {
            running,
            queue,
            queue_policy: config.queue_policy,
            queue_capacity: config.queue_capacity.max(1),
            output_state,
            worker: Mutex::new(Some(worker)),
        })
    }

    pub fn stop(&self) {
        if self.running.swap(false, Ordering::SeqCst) {
            self.queue.condvar.notify_all();
            if let Some(worker) = self.worker.lock().unwrap().take() {
                let _ = worker.join();
            }

            let mut st = self.output_state.lock().unwrap();
            let config_id = st.config_id;
            (st.output)(EncodedVideoPacketRef {
                codec: VideoCodec::H264,
                format: VideoBitstreamFormat::AnnexB,
                pts_ns: 0,
                dts_ns: None,
                is_key: false,
                is_config: false,
                is_eos: true,
                config_id,
                data: &[],
            });
        }
    }
}

impl MediaVideoEncoder for GstreamerH264Encoder {
    fn push_frame(&self, frame: CameraFrameRef<'_>) {
        if !self.running.load(Ordering::Relaxed) {
            return;
        }

        let mut owned = CameraFrameOwned::default();
        if !owned.convert_to_i420(frame) {
            return;
        }

        let mut q = self.queue.queue.lock().unwrap();
        match self.queue_policy {
            VideoQueuePolicy::LatestWins => {
                if q.len() >= self.queue_capacity {
                    q.pop_front();
                }
            }
        }
        q.push_back(owned);
        self.queue.condvar.notify_one();
    }

    fn request_keyframe(&self) -> Result<(), VideoEncodeError> {
        Err(VideoEncodeError::UnsupportedCodec)
    }

    fn stop(&self) {
        Self::stop(self);
    }
}

impl Drop for GstreamerH264Encoder {
    fn drop(&mut self) {
        self.stop();
    }
}

fn worker_loop(
    config: VideoEncoderConfig,
    running: Arc<AtomicBool>,
    queue: Arc<EncoderSharedQueue>,
    output_state: Arc<Mutex<GstreamerH264OutputState>>,
    mut pipeline: GstreamerEncoderPipeline,
) {
    let mut pts_queue = VecDeque::new();

    loop {
        while let Some(sample) = try_pull_sample(&pipeline, 0) {
            process_encoded_sample(&pipeline.lib, sample, &mut pts_queue, &output_state);
        }

        let frame = {
            let mut guard = queue.queue.lock().unwrap();
            if guard.is_empty() && running.load(Ordering::Relaxed) {
                let (next, _) = queue
                    .condvar
                    .wait_timeout(guard, Duration::from_millis(10))
                    .unwrap();
                guard = next;
            }
            guard.pop_front()
        };

        if let Some(frame) = frame {
            if frame.width as u32 != config.width || frame.height as u32 != config.height {
                continue;
            }
            if let Some(data) = pack_i420_frame(&frame) {
                unsafe {
                    let buf = (pipeline.lib.gst_buffer_new_allocate)(ptr::null_mut(), data.len(), ptr::null_mut());
                    if !buf.is_null() {
                        (pipeline.lib.gst_buffer_fill)(buf, 0, data.as_ptr(), data.len());
                        if (pipeline.lib.gst_app_src_push_buffer)(pipeline.appsrc, buf) == GST_FLOW_OK {
                            pts_queue.push_back(frame.timestamp_ns);
                        }
                    }
                }
            }
            continue;
        }

        if !running.load(Ordering::Relaxed) {
            let guard = queue.queue.lock().unwrap();
            if guard.is_empty() {
                break;
            }
        }
    }

    unsafe {
        (pipeline.lib.gst_app_src_end_of_stream)(pipeline.appsrc);
    }

    for _ in 0..64 {
        if let Some(sample) = try_pull_sample(&pipeline, 10_000_000) {
            process_encoded_sample(&pipeline.lib, sample, &mut pts_queue, &output_state);
            continue;
        }
        unsafe {
            if (pipeline.lib.gst_app_sink_is_eos)(pipeline.appsink) != 0 {
                break;
            }
        }
    }

    unsafe {
        pipeline.destroy();
    }
}

fn try_pull_sample(pipeline: &GstreamerEncoderPipeline, timeout_ns: u64) -> Option<*mut GstSample> {
    unsafe {
        let sample = (pipeline.lib.gst_app_sink_try_pull_sample)(pipeline.appsink, timeout_ns);
        if sample.is_null() {
            None
        } else {
            Some(sample)
        }
    }
}

fn pack_i420_frame(frame: &CameraFrameOwned) -> Option<Vec<u8>> {
    if frame.layout != makepad_platform::CameraFrameLayout::I420 || frame.plane_count < 3 {
        return None;
    }

    let y_size = frame.width * frame.height;
    let cw = frame.width.div_ceil(2);
    let ch = frame.height.div_ceil(2);
    let uv_size = cw * ch;

    if frame.planes[0].bytes.len() < y_size
        || frame.planes[1].bytes.len() < uv_size
        || frame.planes[2].bytes.len() < uv_size
    {
        return None;
    }

    let mut out = Vec::with_capacity(y_size + uv_size * 2);
    out.extend_from_slice(&frame.planes[0].bytes[..y_size]);
    out.extend_from_slice(&frame.planes[1].bytes[..uv_size]);
    out.extend_from_slice(&frame.planes[2].bytes[..uv_size]);
    Some(out)
}

fn split_config_and_access_unit_annexb(data: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut sps = Vec::new();
    let mut pps = Vec::new();
    let mut sample = Vec::new();

    for nal in h264_packets::split_annexb_nals(data) {
        if nal.is_empty() {
            continue;
        }
        match nal[0] & 0x1f {
            7 => sps.push(nal.to_vec()),
            8 => pps.push(nal.to_vec()),
            9 => {}
            _ => {
                sample.extend_from_slice(&[0, 0, 0, 1]);
                sample.extend_from_slice(nal);
            }
        }
    }

    let config = if !sps.is_empty() && !pps.is_empty() {
        h264_packets::sps_pps_to_annexb(&sps, &pps)
    } else {
        Vec::new()
    };

    (config, sample)
}

fn process_encoded_sample(
    lib: &GstLib,
    sample: *mut GstSample,
    pts_queue: &mut VecDeque<u64>,
    output_state: &Arc<Mutex<GstreamerH264OutputState>>,
) {
    unsafe {
        let buffer = (lib.gst_sample_get_buffer)(sample);
        if buffer.is_null() {
            (lib.gst_mini_object_unref)(sample);
            return;
        }

        let mut map = GstMapInfo::default();
        if (lib.gst_buffer_map)(buffer, &mut map, GST_MAP_READ) == 0 {
            (lib.gst_mini_object_unref)(sample);
            return;
        }

        let raw = std::slice::from_raw_parts(map.data, map.size).to_vec();
        (lib.gst_buffer_unmap)(buffer, &mut map);
        (lib.gst_mini_object_unref)(sample);

        if raw.is_empty() {
            return;
        }

        let data = if h264_packets::starts_with_annexb(&raw) {
            raw
        } else if let Some(annexb) = h264_packets::avcc_sample_to_annexb(&raw, 4) {
            annexb
        } else {
            return;
        };

        let (config_annexb, sample_annexb) = split_config_and_access_unit_annexb(&data);
        let mut st = output_state.lock().unwrap();

        if !config_annexb.is_empty() {
            if st.active_config_annexb != config_annexb {
                st.config_id = st.config_id.saturating_add(1);
                st.active_config_annexb = config_annexb.clone();
            }
            let config_id = st.config_id;
            if st.last_emitted_config_id != Some(config_id) {
                let config_bytes = st.active_config_annexb.clone();
                (st.output)(EncodedVideoPacketRef {
                    codec: VideoCodec::H264,
                    format: VideoBitstreamFormat::AnnexB,
                    pts_ns: pts_queue.front().copied().unwrap_or(0),
                    dts_ns: None,
                    is_key: false,
                    is_config: true,
                    is_eos: false,
                    config_id,
                    data: &config_bytes,
                });
                st.last_emitted_config_id = Some(config_id);
            }
        }

        if sample_annexb.is_empty() || st.active_config_annexb.is_empty() {
            return;
        }

        let pts_ns = pts_queue.pop_front().unwrap_or(0);
        let is_key = h264_packets::contains_idr_annexb(&sample_annexb);
        let config_id = st.config_id;

        if is_key && st.last_emitted_config_id != Some(config_id) {
            let config_bytes = st.active_config_annexb.clone();
            (st.output)(EncodedVideoPacketRef {
                codec: VideoCodec::H264,
                format: VideoBitstreamFormat::AnnexB,
                pts_ns,
                dts_ns: None,
                is_key: false,
                is_config: true,
                is_eos: false,
                config_id,
                data: &config_bytes,
            });
            st.last_emitted_config_id = Some(config_id);
        }

        (st.output)(EncodedVideoPacketRef {
            codec: VideoCodec::H264,
            format: VideoBitstreamFormat::AnnexB,
            pts_ns,
            dts_ns: None,
            is_key,
            is_config: false,
            is_eos: false,
            config_id,
            data: &sample_annexb,
        });
    }
}
