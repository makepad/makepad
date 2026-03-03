use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(640, 520)
                body +: {
                    main_view := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 12
                        padding: 16

                        Label{
                            text: "Camera Example"
                            draw_text.text_style.font_size: 18
                        }
                        status_label := Label{
                            text: "Waiting for camera..."
                            draw_text.text_style.font_size: 10
                            draw_text.color: #888
                        }
                        camera_video := Video{
                            width: Fill
                            height: Fill
                            autoplay: false
                        }
                    }
                }
            }
        }
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    started: bool,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        match event {
            Event::Startup => {
                log!("[camera-example] Startup event received, requesting camera permission");
                cx.request_permission(makepad_widgets::makepad_platform::permission::Permission::Camera);
            }
            Event::Draw(e) => {
                if !self.started {
                    log!("[camera-example] Draw event, started={}", self.started);
                }
                let _ = e;
            }
            Event::VideoInputs(ev) => {
                log!(
                    "[camera-example] VideoInputs event: {} devices, started={}",
                    ev.descs.len(),
                    self.started
                );
                for (i, desc) in ev.descs.iter().enumerate() {
                    log!(
                        "[camera-example]   device[{}]: name={:?} input_id={:?} formats={}",
                        i,
                        desc.name,
                        desc.input_id,
                        desc.formats.len()
                    );
                    for (j, fmt) in desc.formats.iter().enumerate().take(5) {
                        log!(
                            "[camera-example]     format[{}]: {}x{} {:?} fps={:?}",
                            j,
                            fmt.width,
                            fmt.height,
                            fmt.pixel_format,
                            fmt.frame_rate
                        );
                    }
                }

                if self.started {
                    return;
                }
                if ev.descs.is_empty() {
                    log!("[camera-example] No cameras found");
                    self.ui
                        .label(cx, ids!(status_label))
                        .set_text(cx, "No camera found");
                    return;
                }

                let inputs = ev.find_highest(0);
                log!("[camera-example] find_highest(0) returned {} entries", inputs.len());
                if inputs.is_empty() {
                    self.ui
                        .label(cx, ids!(status_label))
                        .set_text(cx, "No suitable format found");
                    return;
                }

                let (input_id, format_id) = inputs[0];
                let desc = &ev.descs[0];
                log!(
                    "[camera-example] Starting camera: {:?} input_id={:?} format_id={:?}",
                    desc.name,
                    input_id,
                    format_id
                );

                self.ui.label(cx, ids!(status_label)).set_text(
                    cx,
                    &format!("Camera: {}", desc.name),
                );

                let video_ref = self.ui.video(cx, &[live_id!(camera_video)]);
                log!("[camera-example] Got video ref, calling set_source_camera");
                video_ref.set_source_camera(cx, input_id, format_id);
                log!("[camera-example] Calling begin_playback");
                video_ref.begin_playback(cx);
                self.started = true;
                log!("[camera-example] Camera started");
            }
            Event::VideoPlaybackPrepared(ev) => {
                log!(
                    "[camera-example] VideoPlaybackPrepared: video_id={:?} {}x{} duration={}",
                    ev.video_id,
                    ev.video_width,
                    ev.video_height,
                    ev.duration
                );
            }
            Event::VideoTextureUpdated(ev) => {
                log!(
                    "[camera-example] VideoTextureUpdated: video_id={:?} pos={} yuv_enabled={}",
                    ev.video_id,
                    ev.current_position_ms,
                    ev.yuv_enabled
                );
            }
            Event::PermissionResult(result) => {
                log!("[camera-example] PermissionResult: {:?}", result);
                use makepad_widgets::makepad_platform::permission::{Permission, PermissionStatus};
                if result.permission == Permission::Camera {
                    match result.status {
                        PermissionStatus::Granted => {
                            self.ui.label(cx, ids!(status_label)).set_text(cx, "Camera permission granted");
                        }
                        PermissionStatus::DeniedPermanent => {
                            self.ui.label(cx, ids!(status_label)).set_text(cx, "Camera permission denied");
                        }
                        _ => {
                            self.ui.label(cx, ids!(status_label)).set_text(cx, &format!("Camera permission: {:?}", result.status));
                        }
                    }
                }
            }
            Event::VideoDecodingError(ev) => {
                log!(
                    "[camera-example] VideoDecodingError: video_id={:?} error={:?}",
                    ev.video_id,
                    ev.error
                );
            }
            _ => {}
        }
    }
}
