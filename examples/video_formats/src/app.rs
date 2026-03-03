use makepad_widgets::*;

app_main!(App);

const VP8_WEBM: &[u8] = include_bytes!("../data/vp8.webm");
const VP9_WEBM: &[u8] = include_bytes!("../data/vp9.webm");
const AV1_WEBM: &[u8] = include_bytes!("../data/av1.webm");
const AV1_MP4: &[u8] = include_bytes!("../data/av1.mp4");

fn write_test_file(name: &str, data: &[u8]) -> String {
    let dir = std::env::temp_dir().join("makepad_video_test");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(name);
    let _ = std::fs::write(&path, data);
    path.to_string_lossy().to_string()
}

/// Write embedded test videos to temp dir. Call before app_main().
pub fn write_test_files() {
    write_test_file("test_av1.mp4", AV1_MP4);
    write_test_file("test_vp8.webm", VP8_WEBM);
    write_test_file("test_vp9.webm", VP9_WEBM);
    write_test_file("test_av1.webm", AV1_WEBM);
    let dir = std::env::temp_dir().join("makepad_video_test");
    eprintln!("Test files in: {}", dir.display());
}

fn can_play_report() -> String {
    let types = [
        "video/mp4", "video/webm", "video/ogg", "video/quicktime",
        "video/x-matroska", "audio/mp4", "audio/webm", "audio/ogg",
        "audio/mpeg", "audio/flac",
    ];
    let mut parts = Vec::new();
    for t in &types {
        let r = makepad_widgets::makepad_platform::can_play_type(t);
        let r = if r.is_empty() { "no" } else { r };
        parts.push(format!("{t}={r}"));
    }
    parts.join("  ")
}

fn temp_video_path(name: &str) -> String {
    std::env::temp_dir().join("makepad_video_test").join(name)
        .to_string_lossy().to_string()
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(950, 700)
                body +: {
                    main_view := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 12
                        padding: 16

                        Label{
                            text: "Video Format Test"
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
                                width: 200
                                height: Fit
                                flow: Down
                                spacing: 4
                                Label{ text: "AV1 (MP4)" }
                                av1_mp4_video := Video{
                                    width: 200
                                    height: 150
                                    is_looping: true
                                    mute: true
                                }
                            }

                            View{
                                width: 200
                                height: Fit
                                flow: Down
                                spacing: 4
                                Label{ text: "VP8 (WebM)" }
                                vp8_video := Video{
                                    width: 200
                                    height: 150
                                    is_looping: true
                                    mute: true
                                }
                            }

                            View{
                                width: 200
                                height: Fit
                                flow: Down
                                spacing: 4
                                Label{ text: "VP9 (WebM)" }
                                vp9_video := Video{
                                    width: 200
                                    height: 150
                                    is_looping: true
                                    mute: true
                                }
                            }

                            View{
                                width: 200
                                height: Fit
                                flow: Down
                                spacing: 4
                                Label{ text: "AV1 (WebM)" }
                                av1_webm_video := Video{
                                    width: 200
                                    height: 150
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

        // Set video sources after widgets are instantiated (first paint cycle).
        // Video widgets inside script_mod are not borrowable until after first draw.
        if !self.sources_set {
            let videos: &[(&[LiveId], &str)] = &[
                (&[live_id!(av1_mp4_video)], "test_av1.mp4"),
                (&[live_id!(vp8_video)], "test_vp8.webm"),
                (&[live_id!(vp9_video)], "test_vp9.webm"),
                (&[live_id!(av1_webm_video)], "test_av1.webm"),
            ];

            // Check if any video widget is instantiated yet
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

            self.ui.label(cx, ids!(can_play_label)).set_text(cx, &can_play_report());
            self.sources_set = true;
        }
    }
}
