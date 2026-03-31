use crate::{protocol::*, scene::make_tracking_packet, wire::*};
use makepad_widgets::makepad_platform::{
    makepad_live_id::LiveId,
    thread::SignalToUI,
    video::{VideoBitstreamFormat, VideoDecodeOutput, VideoDecoderConfig},
};
use makepad_widgets::*;
use makepad_xr::*;
use std::{
    collections::{BTreeMap, VecDeque},
    net::TcpStream,
    sync::{Arc, Mutex},
    thread,
};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.RemoteEyeSurfaceBase = #(RemoteEyeSurface::register_widget(vm))
    mod.widgets.RemoteEyeSurface = set_type_default() do mod.widgets.RemoteEyeSurfaceBase{
        width: Fill
        height: Fill
        draw_bg +: {
            video_texture: texture_video()
            target_eye: 0.0

            pixel: fn() {
                if (self.target_eye < 0.5 && VIEW_ID != 0) || (self.target_eye >= 0.5 && VIEW_ID == 0) {
                    return #0000
                }
                return self.video_texture.sample_video(self.pos)
            }
        }
    }

    startup() do #(App::script_component(vm)){
        ui: XrRoot{
            window.inner_size: vec2(1600, 960)
            pass.clear_color: #000
            camera.fov_y: 52.0
            camera.distance: 1.4
            env.env_cube: false
            env.depth_mesh: false

            immersive_left := XrView{
                visible: false
                mode: mod.widgets.XrViewMode.World
                logical_size: vec2(1200, 1200)
                pixel_scale: 0.00052
                dpi_factor: 1.0
                depth_scale: 1.0
                immersive_left_body := SolidView{
                    width: Fill
                    height: Fill
                    draw_bg.color: #0000
                    remote_left_surface := mod.widgets.RemoteEyeSurface{
                        draw_bg.target_eye: 0.0
                    }
                }
            }

            immersive_right := XrView{
                visible: false
                mode: mod.widgets.XrViewMode.World
                logical_size: vec2(1200, 1200)
                pixel_scale: 0.00052
                dpi_factor: 1.0
                depth_scale: 1.0
                immersive_right_body := SolidView{
                    width: Fill
                    height: Fill
                    draw_bg.color: #0000
                    remote_right_surface := mod.widgets.RemoteEyeSurface{
                        draw_bg.target_eye: 1.0
                    }
                }
            }

            debug_hud := XrView{
                mode: mod.widgets.XrViewMode.StuckToWrist
                show_in_non_xr: true
                wrist_left: true
                logical_size: vec2(360, 220)
                pixel_scale: 0.00034
                dpi_factor: 1.6
                depth_scale: 120.0

                debug_hud_body := SolidView{
                    width: Fill
                    height: Fill
                    flow: Down
                    padding: 10
                    spacing: 8
                    draw_bg.color: #x102234f5
                    draw_bg.border_radius: 16.0

                    Label{
                        text: "XR Remote"
                        draw_text.color: #xf4fbff
                        draw_text.text_style.font_size: 14.0
                    }

                    connection_field := Label{
                        text: "Connecting..."
                        draw_text.color: #xb7cad9
                    }

                    stream_field := Label{
                        text: "Stream: waiting"
                        draw_text.color: #x8fa8bd
                    }

                    clock_field := Label{
                        text: "Clock: waiting"
                        draw_text.color: #x8fa8bd
                    }
                }
            }

            xr_permissions := mod.widgets.XrPermissionsFlow{}
        }
    }
}

fn remote_video_texture_format(_cx: &Cx) -> TextureFormat {
    TextureFormat::VideoExternal
}

#[derive(Script, ScriptHook, Widget)]
pub struct RemoteEyeSurface {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
}

impl RemoteEyeSurface {
    pub fn set_video_texture(&mut self, cx: &mut Cx, texture: Option<&Texture>) {
        match texture {
            Some(texture) => self.draw_bg.draw_vars.set_texture(0, texture),
            None => self.draw_bg.draw_vars.empty_texture(0),
        }
        self.redraw(cx);
    }
}

impl Widget for RemoteEyeSurface {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if matches!(event, Event::Signal | Event::VideoTextureUpdated(_)) {
            self.redraw(cx);
        }
    }
}

struct ClientShared {
    control_writer: Arc<Mutex<Option<TcpStream>>>,
    control_inbox: Arc<Mutex<Vec<ControlPacket>>>,
    media_inbox: Arc<Mutex<Vec<MediaChunkPacket>>>,
    capabilities: Arc<Mutex<CapabilitiesPacket>>,
    media_ports: Arc<Mutex<Option<ClientMediaChannelsPacket>>>,
}

impl ClientShared {
    fn new() -> Self {
        Self {
            control_writer: Arc::new(Mutex::new(None)),
            control_inbox: Arc::new(Mutex::new(Vec::new())),
            media_inbox: Arc::new(Mutex::new(Vec::new())),
            capabilities: Arc::new(Mutex::new(default_capabilities())),
            media_ports: Arc::new(Mutex::new(None)),
        }
    }

    fn start_threads(&self) {
        let left_socket = bind_udp_socket_any();
        let right_socket = bind_udp_socket_any();
        let ports = ClientMediaChannelsPacket {
            left_port: left_socket.local_addr().map(|addr| addr.port()).unwrap_or(0),
            right_port: right_socket.local_addr().map(|addr| addr.port()).unwrap_or(0),
        };
        *self.media_ports.lock().unwrap() = Some(ports.clone());

        let media_inbox = self.media_inbox.clone();
        thread::spawn(move || {
            let mut buffer = vec![0u8; max_media_packet_bytes()];
            loop {
                match recv_udp_packet::<MediaChunkPacket>(&left_socket, &mut buffer) {
                    Ok(packet) => {
                        media_inbox.lock().unwrap().push(packet);
                        SignalToUI::set_ui_signal();
                    }
                    Err(_) => {}
                }
            }
        });

        let media_inbox = self.media_inbox.clone();
        thread::spawn(move || {
            let mut buffer = vec![0u8; max_media_packet_bytes()];
            loop {
                match recv_udp_packet::<MediaChunkPacket>(&right_socket, &mut buffer) {
                    Ok(packet) => {
                        media_inbox.lock().unwrap().push(packet);
                        SignalToUI::set_ui_signal();
                    }
                    Err(_) => {}
                }
            }
        });

        let host = remote_host();
        let control_addr = format!("{}:{}", host, control_port());
        let control_writer = self.control_writer.clone();
        let control_inbox = self.control_inbox.clone();
        let capabilities = self.capabilities.clone();
        let media_ports = self.media_ports.clone();
        thread::spawn(move || loop {
            let mut stream = connect_with_retry(&control_addr);
            if let Ok(writer) = stream.try_clone() {
                *control_writer.lock().unwrap() = Some(writer);
            }
            let _ = send_framed(
                &mut stream,
                &ControlPacket::Hello(HelloPacket {
                    role: "quest-client".to_string(),
                    protocol_version: XR_REMOTE_PROTOCOL_VERSION,
                }),
            );
            let advertised_capabilities = capabilities.lock().unwrap().clone();
            let _ = send_framed(&mut stream, &ControlPacket::Capabilities(advertised_capabilities));
            if let Some(channels) = media_ports.lock().unwrap().clone() {
                let _ = send_framed(&mut stream, &ControlPacket::ClientMediaChannels(channels));
            }
            let _ = send_framed(
                &mut stream,
                &ControlPacket::KeyframeRequest(KeyframeRequestPacket {
                    eye: XrRemoteEyeTarget::Both,
                }),
            );
            SignalToUI::set_ui_signal();
            while let Ok(packet) = recv_framed::<ControlPacket>(&mut stream) {
                control_inbox.lock().unwrap().push(packet);
                SignalToUI::set_ui_signal();
            }
            *control_writer.lock().unwrap() = None;
            SignalToUI::set_ui_signal();
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

    fn set_capabilities(&self, capabilities: CapabilitiesPacket) {
        *self.capabilities.lock().unwrap() = capabilities;
    }

    fn drain_control(&self) -> Vec<ControlPacket> {
        let mut inbox = self.control_inbox.lock().unwrap();
        std::mem::take(&mut *inbox)
    }

    fn drain_media(&self) -> Vec<MediaChunkPacket> {
        let mut inbox = self.media_inbox.lock().unwrap();
        std::mem::take(&mut *inbox)
    }

    fn media_ports(&self) -> Option<ClientMediaChannelsPacket> {
        self.media_ports.lock().unwrap().clone()
    }
}

impl Default for ClientShared {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
struct CompletedEyeFrame {
    header: MediaChunkHeader,
    bytes: Vec<u8>,
}

struct StereoGroupAssembly {
    left: Option<CompletedEyeFrame>,
    right: Option<CompletedEyeFrame>,
    received_at_ns: u64,
}

struct IncompleteEyeFrame {
    header: MediaChunkHeader,
    received_at_ns: u64,
    chunks: Vec<Option<Vec<u8>>>,
    received_chunks: usize,
}

struct ClientEyeState {
    active_stream: Option<StreamConfigPacket>,
    known_configs: BTreeMap<u32, VideoConfigPacket>,
    incomplete_frames: BTreeMap<u64, IncompleteEyeFrame>,
    decoder_texture: Option<Texture>,
    decoder_started: bool,
    last_config_pushed: u32,
    media_chunks_received: u64,
    completed_frames: u64,
    decoded_updates: u64,
    decoder_errors: u64,
}

impl Default for ClientEyeState {
    fn default() -> Self {
        Self {
            active_stream: None,
            known_configs: BTreeMap::new(),
            incomplete_frames: BTreeMap::new(),
            decoder_texture: None,
            decoder_started: false,
            last_config_pushed: 0,
            media_chunks_received: 0,
            completed_frames: 0,
            decoded_updates: 0,
            decoder_errors: 0,
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    shared: ClientShared,
    #[rust]
    network_started: bool,
    #[rust]
    ping_timer: Timer,
    #[rust]
    latest_connection_text: String,
    #[rust]
    latest_stream_text: String,
    #[rust]
    latest_clock_text: String,
    #[rust]
    last_sent_time_ns: u64,
    #[rust]
    tracking_counter: u64,
    #[rust]
    active_session: Option<SessionConfigPacket>,
    #[rust]
    clock_offset_ns: i64,
    #[rust]
    clock_sync_ready: bool,
    #[rust]
    eye_states: [ClientEyeState; 2],
    #[rust]
    pending_stereo_groups: BTreeMap<u64, StereoGroupAssembly>,
    #[rust]
    ready_stereo_groups: VecDeque<(u64, CompletedEyeFrame, CompletedEyeFrame)>,
    #[rust]
    latest_displayed_group: u64,
    #[rust]
    h265_watchdog_started_at: Option<f64>,
    #[rust]
    h265_fallback_requested: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            ui: WidgetRef::default(),
            shared: ClientShared::new(),
            network_started: false,
            ping_timer: Timer::default(),
            latest_connection_text: format!("Connecting to {}", remote_host()),
            latest_stream_text: "Stream: waiting".to_string(),
            latest_clock_text: "Clock: waiting".to_string(),
            last_sent_time_ns: 0,
            tracking_counter: 0,
            active_session: None,
            clock_offset_ns: 0,
            clock_sync_ready: false,
            eye_states: std::array::from_fn(|_| ClientEyeState::default()),
            pending_stereo_groups: BTreeMap::new(),
            ready_stereo_groups: VecDeque::new(),
            latest_displayed_group: 0,
            h265_watchdog_started_at: None,
            h265_fallback_requested: false,
        }
    }
}

impl App {
    fn decoder_slot(eye: XrRemoteEye) -> usize {
        match eye {
            XrRemoteEye::Left => XR_REMOTE_LEFT_DECODER_SLOT,
            XrRemoteEye::Right => XR_REMOTE_RIGHT_DECODER_SLOT,
        }
    }

    fn decoder_video_id(eye: XrRemoteEye) -> LiveId {
        LiveId::from_str_num("android_realtime_video_decoder", Self::decoder_slot(eye) as u64)
    }

    fn eye_state(&self, eye: XrRemoteEye) -> &ClientEyeState {
        &self.eye_states[eye.index()]
    }

    fn eye_state_mut(&mut self, eye: XrRemoteEye) -> &mut ClientEyeState {
        &mut self.eye_states[eye.index()]
    }

    fn surface_ref(&self, cx: &mut Cx, eye: XrRemoteEye) -> WidgetRef {
        match eye {
            XrRemoteEye::Left => self.ui.widget(cx, ids!(remote_left_surface)),
            XrRemoteEye::Right => self.ui.widget(cx, ids!(remote_right_surface)),
        }
    }

    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.network_started {
            return;
        }
        let mut capabilities = default_capabilities();
        let preferred = preferred_codecs_from_capabilities(&cx.video_capabilities(), false);
        if !preferred.is_empty() {
            capabilities.codecs = preferred;
        }
        self.shared.set_capabilities(capabilities);
        self.shared.start_threads();
        self.ping_timer = cx.start_interval(1.0);
        self.network_started = true;
        if let Some(ports) = self.shared.media_ports() {
            self.latest_connection_text = format!(
                "Connecting to {} tcp:{} udp:{}|{}",
                remote_host(),
                control_port(),
                ports.left_port,
                ports.right_port
            );
        }
        self.refresh_labels(cx);
    }

    fn send_remote_log(&self, cx: &Cx, level: &str, source: &str, text: impl Into<String>) {
        self.shared.send_control(&ControlPacket::LogLine(LogLinePacket {
            timestamp_ns: (cx.seconds_since_app_start() * 1_000_000_000.0) as u64,
            level: level.to_string(),
            source: source.to_string(),
            text: text.into(),
        }));
    }

    fn refresh_labels(&mut self, cx: &mut Cx) {
        self.ui
            .widget(cx, ids!(connection_field))
            .set_text(cx, &self.latest_connection_text);
        self.ui
            .widget(cx, ids!(stream_field))
            .set_text(cx, &self.latest_stream_text);
        self.ui
            .widget(cx, ids!(clock_field))
            .set_text(cx, &self.latest_clock_text);
    }

    fn apply_eye_texture_to_surface(&mut self, cx: &mut Cx, eye: XrRemoteEye) {
        let texture = self.eye_state(eye).decoder_texture.clone();
        let surface = self.surface_ref(cx, eye);
        {
            if let Some(mut inner) = surface.borrow_mut::<RemoteEyeSurface>() {
                inner.set_video_texture(cx, texture.as_ref());
                return;
            }
        }
        if let Some(mut inner) = surface.cast_inner_mut::<RemoteEyeSurface>() {
            inner.set_video_texture(cx, texture.as_ref());
        };
    }

    fn ensure_decoder_texture(&mut self, cx: &mut Cx, eye: XrRemoteEye) -> Option<Texture> {
        if let Some(texture) = self.eye_state(eye).decoder_texture.clone() {
            return Some(texture);
        }
        let texture = Texture::new_with_format(cx, remote_video_texture_format(cx));
        self.eye_state_mut(eye).decoder_texture = Some(texture.clone());
        self.apply_eye_texture_to_surface(cx, eye);
        Some(texture)
    }

    fn reset_eye_decoder(&mut self, cx: &mut Cx, eye: XrRemoteEye) {
        if self.eye_state(eye).decoder_started {
            cx.video_decoder_stop(Self::decoder_slot(eye));
        }
        let state = self.eye_state_mut(eye);
        state.decoder_started = false;
        state.last_config_pushed = 0;
        state.incomplete_frames.clear();
        state.completed_frames = 0;
        state.decoded_updates = 0;
        state.decoder_errors = 0;
        self.set_immersive_visible(cx, false);
    }

    fn reset_stereo_sync(&mut self) {
        self.pending_stereo_groups.clear();
        self.ready_stereo_groups.clear();
        self.latest_displayed_group = 0;
        self.h265_watchdog_started_at = None;
        self.h265_fallback_requested = false;
    }

    fn try_start_eye_decoder(
        &mut self,
        cx: &mut Cx,
        eye: XrRemoteEye,
    ) -> Result<bool, VideoDecodeError> {
        if self.eye_state(eye).decoder_started {
            return Ok(true);
        }
        let Some(stream_config) = self.eye_state(eye).active_stream.clone() else {
            return Ok(false);
        };
        let Some(texture) = self.ensure_decoder_texture(cx, eye) else {
            return Ok(false);
        };
        let config = VideoDecoderConfig {
            codec: stream_config.codec.video_codec(),
            expected_format: VideoBitstreamFormat::AnnexB,
            output: VideoDecodeOutput::Texture {
                texture_id: texture.texture_id(),
            },
            width_hint: Some(stream_config.width),
            height_hint: Some(stream_config.height),
            latency_realtime: true,
        };
        cx.video_decoder_start_box(Self::decoder_slot(eye), config, Box::new(|_| {}))?;
        self.eye_state_mut(eye).decoder_started = true;
        self.apply_eye_texture_to_surface(cx, eye);
        Ok(true)
    }

    fn handle_control_packet(&mut self, cx: &mut Cx, packet: ControlPacket) {
        match packet {
            ControlPacket::Capabilities(cap) => {
                let codecs = cap
                    .codecs
                    .iter()
                    .map(|codec| codec.label())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.latest_connection_text = format!(
                    "Connected [{}] {}x{} @ {} fps",
                    codecs, cap.per_eye_width, cap.per_eye_height, cap.fps
                );
            }
            ControlPacket::SessionConfig(config) => {
                let session_changed = self
                    .active_session
                    .as_ref()
                    .is_none_or(|current| current.session_id != config.session_id);
                self.active_session = Some(config);
                if session_changed {
                    self.reset_stereo_sync();
                }
            }
            ControlPacket::StreamConfig(config) => {
                let eye = config.eye;
                let changed = self.eye_state(eye).active_stream.as_ref() != Some(&config);
                if changed {
                    self.reset_eye_decoder(cx, eye);
                    self.eye_state_mut(eye).known_configs.clear();
                    self.eye_state_mut(eye).active_stream = Some(config.clone());
                    self.reset_stereo_sync();
                    self.ensure_decoder_texture(cx, eye);
                    let _ = self.try_start_eye_decoder(cx, eye);
                } else {
                    self.eye_state_mut(eye).active_stream = Some(config);
                }
            }
            ControlPacket::VideoConfig(config) => {
                let eye = config.eye;
                self.eye_state_mut(eye)
                    .known_configs
                    .insert(config.config_id, config);
            }
            ControlPacket::ClockSync(sync) => {
                if sync.client_time_ns == 0 {
                    self.latest_clock_text = "Clock: awaiting ping".to_string();
                    self.refresh_labels(cx);
                    return;
                }
                self.clock_offset_ns = sync.server_time_ns as i64 - sync.client_time_ns as i64;
                self.clock_sync_ready = true;
                self.latest_clock_text = format!(
                    "Clock offset {} ms",
                    (self.clock_offset_ns as f64 / 1_000_000.0).round()
                );
            }
            ControlPacket::Ping(_)
            | ControlPacket::Hello(_)
            | ControlPacket::ClientMediaChannels(_)
            | ControlPacket::KeyframeRequest(_)
            | ControlPacket::Tracking(_)
            | ControlPacket::InputState(_)
            | ControlPacket::LogLine(_) => {}
        }
        self.refresh_labels(cx);
    }

    fn server_now_ns(&self, cx: &Cx) -> i64 {
        (cx.seconds_since_app_start() * 1_000_000_000.0) as i64 + self.clock_offset_ns
    }

    fn local_now_ns(&self, cx: &Cx) -> u64 {
        (cx.seconds_since_app_start() * 1_000_000_000.0) as u64
    }

    fn media_frame_is_fresh(&self, cx: &Cx, header: &MediaChunkHeader) -> bool {
        let Some(session) = self.active_session.as_ref() else {
            return true;
        };
        if !self.clock_sync_ready {
            return true;
        }
        let slack_ns = session.stale_after_ns.saturating_mul(8).max(1_000_000_000);
        self.server_now_ns(cx).saturating_sub(header.pts_ns as i64) <= slack_ns as i64
    }

    fn ingest_completed_eye_frame(
        &mut self,
        cx: &Cx,
        eye: XrRemoteEye,
        frame: CompletedEyeFrame,
    ) {
        if !self.media_frame_is_fresh(cx, &frame.header) {
            return;
        }
        let now_ns = self.local_now_ns(cx);
        let entry = self
            .pending_stereo_groups
            .entry(frame.header.frame_group_id)
            .or_insert(StereoGroupAssembly {
                left: None,
                right: None,
                received_at_ns: now_ns,
            });
        match eye {
            XrRemoteEye::Left => entry.left = Some(frame),
            XrRemoteEye::Right => entry.right = Some(frame),
        }
        if let (Some(left), Some(right)) = (entry.left.take(), entry.right.take()) {
            let frame_group_id = left.header.frame_group_id;
            self.ready_stereo_groups
                .push_back((frame_group_id, left, right));
            self.pending_stereo_groups.remove(&frame_group_id);
            while self.ready_stereo_groups.len() > 2 {
                let _ = self.ready_stereo_groups.pop_front();
            }
        }
    }

    fn queue_media_packet(&mut self, cx: &Cx, packet: MediaChunkPacket) {
        let Some(session) = self.active_session.as_ref() else {
            return;
        };
        if packet.header.session_id != session.session_id {
            return;
        }
        let eye = packet.header.eye;
        let now_ns = self.local_now_ns(cx);
        let state = self.eye_state_mut(eye);
        state.media_chunks_received = state.media_chunks_received.wrapping_add(1);

        let frame_id = packet.header.frame_id;
        let replace_existing = state
            .incomplete_frames
            .get(&frame_id)
            .is_some_and(|existing| {
                existing.header.chunk_count != packet.header.chunk_count
                    || existing.header.config_id != packet.header.config_id
            });
        if replace_existing {
            state.incomplete_frames.remove(&frame_id);
        }
        let entry = state
            .incomplete_frames
            .entry(frame_id)
            .or_insert_with(|| IncompleteEyeFrame {
                header: packet.header.clone(),
                received_at_ns: now_ns,
                chunks: vec![None; packet.header.chunk_count as usize],
                received_chunks: 0,
            });
        let chunk_index = packet.header.chunk_index as usize;
        if chunk_index >= entry.chunks.len() {
            return;
        }
        if entry.chunks[chunk_index].is_none() {
            entry.received_chunks += 1;
            entry.chunks[chunk_index] = Some(packet.payload);
        }
        if entry.received_chunks != entry.chunks.len() {
            return;
        }
        let completed = state.incomplete_frames.remove(&frame_id).unwrap();
        let mut bytes = Vec::new();
        for chunk in completed.chunks {
            if let Some(chunk) = chunk {
                bytes.extend_from_slice(&chunk);
            }
        }
        state.completed_frames = state.completed_frames.wrapping_add(1);
        self.ingest_completed_eye_frame(
            cx,
            eye,
            CompletedEyeFrame {
                header: completed.header,
                bytes,
            },
        );
    }

    fn prune_stale_media(&mut self, cx: &Cx) {
        let stale_after = self
            .active_session
            .as_ref()
            .map(|session| session.stale_after_ns)
            .unwrap_or(XR_REMOTE_FRAME_STALE_AFTER_NS);
        let now = self.local_now_ns(cx);
        for eye in XrRemoteEye::ALL {
            let state = self.eye_state_mut(eye);
            state.incomplete_frames.retain(|_, frame| {
                now.saturating_sub(frame.received_at_ns) <= stale_after
            });
            while state.incomplete_frames.len() > 12 {
                let Some(first_key) = state.incomplete_frames.keys().next().copied() else {
                    break;
                };
                state.incomplete_frames.remove(&first_key);
            }
        }
        self.pending_stereo_groups.retain(|_, group| {
            now.saturating_sub(group.received_at_ns) <= stale_after
        });
        while self.pending_stereo_groups.len() > 8 {
            let Some(first_key) = self.pending_stereo_groups.keys().next().copied() else {
                break;
            };
            self.pending_stereo_groups.remove(&first_key);
        }
    }

    fn push_config_if_needed(
        &mut self,
        cx: &mut Cx,
        eye: XrRemoteEye,
        config_id: u32,
    ) -> Result<(), VideoDecodeError> {
        if self.eye_state(eye).last_config_pushed == config_id {
            return Ok(());
        }
        let Some(config) = self.eye_state(eye).known_configs.get(&config_id).cloned() else {
            return Err(VideoDecodeError::DecoderNotStarted);
        };
        cx.video_decoder_push_packet(
            Self::decoder_slot(eye),
            makepad_widgets::makepad_platform::video::VideoDecoderPacketRef {
                pts_ns: 0,
                dts_ns: None,
                is_key: false,
                is_config: true,
                config_id: config.config_id,
                data: &config.bytes,
            },
        )?;
        self.eye_state_mut(eye).last_config_pushed = config_id;
        Ok(())
    }

    fn flush_decoders(&mut self, cx: &mut Cx) {
        for eye in XrRemoteEye::ALL {
            match self.try_start_eye_decoder(cx, eye) {
                Ok(true) => {}
                Ok(false) => return,
                Err(err) => {
                    self.latest_stream_text =
                        format!("Stream: {} decoder start failed: {:?}", eye.label(), err);
                    return;
                }
            }
        }
        if self.ready_stereo_groups.len() > 1 {
            let latest = self.ready_stereo_groups.pop_back();
            self.ready_stereo_groups.clear();
            if let Some(latest) = latest {
                self.ready_stereo_groups.push_back(latest);
            }
        }
        let Some((frame_group_id, left, right)) = self.ready_stereo_groups.pop_front() else {
            return;
        };
        if !self.media_frame_is_fresh(cx, &left.header) || !self.media_frame_is_fresh(cx, &right.header) {
            return;
        }

        if self.push_config_if_needed(cx, XrRemoteEye::Left, left.header.config_id).is_err()
            || self.push_config_if_needed(cx, XrRemoteEye::Right, right.header.config_id).is_err()
        {
            self.ready_stereo_groups.push_front((frame_group_id, left, right));
            return;
        }
        if cx
            .video_decoder_push_packet(
                Self::decoder_slot(XrRemoteEye::Left),
                makepad_widgets::makepad_platform::video::VideoDecoderPacketRef {
                    pts_ns: left.header.pts_ns,
                    dts_ns: None,
                    is_key: left.header.is_key,
                    is_config: false,
                    config_id: left.header.config_id,
                    data: &left.bytes,
                },
            )
            .is_err()
        {
            self.ready_stereo_groups.push_front((frame_group_id, left, right));
            return;
        }
        if cx
            .video_decoder_push_packet(
                Self::decoder_slot(XrRemoteEye::Right),
                makepad_widgets::makepad_platform::video::VideoDecoderPacketRef {
                    pts_ns: right.header.pts_ns,
                    dts_ns: None,
                    is_key: right.header.is_key,
                    is_config: false,
                    config_id: right.header.config_id,
                    data: &right.bytes,
                },
            )
            .is_err()
        {
            self.ready_stereo_groups.push_front((frame_group_id, left, right));
            return;
        }
        self.latest_displayed_group = frame_group_id;
        self.latest_stream_text = format!(
            "Stream: group {} queued L{} R{}",
            frame_group_id, left.header.config_id, right.header.config_id
        );
    }

    fn maybe_request_h264_fallback(&mut self, cx: &Cx) {
        let left_stream = self.eye_state(XrRemoteEye::Left).active_stream.as_ref();
        let right_stream = self.eye_state(XrRemoteEye::Right).active_stream.as_ref();
        let Some(left_stream) = left_stream else {
            return;
        };
        let Some(right_stream) = right_stream else {
            return;
        };
        if left_stream.codec != XrRemoteCodec::H265AnnexB
            || right_stream.codec != XrRemoteCodec::H265AnnexB
            || self.h265_fallback_requested
        {
            return;
        }
        let left_state = self.eye_state(XrRemoteEye::Left);
        let right_state = self.eye_state(XrRemoteEye::Right);
        if left_state.completed_frames == 0 || right_state.completed_frames == 0 {
            return;
        }
        if left_state.decoded_updates > 0 || right_state.decoded_updates > 0 {
            return;
        }
        let now = cx.seconds_since_app_start();
        let started_at = self.h265_watchdog_started_at.get_or_insert(now);
        if now - *started_at < 2.0 {
            return;
        }
        let mut fallback = default_capabilities();
        fallback.codecs = vec![XrRemoteCodec::H264AnnexB];
        fallback.per_eye_width = self
            .active_session
            .as_ref()
            .map(|session| session.per_eye_width)
            .unwrap_or(XR_REMOTE_STREAM_WIDTH);
        fallback.per_eye_height = self
            .active_session
            .as_ref()
            .map(|session| session.per_eye_height)
            .unwrap_or(XR_REMOTE_STREAM_HEIGHT);
        fallback.fps = self
            .active_session
            .as_ref()
            .map(|session| session.fps)
            .unwrap_or(XR_REMOTE_STREAM_FPS);
        self.shared.set_capabilities(fallback.clone());
        self.shared.send_control(&ControlPacket::Capabilities(fallback));
        self.shared.send_control(&ControlPacket::KeyframeRequest(KeyframeRequestPacket {
            eye: XrRemoteEyeTarget::Both,
        }));
        self.h265_fallback_requested = true;
        self.latest_stream_text =
            "Stream: H265 stalled on Quest, requesting H264 fallback".to_string();
        self.send_remote_log(
            cx,
            "warn",
            "quest-client",
            "Dual-eye H265 produced no frames, requesting H264 fallback",
        );
    }

    fn sync_immersive_planes(&mut self, cx: &mut Cx, state: &XrState) {
        let Some(session) = self.active_session.as_ref() else {
            self.set_immersive_visible(cx, false);
            return;
        };
        let ready = self.latest_displayed_group > 0
            && self.eye_state(XrRemoteEye::Left).decoded_updates > 0
            && self.eye_state(XrRemoteEye::Right).decoded_updates > 0;
        if !ready {
            self.set_immersive_visible(cx, false);
            return;
        }

        let head_right = state.head_pose.orientation.rotate_vec3(&vec3f(1.0, 0.0, 0.0));
        let head_up = state.head_pose.orientation.rotate_vec3(&vec3f(0.0, 1.0, 0.0));
        let head_forward = state
            .head_pose
            .orientation
            .rotate_vec3(&vec3f(0.0, 0.0, -1.0))
            .normalize();
        let panel_orientation = Quat::look_rotation(head_forward.scale(-1.0), head_up);
        let half_ipd = head_right.scale(session.ipd_meters * 0.5);
        let distance = session.panel_distance_meters;
        let height_m = 2.0 * distance * (session.fov_y_degrees.to_radians() * 0.5).tan();
        let aspect = session.per_eye_width as f32 / session.per_eye_height.max(1) as f32;
        let width_m = height_m * aspect;
        let pixel_scale = 0.00052;
        let logical_size = dvec2(
            (width_m / pixel_scale.max(0.00001)) as f64,
            (height_m / pixel_scale.max(0.00001)) as f64,
        );
        let left_pose = Pose::new(
            panel_orientation,
            state.head_pose.position - half_ipd + head_forward.scale(distance),
        );
        let right_pose = Pose::new(
            panel_orientation,
            state.head_pose.position + half_ipd + head_forward.scale(distance),
        );
        self.set_eye_plane(cx, ids!(immersive_left), left_pose, logical_size, pixel_scale);
        self.set_eye_plane(cx, ids!(immersive_right), right_pose, logical_size, pixel_scale);
        self.set_immersive_visible(cx, true);
    }

    fn set_eye_plane(
        &mut self,
        cx: &mut Cx,
        id_path: &[LiveId],
        pose: Pose,
        logical_size: DVec2,
        pixel_scale: f32,
    ) {
        let widget = self.ui.widget(cx, id_path);
        if let Some(mut view) = widget.borrow_mut::<XrView>() {
            view.set_panel_metrics(cx, logical_size, pixel_scale, 1.0);
            view.set_world_pose_override(cx, Some(pose));
        };
    }

    fn set_immersive_visible(&mut self, cx: &mut Cx, visible: bool) {
        self.ui.widget(cx, ids!(immersive_left)).set_visible(cx, visible);
        self.ui.widget(cx, ids!(immersive_right)).set_visible(cx, visible);
        if !visible {
            let left = self.ui.widget(cx, ids!(immersive_left));
            if let Some(mut view) = left.borrow_mut::<XrView>() {
                view.set_world_pose_override(cx, None);
            };
            let right = self.ui.widget(cx, ids!(immersive_right));
            if let Some(mut view) = right.borrow_mut::<XrView>() {
                view.set_world_pose_override(cx, None);
            };
        }
    }

    fn send_state(&mut self, state: &XrState, session: &SessionConfigPacket) {
        let predicted_display_time_ns = (state.time * 1_000_000_000.0) as u64;
        if predicted_display_time_ns == self.last_sent_time_ns {
            return;
        }
        self.last_sent_time_ns = predicted_display_time_ns;
        self.tracking_counter = self.tracking_counter.wrapping_add(1);
        let tracking = make_tracking_packet(
            self.tracking_counter,
            predicted_display_time_ns,
            state.head_pose,
            session.ipd_meters,
            session.fov_y_degrees,
            session.per_eye_width,
            session.per_eye_height,
            state.anchor,
        );
        self.shared.send_control(&ControlPacket::Tracking(tracking));
        self.shared.send_control(&ControlPacket::InputState(InputStatePacket {
            version: 1,
            time_ns: predicted_display_time_ns,
            state: state.clone(),
        }));
    }

    fn eye_for_video_id(video_id: LiveId) -> Option<XrRemoteEye> {
        if video_id == Self::decoder_video_id(XrRemoteEye::Left) {
            Some(XrRemoteEye::Left)
        } else if video_id == Self::decoder_video_id(XrRemoteEye::Right) {
            Some(XrRemoteEye::Right)
        } else {
            None
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_xr::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if matches!(event, Event::Startup) {
            self.ensure_started(cx);
        }

        if let Event::TextureHandleReady(ev) = event {
            for eye in XrRemoteEye::ALL {
                if self
                    .eye_state(eye)
                    .decoder_texture
                    .as_ref()
                    .is_some_and(|texture| texture.texture_id() == ev.texture_id)
                {
                    let _ = self.try_start_eye_decoder(cx, eye);
                }
            }
        }

        if let Event::XrUpdate(update) = event {
            let session = self
                .active_session
                .clone()
                .unwrap_or_else(default_session_config);
            self.send_state(update.state.as_ref(), &session);
            self.sync_immersive_planes(cx, update.state.as_ref());
        }

        if let Event::VideoTextureUpdated(ev) = event {
            if let Some(eye) = Self::eye_for_video_id(ev.video_id) {
                let state = self.eye_state_mut(eye);
                state.decoded_updates = state.decoded_updates.wrapping_add(1);
                self.h265_watchdog_started_at = None;
                self.latest_stream_text = format!(
                    "Stream: group {} decoded L{} R{}",
                    self.latest_displayed_group,
                    self.eye_state(XrRemoteEye::Left).decoded_updates,
                    self.eye_state(XrRemoteEye::Right).decoded_updates
                );
                self.refresh_labels(cx);
            }
        }

        if let Event::VideoDecodingStatus(ev) = event {
            if let Some(eye) = Self::eye_for_video_id(ev.video_id) {
                self.latest_stream_text = format!("Stream: {} {}", eye.label(), ev.status);
                self.refresh_labels(cx);
            }
        }

        if let Event::VideoDecodingError(ev) = event {
            if let Some(eye) = Self::eye_for_video_id(ev.video_id) {
                let state = self.eye_state_mut(eye);
                state.decoder_errors = state.decoder_errors.wrapping_add(1);
                self.latest_stream_text = format!(
                    "Stream: {} decoder error {}: {}",
                    eye.label(),
                    state.decoder_errors,
                    ev.error
                );
                self.refresh_labels(cx);
            }
        }

        if let Event::Signal = event {
            for packet in self.shared.drain_control() {
                self.handle_control_packet(cx, packet);
            }
            for packet in self.shared.drain_media() {
                self.queue_media_packet(cx, packet);
            }
            self.prune_stale_media(cx);
            self.flush_decoders(cx);
            self.maybe_request_h264_fallback(cx);
            self.refresh_labels(cx);
        }

        if self.ping_timer.is_event(event).is_some() {
            self.maybe_request_h264_fallback(cx);
            self.shared
                .send_control(&ControlPacket::Ping(PingPacket {
                    timestamp_ns: (cx.seconds_since_app_start() * 1_000_000_000.0) as u64,
                }));
        }
    }
}
