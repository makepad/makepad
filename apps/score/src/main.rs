//! Thin desktop frontend for `makepad-score-ui`.

use score_ui::font::ensure_default_font;
use score_ui::library::BundledPiece;
use score_ui::{apply_score_action, key_action, ScoreAction, ScoreAppState};
use makepad_widgets::*;
use std::path::PathBuf;

app_main!(App);

/// The performances the application carries.
///
/// These are not engravings: they are somebody sitting down and playing, with
/// their own dynamics, their own pedalling and their own rubato. That matters
/// more than it sounds like it should — an engraving says which notes, and a
/// modelled piano handed a page of identical velocities with the dampers
/// nailed down plays it exactly as mechanically as that describes. The
/// instrument only sounds like an instrument when it is given a performance.
///
/// Distributed unmodified under CC BY-SA 3.0; see
/// `resources/performances/LICENSE-piano-midi-de.txt`. The ShareAlike term
/// binds adaptations of these files and does not reach this application's own
/// source. [`PERFORMER_CREDIT`] is shown whenever one of them is opened.
const PERFORMER_CREDIT: &str = "Performed by Bernd Krueger · piano-midi.de · CC BY-SA 3.0";

const PERFORMANCES: &[BundledPiece] = &[
    BundledPiece {
        composer: "Bach",
        title: "Prelude No. 1 in C",
        credit: "Bach · Prelude No. 1 in C, BWV 846 · The Well-Tempered Clavier, Book I",
        attribution: Some(PERFORMER_CREDIT),
        extension: "mid",
        bytes: include_bytes!("../resources/performances/bach-wtc1-prelude1.mid"),
    },
    BundledPiece {
        composer: "Beethoven",
        title: "Moonlight Sonata",
        credit: "Beethoven · Piano Sonata No. 14, Op. 27 No. 2 · first movement",
        attribution: Some(PERFORMER_CREDIT),
        extension: "mid",
        bytes: include_bytes!("../resources/performances/beethoven-moonlight-1.mid"),
    },
    BundledPiece {
        composer: "Beethoven",
        title: "Für Elise",
        credit: "Beethoven · Bagatelle in A minor, WoO 59",
        attribution: Some(PERFORMER_CREDIT),
        extension: "mid",
        bytes: include_bytes!("../resources/performances/beethoven-fur-elise.mid"),
    },
    BundledPiece {
        composer: "Debussy",
        title: "Clair de lune",
        credit: "Debussy · Clair de lune · Suite bergamasque, third movement",
        attribution: Some(PERFORMER_CREDIT),
        extension: "mid",
        bytes: include_bytes!("../resources/performances/debussy-clair-de-lune.mid"),
    },
    BundledPiece {
        composer: "Chopin",
        title: "Nocturne in D flat",
        credit: "Chopin · Nocturne in D flat major, Op. 27 No. 2",
        attribution: Some(PERFORMER_CREDIT),
        extension: "mid",
        bytes: include_bytes!("../resources/performances/chopin-nocturne-op27-2.mid"),
    },
    BundledPiece {
        composer: "Chopin",
        title: "Raindrop Prelude",
        credit: "Chopin · Prelude in D flat major, Op. 28 No. 15",
        attribution: Some(PERFORMER_CREDIT),
        extension: "mid",
        bytes: include_bytes!("../resources/performances/chopin-raindrop-prelude.mid"),
    },
    BundledPiece {
        composer: "Schumann",
        title: "Träumerei",
        credit: "Schumann · Träumerei · Kinderszenen, Op. 15 No. 7",
        attribution: Some(PERFORMER_CREDIT),
        extension: "mid",
        bytes: include_bytes!("../resources/performances/schumann-traumerei.mid"),
    },
    BundledPiece {
        composer: "Liszt",
        title: "Liebestraum No. 3",
        credit: "Liszt · Liebestraum No. 3 in A flat, S541",
        attribution: Some(PERFORMER_CREDIT),
        extension: "mid",
        bytes: include_bytes!("../resources/performances/liszt-liebestraum.mid"),
    },
];

/// Which piece is on the desk at launch: the Bach prelude, played. A performance
/// is what introduces the instrument honestly.
const DEFAULT_PIECE: usize = 0;


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
        let piece = &PERFORMANCES[DEFAULT_PIECE];
        self.state.open_bundled_score(
            cx,
            piece.bytes,
            piece.extension,
            piece.title,
            piece.credit,
        );
        self.state.performance_credit = piece.attribution;
        if let Some(credit) = piece.attribution {
            self.state.ui.status = format!("{}   ·   {credit}", piece.credit);
        }
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
                    // The platform grew real file/save panels after this was
                    // written; the score still drives folder selection only.
                    _ => {}
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
        // BEFORE anything engraves. The spacing pass asks for the music font
        // the moment a document exists, and the application's own state builds
        // one during construction — registering the built-in font in the first
        // event would be a frame too late, and the font resolves exactly once.
        ensure_default_font();
        makepad_widgets::script_mod(vm);
        score_ui::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if !self.started && matches!(event, Event::Startup | Event::Draw(_)) {
            self.started = true;
            self.state.install_io(cx);
            self.state.library.set_bundled(PERFORMANCES);
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
        let piece = &PERFORMANCES[DEFAULT_PIECE];
        let document = score_ui::ScoreDocument::open_bundled(
            piece.bytes,
            piece.extension,
            piece.title,
        )
        .expect("the shipped default performance imports");
        assert_eq!(document.title(), piece.title);
        assert!(document.page_count() >= 1);
        assert!(document.path().is_none(), "a shipped piece has no file to save over");
    }

    /// A resource that cannot be read must cost the shipped piece and nothing
    /// else, so the fallback is the built-in demo score.
    #[test]
    fn unreadable_shipped_bytes_fall_back_rather_than_failing_to_start() {
        let piece = &PERFORMANCES[DEFAULT_PIECE];
        assert!(score_ui::ScoreDocument::open_bundled(
            b"not a midi file at all",
            piece.extension,
            piece.title,
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
