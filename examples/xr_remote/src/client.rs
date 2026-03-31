use crate::{protocol::*, wire::*};
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
    sync::{mpsc::TryRecvError, Arc, Mutex},
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
            debug_mono: 0.0

            pixel: fn() {
                if self.debug_mono > 0.5 {
                    return self.video_texture.sample_video(self.pos)
                }
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
                logical_size: vec2(420, 320)
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

                    preview_row := SolidView{
                        width: Fill
                        height: 84
                        flow: Right
                        spacing: 8
                        draw_bg.color: #0000

                        debug_left_surface := mod.widgets.RemoteEyeSurface{
                            width: Fill
                            height: Fill
                            draw_bg.target_eye: 0.0
                            draw_bg.debug_mono: 1.0
                        }

                        debug_right_surface := mod.widgets.RemoteEyeSurface{
                            width: Fill
                            height: Fill
                            draw_bg.target_eye: 1.0
                            draw_bg.debug_mono: 1.0
                        }
                    }

                    decoder_field := Label{
                        text: "Decoder: waiting"
                        draw_text.color: #x8fa8bd
                    }

                    clock_field := Label{
                        text: "XR Net: waiting"
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

fn xr_remote_debug_mono_eye() -> Option<XrRemoteEye> {
    match std::env::var("XR_REMOTE_DEBUG_MONO").ok()?.to_ascii_lowercase().as_str() {
        "left" => Some(XrRemoteEye::Left),
        "right" => Some(XrRemoteEye::Right),
        _ => None,
    }
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

    pub fn set_debug_mono(&mut self, cx: &mut Cx, enabled: bool) {
        self.draw_bg
            .draw_vars
            .set_uniform(cx, id!(debug_mono), &[if enabled { 1.0 } else { 0.0 }]);
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
    ready_frames: VecDeque<CompletedEyeFrame>,
    decoder_texture: Option<Texture>,
    decoder_started: bool,
    configured_config_id: Option<u32>,
    seen_keyframe_config_id: Option<u32>,
    media_chunks_received: u64,
    configs_received: u64,
    completed_frames: u64,
    frames_queued: u64,
    decoded_updates: u64,
    decoder_errors: u64,
    latest_completed_group: u64,
    latest_status: String,
    last_error: String,
    status_prepared: bool,
    status_configured: bool,
    status_output_format: bool,
    status_first_frame_available: bool,
    status_update_ok: bool,
}

impl Default for ClientEyeState {
    fn default() -> Self {
        Self {
            active_stream: None,
            known_configs: BTreeMap::new(),
            incomplete_frames: BTreeMap::new(),
            ready_frames: VecDeque::new(),
            decoder_texture: None,
            decoder_started: false,
            configured_config_id: None,
            seen_keyframe_config_id: None,
            media_chunks_received: 0,
            configs_received: 0,
            completed_frames: 0,
            frames_queued: 0,
            decoded_updates: 0,
            decoder_errors: 0,
            latest_completed_group: 0,
            latest_status: "idle".to_string(),
            last_error: "-".to_string(),
            status_prepared: false,
            status_configured: false,
            status_output_format: false,
            status_first_frame_available: false,
            status_update_ok: false,
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
    xr_net: Option<XrNetNode>,
    #[rust]
    network_started: bool,
    #[rust]
    latest_connection_text: String,
    #[rust]
    latest_stream_text: String,
    #[rust]
    latest_decoder_text: String,
    #[rust]
    latest_clock_text: String,
    #[rust]
    last_sent_time_ns: u64,
    #[rust]
    active_session: Option<SessionConfigPacket>,
    #[rust]
    eye_states: [ClientEyeState; 2],
    #[rust]
    pending_stereo_groups: BTreeMap<u64, StereoGroupAssembly>,
    #[rust]
    ready_stereo_groups: VecDeque<(u64, CompletedEyeFrame, CompletedEyeFrame)>,
    #[rust]
    latest_displayed_group: u64,
    #[rust]
    debug_mono: Option<XrRemoteEye>,
    #[rust]
    startup_log_sent: bool,
}

impl Default for App {
    fn default() -> Self {
        let debug_mono = xr_remote_debug_mono_eye();
        Self {
            ui: WidgetRef::default(),
            shared: ClientShared::new(),
            xr_net: None,
            network_started: false,
            latest_connection_text: format!("Connecting to {}", remote_host()),
            latest_stream_text: "Stream: waiting".to_string(),
            latest_decoder_text: "Decoder: waiting".to_string(),
            latest_clock_text: "XR Net: waiting".to_string(),
            last_sent_time_ns: 0,
            active_session: None,
            eye_states: std::array::from_fn(|_| ClientEyeState::default()),
            pending_stereo_groups: BTreeMap::new(),
            ready_stereo_groups: VecDeque::new(),
            latest_displayed_group: 0,
            debug_mono,
            startup_log_sent: false,
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

    fn debug_surface_ref(&self, cx: &mut Cx, eye: XrRemoteEye) -> WidgetRef {
        match eye {
            XrRemoteEye::Left => self.ui.widget(cx, ids!(debug_left_surface)),
            XrRemoteEye::Right => self.ui.widget(cx, ids!(debug_right_surface)),
        }
    }

    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.network_started {
            return;
        }
        let mut capabilities = default_capabilities();
        capabilities.codecs = preferred_codecs_from_capabilities(&cx.video_capabilities(), false);
        self.shared.set_capabilities(capabilities);
        self.shared.start_threads();
        self.xr_net = match XrNetNode::new() {
            Ok(node) => {
                self.latest_clock_text = "XR Net: ready".to_string();
                Some(node)
            }
            Err(err) => {
                self.latest_clock_text = format!("XR Net unavailable: {err}");
                None
            }
        };
        self.network_started = true;
        let mode = self
            .debug_mono
            .map(|eye| format!(" mono={}", eye.label()))
            .unwrap_or_else(|| " stereo".to_string());
        if let Some(ports) = self.shared.media_ports() {
            self.latest_connection_text = format!(
                "Connecting to {} tcp:{} udp:{}|{}{}",
                remote_host(),
                control_port(),
                ports.left_port,
                ports.right_port,
                mode,
            );
        }
        self.refresh_labels(cx);
    }

    fn startup_mode_text(&self) -> String {
        format!(
            "startup mode={}",
            self.debug_mono
                .map(|eye| format!("mono-{}", eye.label()))
                .unwrap_or_else(|| "stereo".to_string()),
        )
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
        self.latest_decoder_text = format!(
            "Decoder: {}\n{}\n{}",
            self
                .debug_mono
                .map(|eye| format!("mono {}", eye.label()))
                .unwrap_or_else(|| "stereo".to_string()),
            self.eye_debug_summary(XrRemoteEye::Left),
            self.eye_debug_summary(XrRemoteEye::Right),
        );
        let clock_base = self
            .latest_clock_text
            .split(" | group ")
            .next()
            .unwrap_or(&self.latest_clock_text)
            .to_string();
        self.latest_clock_text = format!("{clock_base} | group {}", self.latest_displayed_group);
        self.ui
            .widget(cx, ids!(connection_field))
            .set_text(cx, &self.latest_connection_text);
        self.ui
            .widget(cx, ids!(stream_field))
            .set_text(cx, &self.latest_stream_text);
        self.ui
            .widget(cx, ids!(decoder_field))
            .set_text(cx, &self.latest_decoder_text);
        self.ui
            .widget(cx, ids!(clock_field))
            .set_text(cx, &self.latest_clock_text);
    }

    fn eye_debug_summary(&self, eye: XrRemoteEye) -> String {
        let state = self.eye_state(eye);
        let active_config = state
            .active_stream
            .as_ref()
            .map(|stream| stream.config_id.to_string())
            .unwrap_or_else(|| "-".to_string());
        let milestones = format!(
            "{}{}{}{}{}",
            if state.status_prepared { 'P' } else { '-' },
            if state.status_configured { 'C' } else { '-' },
            if state.status_output_format { 'O' } else { '-' },
            if state.status_first_frame_available { 'F' } else { '-' },
            if state.status_update_ok { 'T' } else { '-' },
        );
        format!(
            "{} cfg{} ch{} cfgs{} fr{} q{} upd{} {} err:{}",
            eye.label(),
            active_config,
            state.media_chunks_received,
            state.configs_received,
            state.completed_frames,
            state.frames_queued,
            state.decoded_updates,
            milestones,
            state.last_error
        )
    }

    fn decoder_ready_to_start(&self, eye: XrRemoteEye) -> bool {
        let state = self.eye_state(eye);
        let Some(stream) = state.active_stream.as_ref() else {
            return false;
        };
        state.decoder_texture.is_some()
            && state.known_configs.contains_key(&stream.config_id)
    }

    fn note_decoder_status(&mut self, cx: &Cx, eye: XrRemoteEye, status: &str) {
        let mut log_lines = Vec::new();
        {
            let state = self.eye_state_mut(eye);
            state.latest_status = status.to_string();
            if status.contains("decoder prepared") && !state.status_prepared {
                state.status_prepared = true;
                log_lines.push(format!("{} prepared", eye.label()));
            }
            if status.contains("configured") && !state.status_configured {
                state.status_configured = true;
                log_lines.push(format!("{} configured", eye.label()));
            }
            if status.contains("output format") && !state.status_output_format {
                state.status_output_format = true;
                log_lines.push(format!("{} {}", eye.label(), status));
            }
            if status.contains("first frame available") && !state.status_first_frame_available {
                state.status_first_frame_available = true;
                log_lines.push(format!("{} first frame available", eye.label()));
            }
            if status.contains("updateTexImage ok") && !state.status_update_ok {
                state.status_update_ok = true;
                log_lines.push(format!("{} updateTexImage ok", eye.label()));
            }
        }
        for line in log_lines {
            self.send_remote_log(cx, "info", "quest-client", line);
        }
    }

    fn apply_eye_texture_to_surface(
        &mut self,
        cx: &mut Cx,
        surface: WidgetRef,
        texture: Option<&Texture>,
        mono: bool,
    ) {
        {
            if let Some(mut inner) = surface.borrow_mut::<RemoteEyeSurface>() {
                inner.set_debug_mono(cx, mono);
                inner.set_video_texture(cx, texture);
                return;
            }
        }
        if let Some(mut inner) = surface.cast_inner_mut::<RemoteEyeSurface>() {
            inner.set_debug_mono(cx, mono);
            inner.set_video_texture(cx, texture);
        };
    }

    fn apply_surface_textures(&mut self, cx: &mut Cx) {
        if let Some(eye) = self.debug_mono {
            let texture = self.eye_state(eye).decoder_texture.clone();
            let immersive_left = self.surface_ref(cx, XrRemoteEye::Left);
            let immersive_right = self.surface_ref(cx, XrRemoteEye::Right);
            let debug_left = self.debug_surface_ref(cx, XrRemoteEye::Left);
            let debug_right = self.debug_surface_ref(cx, XrRemoteEye::Right);
            self.apply_eye_texture_to_surface(cx, immersive_left, texture.as_ref(), true);
            self.apply_eye_texture_to_surface(cx, immersive_right, texture.as_ref(), true);
            self.apply_eye_texture_to_surface(cx, debug_left, texture.as_ref(), true);
            self.apply_eye_texture_to_surface(cx, debug_right, texture.as_ref(), true);
            return;
        }
        let left = self.eye_state(XrRemoteEye::Left).decoder_texture.clone();
        let right = self.eye_state(XrRemoteEye::Right).decoder_texture.clone();
        let immersive_left = self.surface_ref(cx, XrRemoteEye::Left);
        let immersive_right = self.surface_ref(cx, XrRemoteEye::Right);
        let debug_left = self.debug_surface_ref(cx, XrRemoteEye::Left);
        let debug_right = self.debug_surface_ref(cx, XrRemoteEye::Right);
        self.apply_eye_texture_to_surface(cx, immersive_left, left.as_ref(), false);
        self.apply_eye_texture_to_surface(cx, immersive_right, right.as_ref(), false);
        self.apply_eye_texture_to_surface(cx, debug_left, left.as_ref(), true);
        self.apply_eye_texture_to_surface(cx, debug_right, right.as_ref(), true);
    }

    fn ensure_decoder_texture(&mut self, cx: &mut Cx, eye: XrRemoteEye) -> Option<Texture> {
        if let Some(texture) = self.eye_state(eye).decoder_texture.clone() {
            return Some(texture);
        }
        let texture = Texture::new_with_format(cx, remote_video_texture_format(cx));
        self.eye_state_mut(eye).decoder_texture = Some(texture.clone());
        self.apply_surface_textures(cx);
        Some(texture)
    }

    fn reset_eye_decoder(&mut self, cx: &mut Cx, eye: XrRemoteEye) {
        if self.eye_state(eye).decoder_started {
            cx.video_decoder_stop(Self::decoder_slot(eye));
        }
        let state = self.eye_state_mut(eye);
        state.decoder_started = false;
        state.configured_config_id = None;
        state.incomplete_frames.clear();
        state.ready_frames.clear();
        state.seen_keyframe_config_id = None;
        state.completed_frames = 0;
        state.frames_queued = 0;
        state.decoded_updates = 0;
        state.decoder_errors = 0;
        state.latest_completed_group = 0;
        state.latest_status = "decoder reset".to_string();
        state.last_error = "-".to_string();
        state.status_prepared = false;
        state.status_configured = false;
        state.status_output_format = false;
        state.status_first_frame_available = false;
        state.status_update_ok = false;
        self.set_immersive_visible(cx, false);
    }

    fn reset_stereo_sync(&mut self) {
        self.pending_stereo_groups.clear();
        self.ready_stereo_groups.clear();
        self.latest_displayed_group = 0;
    }

    fn try_start_eye_decoder(
        &mut self,
        cx: &mut Cx,
        eye: XrRemoteEye,
    ) -> Result<bool, VideoDecodeError> {
        if self.eye_state(eye).decoder_started {
            if cx.video_decoder_slot_live(Self::decoder_slot(eye)) {
                return Ok(true);
            }
            let state = self.eye_state_mut(eye);
            state.decoder_started = false;
            state.configured_config_id = None;
        }
        if !self.decoder_ready_to_start(eye) {
            return Ok(false);
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
        self.apply_surface_textures(cx);
        self.push_config_if_needed(cx, eye, stream_config.config_id)?;
        self.shared
            .send_control(&ControlPacket::KeyframeRequest(KeyframeRequestPacket {
                eye: match eye {
                    XrRemoteEye::Left => XrRemoteEyeTarget::Left,
                    XrRemoteEye::Right => XrRemoteEyeTarget::Right,
                },
            }));
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
                let previous = self.eye_state(eye).active_stream.clone();
                let changed = previous.as_ref() != Some(&config);
                let bootstrap_stream = previous
                    .as_ref()
                    .is_none_or(|old| old.config_id == 0)
                    && config.config_id != 0;
                if changed {
                    if !bootstrap_stream {
                        self.reset_eye_decoder(cx, eye);
                        self.eye_state_mut(eye).known_configs.clear();
                    }
                    self.eye_state_mut(eye).active_stream = Some(config.clone());
                    if !bootstrap_stream {
                        self.reset_stereo_sync();
                    }
                    self.ensure_decoder_texture(cx, eye);
                    let _ = self.try_start_eye_decoder(cx, eye);
                } else {
                    self.eye_state_mut(eye).active_stream = Some(config);
                }
            }
            ControlPacket::VideoConfig(config) => {
                let eye = config.eye;
                let state = self.eye_state_mut(eye);
                state.configs_received = state.configs_received.wrapping_add(1);
                self.eye_state_mut(eye)
                    .known_configs
                    .insert(config.config_id, config);
                let _ = self.try_start_eye_decoder(cx, eye);
            }
            ControlPacket::Hello(_)
            | ControlPacket::ClientMediaChannels(_)
            | ControlPacket::KeyframeRequest(_)
            | ControlPacket::LogLine(_) => {}
        }
        self.refresh_labels(cx);
    }

    fn local_now_ns(&self, cx: &Cx) -> u64 {
        (cx.seconds_since_app_start() * 1_000_000_000.0) as u64
    }

    fn media_frame_is_fresh(&self, _cx: &Cx, _header: &MediaChunkHeader) -> bool {
        true
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
        {
            let state = self.eye_state_mut(eye);
            state.latest_completed_group = frame.header.frame_group_id;
            if frame.header.is_key {
                state.seen_keyframe_config_id = Some(frame.header.config_id);
            }
            if state.completed_frames == 1 {
                self.send_remote_log(
                    cx,
                    "info",
                    "quest-client",
                    format!(
                        "{} first complete frame cfg{} group {} bytes {}",
                        eye.label(),
                        frame.header.config_id,
                        frame.header.frame_group_id,
                        frame.bytes.len()
                    ),
                );
            }
        }
        if let Some(selected_eye) = self.debug_mono {
            if eye != selected_eye {
                return;
            }
            let state = self.eye_state_mut(eye);
            state.ready_frames.push_back(frame);
            while state.ready_frames.len() > 8 {
                let _ = state.ready_frames.pop_front();
            }
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
        // Reset age on each eye so prune_stale_media does not drop the pair while waiting on the
        // other UDP path (left/right skew often exceeds the stale window).
        entry.received_at_ns = now_ns;
        if let (Some(left), Some(right)) = (entry.left.take(), entry.right.take()) {
            let frame_group_id = left.header.frame_group_id;
            self.ready_stereo_groups
                .push_back((frame_group_id, left, right));
            self.pending_stereo_groups.remove(&frame_group_id);
            while self.ready_stereo_groups.len() > 8 {
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
        // Host may advertise a short window; dual UDP paths still need time to complete a group.
        let pending_stereo_stale_after = stale_after.max(XR_REMOTE_FRAME_STALE_AFTER_NS);
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
            now.saturating_sub(group.received_at_ns) <= pending_stereo_stale_after
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
        if self.eye_state(eye).configured_config_id == Some(config_id) {
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
        self.eye_state_mut(eye).configured_config_id = Some(config_id);
        Ok(())
    }

    fn flush_mono_decoder(&mut self, cx: &mut Cx, eye: XrRemoteEye) {
        let ready_len = self.eye_state(eye).ready_frames.len();
        let Some(frame) = self.eye_state_mut(eye).ready_frames.pop_back() else {
            return;
        };
        let frame_config_id = frame.header.config_id;
        let frame_group_id = frame.header.frame_group_id;
        let frame_is_key = frame.header.is_key;
        if self.eye_state(eye).frames_queued == 0 {
            self.send_remote_log(
                cx,
                "info",
                "quest-client",
                format!(
                    "{} attempting first mono queue group {} cfg{}",
                    eye.label(),
                    frame_group_id,
                    frame_config_id
                ),
            );
        }
        if !self
            .eye_state(eye)
            .known_configs
            .contains_key(&frame_config_id)
        {
            self.eye_state_mut(eye).ready_frames.push_back(frame);
            self.latest_stream_text = format!(
                "Stream: {} waiting for cfg{} (ready {})",
                eye.label(),
                frame_config_id,
                ready_len
            );
            return;
        }
        if let Err(err) = self.push_config_if_needed(cx, eye, frame_config_id) {
            self.eye_state_mut(eye).ready_frames.push_back(frame);
            self.latest_stream_text = format!(
                "Stream: {} config push failed for cfg{}: {:?}",
                eye.label(),
                frame_config_id,
                err
            );
            return;
        }
        if cx
            .video_decoder_push_packet(
                Self::decoder_slot(eye),
                makepad_widgets::makepad_platform::video::VideoDecoderPacketRef {
                    pts_ns: frame.header.pts_ns,
                    dts_ns: None,
                    is_key: frame_is_key,
                    is_config: false,
                    config_id: frame_config_id,
                    data: &frame.bytes,
                },
            )
            .is_err()
        {
            self.latest_stream_text = format!(
                "Stream: {} frame push failed cfg{} group {}",
                eye.label(),
                frame_config_id,
                frame_group_id
            );
            self.send_remote_log(
                cx,
                "warn",
                "quest-client",
                format!(
                    "{} frame push failed cfg{} group {}",
                    eye.label(),
                    frame_config_id,
                    frame_group_id
                ),
            );
            self.eye_state_mut(eye).ready_frames.push_back(frame);
            return;
        }
        let (first_queued, first_queued_key) = {
            let state = self.eye_state_mut(eye);
            let first_queued = state.frames_queued == 0;
            let first_queued_key = first_queued && frame_is_key;
            state.frames_queued = state.frames_queued.wrapping_add(1);
            state.latest_status = format!("queued frame group {}", frame_group_id);
            (first_queued, first_queued_key)
        };
        if first_queued {
            self.send_remote_log(
                cx,
                "info",
                "quest-client",
                format!("{} first frame queued cfg{}", eye.label(), frame_config_id),
            );
        }
        if first_queued_key {
            self.send_remote_log(
                cx,
                "info",
                "quest-client",
                format!("{} first keyframe queued", eye.label()),
            );
        }
        self.latest_displayed_group = frame_group_id;
        self.latest_stream_text = format!(
            "Stream: mono {} group {} cfg{}",
            eye.label(),
            frame_group_id,
            frame_config_id
        );
    }

    fn flush_decoders(&mut self, cx: &mut Cx) {
        if let Some(eye) = self.debug_mono {
            match self.try_start_eye_decoder(cx, eye) {
                Ok(true) => {}
                Ok(false) => return,
                Err(err) => {
                    self.latest_stream_text =
                        format!("Stream: {} decoder start failed: {:?}", eye.label(), err);
                    return;
                }
            }
            self.flush_mono_decoder(cx, eye);
            return;
        }
        // Always try both eyes each tick. Returning on the first Ok(false) skipped the second
        // decoder when left was not ready yet (VideoConfig/texture ordering), so right never started.
        for eye in XrRemoteEye::ALL {
            match self.try_start_eye_decoder(cx, eye) {
                Ok(_) => {}
                Err(err) => {
                    self.latest_stream_text =
                        format!("Stream: {} decoder start failed: {:?}", eye.label(), err);
                    return;
                }
            }
        }
        if !XrRemoteEye::ALL
            .into_iter()
            .all(|eye| self.eye_state(eye).decoder_started)
        {
            return;
        }
        if self.eye_state(XrRemoteEye::Left).configured_config_id.is_some()
            && self.eye_state(XrRemoteEye::Right).configured_config_id.is_some()
            && self.ready_stereo_groups.len() > 1
        {
            let latest = self.ready_stereo_groups.pop_back();
            self.ready_stereo_groups.clear();
            if let Some(latest) = latest {
                self.ready_stereo_groups.push_back(latest);
            }
        }
        let ready_groups = self.ready_stereo_groups.len();
        let Some((frame_group_id, left, right)) = self.ready_stereo_groups.pop_back() else {
            return;
        };
        let left_config_id = left.header.config_id;
        let right_config_id = right.header.config_id;
        let left_is_key = left.header.is_key;
        let right_is_key = right.header.is_key;
        if self.eye_state(XrRemoteEye::Left).frames_queued == 0
            && self.eye_state(XrRemoteEye::Right).frames_queued == 0
        {
            self.send_remote_log(
                cx,
                "info",
                "quest-client",
                format!(
                    "attempting first stereo queue group {} cfg L{} R{}",
                    frame_group_id,
                    left_config_id,
                    right_config_id
                ),
            );
        }
        if !self
            .eye_state(XrRemoteEye::Left)
            .known_configs
            .contains_key(&left_config_id)
            || !self
                .eye_state(XrRemoteEye::Right)
                .known_configs
                .contains_key(&right_config_id)
        {
            self.ready_stereo_groups.push_back((frame_group_id, left, right));
            self.latest_stream_text = format!(
                "Stream: stereo waiting cfg L{} R{} (ready {})",
                left_config_id,
                right_config_id,
                ready_groups
            );
            return;
        }
        if let Err(err) = self.push_config_if_needed(cx, XrRemoteEye::Left, left_config_id) {
            self.ready_stereo_groups.push_back((frame_group_id, left, right));
            self.latest_stream_text =
                format!("Stream: left config push failed for cfg{}: {:?}", left_config_id, err);
            return;
        }
        if let Err(err) = self.push_config_if_needed(cx, XrRemoteEye::Right, right_config_id) {
            self.ready_stereo_groups.push_back((frame_group_id, left, right));
            self.latest_stream_text =
                format!("Stream: right config push failed for cfg{}: {:?}", right_config_id, err);
            return;
        }

        if cx
            .video_decoder_push_packet(
                Self::decoder_slot(XrRemoteEye::Left),
                makepad_widgets::makepad_platform::video::VideoDecoderPacketRef {
                    pts_ns: left.header.pts_ns,
                    dts_ns: None,
                    is_key: left_is_key,
                    is_config: false,
                    config_id: left_config_id,
                    data: &left.bytes,
                },
            )
            .is_err()
        {
            self.latest_stream_text = format!(
                "Stream: left frame push failed cfg{} group {}",
                left_config_id,
                frame_group_id
            );
            self.send_remote_log(
                cx,
                "warn",
                "quest-client",
                format!(
                    "left frame push failed cfg{} group {}",
                    left_config_id,
                    frame_group_id
                ),
            );
            self.ready_stereo_groups.push_front((frame_group_id, left, right));
            return;
        }
        let (left_first_queued, left_first_queued_key) = {
            let state = self.eye_state_mut(XrRemoteEye::Left);
            let first_queued = state.frames_queued == 0;
            let first_key = first_queued && left_is_key;
            state.frames_queued = state.frames_queued.wrapping_add(1);
            state.latest_status = format!("queued frame group {}", frame_group_id);
            (first_queued, first_key)
        };
        if cx
            .video_decoder_push_packet(
                Self::decoder_slot(XrRemoteEye::Right),
                makepad_widgets::makepad_platform::video::VideoDecoderPacketRef {
                    pts_ns: right.header.pts_ns,
                    dts_ns: None,
                    is_key: right_is_key,
                    is_config: false,
                    config_id: right_config_id,
                    data: &right.bytes,
                },
            )
            .is_err()
        {
            self.latest_stream_text = format!(
                "Stream: right frame push failed cfg{} group {}",
                right_config_id,
                frame_group_id
            );
            self.send_remote_log(
                cx,
                "warn",
                "quest-client",
                format!(
                    "right frame push failed cfg{} group {}",
                    right_config_id,
                    frame_group_id
                ),
            );
            self.ready_stereo_groups.push_front((frame_group_id, left, right));
            return;
        }
        let (right_first_queued, right_first_queued_key) = {
            let state = self.eye_state_mut(XrRemoteEye::Right);
            let first_queued = state.frames_queued == 0;
            let first_key = first_queued && right_is_key;
            state.frames_queued = state.frames_queued.wrapping_add(1);
            state.latest_status = format!("queued frame group {}", frame_group_id);
            (first_queued, first_key)
        };
        if left_first_queued {
            self.send_remote_log(
                cx,
                "info",
                "quest-client",
                format!("left first frame queued cfg{}", left_config_id),
            );
        }
        if right_first_queued {
            self.send_remote_log(
                cx,
                "info",
                "quest-client",
                format!("right first frame queued cfg{}", right_config_id),
            );
        }
        if left_first_queued_key {
            self.send_remote_log(cx, "info", "quest-client", "left first keyframe queued");
        }
        if right_first_queued_key {
            self.send_remote_log(cx, "info", "quest-client", "right first keyframe queued");
        }
        self.latest_displayed_group = frame_group_id;
        self.latest_stream_text = format!(
            "Stream: group {} queued L{} R{}",
            frame_group_id, left_config_id, right_config_id
        );
    }

    fn drain_xr_net(&mut self) {
        let mut latest_status = None;
        let mut disconnected = false;
        if let Some(xr_net) = self.xr_net.as_mut() {
            loop {
                match xr_net.incoming_receiver.try_recv() {
                    Ok(XrNetIncoming::Join { peer }) => {
                        latest_status = Some(format!("XR Net: connected {}", peer.addr));
                    }
                    Ok(XrNetIncoming::Leave { peer, .. }) => {
                        latest_status = Some(format!("XR Net: peer left {}", peer.addr));
                    }
                    Ok(XrNetIncoming::State { .. })
                    | Ok(XrNetIncoming::Alignment { .. })
                    | Ok(XrNetIncoming::AlignmentDescriptor { .. }) => {}
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if disconnected {
            self.latest_clock_text = "XR Net: disconnected".to_string();
        } else if let Some(status) = latest_status {
            self.latest_clock_text = status;
        }
    }

    fn send_state(&mut self, state: &XrState) {
        let predicted_display_time_ns = (state.time * 1_000_000_000.0) as u64;
        if predicted_display_time_ns == self.last_sent_time_ns {
            return;
        }
        self.last_sent_time_ns = predicted_display_time_ns;
        let Some(xr_net) = self.xr_net.as_mut() else {
            return;
        };
        xr_net.send_state(state.clone());
        if let Some(anchor) = state.anchor {
            xr_net.send_alignment(anchor, 1.0, state.time);
        }
    }

    fn sync_immersive_planes(&mut self, cx: &mut Cx, state: &XrState) {
        self.apply_surface_textures(cx);
        let Some(session) = self.active_session.clone() else {
            self.set_immersive_visible(cx, false);
            return;
        };
        if let Some(_eye) = self.debug_mono {
            let ready = self
                .debug_mono
                .is_some_and(|eye| self.eye_state(eye).decoded_updates > 0);
            if !ready {
                self.set_immersive_visible(cx, false);
                return;
            }
            let head_up = state.head_pose.orientation.rotate_vec3(&vec3f(0.0, 1.0, 0.0));
            let head_forward = state
                .head_pose
                .orientation
                .rotate_vec3(&vec3f(0.0, 0.0, -1.0))
                .normalize();
            let panel_orientation = Quat::look_rotation(head_forward.scale(-1.0), head_up);
            let distance = session.panel_distance_meters;
            let height_m = 2.0 * distance * (session.fov_y_degrees.to_radians() * 0.5).tan();
            let aspect = session.per_eye_width as f32 / session.per_eye_height.max(1) as f32;
            let width_m = height_m * aspect;
            let pixel_scale = 0.00052;
            let logical_size = dvec2(
                (width_m / pixel_scale.max(0.00001)) as f64,
                (height_m / pixel_scale.max(0.00001)) as f64,
            );
            let mono_pose = Pose::new(
                panel_orientation,
                state.head_pose.position + head_forward.scale(distance),
            );
            self.set_eye_plane(cx, ids!(immersive_left), mono_pose, logical_size, pixel_scale);
            self.ui.widget(cx, ids!(immersive_left)).set_visible(cx, true);
            self.ui.widget(cx, ids!(immersive_right)).set_visible(cx, false);
            return;
        }
        let left_ok = self.eye_state(XrRemoteEye::Left).decoded_updates > 0;
        let right_ok = self.eye_state(XrRemoteEye::Right).decoded_updates > 0;
        let has_queued_group = self.latest_displayed_group > 0;
        if !has_queued_group || (!left_ok && !right_ok) {
            self.set_immersive_visible(cx, false);
            return;
        }

        // One eye decoding: show a single head-locked panel with debug_mono sampling so both
        // physical eyes see video (per-eye VIEW_ID gating would hide the other panel).
        if !left_ok || !right_ok {
            let eye = if left_ok {
                XrRemoteEye::Left
            } else {
                XrRemoteEye::Right
            };
            let texture = self.eye_state(eye).decoder_texture.clone();
            let immersive_left = self.surface_ref(cx, XrRemoteEye::Left);
            let immersive_right = self.surface_ref(cx, XrRemoteEye::Right);
            self.apply_eye_texture_to_surface(cx, immersive_left, texture.as_ref(), true);
            self.apply_eye_texture_to_surface(cx, immersive_right, texture.as_ref(), true);
            let head_up = state.head_pose.orientation.rotate_vec3(&vec3f(0.0, 1.0, 0.0));
            let head_forward = state
                .head_pose
                .orientation
                .rotate_vec3(&vec3f(0.0, 0.0, -1.0))
                .normalize();
            let panel_orientation = Quat::look_rotation(head_forward.scale(-1.0), head_up);
            let distance = session.panel_distance_meters;
            let height_m = 2.0 * distance * (session.fov_y_degrees.to_radians() * 0.5).tan();
            let aspect = session.per_eye_width as f32 / session.per_eye_height.max(1) as f32;
            let width_m = height_m * aspect;
            let pixel_scale = 0.00052;
            let logical_size = dvec2(
                (width_m / pixel_scale.max(0.00001)) as f64,
                (height_m / pixel_scale.max(0.00001)) as f64,
            );
            let mono_pose = Pose::new(
                panel_orientation,
                state.head_pose.position + head_forward.scale(distance),
            );
            self.set_eye_plane(cx, ids!(immersive_left), mono_pose, logical_size, pixel_scale);
            self.ui.widget(cx, ids!(immersive_left)).set_visible(cx, true);
            self.ui.widget(cx, ids!(immersive_right)).set_visible(cx, false);
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
        self.drain_xr_net();

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
            self.send_state(update.state.as_ref());
            self.sync_immersive_planes(cx, update.state.as_ref());
        }

        if let Event::VideoTextureUpdated(ev) = event {
            if let Some(eye) = Self::eye_for_video_id(ev.video_id) {
                let first_update = !self.eye_state(eye).status_update_ok;
                let state = self.eye_state_mut(eye);
                state.decoded_updates = state.decoded_updates.wrapping_add(1);
                state.status_update_ok = true;
                state.latest_status = "updateTexImage ok".to_string();
                if first_update {
                    self.send_remote_log(
                        cx,
                        "info",
                        "quest-client",
                        format!("{} updateTexImage ok", eye.label()),
                    );
                }
                self.latest_stream_text = format!(
                    "Stream: group {} decoded L{} R{}",
                    self.latest_displayed_group,
                    self.eye_state(XrRemoteEye::Left).decoded_updates,
                    self.eye_state(XrRemoteEye::Right).decoded_updates
                );
                self.apply_surface_textures(cx);
                self.refresh_labels(cx);
            }
        }

        if let Event::VideoDecodingStatus(ev) = event {
            if let Some(eye) = Self::eye_for_video_id(ev.video_id) {
                self.note_decoder_status(cx, eye, &ev.status);
                self.latest_stream_text = format!("Stream: {} {}", eye.label(), ev.status);
                self.refresh_labels(cx);
            }
        }

        if let Event::VideoDecodingError(ev) = event {
            if let Some(eye) = Self::eye_for_video_id(ev.video_id) {
                let state = self.eye_state_mut(eye);
                state.decoder_errors = state.decoder_errors.wrapping_add(1);
                state.last_error = ev.error.clone();
                self.latest_stream_text = format!(
                    "Stream: {} decoder error {}: {}",
                    eye.label(),
                    state.decoder_errors,
                    ev.error
                );
                self.send_remote_log(
                    cx,
                    "error",
                    "quest-client",
                    format!("{} decoder error: {}", eye.label(), ev.error),
                );
                self.refresh_labels(cx);
            }
        }

        if let Event::Signal = event {
            for packet in self.shared.drain_control() {
                self.handle_control_packet(cx, packet);
            }
            if !self.startup_log_sent && self.active_session.is_some() {
                self.send_remote_log(cx, "info", "quest-client", self.startup_mode_text());
                self.startup_log_sent = true;
            }
            for packet in self.shared.drain_media() {
                self.queue_media_packet(cx, packet);
            }
            self.prune_stale_media(cx);
            self.flush_decoders(cx);
            self.refresh_labels(cx);
        }
    }
}
