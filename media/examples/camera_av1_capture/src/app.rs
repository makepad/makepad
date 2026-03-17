use makepad_media::{
    mux::{build_av1_mp4, build_h264_mp4},
    EncodedVideoPacketOwned as MediaEncodedVideoPacketOwned,
    VideoBitstreamFormat as MediaVideoBitstreamFormat, VideoCodec as MediaVideoCodec,
};
use makepad_widgets::makepad_platform::{
    permission::{Permission, PermissionStatus},
    video::{
        EncodedVideoPacketOwned, VideoCodec, VideoEncodeSource, VideoEncoderConfig, VideoFormat,
        VideoInputId, VideoInputsEvent, VideoPixelFormat,
    },
};
use makepad_widgets::*;
use std::{
    rc::Rc,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SmokePhase {
    #[default]
    Idle,
    Capturing,
    Stopping,
    Playing,
    Done,
}

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)) {
        ui: Root{
            main_window := Window{
                body +: {
                    View{
                        width: Fill,
                        height: Fill,
                        flow: Down,
                        spacing: 10,
                        padding: 16,

                        Label{
                            text: "Camera AV1 / MP4 Capture Demo"
                            draw_text.text_style.font_size: 18
                        }

                        controls := View{
                            width: Fill,
                            height: Fit,
                            flow: Down,
                            spacing: 8,

                            controls_row_1 := View{
                                width: Fill,
                                height: Fit,
                                flow: Right,
                                spacing: 8,

                                mode_av1_btn := Button { width: Fill text: "Mode: AV1" }
                                mode_h264_btn := Button { width: Fill text: "Mode: H.264" }
                                start_capture_btn := Button { width: Fill text: "Start capture" }
                            }

                            controls_row_2 := View{
                                width: Fill,
                                height: Fit,
                                flow: Right,
                                spacing: 8,

                                stop_capture_btn := Button { width: Fill text: "Stop capture" }
                                playback_btn := Button { width: Fill text: "Playback capture" }
                            }
                        }

                        status_label := Label{
                            text: "Idle"
                            draw_text.color: #888
                        }

                        Label{ text: "Camera preview" }
                        camera_preview_video := Video{
                            width: Fill
                            height: 220
                            autoplay: false
                            show_controls: false
                        }

                        playback_host := View{
                            width: Fill,
                            height: Fill,
                            flow: Down,
                            spacing: 6,
                            visible: false,

                            Label{ text: "Captured playback" }
                            playback_video := Video{
                                width: Fill
                                height: Fill
                                autoplay: false
                                show_controls: true
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Default)]
struct CaptureBuffer {
    capturing: bool,
    packets: Vec<EncodedVideoPacketOwned>,
}

#[derive(Clone)]
struct CameraChoice {
    input_id: VideoInputId,
    format: VideoFormat,
    name: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureMode {
    Av1,
    H264,
}

impl CaptureMode {
    fn label(self) -> &'static str {
        match self {
            CaptureMode::Av1 => "AV1",
            CaptureMode::H264 => "H.264",
        }
    }
}

impl Default for CaptureMode {
    fn default() -> Self {
        Self::H264
    }
}

fn map_codec(codec: VideoCodec) -> MediaVideoCodec {
    match codec {
        VideoCodec::H264 => MediaVideoCodec::H264,
        VideoCodec::H265 => MediaVideoCodec::H265,
        VideoCodec::Av1 => MediaVideoCodec::Av1,
        VideoCodec::Vp8 => MediaVideoCodec::Vp8,
        VideoCodec::Vp9 => MediaVideoCodec::Vp9,
    }
}

fn map_format(format: makepad_widgets::makepad_platform::video::VideoBitstreamFormat) -> MediaVideoBitstreamFormat {
    match format {
        makepad_widgets::makepad_platform::video::VideoBitstreamFormat::AnnexB => {
            MediaVideoBitstreamFormat::AnnexB
        }
        makepad_widgets::makepad_platform::video::VideoBitstreamFormat::Avcc => {
            MediaVideoBitstreamFormat::Avcc
        }
        makepad_widgets::makepad_platform::video::VideoBitstreamFormat::Av1Obu => {
            MediaVideoBitstreamFormat::Av1Obu
        }
        makepad_widgets::makepad_platform::video::VideoBitstreamFormat::RawAccessUnit => {
            MediaVideoBitstreamFormat::RawAccessUnit
        }
    }
}

fn map_packets(packets: &[EncodedVideoPacketOwned]) -> Vec<MediaEncodedVideoPacketOwned> {
    packets
        .iter()
        .map(|pkt| MediaEncodedVideoPacketOwned {
            codec: map_codec(pkt.codec),
            format: map_format(pkt.format),
            pts_ns: pkt.pts_ns,
            dts_ns: pkt.dts_ns,
            is_key: pkt.is_key,
            is_config: pkt.is_config,
            is_eos: pkt.is_eos,
            config_id: pkt.config_id,
            data: pkt.data.clone(),
        })
        .collect()
}

fn h264_packets_starting_at_first_key(
    packets: &[EncodedVideoPacketOwned],
) -> Option<Vec<EncodedVideoPacketOwned>> {
    let first_key_index = packets
        .iter()
        .position(|pkt| !pkt.is_config && !pkt.is_eos && pkt.is_key)?;

    let key_config_id = packets[first_key_index].config_id;

    let mut out = Vec::new();
    for pkt in packets {
        if pkt.is_config && pkt.config_id == key_config_id {
            out.push(pkt.clone());
        }
    }
    for pkt in packets.iter().skip(first_key_index) {
        if pkt.is_eos {
            continue;
        }
        if !pkt.is_config {
            out.push(pkt.clone());
        }
    }

    Some(out)
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    camera_permission: Option<PermissionStatus>,
    #[rust]
    camera_choice: Option<CameraChoice>,
    #[rust]
    capture_buffer: Arc<Mutex<CaptureBuffer>>,
    #[rust]
    last_capture_data: Option<Rc<Vec<u8>>>,
    #[rust]
    pending_playback: bool,
    #[rust]
    selected_mode: CaptureMode,
    #[rust]
    active_capture_mode: Option<CaptureMode>,
    #[rust]
    last_capture_label: String,
    #[rust]
    auto_test_enabled: bool,
    #[rust]
    auto_test_phase: SmokePhase,
    #[rust]
    auto_test_next_frame: NextFrame,
    #[rust]
    auto_test_phase_started_at: f64,
}

impl App {
    fn set_status(&self, cx: &mut Cx, status: &str) {
        self.ui.label(cx, ids!(status_label)).set_text(cx, status);
        if self.auto_test_enabled {
            eprintln!("camera_av1_capture auto-test: status: {status}");
        }
    }

    fn set_mode(&mut self, cx: &mut Cx, mode: CaptureMode) {
        self.selected_mode = mode;
        self.set_status(cx, &format!("Capture mode: {}", mode.label()));
    }

    fn ensure_camera_preview(&mut self, cx: &mut Cx) {
        if self.camera_permission != Some(PermissionStatus::Granted) {
            return;
        }

        let Some(choice) = self.camera_choice.clone() else {
            return;
        };

        let preview = self.ui.video(cx, ids!(camera_preview_video));
        if !preview.is_unprepared() {
            return;
        }

        preview.set_camera_preview_mode(cx, VideoCameraPreviewMode::Texture);
        preview.set_source_camera(cx, choice.input_id, choice.format.format_id);
        preview.begin_playback(cx);
    }

    fn pick_camera_choice(ev: &VideoInputsEvent) -> Option<CameraChoice> {
        let desc = ev.descs.first()?;

        fn supported(fmt: &VideoFormat) -> bool {
            matches!(
                fmt.pixel_format,
                VideoPixelFormat::NV12 | VideoPixelFormat::YUY2 | VideoPixelFormat::YUV420
            )
        }

        fn pix_rank(fmt: VideoPixelFormat) -> usize {
            match fmt {
                VideoPixelFormat::NV12 => 3,
                VideoPixelFormat::YUY2 => 2,
                VideoPixelFormat::YUV420 => 1,
                _ => 0,
            }
        }

        let is_720p_class = |w: usize, h: usize| {
            let long = w.max(h);
            let short = w.min(h);
            long <= 1280 && short <= 720
        };

        let exact_720p = desc
            .formats
            .iter()
            .filter(|f| {
                supported(f)
                    && ((f.width == 1280 && f.height == 720)
                        || (f.width == 720 && f.height == 1280))
            })
            .max_by_key(|f| (pix_rank(f.pixel_format), f.frame_rate.unwrap_or(0.0) as i32))
            .copied();

        let capped = desc
            .formats
            .iter()
            .filter(|f| supported(f) && is_720p_class(f.width, f.height))
            .max_by_key(|f| {
                (
                    pix_rank(f.pixel_format),
                    f.width * f.height,
                    f.frame_rate.unwrap_or(0.0) as i32,
                )
            })
            .copied();

        let fallback = desc
            .formats
            .iter()
            .filter(|f| supported(f))
            .max_by_key(|f| {
                (
                    pix_rank(f.pixel_format),
                    f.width * f.height,
                    f.frame_rate.unwrap_or(0.0) as i32,
                )
            })
            .copied();

        let best = exact_720p.or(capped).or(fallback)?;

        Some(CameraChoice {
            input_id: desc.input_id,
            format: best,
            name: desc.name.clone(),
        })
    }

    fn begin_capture(&mut self, cx: &mut Cx) {
        if self.capture_buffer.lock().unwrap().capturing {
            self.set_status(cx, "Capture is already running");
            return;
        }

        if self.camera_permission != Some(PermissionStatus::Granted) {
            self.set_status(cx, "Camera permission not granted");
            return;
        }
        let Some(choice) = self.camera_choice.clone() else {
            self.set_status(cx, "No camera/format available yet");
            return;
        };

        let mode = self.selected_mode;

        #[cfg(target_os = "android")]
        if mode == CaptureMode::Av1 {
            self.set_status(cx, "AV1 capture is disabled on Android; use MP4 mode");
            return;
        }

        self.active_capture_mode = Some(mode);

        let mut capture = self.capture_buffer.lock().unwrap();
        capture.capturing = true;
        capture.packets.clear();
        drop(capture);

        let shared = self.capture_buffer.clone();
        let fps_num = choice
            .format
            .frame_rate
            .unwrap_or(30.0)
            .round()
            .clamp(1.0, 240.0) as u32;

        let codec = match mode {
            CaptureMode::Av1 => VideoCodec::Av1,
            CaptureMode::H264 => VideoCodec::H264,
        };

        let cfg = VideoEncoderConfig {
            codec,
            source: VideoEncodeSource::Camera {
                input_id: choice.input_id,
                format_id: choice.format.format_id,
            },
            width: choice.format.width as u32,
            height: choice.format.height as u32,
            fps_num,
            fps_den: 1,
            keyint: 30,
            target_bitrate: 2_000_000,
            ..Default::default()
        };

        let start_result = cx.video_encoder_output_try(0, cfg, move |packet| {
            let mut capture = shared.lock().unwrap();
            if !capture.capturing {
                return;
            }
            capture.packets.push(EncodedVideoPacketOwned {
                codec: packet.codec,
                format: packet.format,
                pts_ns: packet.pts_ns,
                dts_ns: packet.dts_ns,
                is_key: packet.is_key,
                is_config: packet.is_config,
                is_eos: packet.is_eos,
                config_id: packet.config_id,
                data: packet.data.to_vec(),
            });
        });

        if let Err(err) = start_result {
            error!("{} capture start failed: {:?}", mode.label(), err);
            let msg = match err {
                makepad_widgets::makepad_platform::video::VideoEncodeError::CodecUnavailable => {
                    format!("{} encoder unavailable on this target", mode.label())
                }
                makepad_widgets::makepad_platform::video::VideoEncodeError::UnsupportedCodec => {
                    format!("{} encoder unsupported on this target", mode.label())
                }
                _ => format!("{} capture start failed", mode.label()),
            };
            self.set_status(cx, &msg);
            return;
        }

        cx.use_video_input(&[(choice.input_id, choice.format.format_id)]);

        self.ui.view(cx, ids!(playback_host)).set_visible(cx, false);
        let playback_video = self.ui.video(cx, ids!(playback_video));
        if !playback_video.is_unprepared() {
            playback_video.stop_and_cleanup_resources(cx);
        }
        self.pending_playback = false;

        if mode == CaptureMode::H264 {
            let _ = cx.video_encoder_request_keyframe(0);
        }

        self.set_status(
            cx,
            &format!(
                "Capturing {} from {} ({}x{} {:?})",
                mode.label(),
                choice.name,
                choice.format.width,
                choice.format.height,
                choice.format.pixel_format
            ),
        );
    }

    fn stop_capture(&mut self, cx: &mut Cx) {
        let Some(choice) = self.camera_choice.clone() else {
            self.set_status(cx, "Capture stopped: no camera choice");
            return;
        };

        let mode = self.active_capture_mode.unwrap_or(self.selected_mode);
        self.active_capture_mode = None;

        let packets = {
            let mut capture = self.capture_buffer.lock().unwrap();
            if !capture.capturing {
                self.set_status(cx, "Capture is not running");
                return;
            }
            capture.capturing = false;
            std::mem::take(&mut capture.packets)
        };

        if packets.is_empty() {
            self.set_status(cx, &format!("Capture stopped: no {} packets captured", mode.label()));
            self.ensure_camera_preview(cx);
            return;
        }

        let packets = match mode {
            CaptureMode::H264 => match h264_packets_starting_at_first_key(&packets) {
                Some(filtered) => filtered,
                None => {
                    self.set_status(cx, "Capture stopped: no keyframe captured; record a bit longer");
                    self.ensure_camera_preview(cx);
                    return;
                }
            },
            CaptureMode::Av1 => packets,
        };

        let fps_num = choice
            .format
            .frame_rate
            .unwrap_or(30.0)
            .round()
            .clamp(1.0, 240.0) as u32;

        let media_packets = map_packets(&packets);
        let mp4 = match mode {
            CaptureMode::Av1 => build_av1_mp4(
                choice.format.width as u16,
                choice.format.height as u16,
                fps_num,
                1,
                &media_packets,
            ),
            CaptureMode::H264 => build_h264_mp4(
                choice.format.width as u16,
                choice.format.height as u16,
                fps_num,
                1,
                &media_packets,
            ),
        };

        let Some(mp4) = mp4 else {
            self.set_status(
                cx,
                &format!(
                    "Capture stopped: unable to remux {} stream to MP4",
                    mode.label()
                ),
            );
            self.ensure_camera_preview(cx);
            return;
        };

        if self.auto_test_enabled {
            let path = std::env::temp_dir().join("makepad-camera-auto-test.mp4");
            match std::fs::write(&path, &mp4) {
                Ok(()) => eprintln!(
                    "camera_av1_capture auto-test: wrote {} bytes to {}",
                    mp4.len(),
                    path.display()
                ),
                Err(err) => eprintln!(
                    "camera_av1_capture auto-test: failed to write {}: {}",
                    path.display(),
                    err
                ),
            }
        }

        self.last_capture_data = Some(Rc::new(mp4));
        self.last_capture_label = format!("{} / MP4", mode.label());
        self.set_status(
            cx,
            &format!(
                "Capture stopped: {} packets remuxed to {} in memory",
                packets.len(),
                self.last_capture_label
            ),
        );
        self.ensure_camera_preview(cx);
    }

    fn play_capture(&mut self, cx: &mut Cx) {
        let Some(data) = self.last_capture_data.clone() else {
            self.set_status(cx, "No capture available yet");
            return;
        };

        self.ui.view(cx, ids!(playback_host)).set_visible(cx, true);

        let video = self.ui.video(cx, ids!(playback_video));
        if !video.is_unprepared() {
            self.pending_playback = true;
            video.stop_and_cleanup_resources(cx);
            return;
        }

        video.set_source_in_memory(data);
        video.begin_playback(cx);
        self.pending_playback = false;
        self.set_status(
            cx,
            &format!("Playing captured {}", self.last_capture_label),
        );
    }

    fn report_capabilities(&self, cx: &mut Cx) {
        let caps = cx.video_capabilities();
        let av1 = caps.codecs.iter().find(|c| c.codec == VideoCodec::Av1);
        let h264 = caps.codecs.iter().find(|c| c.codec == VideoCodec::H264);

        let av1_text = if let Some(c) = av1 {
            format!(
                "AV1 enc(hw={}, sw={}) dec(hw={}, sw={})",
                c.encode_hardware, c.encode_software, c.decode_hardware, c.decode_software
            )
        } else {
            "AV1 unavailable".to_string()
        };

        let h264_text = if let Some(c) = h264 {
            format!(
                "H264 enc(hw={}, sw={}) dec(hw={}, sw={})",
                c.encode_hardware, c.encode_software, c.decode_hardware, c.decode_software
            )
        } else {
            "H264 unavailable".to_string()
        };

        self.set_status(cx, &format!("{} | {}", av1_text, h264_text));
    }

    fn maybe_start_auto_test_capture(&mut self, cx: &mut Cx) {
        if !self.auto_test_enabled || self.auto_test_phase != SmokePhase::Idle {
            return;
        }
        if self.camera_permission != Some(PermissionStatus::Granted) || self.camera_choice.is_none() {
            return;
        }
        self.set_mode(cx, CaptureMode::H264);
        self.begin_capture(cx);
        if self.capture_buffer.lock().unwrap().capturing {
            self.auto_test_phase = SmokePhase::Capturing;
            self.auto_test_phase_started_at = cx.seconds_since_app_start();
            self.auto_test_next_frame = cx.new_next_frame();
            eprintln!("camera_av1_capture auto-test: capture started");
        }
    }

    fn poll_auto_test(&mut self, cx: &mut Cx) {
        if !self.auto_test_enabled {
            return;
        }

        let elapsed = cx.seconds_since_app_start() - self.auto_test_phase_started_at;
        match self.auto_test_phase {
            SmokePhase::Idle => self.maybe_start_auto_test_capture(cx),
            SmokePhase::Capturing => {
                if elapsed >= 2.0 {
                    self.stop_capture(cx);
                    self.auto_test_phase = SmokePhase::Stopping;
                    self.auto_test_phase_started_at = cx.seconds_since_app_start();
                    eprintln!("camera_av1_capture auto-test: capture stopped");
                }
                self.auto_test_next_frame = cx.new_next_frame();
            }
            SmokePhase::Stopping => {
                if elapsed >= 0.5 {
                    self.play_capture(cx);
                    self.auto_test_phase = SmokePhase::Playing;
                    self.auto_test_phase_started_at = cx.seconds_since_app_start();
                    eprintln!("camera_av1_capture auto-test: playback requested");
                }
                self.auto_test_next_frame = cx.new_next_frame();
            }
            SmokePhase::Playing => {
                if elapsed >= 2.0 {
                    self.auto_test_phase = SmokePhase::Done;
                    eprintln!("camera_av1_capture auto-test: done");
                    return;
                }
                self.auto_test_next_frame = cx.new_next_frame();
            }
            SmokePhase::Done => {}
        }
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(mode_av1_btn)).clicked(actions) {
            self.set_mode(cx, CaptureMode::Av1);
        }
        if self.ui.button(cx, ids!(mode_h264_btn)).clicked(actions) {
            self.set_mode(cx, CaptureMode::H264);
        }
        if self.ui.button(cx, ids!(start_capture_btn)).clicked(actions) {
            self.begin_capture(cx);
        }
        if self.ui.button(cx, ids!(stop_capture_btn)).clicked(actions) {
            self.stop_capture(cx);
        }
        if self.ui.button(cx, ids!(playback_btn)).clicked(actions) {
            self.play_capture(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_media::install();
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        match event {
            Event::Startup => {
                cx.request_permission(Permission::Camera);
                cx.video_input(0, |_buf| {});
                self.last_capture_label = "H.264 in MP4".to_string();
                self.auto_test_enabled = std::env::args().any(|arg| arg == "--smoke-test");
                self.auto_test_phase = SmokePhase::Idle;
                self.auto_test_next_frame = NextFrame(0);
                self.auto_test_phase_started_at = cx.seconds_since_app_start();
                self.set_mode(cx, CaptureMode::H264);
                self.report_capabilities(cx);
                self.maybe_start_auto_test_capture(cx);
            }
            Event::PermissionResult(result) => {
                if result.permission == Permission::Camera {
                    self.camera_permission = Some(result.status);
                    self.set_status(cx, &format!("Camera permission: {:?}", result.status));
                    self.ensure_camera_preview(cx);
                    self.maybe_start_auto_test_capture(cx);
                }
            }
            Event::VideoInputs(ev) => {
                self.camera_choice = Self::pick_camera_choice(ev);
                if let Some(choice) = &self.camera_choice {
                    self.set_status(
                        cx,
                        &format!(
                            "Camera ready: {} ({}x{} {:?})",
                            choice.name,
                            choice.format.width,
                            choice.format.height,
                            choice.format.pixel_format
                        ),
                    );
                    self.ensure_camera_preview(cx);
                    self.maybe_start_auto_test_capture(cx);
                } else {
                    self.set_status(cx, "No suitable camera format found");
                }
            }
            Event::VideoPlaybackPrepared(ev) => {
                self.set_status(
                    cx,
                    &format!("Playback prepared: {}x{}", ev.video_width, ev.video_height),
                );
                if self.auto_test_enabled {
                    eprintln!(
                        "camera_av1_capture auto-test: playback prepared {}x{}",
                        ev.video_width,
                        ev.video_height
                    );
                }
            }
            Event::VideoPlaybackResourcesReleased(_) => {
                if self.pending_playback {
                    self.play_capture(cx);
                }
            }
            Event::VideoDecodingError(ev) => {
                self.set_status(cx, &format!("Playback error: {}", ev.error));
            }
            _ => {}
        }

        if self.auto_test_enabled && self.auto_test_next_frame.is_event(event).is_some() {
            self.poll_auto_test(cx);
        }
    }
}
