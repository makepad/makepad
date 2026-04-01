use crate::{gpu_capture::GpuCapture, protocol::*, scene::*, shared_scene::*, wire::*};
use makepad_widgets::makepad_draw::{
    cx_3d::Cx3d, cx_draw::CxDraw, draw_list_2d::DrawListExt, scene_3d::SceneState3D,
};
use makepad_widgets::makepad_micro_serde::SerBin;
#[cfg(not(target_os = "macos"))]
use makepad_widgets::makepad_platform::video::{
    convert_bgra_8888_to_i420, CameraColorMatrix, CameraFrameOwned, CameraFramePlaneRef,
    CameraFrameRef,
};
use makepad_widgets::makepad_platform::{
    event::xr::XrState,
    thread::SignalToUI,
    video::{CameraFrameLayout, VideoEncodeSource, VideoEncoderConfig, VideoQueuePolicy},
    DrawEvent,
};
use makepad_widgets::*;
use makepad_xr::{XrNetIncoming, XrNetNode};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr, TcpStream, UdpSocket},
    sync::{mpsc::TryRecvError, Arc, Mutex},
    thread,
};

#[cfg(target_os = "macos")]
use makepad_widgets::makepad_platform::os::apple::apple_sys::{
    kCFBooleanFalse, kCFBooleanTrue, kCFNumberSInt32Type, kCMSampleAttachmentKey_NotSync,
    kCMTimeInvalid, kCMVideoCodecType_H264, kCMVideoCodecType_HEVC, kCVPixelFormatType_32BGRA,
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_RealTime, kVTEncodeFrameOptionKey_ForceKeyFrame, CFArrayGetCount,
    CFArrayGetValueAtIndex, CFDictionaryContainsKey, CFDictionaryCreate, CFNumberCreate, CFRelease,
    CMBlockBufferCopyDataBytes, CMBlockBufferGetDataLength, CMFormatDescriptionGetMediaSubType,
    CMSampleBufferDataIsReady, CMSampleBufferGetDataBuffer, CMSampleBufferGetFormatDescription,
    CMSampleBufferGetPresentationTimeStamp, CMSampleBufferGetSampleAttachmentsArray,
    CMTimeGetSeconds, CMTimeMakeWithSeconds, CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
    CMVideoFormatDescriptionGetHEVCParameterSetAtIndex, CVImageBufferRef, CVPixelBufferCreate,
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferLockBaseAddress,
    CVPixelBufferRef, CVPixelBufferRelease, CVPixelBufferUnlockBaseAddress, OSStatus,
    VTCompressionSessionCompleteFrames, VTCompressionSessionCreate,
    VTCompressionSessionEncodeFrame, VTCompressionSessionInvalidate,
    VTCompressionSessionPrepareToEncodeFrames, VTCompressionSessionRef, VTEncodeInfoFlags,
    VTSessionSetProperty,
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
                    gpu_scene := mod.widgets.XrRemoteSharedScene{}
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

                        stream_preview_panel := RoundedView{
                            width: Fill
                            height: Fit
                            padding: 12
                            spacing: 10
                            flow: Down
                            draw_bg+: {
                                color: #x16202b
                                border_radius: 10.0
                                border_size: 1.0
                                border_color: #x273646
                            }

                            Label{
                                text: "Quest Render Mode"
                                draw_text.color: #xe4eef7
                                draw_text.text_style.font_size: 14.0
                            }

                            host_render := Label{
                                text: "Render: stream-video | Scene: test-scene"
                                draw_text.color: #x97adc2
                            }

                            View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 8

                                render_stream_button := ButtonFlat{
                                    width: Fit
                                    text: "Stream Video"
                                }

                                render_test_scene_button := ButtonFlat{
                                    width: Fit
                                    text: "Quest Test Scene"
                                }

                                render_tree_scene_button := ButtonFlat{
                                    width: Fit
                                    text: "Quest Tree Scene"
                                }

                                gpu_toggle_button := ButtonFlat{
                                    width: Fit
                                    text: "GPU Pipeline"
                                }
                            }

                            host_pipeline := Label{
                                text: "Pipeline: CPU (software rasterizer)"
                                draw_text.color: #x97adc2
                            }
                        }

                        RoundedView{
                            width: Fill
                            height: Fit
                            padding: 12
                            spacing: 10
                            flow: Down
                            draw_bg+: {
                                color: #x16202b
                                border_radius: 10.0
                                border_size: 1.0
                                border_color: #x273646
                            }

                            Label{
                                text: "Replicated Marker"
                                draw_text.color: #xe4eef7
                                draw_text.text_style.font_size: 14.0
                            }

                            host_marker := Label{
                                text: "Marker: (0.42, 0.34, -0.76) scale 1.00 pulse 0.00"
                                draw_text.color: #x97adc2
                            }

                            View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 8

                                marker_left_button := ButtonFlat{ width: Fit text: "Left" }
                                marker_right_button := ButtonFlat{ width: Fit text: "Right" }
                                marker_up_button := ButtonFlat{ width: Fit text: "Up" }
                                marker_down_button := ButtonFlat{ width: Fit text: "Down" }
                                marker_near_button := ButtonFlat{ width: Fit text: "Near" }
                                marker_far_button := ButtonFlat{ width: Fit text: "Far" }
                                marker_pulse_button := ButtonFlat{ width: Fit text: "Pulse" }
                                marker_reset_button := ButtonFlat{ width: Fit text: "Reset" }
                            }
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

                        RoundedView{
                            width: Fill
                            height: Fit
                            padding: 12
                            spacing: 10
                            flow: Down
                            draw_bg+: {
                                color: #x16202b
                                border_radius: 10.0
                                border_size: 1.0
                                border_color: #x273646
                            }

                            preview_title := Label{
                                text: "Outgoing Stream Preview"
                                draw_text.color: #xe4eef7
                                draw_text.text_style.font_size: 14.0
                            }

                            preview_caption := Label{
                                text: "These are the exact pre-encode eye frames being sent to Quest."
                                draw_text.color: #x97adc2
                            }

                            View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 10

                                RoundedView{
                                    width: Fill
                                    height: Fit
                                    padding: 8
                                    spacing: 6
                                    flow: Down
                                    draw_bg+: {
                                        color: #x101820
                                        border_radius: 8.0
                                        border_size: 1.0
                                        border_color: #x223140
                                    }

                                    Label{
                                        text: "Left eye"
                                        draw_text.color: #xc4d2de
                                    }

                                    preview_left := Image{
                                        width: Fill
                                        height: 150
                                        fit: ImageFit.Smallest
                                    }
                                }

                                RoundedView{
                                    width: Fill
                                    height: Fit
                                    padding: 8
                                    spacing: 6
                                    flow: Down
                                    draw_bg+: {
                                        color: #x101820
                                        border_radius: 8.0
                                        border_size: 1.0
                                        border_color: #x223140
                                    }

                                    Label{
                                        text: "Right eye"
                                        draw_text.color: #xc4d2de
                                    }

                                    preview_right := Image{
                                        width: Fill
                                        height: 150
                                        fit: ImageFit.Smallest
                                    }
                                }
                            }
                        }

                        quest_monitor_panel := RoundedView{
                            visible: false
                            width: Fill
                            height: 360
                            padding: 12
                            spacing: 10
                            flow: Down
                            draw_bg+: {
                                color: #x16202b
                                border_radius: 10.0
                                border_size: 1.0
                                border_color: #x273646
                            }

                            Label{
                                text: "Quest Local Scene Monitor"
                                draw_text.color: #xe4eef7
                                draw_text.text_style.font_size: 14.0
                            }

                            Label{
                                text: "This is the same shared XR scene graph Quest renders locally. Drag to orbit and use the wheel to zoom."
                                draw_text.color: #x97adc2
                            }

                            quest_scene_monitor := mod.widgets.XrRemoteDesktopMonitor{}
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

        let owned_bytes;
        let frame_bytes: &[u8] = if is_key {
            if let Some(config) = self.last_config() {
                owned_bytes = [config.bytes.as_slice(), bytes].concat();
                &owned_bytes
            } else {
                bytes
            }
        } else {
            bytes
        };

        let chunk_count = frame_bytes
            .len()
            .max(1)
            .div_ceil(XR_REMOTE_MEDIA_PAYLOAD_BYTES)
            .min(u16::MAX as usize) as u16;
        for chunk_index in 0..chunk_count {
            let start = chunk_index as usize * XR_REMOTE_MEDIA_PAYLOAD_BYTES;
            let end = (start + XR_REMOTE_MEDIA_PAYLOAD_BYTES).min(frame_bytes.len());
            let payload = if start < end {
                frame_bytes[start..end].to_vec()
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
        *self.sent_bytes.lock().unwrap() += frame_bytes.len() as u64;
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
    current_render_state: Arc<Mutex<Option<RenderStatePacket>>>,
    current_marker_state: Arc<Mutex<Option<MarkerStatePacket>>>,
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
        let media_socket = Arc::new(bind_udp_socket(media_port()));
        Self {
            control_writer,
            control_inbox: Arc::new(Mutex::new(Vec::new())),
            control_peer_ip: Arc::new(Mutex::new(None)),
            current_session_config: Arc::new(Mutex::new(None)),
            current_render_state: Arc::new(Mutex::new(None)),
            current_marker_state: Arc::new(Mutex::new(None)),
            eyes: [
                HostEyeShared::new(XrRemoteEye::Left, media_socket.clone()),
                HostEyeShared::new(XrRemoteEye::Right, media_socket),
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

    fn set_render_state(&self, render_state: Option<RenderStatePacket>) {
        *self.current_render_state.lock().unwrap() = render_state;
    }

    fn set_marker_state(&self, marker_state: Option<MarkerStatePacket>) {
        *self.current_marker_state.lock().unwrap() = marker_state;
    }

    fn send_current_control_state(&self) {
        if let Some(session_config) = self.current_session_config.lock().unwrap().clone() {
            self.send_control(&ControlPacket::SessionConfig(session_config));
        }
        if let Some(render_state) = self.current_render_state.lock().unwrap().clone() {
            self.send_control(&ControlPacket::RenderState(render_state));
        }
        if let Some(marker_state) = self.current_marker_state.lock().unwrap().clone() {
            self.send_control(&ControlPacket::MarkerState(marker_state));
        }
        for eye in XrRemoteEye::ALL {
            let eye_shared = self.eye_shared(eye);
            if let Some(stream_config) = eye_shared.stream_config() {
                if stream_config.config_id != 0 {
                    self.send_control(&ControlPacket::StreamConfig(stream_config));
                }
            }
            if let Some(config) = eye_shared.last_config() {
                self.send_control(&ControlPacket::VideoConfig(config));
            }
        }
    }

    fn set_client_media_channel(&self, channel: ClientMediaChannelPacket) {
        let Some(peer_ip) = *self.control_peer_ip.lock().unwrap() else {
            return;
        };
        let remote_addr = Some(SocketAddr::new(peer_ip, channel.port));
        self.eye_shared(XrRemoteEye::Left)
            .set_remote_addr(remote_addr);
        self.eye_shared(XrRemoteEye::Right)
            .set_remote_addr(remote_addr);
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
                makepad_widgets::makepad_platform::video::VideoCodec::H264 => {
                    kCMVideoCodecType_H264
                }
                makepad_widgets::makepad_platform::video::VideoCodec::H265 => {
                    kCMVideoCodecType_HEVC
                }
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

                if let Err(err) = set_bool_property(
                    session,
                    kVTCompressionPropertyKey_RealTime,
                    config.latency_realtime,
                ) {
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
                    return Err(format!(
                        "VTCompressionSessionPrepareToEncodeFrames failed: {status}"
                    ));
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
                    return Err(format!(
                        "CVPixelBufferLockBaseAddress failed: {lock_status}"
                    ));
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
        let number = CFNumberCreate(
            ptr::null(),
            kCFNumberSInt32Type,
            &value as *const _ as *const c_void,
        );
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
        let bool_ref = if value {
            kCFBooleanTrue
        } else {
            kCFBooleanFalse
        };
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
        if CMBlockBufferCopyDataBytes(data_buffer, 0, data_len, avcc.as_mut_ptr() as *mut c_void)
            != 0
        {
            return;
        }

        let Some(annexb) = avcc_to_annexb(&avcc, nal_header_len) else {
            return;
        };
        let pts_ns = (CMTimeGetSeconds(CMSampleBufferGetPresentationTimeStamp(sample_buffer))
            * 1_000_000_000.0)
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
        if let Err(err) =
            state
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
    xr_net: Option<XrNetNode>,
    #[rust]
    frame_timer: Timer,
    #[rust]
    eye_bgra_frames: [Vec<u8>; 2],
    #[rust]
    eye_depth_buffers: [Vec<f32>; 2],
    #[rust]
    preview_textures: [Option<Texture>; 2],
    #[rust]
    network_started: bool,
    #[rust]
    encoders_started: bool,
    #[rust]
    eye_encoders: [HostEyeEncoder; 2],
    #[rust]
    gpu_capture: GpuCapture,
    #[rust]
    gpu_encoders_started: bool,
    #[rust]
    use_gpu_pipeline: bool,
    #[rust]
    latest_state: XrState,
    #[rust]
    latest_state_received: bool,
    #[rust]
    latest_status: String,
    #[rust]
    latest_pose_text: String,
    #[rust]
    latest_stream_text: String,
    #[rust]
    latest_render_text: String,
    #[rust]
    latest_marker_text: String,
    #[rust]
    latest_remote_log_text: String,
    #[rust]
    latest_pipeline_text: String,
    #[rust]
    frame_group_counter: u64,
    #[rust]
    last_media_ready: bool,
    #[rust]
    session_config: SessionConfigPacket,
    #[rust]
    render_state: RenderStatePacket,
    #[rust]
    marker_state: MarkerStatePacket,
}

impl Default for App {
    fn default() -> Self {
        Self {
            ui: WidgetRef::default(),
            shared: HostShared::new(),
            xr_net: None,
            frame_timer: Timer::default(),
            eye_bgra_frames: std::array::from_fn(|_| Vec::new()),
            eye_depth_buffers: std::array::from_fn(|_| Vec::new()),
            preview_textures: std::array::from_fn(|_| None),
            network_started: false,
            encoders_started: false,
            eye_encoders: std::array::from_fn(|_| HostEyeEncoder::None),
            gpu_capture: GpuCapture::new(),
            gpu_encoders_started: false,
            use_gpu_pipeline: false,
            latest_state: XrState::default(),
            latest_state_received: false,
            latest_status: "Host idle".to_string(),
            latest_pose_text: "Pose: waiting".to_string(),
            latest_stream_text: "Stream: waiting".to_string(),
            latest_render_text: "Render: stream-video | Scene: test-scene".to_string(),
            latest_marker_text: "Marker: (0.42, 0.34, -0.76) scale 1.00 pulse 0.00".to_string(),
            latest_remote_log_text: "Remote: waiting".to_string(),
            latest_pipeline_text: "Pipeline: CPU (software rasterizer)".to_string(),
            frame_group_counter: 0,
            last_media_ready: false,
            session_config: default_session_config(),
            render_state: default_render_state(),
            marker_state: default_marker_state(),
        }
    }
}

impl App {
    fn marker_summary(marker_state: &MarkerStatePacket) -> String {
        format!(
            "Marker: ({:.2}, {:.2}, {:.2}) scale {:.2} pulse {:.2}",
            marker_state.x, marker_state.y, marker_state.z, marker_state.scale, marker_state.pulse,
        )
    }

    fn bump_session_id(&mut self) {
        self.session_config.session_id = self.session_config.session_id.wrapping_add(1);
        if self.session_config.session_id == 0 {
            self.session_config.session_id = 1;
        }
    }

    fn sync_render_state(&self) {
        self.shared
            .set_render_state(Some(self.render_state.clone()));
        self.shared
            .send_control(&ControlPacket::RenderState(self.render_state.clone()));
    }

    fn sync_marker_state(&self) {
        self.shared
            .set_marker_state(Some(self.marker_state.clone()));
        self.shared
            .send_control(&ControlPacket::MarkerState(self.marker_state.clone()));
    }

    fn set_render_state(&mut self, cx: &mut Cx, mode: XrRemoteRenderMode, scene: XrRemoteSceneId) {
        let next = RenderStatePacket { mode, scene };
        if self.render_state == next {
            return;
        }
        self.render_state = next;
        self.latest_render_text = format!(
            "Render: {} | Scene: {}",
            self.render_state.mode.label(),
            self.render_state.scene.label()
        );
        self.sync_render_state();
        self.refresh_labels(cx);
    }

    fn set_marker_state(&mut self, cx: &mut Cx, marker_state: MarkerStatePacket) {
        if self.marker_state == marker_state {
            return;
        }
        self.marker_state = marker_state;
        self.latest_marker_text = Self::marker_summary(&self.marker_state);
        self.sync_marker_state();
        self.refresh_labels(cx);
    }

    fn nudge_marker(&mut self, cx: &mut Cx, dx: f32, dy: f32, dz: f32) {
        let mut next = self.marker_state.clone();
        next.x += dx;
        next.y += dy;
        next.z += dz;
        self.set_marker_state(cx, next);
    }

    fn pulse_marker(&mut self, cx: &mut Cx) {
        let mut next = self.marker_state.clone();
        next.pulse = (next.pulse + 0.25).fract();
        next.scale = 0.9 + next.pulse * 0.6;
        self.set_marker_state(cx, next);
    }

    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.network_started {
            return;
        }
        self.bump_session_id();
        self.shared
            .set_session_config(Some(self.session_config.clone()));
        self.shared
            .set_render_state(Some(self.render_state.clone()));
        self.shared
            .set_marker_state(Some(self.marker_state.clone()));
        for eye in XrRemoteEye::ALL {
            self.shared
                .eye_shared(eye)
                .set_stream_config(Some(default_stream_config(XrRemoteCodec::H265AnnexB, eye)));
        }
        self.shared.start_threads();
        self.xr_net = match XrNetNode::new() {
            Ok(node) => Some(node),
            Err(err) => {
                self.latest_status = format!("XR Net unavailable: {err}");
                None
            }
        };
        self.frame_timer = cx.start_interval(1.0 / self.session_config.fps as f64);
        self.network_started = true;
        if self.xr_net.is_some() {
            self.latest_status = format!(
                "Listening tcp://0.0.0.0:{} udp://0.0.0.0:{} xr_net=ready",
                control_port(),
                media_port()
            );
        }
        self.ensure_encoders(cx);
        self.shared.send_current_control_state();
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
                    crate::log!("xr_remote host: {} udp send failed: {}", eye.label(), err);
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
        let codec = XrRemoteCodec::H265AnnexB;

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
                    self.latest_status = format!(
                        "H265 encoder unavailable: vt_left={left_vt:?} vt_right={right_vt:?}"
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
            self.latest_status =
                format!("H265 encoder unavailable: left={left_err:?} right={right_err:?}");
            self.latest_stream_text = "Stream: encoder unavailable".to_string();
            self.refresh_labels(cx);
        }
    }

    // --- GPU-based encoding path ---

    #[cfg(not(target_os = "macos"))]
    const GPU_LEFT_ENCODER_SLOT: usize = 2;
    #[cfg(not(target_os = "macos"))]
    const GPU_RIGHT_ENCODER_SLOT: usize = 3;

    #[cfg(not(target_os = "macos"))]
    fn gpu_encoder_slot(eye: XrRemoteEye) -> usize {
        match eye {
            XrRemoteEye::Left => Self::GPU_LEFT_ENCODER_SLOT,
            XrRemoteEye::Right => Self::GPU_RIGHT_ENCODER_SLOT,
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn gpu_encoder_config_for_eye(
        &self,
        codec: XrRemoteCodec,
        eye: XrRemoteEye,
    ) -> VideoEncoderConfig {
        let texture_id = self
            .gpu_capture
            .eye_target(eye)
            .map(|t| t.texture_id())
            .unwrap_or_default();
        VideoEncoderConfig {
            codec: codec.video_codec(),
            source: VideoEncodeSource::Texture { texture_id },
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
    fn ensure_gpu_encoders(&mut self, cx: &mut Cx) {
        if self.gpu_encoders_started {
            return;
        }
        self.gpu_capture.ensure_targets(cx);
        let codec = XrRemoteCodec::H265AnnexB;
        let mut ok = true;
        for eye in XrRemoteEye::ALL {
            let host_shared = self.shared.clone();
            let eye_shared = self.shared.eye_shared(eye);
            let config = self.gpu_encoder_config_for_eye(codec, eye);
            match cx.video_encoder_output_try(Self::gpu_encoder_slot(eye), config, move |packet| {
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
                        "xr_remote host: {} gpu udp send failed: {}",
                        eye.label(),
                        err
                    );
                }
            }) {
                Ok(()) => {}
                Err(err) => {
                    self.latest_status = format!("GPU encoder {} failed: {:?}", eye.label(), err);
                    ok = false;
                }
            }
        }
        if ok {
            self.gpu_encoders_started = true;
            self.latest_stream_text = format!(
                "Stream: GPU {} dual-eye {}x{} @ {} fps",
                codec.label(),
                self.session_config.per_eye_width,
                self.session_config.per_eye_height,
                self.session_config.fps
            );
            self.shared.send_current_control_state();
        }
        self.refresh_labels(cx);
    }

    #[cfg(not(target_os = "macos"))]
    fn push_gpu_eye_frame(
        &mut self,
        cx: &mut Cx,
        eye: XrRemoteEye,
        timestamp_ns: u64,
        frame_group_id: u64,
        tracking_id: u64,
        request_keyframe: bool,
    ) -> bool {
        let meta = PendingFrameMeta {
            session_id: self.session_config.session_id,
            frame_group_id,
            frame_id: frame_group_id,
            tracking_id,
            pts_ns: timestamp_ns,
        };
        self.shared
            .eye_shared(eye)
            .queue_pending_meta(timestamp_ns, meta);
        if request_keyframe {
            let _ = cx.video_encoder_request_keyframe(Self::gpu_encoder_slot(eye));
        }
        match cx.video_encoder_capture_texture_frame(Self::gpu_encoder_slot(eye), timestamp_ns) {
            Ok(()) => true,
            Err(err) => {
                crate::log!(
                    "xr_remote host: {} gpu capture failed: {:?}",
                    eye.label(),
                    err
                );
                false
            }
        }
    }

    fn try_start_gpu_pipeline(&mut self, cx: &mut Cx) {
        if self.use_gpu_pipeline {
            return;
        }
        self.gpu_capture.ensure_targets(cx);

        // On macOS, the platform encoder API doesn't support texture-source
        // encoding (CodecUnavailable). Instead, use the existing Mac VT encoder
        // with GPU texture readback.
        #[cfg(target_os = "macos")]
        {
            if self.encoders_started {
                self.use_gpu_pipeline = true;
                self.latest_pipeline_text = format!(
                    "Pipeline: GPU readback (offscreen {}x{})",
                    self.gpu_capture.width, self.gpu_capture.height
                );
                crate::log!("xr_remote host: GPU readback pipeline active (Mac VT encoder)");
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            if !self.gpu_encoders_started {
                self.ensure_gpu_encoders(cx);
            }
            if self.gpu_encoders_started {
                self.use_gpu_pipeline = true;
                self.latest_pipeline_text = format!(
                    "Pipeline: GPU (offscreen {}x{})",
                    self.gpu_capture.width, self.gpu_capture.height
                );
                crate::log!("xr_remote host: GPU capture pipeline active");
            }
        }
    }

    fn render_offscreen_eyes(&mut self, cx: &mut Cx, draw_event: &DrawEvent) {
        if !self.use_gpu_pipeline {
            return;
        }

        // Get the GPU scene widget and update its state before creating CxDraw.
        let gpu_scene = self.ui.widget(cx, ids!(gpu_scene));
        apply_scene_content_state(
            gpu_scene.clone(),
            cx,
            &self.render_state,
            &self.marker_state,
        );

        // Build tracking data for per-eye camera matrices.
        let tracking = make_tracking_packet(
            self.frame_group_counter.wrapping_add(1),
            (self.latest_state.time * 1_000_000_000.0) as u64,
            self.latest_state.head_pose,
            self.session_config.ipd_meters,
            self.session_config.fov_y_degrees,
            self.session_config.per_eye_width,
            self.session_config.per_eye_height,
            if self.latest_state_received {
                self.latest_state.anchor
            } else {
                None
            },
        );

        let width = self.gpu_capture.width;
        let height = self.gpu_capture.height;
        let viewport_rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(width as f64, height as f64),
        };
        let time = cx.seconds_since_app_start();
        let eye_packets = [&tracking.left_eye, &tracking.right_eye];
        let eye_states: [SceneState3D; 2] = [
            Self::build_eye_scene_state(eye_packets[0], viewport_rect, time),
            Self::build_eye_scene_state(eye_packets[1], viewport_rect, time),
        ];

        let mut cx_draw = CxDraw::new(cx, draw_event);
        for eye_idx in 0..2 {
            let scene_state = eye_states[eye_idx];
            if let Some(target) = &mut self.gpu_capture.eyes[eye_idx] {
                cx_draw.begin_pass(&target.pass, Some(1.0));

                // Write per-eye camera matrices into the pass uniforms.
                {
                    let pass_id = target.pass.draw_pass_id();
                    let camera_inv = scene_state.view.invert();
                    let pu = &mut cx_draw.cx.passes[pass_id].pass_uniforms;
                    pu.camera_projection = scene_state.projection;
                    pu.camera_projection_r = scene_state.projection;
                    pu.camera_view = scene_state.view;
                    pu.camera_view_r = scene_state.view;
                    pu.depth_projection = scene_state.projection;
                    pu.depth_projection_r = scene_state.projection;
                    pu.depth_view = scene_state.view;
                    pu.depth_view_r = scene_state.view;
                    pu.camera_inv = camera_inv;
                    pu.camera_inv_r = camera_inv;
                    pu.time = time as f32;
                }

                // Draw 3D scene content into the offscreen pass.
                {
                    let mut cx3d = Cx3d::new(&mut cx_draw);
                    target.draw_list.begin_always(&mut cx3d);
                    cx3d.begin_scene_3d(scene_state);
                    gpu_scene.draw_3d_all(&mut cx3d, &mut Scope::empty());
                    cx3d.end_scene_3d();
                    target.draw_list.end(&mut cx3d);
                }

                cx_draw.end_pass(&target.pass);
            }
        }
    }

    fn build_eye_scene_state(eye: &EyeViewPacket, viewport_rect: Rect, time: f64) -> SceneState3D {
        let view = eye.pose.to_mat4().invert();
        let projection = Mat4f::perspective(eye.fov_y_degrees, eye.aspect, 0.05, 200.0);
        SceneState3D {
            time,
            camera_pos: eye.pose.position,
            view,
            projection,
            viewport_rect,
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
            .widget(cx, ids!(host_render))
            .set_text(cx, &self.latest_render_text);
        self.ui
            .widget(cx, ids!(host_marker))
            .set_text(cx, &self.latest_marker_text);
        self.ui
            .widget(cx, ids!(host_pose))
            .set_text(cx, &self.latest_pose_text);
        self.ui
            .widget(cx, ids!(host_stream))
            .set_text(cx, &stream_text);
        self.ui
            .widget(cx, ids!(host_remote_log))
            .set_text(cx, &self.latest_remote_log_text);
        self.ui
            .widget(cx, ids!(host_pipeline))
            .set_text(cx, &self.latest_pipeline_text);
        self.ui
            .widget(cx, ids!(preview_title))
            .set_text(cx, self.preview_title());
        self.ui
            .widget(cx, ids!(preview_caption))
            .set_text(cx, self.preview_caption());
        let local_scene_mode = self.render_state.mode == XrRemoteRenderMode::LocalScene;
        self.ui
            .widget(cx, ids!(stream_preview_panel))
            .set_visible(cx, !local_scene_mode);
        self.ui
            .widget(cx, ids!(quest_monitor_panel))
            .set_visible(cx, local_scene_mode);
        apply_scene_content_state(
            self.ui.widget(cx, ids!(quest_scene_monitor.scene_content)),
            cx,
            &self.render_state,
            &self.marker_state,
        );
    }

    fn preview_title(&self) -> &'static str {
        match self.render_state.mode {
            XrRemoteRenderMode::Stream => "Outgoing Stream Preview",
            XrRemoteRenderMode::LocalScene => "Quest Local Scene Preview",
        }
    }

    fn preview_caption(&self) -> &'static str {
        match self.render_state.mode {
            XrRemoteRenderMode::Stream => {
                "These are the exact pre-encode eye frames being sent to Quest."
            }
            XrRemoteRenderMode::LocalScene => {
                "This is the host-side expected view of the scene Quest renders locally."
            }
        }
    }

    fn update_preview_texture(&mut self, cx: &mut Cx, eye: XrRemoteEye) {
        let eye_index = eye.index();
        let width = self.session_config.per_eye_width as usize;
        let height = self.session_config.per_eye_height as usize;
        let expected_len = width.saturating_mul(height).saturating_mul(4);
        if self.eye_bgra_frames[eye_index].len() < expected_len {
            return;
        }

        let pixels = self.eye_bgra_frames[eye_index]
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect::<Vec<u32>>();

        let texture = match &self.preview_textures[eye_index] {
            Some(texture) if texture.get_format(cx).vec_width_height() == Some((width, height)) => {
                texture.set_data_u32(cx, width, height, pixels);
                texture.clone()
            }
            _ => {
                let texture = Texture::new_with_format(
                    cx,
                    TextureFormat::VecBGRAu8_32 {
                        data: Some(pixels),
                        width,
                        height,
                        updated: TextureUpdated::Full,
                    },
                );
                self.preview_textures[eye_index] = Some(texture.clone());
                texture
            }
        };

        let image = match eye {
            XrRemoteEye::Left => self.ui.image(cx, ids!(preview_left)),
            XrRemoteEye::Right => self.ui.image(cx, ids!(preview_right)),
        };
        image.set_texture(cx, Some(texture));
    }

    fn update_gpu_preview_textures(&mut self, cx: &mut Cx) {
        for eye in XrRemoteEye::ALL {
            if let Some(target) = self.gpu_capture.eye_target(eye) {
                let texture = target.texture();
                let image = match eye {
                    XrRemoteEye::Left => self.ui.image(cx, ids!(preview_left)),
                    XrRemoteEye::Right => self.ui.image(cx, ids!(preview_right)),
                };
                image.set_texture(cx, Some(texture));
            }
        }
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
                self.shared.send_current_control_state();
            }
            ControlPacket::ClientMediaChannel(channel) => {
                self.shared.set_client_media_channel(channel);
                self.shared.send_current_control_state();
                self.latest_status = "Client media port registered".to_string();
            }
            ControlPacket::KeyframeRequest(request) => {
                self.shared.request_keyframe(request.eye);
                self.latest_stream_text = format!("Stream: keyframe requested {:?}", request.eye);
            }
            ControlPacket::LogLine(line) => {
                self.latest_remote_log_text =
                    format!("Remote [{}] {}: {}", line.level, line.source, line.text);
                crate::log!(
                    "xr_remote remote-log [{}] {} @{}ns: {}",
                    line.level,
                    line.source,
                    line.timestamp_ns,
                    line.text
                );
            }
            ControlPacket::SessionConfig(_)
            | ControlPacket::RenderState(_)
            | ControlPacket::MarkerState(_)
            | ControlPacket::StreamConfig(_)
            | ControlPacket::VideoConfig(_) => {}
        }
        self.refresh_labels(cx);
    }

    fn handle_xr_net_message(&mut self, message: XrNetIncoming) {
        match message {
            XrNetIncoming::Join { peer } => {
                self.latest_status = format!("XR Net peer connected: {}", peer.addr);
            }
            XrNetIncoming::Leave { peer, .. } => {
                self.latest_status = format!("XR Net peer left: {}", peer.addr);
                self.latest_state_received = false;
            }
            XrNetIncoming::State { peer, frame } => {
                self.latest_state = frame.state;
                self.latest_state_received = true;
                self.latest_pose_text = format!(
                    "XR {} head ({:.2}, {:.2}, {:.2})",
                    peer.addr,
                    self.latest_state.head_pose.position.x,
                    self.latest_state.head_pose.position.y,
                    self.latest_state.head_pose.position.z
                );
            }
            XrNetIncoming::Alignment { frame, .. } => {
                self.latest_state.anchor = Some(frame.anchor);
            }
            XrNetIncoming::AlignmentDescriptor { .. } => {}
        }
    }

    fn drain_xr_net(&mut self) {
        let mut disconnected = false;
        let mut queued = Vec::new();
        if let Some(xr_net) = self.xr_net.as_mut() {
            loop {
                match xr_net.incoming_receiver.try_recv() {
                    Ok(message) => queued.push(message),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        for message in queued {
            self.handle_xr_net_message(message);
        }
        if disconnected {
            self.latest_status = "XR Net disconnected".to_string();
            self.latest_state_received = false;
        }
    }

    fn push_eye_frame(
        &mut self,
        cx: &mut Cx,
        tracking: &TrackingPacket,
        frame_group_id: u64,
        encode_timestamp_ns: u64,
        eye: XrRemoteEye,
        encode_enabled: bool,
        request_keyframe: bool,
        skip_rasterize: bool,
    ) -> bool {
        let eye_index = eye.index();
        if !skip_rasterize {
            render_eye_scene(
                &mut self.eye_bgra_frames[eye_index],
                &mut self.eye_depth_buffers[eye_index],
                tracking,
                eye,
                &self.session_config,
                &self.render_state,
                &self.marker_state,
            );
        }
        self.update_preview_texture(cx, eye);

        if !encode_enabled {
            return true;
        }

        let timestamp_ns = encode_timestamp_ns;
        let meta = PendingFrameMeta {
            session_id: self.session_config.session_id,
            frame_group_id,
            frame_id: frame_group_id,
            tracking_id: tracking.tracking_id,
            pts_ns: timestamp_ns,
        };
        self.shared
            .eye_shared(eye)
            .queue_pending_meta(timestamp_ns, meta);

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

    #[cfg(target_os = "macos")]
    fn read_gpu_texture_to_bgra(&mut self, cx: &mut Cx, eye: XrRemoteEye) {
        let Some(target) = self.gpu_capture.eye_target(eye) else {
            return;
        };
        let texture_id = target.texture_id();
        let cx_texture = &cx.textures[texture_id];
        if let Some((_w, _h, bgra)) = cx_texture.read_back_to_bgra() {
            self.eye_bgra_frames[eye.index()] = bgra;
        }
    }

    fn push_frame(&mut self, cx: &mut Cx) {
        let media_ready = self.shared.all_media_connected();
        #[cfg(not(target_os = "macos"))]
        let gpu_platform_encode_enabled = media_ready
            && self.use_gpu_pipeline
            && self.gpu_encoders_started
            && self.render_state.mode == XrRemoteRenderMode::Stream;
        #[cfg(target_os = "macos")]
        let gpu_platform_encode_enabled = false;
        let gpu_readback_encode_enabled = media_ready
            && self.use_gpu_pipeline
            && !self.gpu_encoders_started
            && self.encoders_started
            && self.render_state.mode == XrRemoteRenderMode::Stream;
        let cpu_encode_enabled = media_ready
            && !self.use_gpu_pipeline
            && self.encoders_started
            && self.render_state.mode == XrRemoteRenderMode::Stream;
        let encode_enabled =
            gpu_platform_encode_enabled || gpu_readback_encode_enabled || cpu_encode_enabled;
        let using_live_tracking = self.latest_state_received;
        if !using_live_tracking {
            self.latest_pose_text =
                "Pose: preview fallback camera (waiting for xr_net tracking)".to_string();
        }

        let tracking = make_tracking_packet(
            self.frame_group_counter.wrapping_add(1),
            (self.latest_state.time * 1_000_000_000.0) as u64,
            self.latest_state.head_pose,
            self.session_config.ipd_meters,
            self.session_config.fov_y_degrees,
            self.session_config.per_eye_width,
            self.session_config.per_eye_height,
            if using_live_tracking {
                self.latest_state.anchor
            } else {
                None
            },
        );

        let render_started_ns = (cx.seconds_since_app_start() * 1_000_000_000.0) as u64;
        let frame_group_id = self.frame_group_counter.wrapping_add(1);
        let request_keyframe = encode_enabled
            && (self.shared.any_eye_requires_keyframe()
                || frame_group_id % ((self.session_config.fps / 2).max(1) as u64) == 0);
        let encode_timestamp_ns = render_started_ns.max(self.frame_group_counter.saturating_add(1));

        let (left_ok, right_ok) = if gpu_platform_encode_enabled {
            #[cfg(not(target_os = "macos"))]
            {
                // GPU path: capture from offscreen render textures via platform encoder
                self.update_gpu_preview_textures(cx);
                let left_ok = self.push_gpu_eye_frame(
                    cx,
                    XrRemoteEye::Left,
                    encode_timestamp_ns,
                    frame_group_id,
                    tracking.tracking_id,
                    request_keyframe,
                );
                let right_ok = self.push_gpu_eye_frame(
                    cx,
                    XrRemoteEye::Right,
                    encode_timestamp_ns,
                    frame_group_id,
                    tracking.tracking_id,
                    request_keyframe,
                );
                (left_ok, right_ok)
            }
            #[cfg(target_os = "macos")]
            {
                (false, false)
            }
        } else if gpu_readback_encode_enabled {
            // GPU readback path: read texture pixels back to CPU, encode with existing encoder
            #[cfg(target_os = "macos")]
            {
                self.read_gpu_texture_to_bgra(cx, XrRemoteEye::Left);
                self.read_gpu_texture_to_bgra(cx, XrRemoteEye::Right);
            }
            self.update_gpu_preview_textures(cx);
            let left_ok = self.push_eye_frame(
                cx,
                &tracking,
                frame_group_id,
                encode_timestamp_ns,
                XrRemoteEye::Left,
                true,
                request_keyframe,
                true, // skip rasterize — GPU data already in buffer
            );
            let right_ok = self.push_eye_frame(
                cx,
                &tracking,
                frame_group_id,
                encode_timestamp_ns,
                XrRemoteEye::Right,
                true,
                request_keyframe,
                true, // skip rasterize — GPU data already in buffer
            );
            (left_ok, right_ok)
        } else {
            // CPU path: software rasterize + encode via VT/platform encoder
            let left_ok = self.push_eye_frame(
                cx,
                &tracking,
                frame_group_id,
                encode_timestamp_ns,
                XrRemoteEye::Left,
                cpu_encode_enabled,
                request_keyframe,
                false,
            );
            let right_ok = self.push_eye_frame(
                cx,
                &tracking,
                frame_group_id,
                encode_timestamp_ns,
                XrRemoteEye::Right,
                cpu_encode_enabled,
                request_keyframe,
                false,
            );
            (left_ok, right_ok)
        };
        let render_finished_ns = (cx.seconds_since_app_start() * 1_000_000_000.0) as u64;
        if left_ok && right_ok {
            self.frame_group_counter = frame_group_id;
        }
        self.ui.redraw(cx);
        let render_ms = (render_finished_ns.saturating_sub(render_started_ns) as f64) / 1_000_000.0;
        self.latest_stream_text = if self.render_state.mode == XrRemoteRenderMode::LocalScene {
            format!(
                "Quest local scene: desktop monitor active, video streaming idle ({:.1} ms)",
                render_ms
            )
        } else if encode_enabled {
            format!(
                "Stream: group {} track {} render {:.1} ms cfg L{} R{}",
                frame_group_id,
                tracking.tracking_id,
                render_ms,
                self.shared
                    .eye_shared(XrRemoteEye::Left)
                    .current_config_id(),
                self.shared
                    .eye_shared(XrRemoteEye::Right)
                    .current_config_id(),
            )
        } else if media_ready {
            format!(
                "Stream: media ready, preview only (encoder unavailable) ({:.1} ms)",
                render_ms
            )
        } else if using_live_tracking {
            format!(
                "Stream: previewing live xr_net pose, waiting for dual-eye media client ({:.1} ms)",
                render_ms
            )
        } else {
            format!(
                "Stream: previewing fallback pose, waiting for xr_net tracking and media client ({:.1} ms)",
                render_ms
            )
        };
        self.refresh_labels(cx);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self
            .ui
            .button(cx, ids!(render_stream_button))
            .clicked(actions)
        {
            let scene = self.render_state.scene;
            self.set_render_state(cx, XrRemoteRenderMode::Stream, scene);
        }
        if self
            .ui
            .button(cx, ids!(render_test_scene_button))
            .clicked(actions)
        {
            self.set_render_state(cx, XrRemoteRenderMode::LocalScene, XrRemoteSceneId::Test);
        }
        if self
            .ui
            .button(cx, ids!(render_tree_scene_button))
            .clicked(actions)
        {
            self.set_render_state(cx, XrRemoteRenderMode::LocalScene, XrRemoteSceneId::Tree);
        }
        if self.ui.button(cx, ids!(gpu_toggle_button)).clicked(actions) {
            if self.use_gpu_pipeline {
                // Switch to CPU pipeline
                self.use_gpu_pipeline = false;
                self.latest_pipeline_text = "Pipeline: CPU (software rasterizer)".to_string();
                crate::log!("xr_remote host: switched to CPU pipeline");
            } else {
                // Try to start GPU pipeline
                self.try_start_gpu_pipeline(cx);
                if !self.use_gpu_pipeline {
                    self.latest_pipeline_text =
                        "Pipeline: GPU unavailable, staying on CPU".to_string();
                    crate::log!("xr_remote host: GPU pipeline unavailable");
                } else {
                    crate::log!("xr_remote host: switched to GPU pipeline");
                }
            }
            self.refresh_labels(cx);
        }
        if self
            .ui
            .button(cx, ids!(marker_left_button))
            .clicked(actions)
        {
            self.nudge_marker(cx, -0.08, 0.0, 0.0);
        }
        if self
            .ui
            .button(cx, ids!(marker_right_button))
            .clicked(actions)
        {
            self.nudge_marker(cx, 0.08, 0.0, 0.0);
        }
        if self.ui.button(cx, ids!(marker_up_button)).clicked(actions) {
            self.nudge_marker(cx, 0.0, 0.08, 0.0);
        }
        if self
            .ui
            .button(cx, ids!(marker_down_button))
            .clicked(actions)
        {
            self.nudge_marker(cx, 0.0, -0.08, 0.0);
        }
        if self
            .ui
            .button(cx, ids!(marker_near_button))
            .clicked(actions)
        {
            self.nudge_marker(cx, 0.0, 0.0, 0.08);
        }
        if self.ui.button(cx, ids!(marker_far_button)).clicked(actions) {
            self.nudge_marker(cx, 0.0, 0.0, -0.08);
        }
        if self
            .ui
            .button(cx, ids!(marker_pulse_button))
            .clicked(actions)
        {
            self.pulse_marker(cx);
        }
        if self
            .ui
            .button(cx, ids!(marker_reset_button))
            .clicked(actions)
        {
            self.set_marker_state(cx, default_marker_state());
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_xr::script_mod(vm);
        crate::shared_scene::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if matches!(event, Event::Startup) {
            self.ensure_started(cx);
            self.ensure_encoders(cx);
            self.try_start_gpu_pipeline(cx);
        }
        // Render offscreen eye passes during the draw cycle so Metal
        // allocates and clears the render textures. The next frame timer
        // tick will capture from these textures for encoding.
        if let Event::Draw(draw_event) = event {
            self.render_offscreen_eyes(cx, draw_event);
        }
        self.drain_xr_net();
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
