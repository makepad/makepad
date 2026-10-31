//! The video player example: one `VideoPlayer` filling the window.
//!
//! Everything a person touches is the library's `VideoPlayer` — the picture
//! fitted whole, the frosted transport bar (play/pause, elapsed, scrubber,
//! total, skip), the volume pill, tap to show or hide, double tap on a side
//! to skip ten seconds, Space to play or pause. This file only chooses what
//! to play:
//!
//!  - `video_player <file or URL>` plays that, otherwise a public HLS stream;
//!  - the Open chip (top left, with the other controls) opens the system
//!    file picker; on Android that is the document picker and the answer is
//!    a `content://` URI;
//!  - a file dropped on the window plays.
//!
//! `--mute` (or a hidden test run, `MAKEPAD_HIDE_WINDOWS=1`) starts muted.

pub use makepad_widgets;

use makepad_widgets::*;

mod picker;
use picker::{pick_local_video, picked_video};

app_main!(App);

// A public HLS (m3u8) adaptive-bitrate stream, played natively by the
// platform's player, so the example shows real streaming rather than a
// single progressive MP4.
const VIDEO_URL: &str = "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8";

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Video Player"
                window.inner_size: vec2(1024, 576)
                pass.clear_color: #x000000
                body +: {
                    player := VideoPlayer{
                        controls +: {
                            top_row +: {
                                leading +: {
                                    open_button := PlayerGlass{
                                        width: Fit
                                        height: 48
                                        padding: Inset{left: 20 right: 20}
                                        cursor: MouseCursor.Hand
                                        grab_key_focus: false
                                        draw_bg +: {corner_radius: 24.0}
                                        PlayerClock{text: "Open"}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    started: bool,
}

impl App {
    fn player(&self, cx: &mut Cx) -> VideoPlayerRef {
        self.ui.video_player(cx, ids!(player))
    }

    fn start(&mut self, cx: &mut Cx) {
        self.started = true;
        let args: Vec<String> = std::env::args().collect();
        let source = args
            .iter()
            .skip(1)
            .find(|arg| !arg.starts_with("--"))
            .cloned()
            .unwrap_or_else(|| VIDEO_URL.to_string());
        let hidden = std::env::var("MAKEPAD_HIDE_WINDOWS").is_ok_and(|v| v == "1");
        let player = self.player(cx);
        if hidden || args.iter().any(|arg| arg == "--mute") {
            player.set_muted(cx, true);
        }
        player.open(cx, &source);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            if let Some(path) = picked_video(action) {
                self.player(cx).open(cx, &path);
            }
        }
        if self.ui.view(cx, ids!(open_button)).finger_up(actions).is_some_and(|fe| fe.is_over && !fe.cancelled) {
            pick_local_video(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if !self.started {
            self.start(cx);
        }
        if let Event::Drop(drop) = event {
            if let Some(path) = drop.items.iter().find_map(|item| match item {
                DragItem::FilePath { path, .. } => Some(path.clone()),
                _ => None,
            }) {
                self.player(cx).open(cx, &path);
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
