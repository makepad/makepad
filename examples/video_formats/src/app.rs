use makepad_widgets::*;
use std::rc::Rc;

app_main!(App);

const AV1_MP4: &[u8] = include_bytes!("../data/av1.mp4");

fn can_play_report() -> String {
    let types = ["video/mp4"];
    let mut parts = Vec::new();
    for t in &types {
        let r = makepad_widgets::makepad_platform::can_play_type(t);
        let r = if r.is_empty() { "no" } else { r };
        parts.push(format!("{t}={r}"));
    }
    parts.join("  ")
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(500, 400)
                body +: {
                    main_view := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 12
                        padding: 16

                        Label{
                            text: "Video Format Test — AV1/MP4"
                            draw_text.text_style.font_size: 18
                        }
                        can_play_label := Label{
                            text: ""
                            draw_text.text_style.font_size: 9
                            draw_text.color: #888
                        }
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 16

                            View{
                                width: 400
                                height: Fit
                                flow: Down
                                spacing: 4
                                Label{ text: "AV1 (MP4)" }
                                av1_mp4_video := Video{
                                    width: 400
                                    height: 300
                                    is_looping: true
                                    mute: true
                                }
                            }
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
    sources_set: bool,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        if !self.sources_set {
            let video_id = &[live_id!(av1_mp4_video)];

            let vref = self.ui.video(cx, video_id);
            if vref.borrow().is_none() {
                return;
            }

            vref.set_source_in_memory(Rc::new(AV1_MP4.to_vec()));
            vref.begin_playback(cx);

            self.ui
                .label(cx, ids!(can_play_label))
                .set_text(cx, &can_play_report());
            self.sources_set = true;
        }
    }
}
