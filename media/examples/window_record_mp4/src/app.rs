use makepad_media::{mux::build_h264_mp4, EncodedVideoPacketOwned};
use makepad_widgets::makepad_platform::{VideoCodec, VideoEncodeSource, VideoEncoderConfig};
use makepad_widgets::*;
use std::sync::{Arc, Mutex};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)) {
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(900, 620)
                body +: {
                    root := View{
                        width: Fill,
                        height: Fill,
                        flow: Down,
                        spacing: 10,
                        padding: 12,

                        controls := View{
                            width: Fill,
                            height: Fit,
                            flow: Right,
                            spacing: 8,
                            record_btn := Button { text: "Start recording" }
                        }

                        status := Label{
                            text: "Idle"
                            draw_text.color: #888
                        }

                        record_surface := View{
                            width: Fill,
                            height: Fill,
                            texture_caching: true
                            show_bg: true
                            draw_bg: { color: #20242a }

                            content := View{
                                width: Fill,
                                height: Fill,
                                flow: Down,
                                align: {x: 0.5, y: 0.5}
                                spacing: 20

                                title := Label{
                                    text: "Window Recording Demo"
                                    draw_text.text_style.font_size: 28
                                }

                                clock := Label{
                                    text: "t = 0.00s"
                                    draw_text.text_style.font_size: 18
                                    draw_text.color: #f6
                                }

                                pulse := View{
                                    width: 260,
                                    height: 120,
                                    show_bg: true
                                    draw_bg: { color: #3a7 }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Default)]
struct CaptureState {
    recording: bool,
    packets: Vec<EncodedVideoPacketOwned>,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    started_at: f64,
    #[rust]
    capture: Arc<Mutex<CaptureState>>,
    #[rust]
    next_frame: Option<NextFrame>,
    #[rust]
    width: u32,
    #[rust]
    height: u32,
    #[rust]
    fps_num: u32,
    #[rust]
    fps_den: u32,
}

impl App {
    fn set_status(&self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(status)).set_text(cx, text);
    }

    fn start_recording(&mut self, cx: &mut Cx) {
        let caps = cx.video_capabilities();
        let Some(h264) = caps.codecs.iter().find(|c| c.codec == VideoCodec::H264) else {
            self.set_status(cx, "H264 encoder unavailable");
            return;
        };
        if !h264.supports_texture_source || (!h264.encode_hardware && !h264.encode_software) {
            self.set_status(cx, "Texture recording is not supported on this platform");
            return;
        }

        let view = self.ui.view(cx, ids!(record_surface));
        let Some(texture_id) = view.cached_texture_id() else {
            self.set_status(cx, "Record surface not initialized yet; try again");
            self.ui.redraw(cx);
            return;
        };

        let rect = view.area().rect(cx);
        let mut width = rect.size.x.max(2.0) as u32;
        let mut height = rect.size.y.max(2.0) as u32;
        if width % 2 != 0 {
            width += 1;
        }
        if height % 2 != 0 {
            height += 1;
        }

        self.width = width;
        self.height = height;
        self.fps_num = 30;
        self.fps_den = 1;

        {
            let mut cap = self.capture.lock().unwrap();
            cap.recording = true;
            cap.packets.clear();
        }

        let shared = self.capture.clone();
        let config = VideoEncoderConfig {
            codec: VideoCodec::H264,
            source: VideoEncodeSource::Texture { texture_id },
            width,
            height,
            fps_num: self.fps_num,
            fps_den: self.fps_den,
            target_bitrate: 3_000_000,
            keyint: 60,
            latency_realtime: true,
            codec_mode: 8,
            queue_policy: makepad_widgets::makepad_platform::VideoQueuePolicy::LatestWins,
            queue_capacity: 2,
        };

        let result = cx.video_encoder_output_try(0, config, move |packet| {
            let mut cap = shared.lock().unwrap();
            if !cap.recording {
                return;
            }
            cap.packets.push(EncodedVideoPacketOwned {
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

        if let Err(err) = result {
            let mut cap = self.capture.lock().unwrap();
            cap.recording = false;
            cap.packets.clear();
            self.set_status(cx, &format!("Failed to start encoder: {:?}", err));
            return;
        }

        self.next_frame = Some(cx.new_next_frame());
        self.ui.button(cx, ids!(record_btn)).set_text(cx, "Stop recording");
        self.set_status(cx, &format!("Recording {}x{} H264...", width, height));
        self.ui.redraw(cx);
    }

    fn stop_recording(&mut self, cx: &mut Cx) {
        self.next_frame = None;

        let packets = {
            let mut cap = self.capture.lock().unwrap();
            cap.recording = false;
            std::mem::take(&mut cap.packets)
        };

        self.ui.button(cx, ids!(record_btn)).set_text(cx, "Start recording");

        if packets.is_empty() {
            self.set_status(cx, "No packets captured");
            return;
        }

        let mp4 = build_h264_mp4(self.width as u16, self.height as u16, self.fps_num, self.fps_den, &packets);
        let Some(mp4) = mp4 else {
            self.set_status(cx, "MP4 mux failed");
            return;
        };

        let path = std::env::temp_dir().join("makepad-window-record.mp4");
        if std::fs::write(&path, &mp4).is_err() {
            self.set_status(cx, "Failed to write MP4 file");
            return;
        }

        self.set_status(cx, &format!("Saved {} bytes to {}", mp4.len(), path.display()));
    }

    fn is_recording(&self) -> bool {
        self.capture.lock().unwrap().recording
    }

    fn tick(&mut self, cx: &mut Cx) {
        if !self.is_recording() {
            return;
        }
        let t = cx.seconds_since_app_start() - self.started_at;
        self.ui
            .label(cx, ids!(clock))
            .set_text(cx, &format!("t = {:.2}s", t));

        let phase = ((t * 2.0).sin() * 0.5 + 0.5) as f32;
        let color = vec4(
            0.2 + 0.6 * phase,
            0.6 - 0.4 * phase,
            0.4 + 0.4 * phase,
            1.0,
        );
        self.ui.view(cx, ids!(pulse)).set_uniform(cx, id!(color), &[color.x, color.y, color.z, color.w]);

        self.next_frame = Some(cx.new_next_frame());
        self.ui.redraw(cx);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(record_btn)).clicked(actions) {
            if self.is_recording() {
                self.stop_recording(cx);
            } else {
                self.start_recording(cx);
            }
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

        if let Event::Startup = event {
            self.started_at = cx.seconds_since_app_start();
            self.set_status(cx, "Idle");
            self.ui.redraw(cx);
        }

        if let Some(next) = self.next_frame {
            if next.is_event(event).is_some() {
                self.tick(cx);
            }
        }
    }
}
