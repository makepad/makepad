use crate::{protocol::*, wire::*};
use makepad_widgets::makepad_platform::{
    event::xr::XrState,
    makepad_live_id::LiveId,
    thread::SignalToUI,
    video::{VideoBitstreamFormat, VideoCodec, VideoDecodeOutput, VideoDecoderConfig},
};
use makepad_widgets::*;
use makepad_xr::*;
use std::{
    collections::VecDeque,
    net::TcpStream,
    sync::{Arc, Mutex},
    thread,
};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.RemoteVideoBase = #(RemoteVideo::register_widget(vm))
    mod.widgets.RemoteVideo = set_type_default() do mod.widgets.RemoteVideoBase{
        width: Fill
        height: Fill
        draw_bg +: {
            video_texture: texture_video()
            pixel: fn() {
                let color = self.video_texture.sample_video(self.pos)
                // Same convention as mod.widgets.Video: external/YCbCr may deliver premultiplied RGBA.
                return Pal.premul(vec4(color.xyz, color.w))
            }
        }
    }

    startup() do #(App::script_component(vm)){
        ui: XrRoot{
            window.inner_size: vec2(1600, 960)
            pass.clear_color: #0000
            camera.fov_y: 52.0
            camera.distance: 1.4
            env.env_cube: true
            env.depth_mesh: false

            debug_beacon := Cube{
                size: vec3(0.10, 0.10, 0.10)
                corner_radius: 0.02
                roughness: 0.16
                metallic: 0.02
                color: #xff5a45
                pos: vec3(0.0, 0.0, -0.55)
            }

            remote_panel := XrView{
                mode: mod.widgets.XrViewMode.World
                show_in_non_xr: true
                logical_size: vec2(1400, 920)
                pixel_scale: 0.00030
                dpi_factor: 2.0
                pos: vec3(0.0, 0.02, -0.58)

                remote_panel_body := SolidView{
                    width: Fill
                    height: Fill
                    flow: Down
                    padding: 18
                    spacing: 12
                    draw_bg.color: #x14263bee

                    Label{
                        text: "Quest MR Remote"
                        draw_text.color: #xf4fbff
                        draw_text.text_style.font_size: 18.0
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

                    video_shell := SolidView{
                        width: Fill
                        height: 160
                        draw_bg.color: #x081018ff
                        Label{
                            text: "Live video preview is pinned to the wrist HUD"
                            draw_text.color: #x8fa8bd
                        }
                    }
                }
            }

            debug_hud := XrView{
                mode: mod.widgets.XrViewMode.StuckToWrist
                show_in_non_xr: true
                wrist_left: true
                logical_size: vec2(320, 220)
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

                    hud_connection_field := Label{
                        text: "Connecting..."
                        draw_text.color: #xb7cad9
                    }

                    hud_stream_field := Label{
                        text: "Stream: waiting"
                        draw_text.color: #x8fa8bd
                    }

                    hud_clock_field := Label{
                        text: "Clock: waiting"
                        draw_text.color: #x8fa8bd
                    }

                    hud_video_shell := SolidView{
                        width: Fill
                        height: 110
                        draw_bg.color: #x081018ff
                        draw_bg.border_radius: 10.0
                        remote_video := mod.widgets.RemoteVideo{}
                    }
                }
            }

            xr_permissions := mod.widgets.XrPermissionsFlow{}
        }
    }
}

fn remote_video_texture_format(cx: &Cx) -> TextureFormat {
    match cx.os_type() {
        OsType::Android(_) => TextureFormat::VideoExternal,
        _ => TextureFormat::VideoExternal,
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct RemoteVideo {
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
    #[rust]
    video_texture: Option<Texture>,
    #[rust]
    decoder_started: bool,
}

impl RemoteVideo {
    fn decoder_video_id() -> LiveId {
        LiveId::from_str_num("android_realtime_video_decoder", XR_REMOTE_DECODER_SLOT as u64)
    }

    fn ensure_texture(&mut self, cx: &mut Cx) {
        if self.video_texture.is_some() {
            return;
        }
        let texture = Texture::new_with_format(cx, remote_video_texture_format(cx));
        self.draw_bg.draw_vars.set_texture(0, &texture);
        self.video_texture = Some(texture);
        self.redraw(cx);
    }

    fn try_start_decoder(&mut self, cx: &mut Cx) -> Result<bool, VideoDecodeError> {
        if self.decoder_started {
            return Ok(true);
        }
        let Some(texture) = self.video_texture.as_ref() else {
            return Ok(false);
        };
        let config = VideoDecoderConfig {
            codec: VideoCodec::H264,
            expected_format: VideoBitstreamFormat::AnnexB,
            output: VideoDecodeOutput::Texture {
                texture_id: texture.texture_id(),
            },
            width_hint: Some(XR_REMOTE_STREAM_WIDTH),
            height_hint: Some(XR_REMOTE_STREAM_HEIGHT),
            latency_realtime: true,
        };
        cx.video_decoder_start_box(XR_REMOTE_DECODER_SLOT, config, Box::new(|_| {}))?;
        self.decoder_started = true;
        self.redraw(cx);
        Ok(true)
    }

    pub fn kick_decoder(&mut self, cx: &mut Cx) -> Result<bool, VideoDecodeError> {
        self.ensure_texture(cx);
        self.try_start_decoder(cx)
    }
}

impl Widget for RemoteVideo {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_texture(cx);
        self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event {
            Event::TextureHandleReady(ev) => {
                if let Some(texture) = &self.video_texture {
                    if ev.texture_id == texture.texture_id() {
                        let _ = self.try_start_decoder(cx);
                    }
                }
            }
            Event::Signal => {
                if !self.decoder_started {
                    let _ = self.try_start_decoder(cx);
                } else {
                    self.redraw(cx);
                }
            }
            Event::VideoTextureUpdated(ev) => {
                if ev.video_id == Self::decoder_video_id() {
                    self.redraw(cx);
                }
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct ClientShared {
    control_writer: Arc<Mutex<Option<TcpStream>>>,
    control_inbox: Arc<Mutex<Vec<ControlPacket>>>,
    video_inbox: Arc<Mutex<Vec<VideoPacket>>>,
}

impl ClientShared {
    fn new() -> Self {
        Self {
            control_writer: Arc::new(Mutex::new(None)),
            control_inbox: Arc::new(Mutex::new(Vec::new())),
            video_inbox: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn start_threads(&self) {
        let host = remote_host();
        let control_addr = format!("{}:{}", host, control_port());
        let control_writer = self.control_writer.clone();
        let control_inbox = self.control_inbox.clone();
        thread::spawn(move || loop {
            let mut stream = connect_with_retry(&control_addr);
            if let Ok(writer) = stream.try_clone() {
                *control_writer.lock().unwrap() = Some(writer);
            }
            let _ = send_framed(
                &mut stream,
                &ControlPacket::Hello(HelloPacket {
                    role: "quest-client".to_string(),
                    protocol_version: 1,
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

        let host = remote_host();
        let video_addr = format!("{}:{}", host, video_port());
        let video_inbox = self.video_inbox.clone();
        thread::spawn(move || loop {
            let mut stream = connect_with_retry(&video_addr);
            while let Ok(packet) = recv_framed::<VideoPacket>(&mut stream) {
                video_inbox.lock().unwrap().push(packet);
                SignalToUI::set_ui_signal();
            }
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

    fn drain_control(&self) -> Vec<ControlPacket> {
        let mut inbox = self.control_inbox.lock().unwrap();
        std::mem::take(&mut *inbox)
    }

    fn drain_video(&self) -> Vec<VideoPacket> {
        let mut inbox = self.video_inbox.lock().unwrap();
        std::mem::take(&mut *inbox)
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
    pending_video_packets: VecDeque<VideoPacket>,
    #[rust]
    latest_connection_text: String,
    #[rust]
    latest_stream_text: String,
    #[rust]
    latest_clock_text: String,
    #[rust]
    video_packets_received: u64,
    #[rust]
    video_frames_received: u64,
    #[rust]
    video_configs_received: u64,
    #[rust]
    decoder_error_count: u64,
    #[rust]
    decoder_status_seen: bool,
    #[rust]
    last_sent_time_ns: u64,
    #[rust]
    active_stream: Option<StreamConfigPacket>,
    #[rust]
    decoded_frame_updates: u64,
    #[rust]
    world_panel_placed: bool,
    #[rust]
    logged_stream_config_rx: bool,
    #[rust]
    logged_video_config_rx: bool,
    #[rust]
    logged_video_frame_rx: bool,
    #[rust]
    logged_video_config_push: bool,
    #[rust]
    logged_video_frame_push: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            ui: WidgetRef::default(),
            shared: ClientShared::new(),
            network_started: false,
            ping_timer: Timer::default(),
            pending_video_packets: VecDeque::new(),
            latest_connection_text: format!("Connecting to {}", remote_host()),
            latest_stream_text: "Stream: waiting".to_string(),
            latest_clock_text: "Clock: waiting".to_string(),
            video_packets_received: 0,
            video_frames_received: 0,
            video_configs_received: 0,
            decoder_error_count: 0,
            decoder_status_seen: false,
            last_sent_time_ns: 0,
            active_stream: None,
            decoded_frame_updates: 0,
            world_panel_placed: false,
            logged_stream_config_rx: false,
            logged_video_config_rx: false,
            logged_video_frame_rx: false,
            logged_video_config_push: false,
            logged_video_frame_push: false,
        }
    }
}

impl App {
    fn remote_video_widget(&self, cx: &mut Cx) -> WidgetRef {
        let direct = self.ui.widget(cx, ids!(remote_video));
        if !direct.is_empty() {
            return direct;
        }
        let named_body_path = self
            .ui
            .widget(cx, ids!(debug_hud.debug_hud_body.hud_video_shell.remote_video));
        if !named_body_path.is_empty() {
            return named_body_path;
        }
        let hud_path = self.ui.widget(cx, ids!(debug_hud.hud_video_shell.remote_video));
        if !hud_path.is_empty() {
            return hud_path;
        }
        self.ui.widget(cx, ids!(hud_video_shell.remote_video))
    }

    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.network_started {
            return;
        }
        self.shared.start_threads();
        self.ping_timer = cx.start_interval(1.0);
        self.network_started = true;
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
            .widget(cx, ids!(hud_connection_field))
            .set_text(cx, &self.latest_connection_text);
        self.ui
            .widget(cx, ids!(stream_field))
            .set_text(cx, &self.latest_stream_text);
        self.ui
            .widget(cx, ids!(hud_stream_field))
            .set_text(cx, &self.latest_stream_text);
        self.ui
            .widget(cx, ids!(clock_field))
            .set_text(cx, &self.latest_clock_text);
        self.ui
            .widget(cx, ids!(hud_clock_field))
            .set_text(cx, &self.latest_clock_text);
    }

    fn ensure_remote_video_decoder(&mut self, cx: &mut Cx) -> Result<bool, VideoDecodeError> {
        let remote_video = self.remote_video_widget(cx);
        if remote_video.is_empty() {
            crate::warning!("xr_remote client: remote video widget unavailable");
            self.latest_stream_text = "Stream: remote video widget unavailable".to_string();
            return Ok(false);
        }
        if let Some(mut video) = remote_video.borrow_mut::<RemoteVideo>() {
            return video.kick_decoder(cx);
        }
        if let Some(mut video) = remote_video.cast_inner_mut::<RemoteVideo>() {
            return video.kick_decoder(cx);
        }
        crate::warning!("xr_remote client: remote video widget cast failed");
        self.latest_stream_text = "Stream: remote video widget cast failed".to_string();
        Ok(false)
    }

    fn handle_control_packet(&mut self, cx: &mut Cx, packet: ControlPacket) {
        match packet {
            ControlPacket::Capabilities(cap) => {
                self.latest_connection_text = format!(
                    "Connected: {} {}x{} @ {} fps",
                    cap.codec, cap.width, cap.height, cap.fps
                );
            }
            ControlPacket::ClockSync(sync) => {
                self.latest_clock_text = format!(
                    "Clock offset approx {} ms",
                    ((sync.server_time_ns as i64 - sync.client_time_ns as i64) as f64 / 1_000_000.0)
                        .round()
                );
            }
            ControlPacket::Ping(_)
            | ControlPacket::Hello(_)
            | ControlPacket::HeadPose(_)
            | ControlPacket::InputState(_)
            | ControlPacket::LogLine(_) => {}
        }
        self.refresh_labels(cx);
    }

    fn queue_video_packet(&mut self, packet: VideoPacket) {
        self.video_packets_received = self.video_packets_received.wrapping_add(1);
        match &packet {
            VideoPacket::StreamConfig(config) => {
                if !self.logged_stream_config_rx {
                    self.logged_stream_config_rx = true;
                    crate::log!(
                        "xr_remote client: received stream config {}x{} @ {} fps codec={} config_id={}",
                        config.width,
                        config.height,
                        config.fps,
                        config.codec,
                        config.config_id
                    );
                }
                if let Some(active) = &self.active_stream {
                    if active.width != config.width || active.height != config.height {
                        self.latest_stream_text =
                            "Stream: renegotiation rejected in prototype".to_string();
                        return;
                    }
                }
                self.active_stream = Some(config.clone());
                self.latest_stream_text = format!(
                    "Stream: cfg {}x{} @ {} fps, rx {}",
                    config.width, config.height, config.fps, self.video_packets_received
                );
            }
            VideoPacket::VideoConfig(_) => {
                if !self.logged_video_config_rx {
                    self.logged_video_config_rx = true;
                    crate::log!("xr_remote client: received video config packet");
                }
                self.video_configs_received = self.video_configs_received.wrapping_add(1);
                if !self.decoder_status_seen {
                    self.latest_stream_text = format!(
                        "Stream: config {} queued {}",
                        self.video_configs_received,
                        self.pending_video_packets.len() + 1
                    );
                }
                self.pending_video_packets.push_back(packet);
            }
            VideoPacket::VideoFrame(_) => {
                if !self.logged_video_frame_rx {
                    self.logged_video_frame_rx = true;
                    crate::log!("xr_remote client: received first video frame packet");
                }
                self.video_frames_received = self.video_frames_received.wrapping_add(1);
                if !self.decoder_status_seen {
                    self.latest_stream_text = format!(
                        "Stream: frame {} queued {}",
                        self.video_frames_received,
                        self.pending_video_packets.len() + 1
                    );
                }
                self.pending_video_packets.push_back(packet);
                while self.pending_video_packets.len() > 24 {
                    let keep = matches!(
                        self.pending_video_packets.front(),
                        Some(VideoPacket::VideoConfig(_))
                    );
                    if keep {
                        break;
                    }
                    let _ = self.pending_video_packets.pop_front();
                }
            }
        }
    }

    fn flush_decoder(&mut self, cx: &mut Cx) {
        match self.ensure_remote_video_decoder(cx) {
            Ok(true) => {}
            Ok(false) => {
                if !self.pending_video_packets.is_empty()
                    && !self.decoder_status_seen
                    && !self.latest_stream_text.contains("widget")
                {
                    self.latest_stream_text = format!(
                        "Stream: waiting for decoder start, queued {}",
                        self.pending_video_packets.len()
                    );
                }
                return;
            }
            Err(err) => {
                self.latest_stream_text = format!("Stream: decoder start failed: {:?}", err);
                return;
            }
        }
        let mut remaining = VecDeque::new();
        while let Some(packet) = self.pending_video_packets.pop_front() {
            match packet {
                VideoPacket::VideoConfig(config) => {
                    let result = cx.video_decoder_push_packet(
                        XR_REMOTE_DECODER_SLOT,
                        makepad_widgets::makepad_platform::video::VideoDecoderPacketRef {
                            pts_ns: 0,
                            dts_ns: None,
                            is_key: false,
                            is_config: true,
                            config_id: config.config_id,
                            data: &config.bytes,
                        },
                    );
                    if result.is_err() {
                        crate::warning!(
                            "xr_remote client: video config push failed config_id={} bytes={} err={:?}",
                            config.config_id,
                            config.bytes.len(),
                            result.err()
                        );
                        remaining.push_back(VideoPacket::VideoConfig(config));
                        break;
                    }
                    if !self.logged_video_config_push {
                        self.logged_video_config_push = true;
                        crate::log!(
                            "xr_remote client: pushed video config config_id={} bytes={}",
                            config.config_id,
                            config.bytes.len()
                        );
                    }
                }
                VideoPacket::VideoFrame(frame) => {
                    let result = cx.video_decoder_push_packet(
                        XR_REMOTE_DECODER_SLOT,
                        makepad_widgets::makepad_platform::video::VideoDecoderPacketRef {
                            pts_ns: frame.pts_ns,
                            dts_ns: None,
                            is_key: frame.is_key,
                            is_config: false,
                            config_id: frame.config_id,
                            data: &frame.bytes,
                        },
                    );
                    if result.is_err() {
                        crate::warning!(
                            "xr_remote client: video frame push failed pts_ns={} bytes={} err={:?}",
                            frame.pts_ns,
                            frame.bytes.len(),
                            result.err()
                        );
                        remaining.push_back(VideoPacket::VideoFrame(frame));
                        break;
                    }
                    if !self.logged_video_frame_push {
                        self.logged_video_frame_push = true;
                        crate::log!(
                            "xr_remote client: pushed first video frame pts_ns={} bytes={} key={}",
                            frame.pts_ns,
                            frame.bytes.len(),
                            frame.is_key
                        );
                    }
                }
                VideoPacket::StreamConfig(config) => {
                    remaining.push_back(VideoPacket::StreamConfig(config));
                }
            }
        }
        while let Some(packet) = remaining.pop_back() {
            self.pending_video_packets.push_front(packet);
        }
    }

    fn send_state(&mut self, packet: &XrState) {
        let time_ns = (packet.time * 1_000_000_000.0) as u64;
        if time_ns == self.last_sent_time_ns {
            return;
        }
        self.last_sent_time_ns = time_ns;
        self.shared.send_control(&ControlPacket::HeadPose(HeadPosePacket {
            time_ns,
            pose: packet.head_pose,
        }));
        self.shared.send_control(&ControlPacket::InputState(InputStatePacket {
            time_ns,
            state: packet.clone(),
        }));
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
        if let Event::XrUpdate(update) = event {
            if !self.world_panel_placed {
                let remote_panel = self.ui.widget(cx, ids!(remote_panel));
                cx.with_vm(|vm| {
                    let _ = remote_panel.script_call(vm, live_id!(show_in_front_of_face), NIL);
                });
                self.world_panel_placed = true;
                self.latest_connection_text =
                    format!("XR active, connecting to {}", remote_host());
                self.refresh_labels(cx);
            }
            self.send_state(update.state.as_ref());
        }
        if let Event::VideoTextureUpdated(ev) = event {
            if ev.video_id == RemoteVideo::decoder_video_id() {
                self.decoder_status_seen = true;
                self.decoded_frame_updates = self.decoded_frame_updates.wrapping_add(1);
                self.latest_stream_text = format!(
                    "Stream: decoded {} frames",
                    self.decoded_frame_updates
                );
                if self.decoded_frame_updates == 1 {
                    self.send_remote_log(
                        cx,
                        "info",
                        "quest-client",
                        "first VideoTextureUpdated received",
                    );
                }
                self.refresh_labels(cx);
            }
        }
        if let Event::VideoDecodingStatus(ev) = event {
            if ev.video_id == RemoteVideo::decoder_video_id() {
                self.decoder_status_seen = true;
                self.latest_stream_text = format!("Stream: {}", ev.status);
                self.send_remote_log(
                    cx,
                    "info",
                    "quest-decoder",
                    format!("status: {}", ev.status),
                );
                self.refresh_labels(cx);
            }
        }
        if let Event::VideoDecodingError(ev) = event {
            if ev.video_id == RemoteVideo::decoder_video_id() {
                self.decoder_status_seen = true;
                self.decoder_error_count = self.decoder_error_count.wrapping_add(1);
                self.latest_stream_text = format!(
                    "Stream: decoder error {}: {}",
                    self.decoder_error_count, ev.error
                );
                self.send_remote_log(
                    cx,
                    "error",
                    "quest-decoder",
                    format!("error {}: {}", self.decoder_error_count, ev.error),
                );
                self.refresh_labels(cx);
            }
        }
        if let Event::Signal = event {
            for packet in self.shared.drain_control() {
                self.handle_control_packet(cx, packet);
            }
            for packet in self.shared.drain_video() {
                self.queue_video_packet(packet);
            }
            self.flush_decoder(cx);
            self.refresh_labels(cx);
        }
        if self.ping_timer.is_event(event).is_some() {
            self.shared
                .send_control(&ControlPacket::Ping(PingPacket {
                    timestamp_ns: (cx.seconds_since_app_start() * 1_000_000_000.0) as u64,
                }));
        }
    }
}
