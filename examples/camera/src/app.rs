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

        if let Event::VideoInputs(ev) = event {
            if self.started {
                return;
            }
            if ev.descs.is_empty() {
                self.ui
                    .label(cx, ids!(status_label))
                    .set_text(cx, "No camera found");
                return;
            }

            let inputs = ev.find_highest(0);
            if inputs.is_empty() {
                self.ui
                    .label(cx, ids!(status_label))
                    .set_text(cx, "No suitable format found");
                return;
            }

            let (input_id, format_id) = inputs[0];
            let desc = &ev.descs[0];
            self.ui.label(cx, ids!(status_label)).set_text(
                cx,
                &format!("Camera: {}", desc.name),
            );

            // Allocate texture and start camera playback
            let video_ref = self.ui.video(cx, &[live_id!(camera_video)]);
            video_ref.set_source_camera(cx, input_id, format_id);
            video_ref.begin_playback(cx);
            self.started = true;
        }
    }
}
