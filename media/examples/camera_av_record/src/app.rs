use makepad_media::{
    mux::build_h264_mp4,
    AudioCaptureConfig, AudioCaptureFrameizer, AudioPlayoutConfig, MakepadAudioOutputAdapter,
    PcmAudioFrame, PcmAudioPlayoutBuffer,
};
use makepad_widgets::makepad_platform::{
    audio::AudioDeviceId,
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

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)) {
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(900, 700)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 10
                        padding: 16

                        Label{
                            text: "Camera + Audio Record & Playback"
                            draw_text.text_style.font_size: 18
                        }

                        controls := View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 8

                            record_btn := Button { width: Fill text: "Record" }
                            stop_btn := Button { width: Fill text: "Stop" }
                            play_btn := Button { width: Fill text: "Play" }
                        }

                        status_label := Label{
                            text: "Idle"
                            draw_text.color: #888
                        }

                        Label{ text: "Camera preview" }
                        camera_preview_video := Video{
                            width: Fill
                            height: 260
                            autoplay: false
                            show_controls: false
                        }

                        playback_host := View{
                            width: Fill
                            height: Fill
                            flow: Down
                            spacing: 6
                            visible: false

                            Label{ text: "Recorded playback" }
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
struct VideoCaptureBuffer {
    capturing: bool,
    packets: Vec<EncodedVideoPacketOwned>,
}

#[derive(Default)]
struct AudioCaptureBuffer {
    capturing: bool,
    frames: Vec<PcmAudioFrame>,
}

#[derive(Clone)]
struct CameraChoice {
    input_id: VideoInputId,
    format: VideoFormat,
    name: String,
}

/// Shared slot so the audio output callback always reads from the current adapter.
struct SharedAudioAdapter {
    adapter: Option<MakepadAudioOutputAdapter>,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    camera_permission: Option<PermissionStatus>,
    #[rust]
    audio_permission: Option<PermissionStatus>,
    #[rust]
    camera_choice: Option<CameraChoice>,
    #[rust]
    video_capture: Arc<Mutex<VideoCaptureBuffer>>,
    #[rust]
    audio_capture: Arc<Mutex<AudioCaptureBuffer>>,
    #[rust]
    last_video_data: Option<Rc<Vec<u8>>>,
    #[rust]
    last_audio_frames: Option<Vec<PcmAudioFrame>>,
    #[rust]
    pending_playback: bool,
    #[rust]
    shared_audio_adapter: Arc<Mutex<SharedAudioAdapter>>,
    #[rust]
    audio_output_installed: bool,
    #[rust]
    default_audio_inputs: Vec<AudioDeviceId>,
    #[rust]
    default_audio_outputs: Vec<AudioDeviceId>,
}

impl Default for SharedAudioAdapter {
    fn default() -> Self {
        Self { adapter: None }
    }
}

impl App {
    fn set_status(&self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(status_label)).set_text(cx, text);
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

    fn pick_camera(ev: &VideoInputsEvent) -> Option<CameraChoice> {
        let desc = ev.descs.first()?;

        let supported = |f: &VideoFormat| {
            matches!(
                f.pixel_format,
                VideoPixelFormat::NV12 | VideoPixelFormat::YUY2 | VideoPixelFormat::YUV420
            )
        };

        let rank = |f: &VideoFormat| -> (usize, usize, i32) {
            let pix = match f.pixel_format {
                VideoPixelFormat::NV12 => 3,
                VideoPixelFormat::YUY2 => 2,
                VideoPixelFormat::YUV420 => 1,
                _ => 0,
            };
            (pix, f.width * f.height, f.frame_rate.unwrap_or(0.0) as i32)
        };

        let best = desc
            .formats
            .iter()
            .filter(|f| {
                supported(f) && f.width.max(f.height) <= 1280 && f.width.min(f.height) <= 720
            })
            .max_by_key(|f| rank(f))
            .or_else(|| {
                desc.formats
                    .iter()
                    .filter(|f| supported(f))
                    .max_by_key(|f| rank(f))
            })?;

        Some(CameraChoice {
            input_id: desc.input_id,
            format: *best,
            name: desc.name.clone(),
        })
    }

    fn begin_recording(&mut self, cx: &mut Cx) {
        if self.video_capture.lock().unwrap().capturing {
            self.set_status(cx, "Already recording");
            return;
        }
        if self.camera_permission != Some(PermissionStatus::Granted) {
            self.set_status(cx, "Camera permission not granted");
            return;
        }
        let Some(choice) = self.camera_choice.clone() else {
            self.set_status(cx, "No camera available");
            return;
        };

        // Start video encoder.
        {
            let mut vc = self.video_capture.lock().unwrap();
            vc.capturing = true;
            vc.packets.clear();
        }

        let video_shared = self.video_capture.clone();
        let fps_num = choice
            .format
            .frame_rate
            .unwrap_or(30.0)
            .round()
            .clamp(1.0, 240.0) as u32;

        let cfg = VideoEncoderConfig {
            codec: VideoCodec::H264,
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
            let mut vc = video_shared.lock().unwrap();
            if !vc.capturing {
                return;
            }
            vc.packets.push(EncodedVideoPacketOwned {
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
            self.video_capture.lock().unwrap().capturing = false;
            self.set_status(cx, &format!("H.264 encoder failed: {:?}", err));
            return;
        }

        let _ = cx.video_encoder_request_keyframe(0);
        cx.use_video_input(&[(choice.input_id, choice.format.format_id)]);

        // Start audio capture.
        {
            let mut ac = self.audio_capture.lock().unwrap();
            ac.capturing = true;
            ac.frames.clear();
        }

        let audio_shared = self.audio_capture.clone();
        let audio_cfg = AudioCaptureConfig {
            sample_rate: 48_000,
            channels: 1,
            frame_samples: 960,
        };
        let mut frameizer = AudioCaptureFrameizer::new(audio_cfg).unwrap();

        cx.audio_input(0, move |info, buffer| {
            let mut ac = audio_shared.lock().unwrap();
            if !ac.capturing {
                return;
            }
            let sample_rate = info.sample_rate.round() as u32;
            if let Ok(frames) = frameizer.push_buffer(sample_rate, buffer) {
                ac.frames.extend(frames);
            }
        });

        // Start default mic.
        if !self.default_audio_inputs.is_empty() {
            cx.use_audio_inputs(&self.default_audio_inputs);
        }

        // Hide playback area and stop audio.
        self.shared_audio_adapter.lock().unwrap().adapter = None;
        self.ui.view(cx, ids!(playback_host)).set_visible(cx, false);
        let pv = self.ui.video(cx, ids!(playback_video));
        if !pv.is_unprepared() {
            pv.stop_and_cleanup_resources(cx);
        }
        self.pending_playback = false;

        self.set_status(
            cx,
            &format!(
                "Recording from {} ({}x{} {:?})",
                choice.name, choice.format.width, choice.format.height, choice.format.pixel_format
            ),
        );
    }

    fn stop_recording(&mut self, cx: &mut Cx) {
        let Some(choice) = self.camera_choice.clone() else {
            self.set_status(cx, "No camera");
            return;
        };

        // Stop audio capture.
        let audio_frames = {
            let mut ac = self.audio_capture.lock().unwrap();
            ac.capturing = false;
            std::mem::take(&mut ac.frames)
        };

        // Stop video capture.
        let video_packets = {
            let mut vc = self.video_capture.lock().unwrap();
            if !vc.capturing {
                self.set_status(cx, "Not recording");
                return;
            }
            vc.capturing = false;
            std::mem::take(&mut vc.packets)
        };

        if video_packets.is_empty() {
            self.set_status(cx, "No video packets captured; record longer");
            self.ensure_camera_preview(cx);
            return;
        }

        let video_packets = match h264_from_first_key(&video_packets) {
            Some(p) => p,
            None => {
                self.set_status(cx, "No keyframe captured; record longer");
                self.ensure_camera_preview(cx);
                return;
            }
        };

        let fps_num = choice
            .format
            .frame_rate
            .unwrap_or(30.0)
            .round()
            .clamp(1.0, 240.0) as u32;

        let media_packets = map_video_packets(&video_packets);
        let mp4 = build_h264_mp4(
            choice.format.width as u16,
            choice.format.height as u16,
            fps_num,
            1,
            &media_packets,
        );

        let Some(mp4) = mp4 else {
            self.set_status(cx, "MP4 mux failed");
            self.ensure_camera_preview(cx);
            return;
        };

        self.last_video_data = Some(Rc::new(mp4));
        self.last_audio_frames = if audio_frames.is_empty() {
            None
        } else {
            Some(audio_frames)
        };

        let audio_info = if let Some(ref frames) = self.last_audio_frames {
            format!(", {} audio frames", frames.len())
        } else {
            " (no audio)".to_string()
        };

        self.set_status(
            cx,
            &format!(
                "Stopped: {} video packets remuxed to MP4{}",
                video_packets.len(),
                audio_info
            ),
        );
        self.ensure_camera_preview(cx);
    }

    fn play_capture(&mut self, cx: &mut Cx) {
        let Some(data) = self.last_video_data.clone() else {
            self.set_status(cx, "No recording available");
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

        // Start audio playout if we have captured audio.
        if let Some(frames) = self.last_audio_frames.clone() {
            self.setup_audio_playout(cx, &frames);
        }

        self.set_status(cx, "Playing recorded capture");
    }

    fn setup_audio_playout(&mut self, cx: &mut Cx, frames: &[PcmAudioFrame]) {
        let config = AudioPlayoutConfig {
            sample_rate: 48_000,
            channels: 1,
            frame_samples: 960,
            target_delay_ms: 40,
            max_buffer_ms: 500,
        };
        let (playout, adapter) = PcmAudioPlayoutBuffer::new(config).unwrap();

        for frame in frames {
            let _ = playout.push_frame(frame.clone());
        }
        playout.set_end_of_stream();

        // Swap the adapter into the shared slot so the callback picks it up.
        self.shared_audio_adapter.lock().unwrap().adapter = Some(adapter);

        // Install the audio output callback once; it reads from the shared slot.
        if !self.audio_output_installed {
            self.audio_output_installed = true;
            let shared = self.shared_audio_adapter.clone();
            cx.audio_output(0, move |info, output| {
                if let Ok(mut guard) = shared.try_lock() {
                    if let Some(adapter) = &mut guard.adapter {
                        adapter.fill_output(info, output);
                        return;
                    }
                }
                output.zero();
            });
            if !self.default_audio_outputs.is_empty() {
                cx.use_audio_outputs(&self.default_audio_outputs);
            }
        }
    }
}

fn map_video_packets(
    packets: &[EncodedVideoPacketOwned],
) -> Vec<makepad_media::EncodedVideoPacketOwned> {
    packets
        .iter()
        .map(|pkt| makepad_media::EncodedVideoPacketOwned {
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

fn map_codec(
    codec: makepad_widgets::makepad_platform::video::VideoCodec,
) -> makepad_media::VideoCodec {
    match codec {
        makepad_widgets::makepad_platform::video::VideoCodec::H264 => {
            makepad_media::VideoCodec::H264
        }
        makepad_widgets::makepad_platform::video::VideoCodec::H265 => {
            makepad_media::VideoCodec::H265
        }
        makepad_widgets::makepad_platform::video::VideoCodec::Av1 => {
            makepad_media::VideoCodec::Av1
        }
        makepad_widgets::makepad_platform::video::VideoCodec::Vp8 => {
            makepad_media::VideoCodec::Vp8
        }
        makepad_widgets::makepad_platform::video::VideoCodec::Vp9 => {
            makepad_media::VideoCodec::Vp9
        }
    }
}

fn map_format(
    format: makepad_widgets::makepad_platform::video::VideoBitstreamFormat,
) -> makepad_media::VideoBitstreamFormat {
    match format {
        makepad_widgets::makepad_platform::video::VideoBitstreamFormat::AnnexB => {
            makepad_media::VideoBitstreamFormat::AnnexB
        }
        makepad_widgets::makepad_platform::video::VideoBitstreamFormat::Avcc => {
            makepad_media::VideoBitstreamFormat::Avcc
        }
        makepad_widgets::makepad_platform::video::VideoBitstreamFormat::Av1Obu => {
            makepad_media::VideoBitstreamFormat::Av1Obu
        }
        makepad_widgets::makepad_platform::video::VideoBitstreamFormat::RawAccessUnit => {
            makepad_media::VideoBitstreamFormat::RawAccessUnit
        }
    }
}

fn h264_from_first_key(
    packets: &[EncodedVideoPacketOwned],
) -> Option<Vec<EncodedVideoPacketOwned>> {
    let first_key = packets
        .iter()
        .position(|p| !p.is_config && !p.is_eos && p.is_key)?;

    let key_config_id = packets[first_key].config_id;

    let mut out = Vec::new();
    for p in packets {
        if p.is_config && p.config_id == key_config_id {
            out.push(p.clone());
        }
    }
    for p in packets.iter().skip(first_key) {
        if p.is_eos {
            continue;
        }
        if !p.is_config {
            out.push(p.clone());
        }
    }
    Some(out)
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(record_btn)).clicked(actions) {
            self.begin_recording(cx);
        }
        if self.ui.button(cx, ids!(stop_btn)).clicked(actions) {
            self.stop_recording(cx);
        }
        if self.ui.button(cx, ids!(play_btn)).clicked(actions) {
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
                cx.request_permission(Permission::AudioInput);
                cx.video_input(0, |_buf| {});
                self.set_status(cx, "Waiting for permissions...");
            }
            Event::PermissionResult(result) => {
                if result.permission == Permission::Camera {
                    self.camera_permission = Some(result.status);
                    self.ensure_camera_preview(cx);
                }
                if result.permission == Permission::AudioInput {
                    self.audio_permission = Some(result.status);
                }
                self.update_permission_status(cx);
            }
            Event::VideoInputs(ev) => {
                self.camera_choice = Self::pick_camera(ev);
                if let Some(choice) = &self.camera_choice {
                    self.set_status(
                        cx,
                        &format!(
                            "Camera: {} ({}x{} {:?})",
                            choice.name,
                            choice.format.width,
                            choice.format.height,
                            choice.format.pixel_format
                        ),
                    );
                    self.ensure_camera_preview(cx);
                } else {
                    self.set_status(cx, "No suitable camera format found");
                }
            }
            Event::AudioDevices(ev) => {
                self.default_audio_inputs = ev.default_input();
                self.default_audio_outputs = ev.default_output();
            }
            Event::VideoPlaybackPrepared(ev) => {
                self.set_status(
                    cx,
                    &format!(
                        "Playback prepared: {}x{}",
                        ev.video_width, ev.video_height
                    ),
                );
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
    }
}

impl App {
    fn update_permission_status(&self, cx: &mut Cx) {
        let cam = match self.camera_permission {
            Some(PermissionStatus::Granted) => "granted",
            Some(PermissionStatus::DeniedPermanent) => "denied",
            _ => "pending",
        };
        let mic = match self.audio_permission {
            Some(PermissionStatus::Granted) => "granted",
            Some(PermissionStatus::DeniedPermanent) => "denied",
            _ => "pending",
        };
        if cam == "granted" && mic == "granted" {
            self.set_status(cx, "Ready to record");
        } else {
            self.set_status(cx, &format!("Camera: {}, Microphone: {}", cam, mic));
        }
    }
}
