use makepad_widgets::*;

app_main!(App);

const AV1_MP4: &[u8] = include_bytes!("../data/av1.mp4");

fn write_test_file(name: &str, data: &[u8]) -> String {
    let dir = std::env::temp_dir().join("makepad_video_test");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(name);
    let _ = std::fs::write(&path, data);
    path.to_string_lossy().to_string()
}

/// Write embedded test video to temp dir. Call before app_main().
pub fn write_test_files() {
    write_test_file("test_av1.mp4", AV1_MP4);
    let dir = std::env::temp_dir().join("makepad_video_test");
    eprintln!("Test files in: {}", dir.display());
}

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

fn temp_video_path(name: &str) -> String {
    std::env::temp_dir()
        .join("makepad_video_test")
        .join(name)
        .to_string_lossy()
        .to_string()
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
            let videos: &[(&[LiveId], &str)] = &[(&[live_id!(av1_mp4_video)], "test_av1.mp4")];

            let first_ref = self.ui.video(cx, videos[0].0);
            if first_ref.borrow().is_none() {
                return;
            }

            for (id, name) in videos {
                let path = temp_video_path(name);
                let vref = self.ui.video(cx, *id);
                vref.set_source(VideoDataSource::Filesystem { path });
                vref.begin_playback(cx);
            }

            self.ui
                .label(cx, ids!(can_play_label))
                .set_text(cx, &can_play_report());
            self.sources_set = true;
        }
    }
}
