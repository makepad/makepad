//! Thin desktop frontend for `makepad-score-ui`.

use score_ui::{apply_score_action, key_action, ScoreAction, ScoreAppState};
use makepad_widgets::*;
use std::path::PathBuf;

app_main!(App);

/// The piece the application ships, so a fresh start has real music on the
/// desk instead of a synthetic exercise: Mozart's Rondo alla Turca, the third
/// movement of the Piano Sonata No. 11 in A major, K331.
///
/// The engraving and this MIDI rendering of it are Mutopia's, published there
/// as public domain — no attribution obligation and no share-alike, which is
/// what makes it safe to carry inside a permissively licensed binary.
const DEFAULT_SCORE: &[u8] = include_bytes!("../resources/mozart-k331-rondo-alla-turca.mid");
const DEFAULT_SCORE_KIND: &str = "mid";
const DEFAULT_SCORE_TITLE: &str = "Rondo alla Turca";
const DEFAULT_SCORE_CREDIT: &str =
    "Mozart · Rondo alla Turca · Piano Sonata No. 11 in A, K331, third movement";

script_mod! {
    use mod.prelude.score.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Score"
                window.inner_size: vec2(1440, 960)
                pass +: {clear_color: score.color_surround}
                body +: {
                    flow: Down
                    spacing: 0
                    margin: 0
                    padding: 0
                    shell := ScoreShell{}
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
    state: ScoreAppState,
    #[rust]
    started: bool,
}

impl App {
    fn dispatch(&mut self, cx: &mut Cx, action: &ScoreAction) {
        if apply_score_action(cx, &mut self.state, action) {
            self.ui.redraw(cx);
        }
    }

    /// What is on the desk when the application opens: the file the user
    /// named, or the piece the application ships. A shipped resource that
    /// cannot be read costs the piece and nothing else — the built-in demo
    /// score stays, and the status line says what happened.
    fn open_initial_score(&mut self, cx: &mut Cx) {
        // The first NON-FLAG argument. `--remote` and friends belong to the
        // platform layer, and taking argument one blindly meant a remote-driven
        // launch tried to open a file called "--remote" and fell back to the
        // demo score — which is exactly how this was found.
        let named = std::env::args_os().skip(1).find(|argument| {
            !argument.to_string_lossy().starts_with('-')
        });
        if let Some(path) = named.map(PathBuf::from) {
            self.dispatch(cx, &ScoreAction::OpenPath(path));
            return;
        }
        self.state.open_bundled_score(
            cx,
            DEFAULT_SCORE,
            DEFAULT_SCORE_KIND,
            DEFAULT_SCORE_TITLE,
            DEFAULT_SCORE_CREDIT,
        );
        self.ui.redraw(cx);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let mut focus_gained = false;
        let mut focus_lost = false;
        for action in actions {
            if let Some(action) = action.downcast_ref::<ScoreAction>() {
                self.dispatch(cx, action);
            }
            // The native panel answers long after the click that opened it.
            if let Some(picked) = action.downcast_ref::<FileDialogAction>() {
                match picked {
                    FileDialogAction::FolderSelected(path) => {
                        let path = path.clone();
                        if self.state.ui.dialog == score_ui::DialogKind::Library {
                            // The library browses a folder; a file picked
                            // inside one means that folder.
                            let dir = if path.is_dir() {
                                path
                            } else {
                                path.parent().map(std::path::Path::to_path_buf).unwrap_or(path)
                            };
                            self.dispatch(cx, &ScoreAction::SetLibraryDir(dir.clone()));
                            self.ui
                                .text_input(cx, ids!(library_dir_input))
                                .set_text(cx, &dir.display().to_string());
                        } else if self.state.ui.dialog == score_ui::DialogKind::SaveAs {
                            let name = self
                                .state
                                .document
                                .suggested_save_path()
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "score.mpscore".to_string());
                            let target = if path.is_dir() { path.join(name) } else { path };
                            self.state.ui.status = format!("Save target {}", target.display());
                            self.ui
                                .text_input(cx, ids!(dialog_path))
                                .set_text(cx, &target.display().to_string());
                        } else {
                            self.dispatch(cx, &ScoreAction::OpenPath(path));
                        }
                    }
                    FileDialogAction::FolderCancelled => {
                        self.state.ui.status = "File panel cancelled".into();
                    }
                    FileDialogAction::None => {}
                }
                self.ui.redraw(cx);
            }
            if let Some(widget) = action.downcast_ref::<WidgetAction>() {
                match widget.action.downcast_ref::<TextInputAction>() {
                    Some(TextInputAction::KeyFocus) => focus_gained = true,
                    Some(TextInputAction::KeyFocusLost) => focus_lost = true,
                    _ => {}
                }
            }
        }
        if focus_gained {
            self.state.ui.text_input_focused = true;
        } else if focus_lost {
            self.state.ui.text_input_focused = false;
        }
    }

    fn handle_audio_devices(&mut self, cx: &mut Cx, event: &AudioDevicesEvent) {
        self.state.handle_audio_devices(cx, event);
    }

    fn handle_midi_ports(&mut self, cx: &mut Cx, event: &MidiPortsEvent) {
        self.state.handle_midi_ports(cx, event);
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        score_ui::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if !self.started && matches!(event, Event::Startup | Event::Draw(_)) {
            self.started = true;
            self.state.install_io(cx);
            self.open_initial_score(cx);
        }
        self.state.pump_midi();
        if let Event::KeyDown(key) = event {
            if let Some(action) = key_action(key, &self.state) {
                self.dispatch(cx, &action);
                return;
            }
        }
        self.match_event(cx, event);
        self.ui
            .handle_event(cx, event, &mut Scope::with_data(&mut self.state));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped piece has to be real music the application can actually
    /// open, and it has to be titled by the application: the MIDI titles
    /// itself after its first track, which is not the name of the piece.
    #[test]
    fn the_shipped_piece_opens_and_is_named_for_what_it_is() {
        let document = score_ui::ScoreDocument::open_bundled(
            DEFAULT_SCORE,
            DEFAULT_SCORE_KIND,
            DEFAULT_SCORE_TITLE,
        )
        .expect("the shipped Rondo alla Turca imports");
        assert_eq!(document.title(), DEFAULT_SCORE_TITLE);
        assert!(document.page_count() >= 1);
        assert!(document.path().is_none(), "a shipped piece has no file to save over");
    }

    /// A resource that cannot be read must cost the shipped piece and nothing
    /// else, so the fallback is the built-in demo score.
    #[test]
    fn unreadable_shipped_bytes_fall_back_rather_than_failing_to_start() {
        assert!(score_ui::ScoreDocument::open_bundled(
            b"not a midi file at all",
            DEFAULT_SCORE_KIND,
            DEFAULT_SCORE_TITLE,
        )
        .is_err());
        assert!(score_ui::ScoreDocument::demo().is_ok());
    }

    #[test]
    fn application_dsl_mounts_score_shell() {
        let mut cx = Cx::new(Box::new(|_cx: &mut Cx, _event: &Event| {}));
        let mut std = ();
        let mut vm = ScriptVm {
            host: &mut cx,
            std: &mut std,
            bx: Box::new(ScriptVmBase::new()),
        };
        vm.bx.captured_errors = Some(Vec::new());
        makepad_widgets::makepad_platform::script::script_mod(&mut vm);
        makepad_widgets::script_mod(&mut vm);
        score_ui::script_mod(&mut vm);
        super::script_mod(&mut vm);
        let errors = vm.take_errors();
        assert!(errors.is_empty(), "score application DSL errors: {errors:#?}");
    }
}
