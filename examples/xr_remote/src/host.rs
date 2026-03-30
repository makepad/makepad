use crate::{protocol::*, wire::*};
use makepad_widgets::makepad_platform::{
    event::xr::XrState,
    thread::SignalToUI,
    video::{
        CameraFrameLayout, VideoCodec, VideoEncodeSource, VideoEncoderConfig, VideoQueuePolicy,
    },
};
use makepad_widgets::*;
use std::{
    io::Read,
    net::TcpStream,
    sync::{Arc, Mutex},
    thread,
};

#[cfg(not(target_os = "macos"))]
use makepad_widgets::makepad_platform::video::{
    convert_bgra_8888_to_i420, CameraColorMatrix, CameraFrameOwned, CameraFramePlaneRef,
    CameraFrameRef,
};

#[cfg(target_os = "macos")]
use makepad_widgets::makepad_platform::os::apple::apple_sys::{
    CFArrayGetCount, CFArrayGetValueAtIndex, CFDictionaryContainsKey, CFDictionaryCreate,
    CFNumberCreate, CFRelease, CMBlockBufferCopyDataBytes, CMBlockBufferGetDataLength,
    CMSampleBufferDataIsReady, CMSampleBufferGetDataBuffer, CMSampleBufferGetFormatDescription,
    CMSampleBufferGetPresentationTimeStamp, CMSampleBufferGetSampleAttachmentsArray,
    CMTimeGetSeconds, CMTimeMakeWithSeconds, CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
    CVImageBufferRef, CVPixelBufferCreate, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBytesPerRow, CVPixelBufferLockBaseAddress, CVPixelBufferRef,
    CVPixelBufferRelease, CVPixelBufferUnlockBaseAddress, OSStatus, VTCompressionSessionCreate,
    VTCompressionSessionEncodeFrame, VTCompressionSessionInvalidate,
    VTCompressionSessionPrepareToEncodeFrames, VTCompressionSessionRef, VTEncodeInfoFlags,
    VTSessionSetProperty, kCFBooleanFalse, kCFBooleanTrue, kCFNumberSInt32Type,
    kCMSampleAttachmentKey_NotSync, kCMVideoCodecType_H264, kCVPixelFormatType_32BGRA,
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_RealTime, kVTEncodeFrameOptionKey_ForceKeyFrame,
};
#[cfg(target_os = "macos")]
use std::{ffi::c_void, ptr, slice};

script_mod! {
    use mod.prelude.widgets.*

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(920, 620)
                body +: {
                    SolidView{
                        width: Fill
                        height: Fill
                        flow: Down
                        padding: 20
                        spacing: 12
                        draw_bg.color: #x111923

                        Label{
                            text: "XR Remote Host"
                            draw_text.color: #xf2f7fb
                            draw_text.text_style.font_size: 22.0
                        }

                        host_status := Label{
                            text: "Booting host..."
                            draw_text.color: #xc4d2de
                        }

                        host_pose := Label{
                            text: "Pose: waiting"
                            draw_text.color: #x97adc2
                        }

                        host_stream := Label{
                            text: "Stream: waiting"
                            draw_text.color: #x97adc2
                        }

                        host_remote_log := Label{
                            text: "Remote: waiting"
                            draw_text.color: #x97adc2
                        }
                    }
                }
            }
        }
    }
}

#[derive(Default, Clone)]
struct HostShared {
    control_writer: Arc<Mutex<Option<TcpStream>>>,
    video_writer: Arc<Mutex<Option<TcpStream>>>,
    control_inbox: Arc<Mutex<Vec<ControlPacket>>>,
    last_config: Arc<Mutex<Option<VideoConfigPacket>>>,
    video_client_connected: Arc<Mutex<bool>>,
    require_keyframe_after_connect: Arc<Mutex<bool>>,
    sent_video_packets: Arc<Mutex<u64>>,
    sent_video_bytes: Arc<Mutex<u64>>,
}

impl HostShared {
    fn new() -> Self {
        Self {
            control_writer: Arc::new(Mutex::new(None)),
            video_writer: Arc::new(Mutex::new(None)),
            control_inbox: Arc::new(Mutex::new(Vec::new())),
            last_config: Arc::new(Mutex::new(None)),
            video_client_connected: Arc::new(Mutex::new(false)),
            require_keyframe_after_connect: Arc::new(Mutex::new(false)),
            sent_video_packets: Arc::new(Mutex::new(0)),
            sent_video_bytes: Arc::new(Mutex::new(0)),
        }
    }

    fn start_threads(&self) {
        let control_writer = self.control_writer.clone();
        let control_inbox = self.control_inbox.clone();
        thread::spawn(move || {
            let listener = bind_listener(control_port());
            for incoming in listener.incoming() {
                let Ok(mut stream) = incoming else {
                    continue;
                };
                let _ = stream.set_nodelay(true);
                if let Ok(writer) = stream.try_clone() {
                    *control_writer.lock().unwrap() = Some(writer);
                }
                SignalToUI::set_ui_signal();
                while let Ok(packet) = recv_framed::<ControlPacket>(&mut stream) {
                    control_inbox.lock().unwrap().push(packet);
                    SignalToUI::set_ui_signal();
                }
                *control_writer.lock().unwrap() = None;
                SignalToUI::set_ui_signal();
            }
        });

        let video_writer = self.video_writer.clone();
        let last_config = self.last_config.clone();
        let video_client_connected = self.video_client_connected.clone();
        let require_keyframe_after_connect = self.require_keyframe_after_connect.clone();
        thread::spawn(move || {
            let listener = bind_listener(video_port());
            for incoming in listener.incoming() {
                let Ok(mut stream) = incoming else {
                    continue;
                };
                let _ = stream.set_nodelay(true);
                let stream_config = VideoPacket::StreamConfig(StreamConfigPacket {
                    codec: "h264-annexb".to_string(),
                    width: XR_REMOTE_STREAM_WIDTH,
                    height: XR_REMOTE_STREAM_HEIGHT,
                    fps: XR_REMOTE_STREAM_FPS,
                    config_id: 0,
                });
                let _ = send_framed(&mut stream, &stream_config);
                if let Some(config) = last_config.lock().unwrap().clone() {
                    let _ = send_framed(&mut stream, &VideoPacket::VideoConfig(config));
                }
                if let Ok(writer) = stream.try_clone() {
                    *video_writer.lock().unwrap() = Some(writer);
                }
                *require_keyframe_after_connect.lock().unwrap() = true;
                *video_client_connected.lock().unwrap() = true;
                SignalToUI::set_ui_signal();

                let mut probe = [0u8; 1];
                loop {
                    match stream.read(&mut probe) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }

                *video_writer.lock().unwrap() = None;
                *video_client_connected.lock().unwrap() = false;
                *require_keyframe_after_connect.lock().unwrap() = false;
                SignalToUI::set_ui_signal();
            }
        });
    }

    fn send_control(&self, packet: &ControlPacket) {
        let mut guard = self.control_writer.lock().unwrap();
        let Some(stream) = guard.as_mut() else {
            return;
        };
        if send_framed(stream, packet).is_err() {
            *guard = None;
            SignalToUI::set_ui_signal();
        }
    }

    fn send_video_packet(&self, packet: &VideoPacket) {
        if let VideoPacket::VideoFrame(frame) = packet {
            let mut require_keyframe = self.require_keyframe_after_connect.lock().unwrap();
            if *require_keyframe {
                if !frame.is_key {
                    return;
                }
                *require_keyframe = false;
            }
        }
        let mut guard = self.video_writer.lock().unwrap();
        let Some(stream) = guard.as_mut() else {
            return;
        };
        if send_framed(stream, packet).is_err() {
            *guard = None;
            *self.video_client_connected.lock().unwrap() = false;
            *self.require_keyframe_after_connect.lock().unwrap() = false;
            SignalToUI::set_ui_signal();
            return;
        }
        let packet_bytes = match packet {
            VideoPacket::StreamConfig(_) => 0,
            VideoPacket::VideoConfig(config) => config.bytes.len() as u64,
            VideoPacket::VideoFrame(frame) => frame.bytes.len() as u64,
        };
        *self.sent_video_packets.lock().unwrap() += 1;
        *self.sent_video_bytes.lock().unwrap() += packet_bytes;
    }

    fn drain_control(&self) -> Vec<ControlPacket> {
        let mut inbox = self.control_inbox.lock().unwrap();
        std::mem::take(&mut *inbox)
    }

    fn video_debug_counters(&self) -> (bool, u64, u64) {
        (
            *self.video_client_connected.lock().unwrap(),
            *self.sent_video_packets.lock().unwrap(),
            *self.sent_video_bytes.lock().unwrap(),
        )
    }
}

#[derive(Default)]
enum HostEncoder {
    #[default]
    None,
    #[cfg(not(target_os = "macos"))]
    Platform,
    #[cfg(target_os = "macos")]
    Mac(MacHostEncoder),
}

#[cfg(target_os = "macos")]
type MacHostEncoder = mac_vt_h264::Encoder;

#[cfg(target_os = "macos")]
mod mac_vt_h264 {
    use super::*;

    struct CallbackState {
        shared: HostShared,
        config_id: u32,
        last_config: Vec<u8>,
    }

    pub struct Encoder {
        session: VTCompressionSessionRef,
        state: *mut Mutex<CallbackState>,
        width: usize,
        height: usize,
        fps_num: u32,
        fps_den: u32,
    }

    impl Encoder {
        pub fn new(config: VideoEncoderConfig, shared: HostShared) -> Result<Self, String> {
            unsafe {
                let state = Box::into_raw(Box::new(Mutex::new(CallbackState {
                    shared,
                    config_id: 0,
                    last_config: Vec::new(),
                })));

                let mut session: VTCompressionSessionRef = ptr::null_mut();
                let status = VTCompressionSessionCreate(
                    ptr::null(),
                    config.width,
                    config.height,
                    kCMVideoCodecType_H264,
                    ptr::null(),
                    ptr::null(),
                    ptr::null_mut(),
                    Some(output_callback),
                    state as *mut c_void,
                    &mut session,
                );
                if status != 0 || session.is_null() {
                    drop(Box::from_raw(state));
                    return Err(format!("VTCompressionSessionCreate failed: {status}"));
                }

                if let Err(err) = set_bool_property(session, kVTCompressionPropertyKey_RealTime, config.latency_realtime) {
                    VTCompressionSessionInvalidate(session);
                    CFRelease(session);
                    drop(Box::from_raw(state));
                    return Err(err);
                }
                if let Err(err) =
                    set_i32_property(session, kVTCompressionPropertyKey_AverageBitRate, config.target_bitrate as i32)
                {
                    VTCompressionSessionInvalidate(session);
                    CFRelease(session);
                    drop(Box::from_raw(state));
                    return Err(err);
                }
                if let Err(err) =
                    set_i32_property(session, kVTCompressionPropertyKey_ExpectedFrameRate, config.fps_num as i32)
                {
                    VTCompressionSessionInvalidate(session);
                    CFRelease(session);
                    drop(Box::from_raw(state));
                    return Err(err);
                }
                if let Err(err) =
                    set_i32_property(session, kVTCompressionPropertyKey_MaxKeyFrameInterval, config.keyint as i32)
                {
                    VTCompressionSessionInvalidate(session);
                    CFRelease(session);
                    drop(Box::from_raw(state));
                    return Err(err);
                }
                if let Err(err) =
                    set_bool_property(session, kVTCompressionPropertyKey_AllowFrameReordering, false)
                {
                    VTCompressionSessionInvalidate(session);
                    CFRelease(session);
                    drop(Box::from_raw(state));
                    return Err(err);
                }

                let status = VTCompressionSessionPrepareToEncodeFrames(session);
                if status != 0 {
                    VTCompressionSessionInvalidate(session);
                    CFRelease(session);
                    drop(Box::from_raw(state));
                    return Err(format!("VTCompressionSessionPrepareToEncodeFrames failed: {status}"));
                }

                Ok(Self {
                    session,
                    state,
                    width: config.width as usize,
                    height: config.height as usize,
                    fps_num: config.fps_num,
                    fps_den: config.fps_den,
                })
            }
        }

        pub fn encode_bgra(
            &mut self,
            bgra: &[u8],
            timestamp_ns: u64,
            force_keyframe: bool,
        ) -> Result<(), String> {
            if bgra.len() < self.width.saturating_mul(self.height).saturating_mul(4) {
                return Err("BGRA frame buffer is too small".to_string());
            }
            unsafe {
                let mut pixel_buffer: CVPixelBufferRef = ptr::null_mut();
                let status = CVPixelBufferCreate(
                    ptr::null(),
                    self.width,
                    self.height,
                    kCVPixelFormatType_32BGRA,
                    ptr::null(),
                    &mut pixel_buffer,
                );
                if status != 0 || pixel_buffer.is_null() {
                    return Err(format!("CVPixelBufferCreate failed: {status}"));
                }

                let lock_status = CVPixelBufferLockBaseAddress(pixel_buffer, 0);
                if lock_status != 0 {
                    CVPixelBufferRelease(pixel_buffer);
                    return Err(format!("CVPixelBufferLockBaseAddress failed: {lock_status}"));
                }

                let dst = CVPixelBufferGetBaseAddress(pixel_buffer) as *mut u8;
                let row_stride = CVPixelBufferGetBytesPerRow(pixel_buffer);
                let src_row_stride = self.width * 4;
                for row in 0..self.height {
                    ptr::copy_nonoverlapping(
                        bgra.as_ptr().add(row * src_row_stride),
                        dst.add(row * row_stride),
                        src_row_stride,
                    );
                }

                let _ = CVPixelBufferUnlockBaseAddress(pixel_buffer, 0);

                let pts = CMTimeMakeWithSeconds(timestamp_ns as f64 / 1_000_000_000.0, 600);
                let duration = CMTimeMakeWithSeconds(
                    self.fps_den.max(1) as f64 / self.fps_num.max(1) as f64,
                    600,
                );
                let frame_props = if force_keyframe {
                    create_force_keyframe_dict()
                } else {
                    ptr::null()
                };
                let mut info_flags: VTEncodeInfoFlags = 0;
                let status = VTCompressionSessionEncodeFrame(
                    self.session,
                    pixel_buffer as CVImageBufferRef,
                    pts,
                    duration,
                    frame_props,
                    ptr::null_mut(),
                    &mut info_flags,
                );
                if !frame_props.is_null() {
                    CFRelease(frame_props);
                }
                CVPixelBufferRelease(pixel_buffer);

                if status != 0 {
                    return Err(format!("VTCompressionSessionEncodeFrame failed: {status}"));
                }
            }
            Ok(())
        }
    }

    impl Drop for Encoder {
        fn drop(&mut self) {
            unsafe {
                VTCompressionSessionInvalidate(self.session);
                CFRelease(self.session);
                drop(Box::from_raw(self.state));
            }
        }
    }

    unsafe fn set_i32_property(
        session: VTCompressionSessionRef,
        key: makepad_widgets::makepad_platform::os::apple::apple_sys::CFStringRef,
        value: i32,
    ) -> Result<(), String> {
        let number = CFNumberCreate(ptr::null(), kCFNumberSInt32Type, &value as *const _ as *const c_void);
        if number.is_null() {
            return Err("CFNumberCreate failed".to_string());
        }
        let status = VTSessionSetProperty(session, key, number);
        CFRelease(number);
        if status != 0 {
            return Err(format!("VTSessionSetProperty failed: {status}"));
        }
        Ok(())
    }

    unsafe fn set_bool_property(
        session: VTCompressionSessionRef,
        key: makepad_widgets::makepad_platform::os::apple::apple_sys::CFStringRef,
        value: bool,
    ) -> Result<(), String> {
        let bool_ref = if value { kCFBooleanTrue } else { kCFBooleanFalse };
        let status = VTSessionSetProperty(session, key, bool_ref);
        if status != 0 {
            return Err(format!("VTSessionSetProperty failed: {status}"));
        }
        Ok(())
    }

    unsafe fn create_force_keyframe_dict(
    ) -> makepad_widgets::makepad_platform::os::apple::apple_sys::CFDictionaryRef {
        let keys = [kVTEncodeFrameOptionKey_ForceKeyFrame as *const c_void];
        let values = [kCFBooleanTrue as *const c_void];
        CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            ptr::null(),
            ptr::null(),
        )
    }

    unsafe extern "C" fn output_callback(
        output_callback_ref_con: *mut c_void,
        _source_frame_ref_con: *mut c_void,
        status: OSStatus,
        _info_flags: VTEncodeInfoFlags,
        sample_buffer: makepad_widgets::makepad_platform::os::apple::apple_sys::CMSampleBufferRef,
    ) {
        if status != 0 || output_callback_ref_con.is_null() || sample_buffer.is_null() {
            return;
        }
        if !CMSampleBufferDataIsReady(sample_buffer) {
            return;
        }

        let state_mutex = &*(output_callback_ref_con as *const Mutex<CallbackState>);
        let format_desc = CMSampleBufferGetFormatDescription(sample_buffer);
        if format_desc.is_null() {
            return;
        }
        let Some((config_bytes, nal_header_len)) = extract_config_and_nal_size(format_desc) else {
            return;
        };

        let data_buffer = CMSampleBufferGetDataBuffer(sample_buffer);
        if data_buffer.is_null() {
            return;
        }
        let data_len = CMBlockBufferGetDataLength(data_buffer);
        if data_len <= 0 {
            return;
        }
        let mut avcc = vec![0u8; data_len as usize];
        if CMBlockBufferCopyDataBytes(
            data_buffer,
            0,
            data_len,
            avcc.as_mut_ptr() as *mut c_void,
        ) != 0
        {
            return;
        }

        let Some(annexb) = avcc_to_annexb(&avcc, nal_header_len) else {
            return;
        };
        let pts_ns =
            (CMTimeGetSeconds(CMSampleBufferGetPresentationTimeStamp(sample_buffer)) * 1_000_000_000.0)
                .max(0.0) as u64;
        let is_key = sample_is_keyframe(sample_buffer);

        let mut state = state_mutex.lock().unwrap();
        if state.last_config != config_bytes {
            state.config_id = state.config_id.wrapping_add(1);
            if state.config_id == 0 {
                state.config_id = 1;
            }
            state.last_config = config_bytes.clone();
            let config = VideoConfigPacket {
                config_id: state.config_id,
                bytes: config_bytes,
            };
            *state.shared.last_config.lock().unwrap() = Some(config.clone());
            state.shared.send_video_packet(&VideoPacket::VideoConfig(config));
        }

        state
            .shared
            .send_video_packet(&VideoPacket::VideoFrame(VideoFramePacket {
                pts_ns,
                is_key,
                is_eos: false,
                config_id: state.config_id.max(1),
                bytes: annexb,
            }));
    }

    unsafe fn sample_is_keyframe(
        sample_buffer: makepad_widgets::makepad_platform::os::apple::apple_sys::CMSampleBufferRef,
    ) -> bool {
        let attachments = CMSampleBufferGetSampleAttachmentsArray(sample_buffer, false);
        if attachments.is_null() || CFArrayGetCount(attachments) == 0 {
            return true;
        }
        let dict = CFArrayGetValueAtIndex(attachments, 0)
            as makepad_widgets::makepad_platform::os::apple::apple_sys::CFDictionaryRef;
        CFDictionaryContainsKey(dict, kCMSampleAttachmentKey_NotSync as *const c_void) == 0
    }

    unsafe fn extract_config_and_nal_size(
        format_desc: makepad_widgets::makepad_platform::os::apple::apple_sys::CMFormatDescriptionRef,
    ) -> Option<(Vec<u8>, usize)> {
        let mut out = Vec::new();
        let mut param_count = 0usize;
        let mut nal_header_len = 4i32;

        let mut index = 0usize;
        loop {
            let mut param_ptr: *const u8 = ptr::null();
            let mut param_size = 0usize;
            let mut count_out = 0usize;
            let mut nal_out = 0i32;
            let status = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                format_desc,
                index,
                &mut param_ptr,
                &mut param_size,
                &mut count_out,
                &mut nal_out,
            );
            if status != 0 || param_ptr.is_null() || param_size == 0 {
                break;
            }
            if index == 0 {
                param_count = count_out;
                nal_header_len = nal_out;
            }
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(slice::from_raw_parts(param_ptr, param_size));

            index += 1;
            if index >= param_count {
                break;
            }
        }

        if out.is_empty() {
            None
        } else {
            Some((out, nal_header_len.max(1) as usize))
        }
    }

    fn avcc_to_annexb(avcc: &[u8], nal_header_len: usize) -> Option<Vec<u8>> {
        if !(1..=4).contains(&nal_header_len) {
            return None;
        }
        let mut out = Vec::with_capacity(avcc.len() + 32);
        let mut offset = 0usize;
        while offset + nal_header_len <= avcc.len() {
            let mut nal_size = 0usize;
            for i in 0..nal_header_len {
                nal_size = (nal_size << 8) | avcc[offset + i] as usize;
            }
            offset += nal_header_len;
            if offset + nal_size > avcc.len() {
                return None;
            }
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(&avcc[offset..offset + nal_size]);
            offset += nal_size;
        }
        Some(out)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    shared: HostShared,
    #[rust]
    frame_timer: Timer,
    #[rust]
    #[cfg(not(target_os = "macos"))]
    frame_owned: CameraFrameOwned,
    #[rust]
    bgra_frame: Vec<u8>,
    #[rust]
    network_started: bool,
    #[rust]
    encoder_started: bool,
    #[rust]
    host_encoder: HostEncoder,
    #[rust]
    latest_state: XrState,
    #[rust]
    latest_status: String,
    #[rust]
    latest_pose_text: String,
    #[rust]
    latest_stream_text: String,
    #[rust]
    latest_remote_log_text: String,
    #[rust]
    frame_counter: u64,
    #[rust]
    last_video_connected: bool,
    #[rust]
    force_keyframe_for_new_client: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            ui: WidgetRef::default(),
            shared: HostShared::new(),
            frame_timer: Timer::default(),
            #[cfg(not(target_os = "macos"))]
            frame_owned: CameraFrameOwned::default(),
            bgra_frame: Vec::new(),
            network_started: false,
            encoder_started: false,
            host_encoder: HostEncoder::None,
            latest_state: XrState::default(),
            latest_status: "Host idle".to_string(),
            latest_pose_text: "Pose: waiting".to_string(),
            latest_stream_text: "Stream: waiting".to_string(),
            latest_remote_log_text: "Remote: waiting".to_string(),
            frame_counter: 0,
            last_video_connected: false,
            force_keyframe_for_new_client: false,
        }
    }
}

impl App {
    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.network_started {
            return;
        }
        self.shared.start_threads();
        self.frame_timer = cx.start_interval(1.0 / XR_REMOTE_STREAM_FPS as f64);
        self.network_started = true;
        self.latest_status = format!(
            "Listening on tcp://0.0.0.0:{} and tcp://0.0.0.0:{}",
            control_port(),
            video_port()
        );
        self.refresh_labels(cx);
    }

    fn ensure_encoder(&mut self, cx: &mut Cx) {
        if self.encoder_started {
            return;
        }
        let config = VideoEncoderConfig {
            codec: VideoCodec::H264,
            source: VideoEncodeSource::CpuFrames {
                layout: CameraFrameLayout::I420,
            },
            width: XR_REMOTE_STREAM_WIDTH,
            height: XR_REMOTE_STREAM_HEIGHT,
            fps_num: XR_REMOTE_STREAM_FPS,
            fps_den: 1,
            target_bitrate: 8_000_000,
            keyint: 2,
            latency_realtime: true,
            codec_mode: 8,
            queue_policy: VideoQueuePolicy::LatestWins,
            queue_capacity: 2,
        };

        #[cfg(target_os = "macos")]
        {
            match MacHostEncoder::new(config, self.shared.clone()) {
                Ok(encoder) => {
                    self.host_encoder = HostEncoder::Mac(encoder);
                    self.encoder_started = true;
                    self.latest_stream_text = format!(
                        "Stream: H264 {}x{} @ {} fps (VideoToolbox)",
                        XR_REMOTE_STREAM_WIDTH, XR_REMOTE_STREAM_HEIGHT, XR_REMOTE_STREAM_FPS
                    );
                    self.refresh_labels(cx);
                    return;
                }
                Err(err) => {
                    self.latest_status = format!("Mac encoder unavailable: {err}");
                    self.latest_stream_text = "Stream: encoder unavailable".to_string();
                    self.refresh_labels(cx);
                    return;
                }
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            let shared = self.shared.clone();
            let result = cx.video_encoder_output_try(
                XR_REMOTE_ENCODER_SLOT,
                config,
                move |packet| {
                    if packet.codec != VideoCodec::H264
                        || packet.format
                            != makepad_widgets::makepad_platform::video::VideoBitstreamFormat::AnnexB
                    {
                        return;
                    }
                    if packet.is_config {
                        let config = VideoConfigPacket {
                            config_id: packet.config_id,
                            bytes: packet.data.to_vec(),
                        };
                        *shared.last_config.lock().unwrap() = Some(config.clone());
                        shared.send_video_packet(&VideoPacket::VideoConfig(config));
                        return;
                    }
                    shared.send_video_packet(&VideoPacket::VideoFrame(VideoFramePacket {
                        pts_ns: packet.pts_ns,
                        is_key: packet.is_key,
                        is_eos: packet.is_eos,
                        config_id: packet.config_id,
                        bytes: packet.data.to_vec(),
                    }));
                },
            );
            if let Err(err) = result {
                self.latest_status = format!("Encoder unavailable: {:?}", err);
                self.latest_stream_text = "Stream: encoder unavailable".to_string();
                self.refresh_labels(cx);
                return;
            }
            self.host_encoder = HostEncoder::Platform;
            self.encoder_started = true;
            self.latest_stream_text = format!(
                "Stream: H264 {}x{} @ {} fps",
                XR_REMOTE_STREAM_WIDTH, XR_REMOTE_STREAM_HEIGHT, XR_REMOTE_STREAM_FPS
            );
            self.refresh_labels(cx);
        }
    }

    fn refresh_labels(&mut self, cx: &mut Cx) {
        let (video_connected, sent_packets, sent_bytes) = self.shared.video_debug_counters();
        let stream_text = format!(
            "{} | video {} | packets {} | bytes {}",
            self.latest_stream_text,
            if video_connected {
                "connected"
            } else {
                "waiting"
            },
            sent_packets,
            sent_bytes
        );
        self.ui
            .widget(cx, ids!(host_status))
            .set_text(cx, &self.latest_status);
        self.ui
            .widget(cx, ids!(host_pose))
            .set_text(cx, &self.latest_pose_text);
        self.ui
            .widget(cx, ids!(host_stream))
            .set_text(cx, &stream_text);
        self.ui
            .widget(cx, ids!(host_remote_log))
            .set_text(cx, &self.latest_remote_log_text);
    }

    fn handle_control_packet(&mut self, cx: &mut Cx, packet: ControlPacket) {
        match packet {
            ControlPacket::Hello(hello) => {
                self.latest_status = format!("Control client connected: {}", hello.role);
                self.shared
                    .send_control(&ControlPacket::Capabilities(default_capabilities()));
                self.shared.send_control(&ControlPacket::ClockSync(ClockSyncPacket {
                    client_time_ns: 0,
                    server_time_ns: (cx.seconds_since_app_start() * 1_000_000_000.0) as u64,
                }));
            }
            ControlPacket::HeadPose(pose) => {
                self.latest_state.head_pose = pose.pose;
                self.latest_pose_text = format!(
                    "Pose: head ({:.2}, {:.2}, {:.2})",
                    pose.pose.position.x, pose.pose.position.y, pose.pose.position.z
                );
            }
            ControlPacket::InputState(input) => {
                self.latest_state = input.state;
                self.latest_pose_text = format!(
                    "Pose: head ({:.2}, {:.2}, {:.2})",
                    self.latest_state.head_pose.position.x,
                    self.latest_state.head_pose.position.y,
                    self.latest_state.head_pose.position.z
                );
            }
            ControlPacket::Ping(ping) => {
                self.shared.send_control(&ControlPacket::ClockSync(ClockSyncPacket {
                    client_time_ns: ping.timestamp_ns,
                    server_time_ns: (cx.seconds_since_app_start() * 1_000_000_000.0) as u64,
                }));
            }
            ControlPacket::LogLine(line) => {
                self.latest_remote_log_text = format!(
                    "Remote [{}] {}: {}",
                    line.level,
                    line.source,
                    line.text
                );
                crate::log!(
                    "xr_remote remote-log [{}] {} @{}ns: {}",
                    line.level,
                    line.source,
                    line.timestamp_ns,
                    line.text
                );
            }
            ControlPacket::Capabilities(_) | ControlPacket::ClockSync(_) => {}
        }
        self.refresh_labels(cx);
    }

    fn render_demo_frame(&mut self) {
        let width = XR_REMOTE_STREAM_WIDTH as usize;
        let height = XR_REMOTE_STREAM_HEIGHT as usize;
        self.bgra_frame.resize(width * height * 4, 0);

        for px in self.bgra_frame.chunks_exact_mut(4) {
            px[0] = 0;
            px[1] = 255;
            px[2] = 0;
            px[3] = 255;
        }

        let head = self.latest_state.head_pose.position;
        let yaw = head.x * 0.7;
        let wobble = (self.frame_counter as f32 * 0.035).sin();

        let center_x = width as f32 * (0.5 + head.x * 0.18 + wobble * 0.04);
        let center_y = height as f32 * (0.52 - head.y * 0.12);
        let panel_w = width as f32 * (0.18 + (head.z.abs() * 0.04).min(0.08));
        let panel_h = height as f32 * 0.24;
        let depth_offset = yaw.sin() * 120.0;

        self.fill_rect(
            center_x - panel_w * 0.5 + depth_offset,
            center_y - panel_h * 0.5,
            panel_w,
            panel_h,
            [46, 72, 214, 255],
        );
        self.fill_rect(
            center_x - panel_w * 0.32 - depth_offset * 0.4,
            center_y - panel_h * 0.18,
            panel_w * 0.62,
            panel_h * 0.54,
            [250, 164, 54, 255],
        );
        self.fill_circle(
            center_x + depth_offset * 0.35,
            center_y - panel_h * 0.72,
            width as f32 * 0.06,
            [232, 92, 84, 255],
        );
    }

    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, rgba: [u8; 4]) {
        let width = XR_REMOTE_STREAM_WIDTH as usize;
        let height = XR_REMOTE_STREAM_HEIGHT as usize;
        let x0 = x.max(0.0) as usize;
        let y0 = y.max(0.0) as usize;
        let x1 = (x + w).min(width as f32) as usize;
        let y1 = (y + h).min(height as f32) as usize;
        for row in y0..y1 {
            for col in x0..x1 {
                let idx = (row * width + col) * 4;
                self.bgra_frame[idx] = rgba[2];
                self.bgra_frame[idx + 1] = rgba[1];
                self.bgra_frame[idx + 2] = rgba[0];
                self.bgra_frame[idx + 3] = rgba[3];
            }
        }
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, rgba: [u8; 4]) {
        let width = XR_REMOTE_STREAM_WIDTH as usize;
        let height = XR_REMOTE_STREAM_HEIGHT as usize;
        let x0 = (cx - radius).max(0.0) as usize;
        let y0 = (cy - radius).max(0.0) as usize;
        let x1 = (cx + radius).min(width as f32) as usize;
        let y1 = (cy + radius).min(height as f32) as usize;
        let rr = radius * radius;
        for row in y0..y1 {
            for col in x0..x1 {
                let dx = col as f32 - cx;
                let dy = row as f32 - cy;
                if dx * dx + dy * dy > rr {
                    continue;
                }
                let idx = (row * width + col) * 4;
                self.bgra_frame[idx] = rgba[2];
                self.bgra_frame[idx + 1] = rgba[1];
                self.bgra_frame[idx + 2] = rgba[0];
                self.bgra_frame[idx + 3] = rgba[3];
            }
        }
    }

    fn push_frame(&mut self, cx: &mut Cx) {
        if !self.encoder_started {
            return;
        }
        let timestamp_ns = (cx.seconds_since_app_start() * 1_000_000_000.0) as u64;
        self.render_demo_frame();
        let request_keyframe = self.force_keyframe_for_new_client
            || self.frame_counter % ((XR_REMOTE_STREAM_FPS / 2).max(1) as u64) == 0;

        let mut encoder_failed = None;
        match &mut self.host_encoder {
            HostEncoder::None => {}
            #[cfg(not(target_os = "macos"))]
            HostEncoder::Platform => {
                let _ = convert_bgra_8888_to_i420(
                    &self.bgra_frame,
                    XR_REMOTE_STREAM_WIDTH as usize,
                    XR_REMOTE_STREAM_HEIGHT as usize,
                    timestamp_ns,
                    CameraColorMatrix::BT709,
                    &mut self.frame_owned,
                );
                let frame = CameraFrameRef {
                    timestamp_ns: self.frame_owned.timestamp_ns,
                    width: self.frame_owned.width,
                    height: self.frame_owned.height,
                    layout: self.frame_owned.layout,
                    matrix: self.frame_owned.matrix,
                    plane_count: self.frame_owned.plane_count,
                    planes: [
                        CameraFramePlaneRef {
                            bytes: &self.frame_owned.planes[0].bytes,
                            row_stride: self.frame_owned.planes[0].row_stride,
                            pixel_stride: self.frame_owned.planes[0].pixel_stride,
                        },
                        CameraFramePlaneRef {
                            bytes: &self.frame_owned.planes[1].bytes,
                            row_stride: self.frame_owned.planes[1].row_stride,
                            pixel_stride: self.frame_owned.planes[1].pixel_stride,
                        },
                        CameraFramePlaneRef {
                            bytes: &self.frame_owned.planes[2].bytes,
                            row_stride: self.frame_owned.planes[2].row_stride,
                            pixel_stride: self.frame_owned.planes[2].pixel_stride,
                        },
                    ],
                };
                cx.video_encoder_push_frame(XR_REMOTE_ENCODER_SLOT, frame);
                if request_keyframe {
                    let _ = cx.video_encoder_request_keyframe(XR_REMOTE_ENCODER_SLOT);
                }
            }
            #[cfg(target_os = "macos")]
            HostEncoder::Mac(encoder) => {
                if let Err(err) = encoder.encode_bgra(&self.bgra_frame, timestamp_ns, request_keyframe) {
                    encoder_failed = Some(err);
                }
            }
        }

        if let Some(err) = encoder_failed {
            self.host_encoder = HostEncoder::None;
            self.encoder_started = false;
            self.latest_status = format!("Encoder stopped: {err}");
            self.latest_stream_text = "Stream: encoder stopped".to_string();
            self.refresh_labels(cx);
            return;
        }

        if request_keyframe {
            self.force_keyframe_for_new_client = false;
        }
        self.frame_counter = self.frame_counter.wrapping_add(1);
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if matches!(event, Event::Startup) {
            self.ensure_started(cx);
            self.ensure_encoder(cx);
        }
        if let Event::Signal = event {
            for packet in self.shared.drain_control() {
                self.handle_control_packet(cx, packet);
            }
            let (video_connected, _, _) = self.shared.video_debug_counters();
            if video_connected && !self.last_video_connected {
                self.force_keyframe_for_new_client = true;
                crate::log!("xr_remote host: new video client connected, forcing keyframe");
            }
            self.last_video_connected = video_connected;
            self.refresh_labels(cx);
        }
        if self.frame_timer.is_event(event).is_some() {
            self.push_frame(cx);
        }
    }
}
