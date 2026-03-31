use crate::{protocol::*, scene::*, wire::*};
use makepad_widgets::makepad_micro_serde::SerBin;
use makepad_widgets::makepad_platform::{
    event::xr::XrState,
    thread::SignalToUI,
    video::{CameraFrameLayout, VideoEncodeSource, VideoEncoderConfig, VideoQueuePolicy},
};
#[cfg(not(target_os = "macos"))]
use makepad_widgets::makepad_platform::video::{
    convert_bgra_8888_to_i420, CameraColorMatrix, CameraFrameOwned, CameraFramePlaneRef,
    CameraFrameRef,
};
use makepad_widgets::*;
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr, TcpStream, UdpSocket},
    sync::{Arc, Mutex},
    thread,
};

#[cfg(target_os = "macos")]
use makepad_widgets::makepad_platform::os::apple::apple_sys::{
    CFArrayGetCount, CFArrayGetValueAtIndex, CFDictionaryContainsKey, CFDictionaryCreate,
    CFNumberCreate, CFRelease, CMBlockBufferCopyDataBytes, CMBlockBufferGetDataLength,
    CMFormatDescriptionGetMediaSubType, CMSampleBufferDataIsReady, CMSampleBufferGetDataBuffer,
    CMSampleBufferGetFormatDescription, CMSampleBufferGetPresentationTimeStamp,
    CMSampleBufferGetSampleAttachmentsArray, CMTimeGetSeconds, CMTimeMakeWithSeconds,
    CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
    CMVideoFormatDescriptionGetHEVCParameterSetAtIndex, CVImageBufferRef, CVPixelBufferCreate,
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferLockBaseAddress,
    CVPixelBufferRef, CVPixelBufferRelease, CVPixelBufferUnlockBaseAddress, OSStatus,
    VTCompressionSessionCompleteFrames, VTCompressionSessionCreate, VTCompressionSessionEncodeFrame,
    VTCompressionSessionInvalidate, VTCompressionSessionPrepareToEncodeFrames,
    VTCompressionSessionRef, VTEncodeInfoFlags, VTSessionSetProperty, kCFBooleanFalse,
    kCFBooleanTrue, kCFNumberSInt32Type, kCMSampleAttachmentKey_NotSync, kCMTimeInvalid,
    kCMVideoCodecType_H264, kCMVideoCodecType_HEVC,
    kCVPixelFormatType_32BGRA, kVTCompressionPropertyKey_AllowFrameReordering,
    kVTCompressionPropertyKey_AverageBitRate, kVTCompressionPropertyKey_ExpectedFrameRate,
    kVTCompressionPropertyKey_MaxKeyFrameInterval, kVTCompressionPropertyKey_RealTime,
    kVTEncodeFrameOptionKey_ForceKeyFrame,
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

#[derive(Clone, Debug)]
struct PendingFrameMeta {
    session_id: u64,
    frame_group_id: u64,
    frame_id: u64,
    tracking_id: u64,
    pts_ns: u64,
}

#[derive(Clone)]
struct HostEyeShared {
    eye: XrRemoteEye,
    media_socket: Arc<UdpSocket>,
    remote_addr: Arc<Mutex<Option<SocketAddr>>>,
    last_config: Arc<Mutex<Option<VideoConfigPacket>>>,
    current_stream_config: Arc<Mutex<Option<StreamConfigPacket>>>,
    current_config_id: Arc<Mutex<u32>>,
    pending_meta: Arc<Mutex<BTreeMap<u64, PendingFrameMeta>>>,
    sent_packets: Arc<Mutex<u64>>,
    sent_bytes: Arc<Mutex<u64>>,
    require_keyframe: Arc<Mutex<bool>>,
}

impl HostEyeShared {
    fn new(eye: XrRemoteEye, media_socket: Arc<UdpSocket>) -> Self {
        Self {
            eye,
            media_socket,
            remote_addr: Arc::new(Mutex::new(None)),
            last_config: Arc::new(Mutex::new(None)),
            current_stream_config: Arc::new(Mutex::new(None)),
            current_config_id: Arc::new(Mutex::new(0)),
            pending_meta: Arc::new(Mutex::new(BTreeMap::new())),
            sent_packets: Arc::new(Mutex::new(0)),
            sent_bytes: Arc::new(Mutex::new(0)),
            require_keyframe: Arc::new(Mutex::new(false)),
        }
    }

    fn set_remote_addr(&self, addr: Option<SocketAddr>) {
        *self.remote_addr.lock().unwrap() = addr;
        if addr.is_none() {
            *self.require_keyframe.lock().unwrap() = false;
        }
    }

    fn remote_connected(&self) -> bool {
        self.remote_addr.lock().unwrap().is_some()
    }

    fn request_keyframe(&self) {
        *self.require_keyframe.lock().unwrap() = true;
    }

    fn requires_keyframe(&self) -> bool {
        *self.require_keyframe.lock().unwrap()
    }

    fn clear_keyframe_request(&self) {
        *self.require_keyframe.lock().unwrap() = false;
    }

    fn reset_stream_state(&self) {
        *self.last_config.lock().unwrap() = None;
        *self.current_stream_config.lock().unwrap() = None;
        *self.current_config_id.lock().unwrap() = 0;
        self.pending_meta.lock().unwrap().clear();
        *self.require_keyframe.lock().unwrap() = false;
    }

    fn set_stream_config(&self, stream_config: Option<StreamConfigPacket>) {
        *self.current_stream_config.lock().unwrap() = stream_config;
    }

    fn stream_config(&self) -> Option<StreamConfigPacket> {
        self.current_stream_config.lock().unwrap().clone()
    }

    fn last_config(&self) -> Option<VideoConfigPacket> {
        self.last_config.lock().unwrap().clone()
    }

    fn current_config_id(&self) -> u32 {
        *self.current_config_id.lock().unwrap()
    }

    fn replace_config(&self, config: VideoConfigPacket) -> Option<StreamConfigPacket> {
        *self.last_config.lock().unwrap() = Some(config.clone());
        *self.current_config_id.lock().unwrap() = config.config_id;
        let mut stream_config = self.current_stream_config.lock().unwrap();
        if let Some(stream) = stream_config.as_mut() {
            stream.config_id = config.config_id;
        }
        stream_config.clone()
    }

    fn queue_pending_meta(&self, pts_ns: u64, meta: PendingFrameMeta) {
        let mut pending = self.pending_meta.lock().unwrap();
        pending.insert(pts_ns, meta);
        while pending.len() > 16 {
            let Some(first_key) = pending.keys().next().copied() else {
                break;
            };
            pending.remove(&first_key);
        }
    }

    fn take_pending_meta(&self, pts_ns: u64) -> Option<PendingFrameMeta> {
        self.pending_meta.lock().unwrap().remove(&pts_ns)
    }

    fn send_media_frame(
        &self,
        meta: PendingFrameMeta,
        config_id: u32,
        is_key: bool,
        bytes: &[u8],
    ) -> std::io::Result<bool> {
        let Some(remote_addr) = *self.remote_addr.lock().unwrap() else {
            return Ok(false);
        };
        if self.requires_keyframe() && !is_key {
            return Ok(false);
        }

        let chunk_count = bytes
            .len()
            .max(1)
            .div_ceil(XR_REMOTE_MEDIA_PAYLOAD_BYTES)
            .min(u16::MAX as usize) as u16;
        for chunk_index in 0..chunk_count {
            let start = chunk_index as usize * XR_REMOTE_MEDIA_PAYLOAD_BYTES;
            let end = (start + XR_REMOTE_MEDIA_PAYLOAD_BYTES).min(bytes.len());
            let payload = if start < end {
                bytes[start..end].to_vec()
            } else {
                Vec::new()
            };
            let packet = MediaChunkPacket {
                header: MediaChunkHeader {
                    session_id: meta.session_id,
                    eye: self.eye,
                    frame_group_id: meta.frame_group_id,
                    frame_id: meta.frame_id,
                    tracking_id: meta.tracking_id,
                    pts_ns: meta.pts_ns,
                    config_id,
                    is_key,
                    chunk_index,
                    chunk_count,
                },
                payload,
            };
            let bytes = packet.serialize_bin();
            if bytes.len() > max_media_packet_bytes() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "media chunk exceeds mtu budget",
                ));
            }
            self.media_socket.send_to(&bytes, remote_addr)?;
            *self.sent_packets.lock().unwrap() += 1;
        }
        *self.sent_bytes.lock().unwrap() += bytes.len() as u64;
        if is_key {
            self.clear_keyframe_request();
        }
        Ok(true)
    }

    fn debug_counters(&self) -> (bool, u64, u64) {
        (
            self.remote_connected(),
            *self.sent_packets.lock().unwrap(),
            *self.sent_bytes.lock().unwrap(),
        )
    }
}

#[derive(Clone)]
struct HostShared {
    control_writer: Arc<Mutex<Option<TcpStream>>>,
    control_inbox: Arc<Mutex<Vec<ControlPacket>>>,
    control_peer_ip: Arc<Mutex<Option<IpAddr>>>,
    current_session_config: Arc<Mutex<Option<SessionConfigPacket>>>,
    eyes: [HostEyeShared; 2],
}

impl Default for HostShared {
    fn default() -> Self {
        Self::new()
    }
}

impl HostShared {
    fn new() -> Self {
        let control_writer = Arc::new(Mutex::new(None));
        let left_socket = Arc::new(bind_udp_socket(left_media_port()));
        let right_socket = Arc::new(bind_udp_socket(right_media_port()));
        Self {
            control_writer,
            control_inbox: Arc::new(Mutex::new(Vec::new())),
            control_peer_ip: Arc::new(Mutex::new(None)),
            current_session_config: Arc::new(Mutex::new(None)),
            eyes: [
                HostEyeShared::new(XrRemoteEye::Left, left_socket),
                HostEyeShared::new(XrRemoteEye::Right, right_socket),
            ],
        }
    }

    fn eye_shared(&self, eye: XrRemoteEye) -> HostEyeShared {
        self.eyes[eye.index()].clone()
    }

    fn start_threads(&self) {
        let control_writer = self.control_writer.clone();
        let control_inbox = self.control_inbox.clone();
        let control_peer_ip = self.control_peer_ip.clone();
        let shared = self.clone();
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
                *control_peer_ip.lock().unwrap() = stream.peer_addr().ok().map(|addr| addr.ip());
                SignalToUI::set_ui_signal();
                while let Ok(packet) = recv_framed::<ControlPacket>(&mut stream) {
                    control_inbox.lock().unwrap().push(packet);
                    SignalToUI::set_ui_signal();
                }
                *control_writer.lock().unwrap() = None;
                *control_peer_ip.lock().unwrap() = None;
                shared.clear_media_clients();
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
            *self.control_peer_ip.lock().unwrap() = None;
            self.clear_media_clients();
            SignalToUI::set_ui_signal();
        }
    }

    fn set_session_config(&self, session_config: Option<SessionConfigPacket>) {
        *self.current_session_config.lock().unwrap() = session_config;
    }

    fn reset_stream_state(&self) {
        *self.current_session_config.lock().unwrap() = None;
        for eye in XrRemoteEye::ALL {
            self.eye_shared(eye).reset_stream_state();
        }
    }

    fn send_current_control_state(&self) {
        if let Some(session_config) = self.current_session_config.lock().unwrap().clone() {
            self.send_control(&ControlPacket::SessionConfig(session_config));
        }
        for eye in XrRemoteEye::ALL {
            let eye_shared = self.eye_shared(eye);
            if let Some(stream_config) = eye_shared.stream_config() {
                self.send_control(&ControlPacket::StreamConfig(stream_config));
            }
            if let Some(config) = eye_shared.last_config() {
                self.send_control(&ControlPacket::VideoConfig(config));
            }
        }
    }

    fn set_client_media_channels(&self, channels: ClientMediaChannelsPacket) {
        let Some(peer_ip) = *self.control_peer_ip.lock().unwrap() else {
            return;
        };
        self.eye_shared(XrRemoteEye::Left)
            .set_remote_addr(Some(SocketAddr::new(peer_ip, channels.left_port)));
        self.eye_shared(XrRemoteEye::Right)
            .set_remote_addr(Some(SocketAddr::new(peer_ip, channels.right_port)));
        self.request_keyframe(XrRemoteEyeTarget::Both);
    }

    fn clear_media_clients(&self) {
        for eye in XrRemoteEye::ALL {
            self.eye_shared(eye).set_remote_addr(None);
        }
    }

    fn request_keyframe(&self, target: XrRemoteEyeTarget) {
        match target {
            XrRemoteEyeTarget::Left => self.eye_shared(XrRemoteEye::Left).request_keyframe(),
            XrRemoteEyeTarget::Right => self.eye_shared(XrRemoteEye::Right).request_keyframe(),
            XrRemoteEyeTarget::Both => {
                self.eye_shared(XrRemoteEye::Left).request_keyframe();
                self.eye_shared(XrRemoteEye::Right).request_keyframe();
            }
        }
    }

    fn all_media_connected(&self) -> bool {
        XrRemoteEye::ALL
            .iter()
            .all(|eye| self.eye_shared(*eye).remote_connected())
    }

    fn any_eye_requires_keyframe(&self) -> bool {
        XrRemoteEye::ALL
            .iter()
            .any(|eye| self.eye_shared(*eye).requires_keyframe())
    }

    fn drain_control(&self) -> Vec<ControlPacket> {
        let mut inbox = self.control_inbox.lock().unwrap();
        std::mem::take(&mut *inbox)
    }
}

#[derive(Default)]
enum HostEyeEncoder {
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
        host_shared: HostShared,
        eye_shared: HostEyeShared,
        eye: XrRemoteEye,
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
        pub fn new(
            config: VideoEncoderConfig,
            host_shared: HostShared,
            eye_shared: HostEyeShared,
            eye: XrRemoteEye,
        ) -> Result<Self, String> {
            let codec_type = match config.codec {
                makepad_widgets::makepad_platform::video::VideoCodec::H264 => kCMVideoCodecType_H264,
                makepad_widgets::makepad_platform::video::VideoCodec::H265 => kCMVideoCodecType_HEVC,
                other => return Err(format!("unsupported vt codec: {other:?}")),
            };
            unsafe {
                let state = Box::into_raw(Box::new(Mutex::new(CallbackState {
                    host_shared,
                    eye_shared,
                    eye,
                    config_id: 0,
                    last_config: Vec::new(),
                })));

                let mut session: VTCompressionSessionRef = ptr::null_mut();
                let status = VTCompressionSessionCreate(
                    ptr::null(),
                    config.width,
                    config.height,
                    codec_type,
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

                if let Err(err) =
                    set_bool_property(session, kVTCompressionPropertyKey_RealTime, config.latency_realtime)
                {
                    VTCompressionSessionInvalidate(session);
                    CFRelease(session);
                    drop(Box::from_raw(state));
                    return Err(err);
                }
                if let Err(err) = set_i32_property(
                    session,
                    kVTCompressionPropertyKey_AverageBitRate,
                    config.target_bitrate as i32,
                ) {
                    VTCompressionSessionInvalidate(session);
                    CFRelease(session);
                    drop(Box::from_raw(state));
                    return Err(err);
                }
                if let Err(err) = set_i32_property(
                    session,
                    kVTCompressionPropertyKey_ExpectedFrameRate,
                    config.fps_num as i32,
                ) {
                    VTCompressionSessionInvalidate(session);
                    CFRelease(session);
                    drop(Box::from_raw(state));
                    return Err(err);
                }
                if let Err(err) = set_i32_property(
                    session,
                    kVTCompressionPropertyKey_MaxKeyFrameInterval,
                    config.keyint as i32,
                ) {
                    VTCompressionSessionInvalidate(session);
                    CFRelease(session);
                    drop(Box::from_raw(state));
                    return Err(err);
                }
                if let Err(err) = set_bool_property(
                    session,
                    kVTCompressionPropertyKey_AllowFrameReordering,
                    false,
                ) {
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
            frame_key: u64,
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
                let frame_key_ptr = Box::into_raw(Box::new(frame_key));
                let mut info_flags: VTEncodeInfoFlags = 0;
                let status = VTCompressionSessionEncodeFrame(
                    self.session,
                    pixel_buffer as CVImageBufferRef,
                    pts,
                    duration,
                    frame_props,
                    frame_key_ptr as *mut c_void,
                    &mut info_flags,
                );
                if !frame_props.is_null() {
                    CFRelease(frame_props);
                }
                let _ = VTCompressionSessionCompleteFrames(self.session, kCMTimeInvalid);
                CVPixelBufferRelease(pixel_buffer);

                if status != 0 {
                    drop(Box::from_raw(frame_key_ptr));
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
        let number =
            CFNumberCreate(ptr::null(), kCFNumberSInt32Type, &value as *const _ as *const c_void);
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
        source_frame_ref_con: *mut c_void,
        status: OSStatus,
        _info_flags: VTEncodeInfoFlags,
        sample_buffer: makepad_widgets::makepad_platform::os::apple::apple_sys::CMSampleBufferRef,
    ) {
        let frame_key = if source_frame_ref_con.is_null() {
            None
        } else {
            Some(*Box::from_raw(source_frame_ref_con as *mut u64))
        };
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
                eye: state.eye,
                config_id: state.config_id,
                bytes: config_bytes,
            };
            if let Some(stream_config) = state.eye_shared.replace_config(config.clone()) {
                state
                    .host_shared
                    .send_control(&ControlPacket::StreamConfig(stream_config));
            }
            state
                .host_shared
                .send_control(&ControlPacket::VideoConfig(config));
        }

        let Some(meta) = state
            .eye_shared
            .take_pending_meta(frame_key.unwrap_or(pts_ns))
        else {
            return;
        };
        if let Err(err) = state
            .eye_shared
            .send_media_frame(meta, state.config_id.max(1), is_key, &annexb)
        {
            crate::log!(
                "xr_remote host: {} vt send failed: {}",
                state.eye.label(),
                err
            );
        }
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

    unsafe fn extract_h264_config_and_nal_size(
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

    unsafe fn extract_hevc_config_and_nal_size(
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
            let status = CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
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

    unsafe fn extract_config_and_nal_size(
        format_desc: makepad_widgets::makepad_platform::os::apple::apple_sys::CMFormatDescriptionRef,
    ) -> Option<(Vec<u8>, usize)> {
        let media_sub_type = CMFormatDescriptionGetMediaSubType(format_desc);
        if media_sub_type == kCMVideoCodecType_H264 {
            extract_h264_config_and_nal_size(format_desc)
        } else if media_sub_type == kCMVideoCodecType_HEVC {
            extract_hevc_config_and_nal_size(format_desc)
        } else {
            None
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
    eye_bgra_frames: [Vec<u8>; 2],
    #[rust]
    eye_depth_buffers: [Vec<f32>; 2],
    #[rust]
    network_started: bool,
    #[rust]
    encoders_started: bool,
    #[rust]
    eye_encoders: [HostEyeEncoder; 2],
    #[rust]
    latest_tracking: Option<TrackingPacket>,
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
    frame_group_counter: u64,
    #[rust]
    last_media_ready: bool,
    #[rust]
    host_supported_codecs: Vec<XrRemoteCodec>,
    #[rust]
    client_capabilities: Option<CapabilitiesPacket>,
    #[rust]
    negotiated_codec: Option<XrRemoteCodec>,
    #[rust]
    encoder_failed_h265_once: bool,
    #[rust]
    session_config: SessionConfigPacket,
}

impl Default for App {
    fn default() -> Self {
        Self {
            ui: WidgetRef::default(),
            shared: HostShared::new(),
            frame_timer: Timer::default(),
            eye_bgra_frames: std::array::from_fn(|_| Vec::new()),
            eye_depth_buffers: std::array::from_fn(|_| Vec::new()),
            network_started: false,
            encoders_started: false,
            eye_encoders: std::array::from_fn(|_| HostEyeEncoder::None),
            latest_tracking: None,
            latest_state: XrState::default(),
            latest_status: "Host idle".to_string(),
            latest_pose_text: "Pose: waiting".to_string(),
            latest_stream_text: "Stream: waiting".to_string(),
            latest_remote_log_text: "Remote: waiting".to_string(),
            frame_group_counter: 0,
            last_media_ready: false,
            host_supported_codecs: Vec::new(),
            client_capabilities: None,
            negotiated_codec: None,
            encoder_failed_h265_once: false,
            session_config: default_session_config(),
        }
    }
}

impl App {
    fn refresh_host_capabilities(&mut self, cx: &mut Cx) {
        self.host_supported_codecs = preferred_codecs_from_capabilities(&cx.video_capabilities(), true);
    }

    fn advertised_host_capabilities(&self) -> CapabilitiesPacket {
        let mut capabilities = default_capabilities();
        capabilities.codecs = self.host_supported_codecs.clone();
        capabilities.per_eye_width = self.session_config.per_eye_width;
        capabilities.per_eye_height = self.session_config.per_eye_height;
        capabilities.fps = self.session_config.fps;
        capabilities
    }

    fn choose_negotiated_codec(&self) -> Option<XrRemoteCodec> {
        let client_capabilities = self.client_capabilities.as_ref()?;
        self.host_supported_codecs
            .iter()
            .copied()
            .find(|codec| client_capabilities.codecs.contains(codec))
    }

    fn bump_session_id(&mut self) {
        self.session_config.session_id = self.session_config.session_id.wrapping_add(1);
        if self.session_config.session_id == 0 {
            self.session_config.session_id = 1;
        }
        self.session_config.left_media_port = left_media_port();
        self.session_config.right_media_port = right_media_port();
    }

    fn reset_encoder_state(&mut self) {
        self.eye_encoders = std::array::from_fn(|_| HostEyeEncoder::None);
        self.encoders_started = false;
        self.shared.reset_stream_state();
    }

    fn apply_negotiated_codec(&mut self, cx: &mut Cx, codec: Option<XrRemoteCodec>) {
        if self.negotiated_codec == codec && self.encoders_started {
            return;
        }
        self.reset_encoder_state();
        self.negotiated_codec = codec;
        if let Some(codec) = codec {
            self.bump_session_id();
            self.shared.set_session_config(Some(self.session_config.clone()));
            for eye in XrRemoteEye::ALL {
                self.shared
                    .eye_shared(eye)
                    .set_stream_config(Some(default_stream_config(codec, eye)));
            }
            self.latest_stream_text = format!(
                "Stream: {} {}x{} per eye @ {} fps",
                codec.label(),
                self.session_config.per_eye_width,
                self.session_config.per_eye_height,
                self.session_config.fps
            );
            self.ensure_encoders(cx);
            self.shared.send_current_control_state();
        } else {
            self.latest_stream_text = "Stream: waiting for codec negotiation".to_string();
        }
        self.refresh_labels(cx);
    }

    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.network_started {
            return;
        }
        self.refresh_host_capabilities(cx);
        self.shared.set_session_config(Some(self.session_config.clone()));
        self.shared.start_threads();
        self.frame_timer = cx.start_interval(1.0 / self.session_config.fps as f64);
        self.network_started = true;
        self.latest_status = format!(
            "Listening tcp://0.0.0.0:{} and udp://0.0.0.0:{}|{}",
            control_port(),
            left_media_port(),
            right_media_port()
        );
        self.refresh_labels(cx);
    }

    #[cfg(not(target_os = "macos"))]
    fn encoder_slot(eye: XrRemoteEye) -> usize {
        match eye {
            XrRemoteEye::Left => XR_REMOTE_LEFT_ENCODER_SLOT,
            XrRemoteEye::Right => XR_REMOTE_RIGHT_ENCODER_SLOT,
        }
    }

    fn encoder_config_for_eye(&self, codec: XrRemoteCodec) -> VideoEncoderConfig {
        VideoEncoderConfig {
            codec: codec.video_codec(),
            source: VideoEncodeSource::CpuFrames {
                layout: CameraFrameLayout::I420,
            },
            width: self.session_config.per_eye_width,
            height: self.session_config.per_eye_height,
            fps_num: self.session_config.fps,
            fps_den: 1,
            target_bitrate: 6_000_000,
            keyint: 2,
            latency_realtime: true,
            codec_mode: 8,
            queue_policy: VideoQueuePolicy::LatestWins,
            queue_capacity: 2,
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn start_platform_encoder(
        &mut self,
        cx: &mut Cx,
        eye: XrRemoteEye,
        codec: XrRemoteCodec,
    ) -> Result<(), makepad_widgets::makepad_platform::video::VideoEncodeError> {
        let host_shared = self.shared.clone();
        let eye_shared = self.shared.eye_shared(eye);
        cx.video_encoder_output_try(
            Self::encoder_slot(eye),
            self.encoder_config_for_eye(codec),
            move |packet| {
                if packet.codec != codec.video_codec()
                    || packet.format
                        != makepad_widgets::makepad_platform::video::VideoBitstreamFormat::AnnexB
                {
                    return;
                }
                if packet.is_config {
                    let config = VideoConfigPacket {
                        eye,
                        config_id: packet.config_id,
                        bytes: packet.data.to_vec(),
                    };
                    if let Some(stream_config) = eye_shared.replace_config(config.clone()) {
                        host_shared.send_control(&ControlPacket::StreamConfig(stream_config));
                    }
                    host_shared.send_control(&ControlPacket::VideoConfig(config));
                    return;
                }
                if packet.is_eos {
                    return;
                }
                let Some(meta) = eye_shared.take_pending_meta(packet.pts_ns) else {
                    return;
                };
                if let Err(err) = eye_shared.send_media_frame(
                    meta,
                    packet.config_id.max(eye_shared.current_config_id()),
                    packet.is_key,
                    packet.data,
                ) {
                    crate::log!(
                        "xr_remote host: {} udp send failed: {}",
                        eye.label(),
                        err
                    );
                }
            },
        )?;
        self.eye_encoders[eye.index()] = HostEyeEncoder::Platform;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn start_mac_encoder(&mut self, eye: XrRemoteEye, codec: XrRemoteCodec) -> Result<(), String> {
        let encoder = MacHostEncoder::new(
            self.encoder_config_for_eye(codec),
            self.shared.clone(),
            self.shared.eye_shared(eye),
            eye,
        )?;
        self.eye_encoders[eye.index()] = HostEyeEncoder::Mac(encoder);
        Ok(())
    }

    fn ensure_encoders(&mut self, cx: &mut Cx) {
        if self.encoders_started {
            return;
        }
        let Some(codec) = self.negotiated_codec else {
            self.latest_stream_text = "Stream: waiting for codec negotiation".to_string();
            self.refresh_labels(cx);
            return;
        };

        #[cfg(target_os = "macos")]
        {
            match (
                self.start_mac_encoder(XrRemoteEye::Left, codec),
                self.start_mac_encoder(XrRemoteEye::Right, codec),
            ) {
                (Ok(()), Ok(())) => {
                    self.encoders_started = true;
                    self.latest_stream_text = format!(
                        "Stream: {} dual-eye {}x{} @ {} fps (VideoToolbox)",
                        codec.label(),
                        self.session_config.per_eye_width,
                        self.session_config.per_eye_height,
                        self.session_config.fps
                    );
                    self.shared.send_current_control_state();
                    self.refresh_labels(cx);
                    return;
                }
                (left_vt, right_vt) => {
                    if codec == XrRemoteCodec::H265AnnexB
                        && !self.encoder_failed_h265_once
                        && self.host_supported_codecs.contains(&XrRemoteCodec::H264AnnexB)
                    {
                        self.encoder_failed_h265_once = true;
                        self.latest_status = format!(
                            "VideoToolbox H265 unavailable, falling back to H264: left={left_vt:?} right={right_vt:?}"
                        );
                        self.apply_negotiated_codec(cx, Some(XrRemoteCodec::H264AnnexB));
                        return;
                    }
                    self.latest_status = format!(
                        "Encoder unavailable: vt_left={left_vt:?} vt_right={right_vt:?}"
                    );
                    self.latest_stream_text = "Stream: encoder unavailable".to_string();
                    self.refresh_labels(cx);
                    return;
                }
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            let left_result = self.start_platform_encoder(cx, XrRemoteEye::Left, codec);
            let right_result = self.start_platform_encoder(cx, XrRemoteEye::Right, codec);
            if left_result.is_ok() && right_result.is_ok() {
                self.encoders_started = true;
                self.latest_stream_text = format!(
                    "Stream: {} dual-eye {}x{} @ {} fps",
                    codec.label(),
                    self.session_config.per_eye_width,
                    self.session_config.per_eye_height,
                    self.session_config.fps
                );
                self.shared.send_current_control_state();
                self.refresh_labels(cx);
                return;
            }
            let left_err = left_result.err();
            let right_err = right_result.err();
            if codec == XrRemoteCodec::H265AnnexB
                && !self.encoder_failed_h265_once
                && self.host_supported_codecs.contains(&XrRemoteCodec::H264AnnexB)
            {
                self.encoder_failed_h265_once = true;
                self.latest_status = format!(
                    "H265 encoder unavailable, falling back: left={left_err:?} right={right_err:?}"
                );
                self.apply_negotiated_codec(cx, Some(XrRemoteCodec::H264AnnexB));
                return;
            }
            self.latest_status = format!(
                "Encoder unavailable: left={left_err:?} right={right_err:?}"
            );
            self.latest_stream_text = "Stream: encoder unavailable".to_string();
            self.refresh_labels(cx);
        }
    }

    fn refresh_labels(&mut self, cx: &mut Cx) {
        let (left_connected, left_packets, left_bytes) =
            self.shared.eye_shared(XrRemoteEye::Left).debug_counters();
        let (right_connected, right_packets, right_bytes) =
            self.shared.eye_shared(XrRemoteEye::Right).debug_counters();
        let stream_text = format!(
            "{} | L {} p{} b{} | R {} p{} b{}",
            self.latest_stream_text,
            if left_connected { "ready" } else { "wait" },
            left_packets,
            left_bytes,
            if right_connected { "ready" } else { "wait" },
            right_packets,
            right_bytes
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
                let protocol_status = if hello.protocol_version == XR_REMOTE_PROTOCOL_VERSION {
                    format!("proto v{}", hello.protocol_version)
                } else {
                    format!(
                        "proto mismatch host={} client={}",
                        XR_REMOTE_PROTOCOL_VERSION, hello.protocol_version
                    )
                };
                self.latest_status = format!(
                    "Control client connected: {} ({protocol_status})",
                    hello.role
                );
                self.shared
                    .send_control(&ControlPacket::Capabilities(self.advertised_host_capabilities()));
                self.shared.send_current_control_state();
                self.shared.send_control(&ControlPacket::ClockSync(ClockSyncPacket {
                    client_time_ns: 0,
                    server_time_ns: (cx.seconds_since_app_start() * 1_000_000_000.0) as u64,
                }));
            }
            ControlPacket::Capabilities(capabilities) => {
                let labels = capabilities
                    .codecs
                    .iter()
                    .map(|codec| codec.label())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.latest_status = format!("Client capabilities: [{labels}]");
                self.client_capabilities = Some(capabilities);
                self.apply_negotiated_codec(cx, self.choose_negotiated_codec());
            }
            ControlPacket::ClientMediaChannels(channels) => {
                self.shared.set_client_media_channels(channels);
                self.shared.send_current_control_state();
                self.latest_status = "Client media channels registered".to_string();
            }
            ControlPacket::KeyframeRequest(request) => {
                self.shared.request_keyframe(request.eye);
                self.latest_stream_text = format!("Stream: keyframe requested {:?}", request.eye);
            }
            ControlPacket::Tracking(tracking) => {
                self.latest_state.head_pose = tracking.head_pose;
                self.latest_tracking = Some(tracking.clone());
                self.latest_pose_text = format!(
                    "Track {} head ({:.2}, {:.2}, {:.2})",
                    tracking.tracking_id,
                    tracking.head_pose.position.x,
                    tracking.head_pose.position.y,
                    tracking.head_pose.position.z
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
            ControlPacket::SessionConfig(_)
            | ControlPacket::StreamConfig(_)
            | ControlPacket::VideoConfig(_)
            | ControlPacket::ClockSync(_) => {}
        }
        self.refresh_labels(cx);
    }

    fn push_eye_frame(
        &mut self,
        cx: &mut Cx,
        tracking: &TrackingPacket,
        frame_group_id: u64,
        encode_timestamp_ns: u64,
        eye: XrRemoteEye,
        request_keyframe: bool,
    ) -> bool {
        let eye_index = eye.index();
        render_eye_scene(
            &mut self.eye_bgra_frames[eye_index],
            &mut self.eye_depth_buffers[eye_index],
            tracking,
            eye,
            &self.session_config,
        );

        let timestamp_ns = encode_timestamp_ns;
        let meta = PendingFrameMeta {
            session_id: self.session_config.session_id,
            frame_group_id,
            frame_id: frame_group_id,
            tracking_id: tracking.tracking_id,
            pts_ns: timestamp_ns,
        };
        self.shared.eye_shared(eye).queue_pending_meta(timestamp_ns, meta);

        #[cfg(not(target_os = "macos"))]
        {
            match &mut self.eye_encoders[eye_index] {
                HostEyeEncoder::None => false,
                HostEyeEncoder::Platform => {
                    let mut frame_owned = CameraFrameOwned::default();
                    let _ = convert_bgra_8888_to_i420(
                        &self.eye_bgra_frames[eye_index],
                        self.session_config.per_eye_width as usize,
                        self.session_config.per_eye_height as usize,
                        timestamp_ns,
                        CameraColorMatrix::BT709,
                        &mut frame_owned,
                    );
                    let frame = CameraFrameRef {
                        timestamp_ns: frame_owned.timestamp_ns,
                        width: frame_owned.width,
                        height: frame_owned.height,
                        layout: frame_owned.layout,
                        matrix: frame_owned.matrix,
                        plane_count: frame_owned.plane_count,
                        planes: [
                            CameraFramePlaneRef {
                                bytes: &frame_owned.planes[0].bytes,
                                row_stride: frame_owned.planes[0].row_stride,
                                pixel_stride: frame_owned.planes[0].pixel_stride,
                            },
                            CameraFramePlaneRef {
                                bytes: &frame_owned.planes[1].bytes,
                                row_stride: frame_owned.planes[1].row_stride,
                                pixel_stride: frame_owned.planes[1].pixel_stride,
                            },
                            CameraFramePlaneRef {
                                bytes: &frame_owned.planes[2].bytes,
                                row_stride: frame_owned.planes[2].row_stride,
                                pixel_stride: frame_owned.planes[2].pixel_stride,
                            },
                        ],
                    };
                    cx.video_encoder_push_frame(Self::encoder_slot(eye), frame);
                    if request_keyframe {
                        let _ = cx.video_encoder_request_keyframe(Self::encoder_slot(eye));
                    }
                    true
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            match &mut self.eye_encoders[eye_index] {
                HostEyeEncoder::None => false,
                HostEyeEncoder::Mac(encoder) => {
                    if let Err(err) = encoder.encode_bgra(
                        &self.eye_bgra_frames[eye_index],
                        timestamp_ns,
                        timestamp_ns,
                        request_keyframe,
                    ) {
                        self.eye_encoders[eye_index] = HostEyeEncoder::None;
                        self.encoders_started = false;
                        self.latest_status = format!("{} encoder stopped: {err}", eye.label());
                        self.latest_stream_text = "Stream: encoder stopped".to_string();
                        self.refresh_labels(cx);
                        return false;
                    }
                    true
                }
            }
        }
    }

    fn push_frame(&mut self, cx: &mut Cx) {
        if !self.encoders_started {
            return;
        }
        if !self.shared.all_media_connected() {
            self.latest_stream_text = "Stream: waiting for dual-eye media client".to_string();
            self.refresh_labels(cx);
            return;
        }
        let Some(tracking) = self.latest_tracking.clone() else {
            self.latest_stream_text = "Stream: waiting for predicted tracking".to_string();
            self.refresh_labels(cx);
            return;
        };

        let render_started_ns = (cx.seconds_since_app_start() * 1_000_000_000.0) as u64;
        let frame_group_id = self.frame_group_counter.wrapping_add(1);
        let request_keyframe = self.shared.any_eye_requires_keyframe()
            || frame_group_id % ((self.session_config.fps / 2).max(1) as u64) == 0;
        let encode_timestamp_ns = render_started_ns.max(self.frame_group_counter.saturating_add(1));
        let left_ok = self.push_eye_frame(
            cx,
            &tracking,
            frame_group_id,
            encode_timestamp_ns,
            XrRemoteEye::Left,
            request_keyframe,
        );
        let right_ok = self.push_eye_frame(
            cx,
            &tracking,
            frame_group_id,
            encode_timestamp_ns,
            XrRemoteEye::Right,
            request_keyframe,
        );
        let render_finished_ns = (cx.seconds_since_app_start() * 1_000_000_000.0) as u64;
        if left_ok && right_ok {
            self.frame_group_counter = frame_group_id;
        }
        self.latest_stream_text = format!(
            "Stream: group {} track {} render {:.1} ms cfg L{} R{}",
            frame_group_id,
            tracking.tracking_id,
            (render_finished_ns.saturating_sub(render_started_ns) as f64) / 1_000_000.0,
            self.shared.eye_shared(XrRemoteEye::Left).current_config_id(),
            self.shared.eye_shared(XrRemoteEye::Right).current_config_id(),
        );
        self.refresh_labels(cx);
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
            self.ensure_encoders(cx);
        }
        if let Event::Signal = event {
            for packet in self.shared.drain_control() {
                self.handle_control_packet(cx, packet);
            }
            let media_ready = self.shared.all_media_connected();
            if media_ready && !self.last_media_ready {
                self.shared.request_keyframe(XrRemoteEyeTarget::Both);
                crate::log!("xr_remote host: dual-eye media client connected, forcing keyframe");
            }
            self.last_media_ready = media_ready;
            self.refresh_labels(cx);
        }
        if self.frame_timer.is_event(event).is_some() {
            self.push_frame(cx);
        }
    }
}
