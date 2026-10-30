//! The SCORE desk: one score, live-coded music (makepad-score-live), in
//! three areas under the window's caption (`SynthCaption`: the score's name
//! and state, its transport, the view toggles, save and the menu):
//!
//! - the code, each node a block with its lens under its first line
//!   (`code/`): the score as written and played with;
//! - the side panel's tabs: the AI chat, the Library (scores, samples and
//!   the batch of songs written at once: `lib_pane.rs`, `batch.rs`) and the
//!   score's own editor panel (`panel.rs`);
//! - the tracker across the bottom (`tracker.rs`: the loops as channels
//!   in time, tracker cells or piano rolls, each with its waveform, mix and
//!   instrument; seeking and loop ranges are its).
//!
//! One source of truth: the desk's source. Typing, the lenses, the panel's
//! controls and the AI all change it; each change that loads plays at once
//! (loops switch at their next cycle), and one that does not keeps the last
//! good score playing with its problems shown where they are. A control
//! under a hand sounds before it is written: it lands in the playing
//! graph, and its release is written.
//!
//! The score plays through the app's audio root (`crate::audio`): the
//! composer (`LiveScore`) on the engine's clock feeds the slot's player.

mod batch;
mod chat;
pub mod code;
mod lib_pane;
pub mod panel;
pub mod run;
pub mod score_doc;
mod tabs;
mod tracker;
mod tracker_desk;
mod tracker_model;
mod transport;
mod vary;

use crate::audio::{self, Engine};
use crate::library::Document;
use code::folds::{self, Folds};
use code::{LoopLive, Marks, ScoreEditor, ScoreEditorAction};
use makepad_code_editor::code_view::{CodeView, CodeViewAction};
use makepad_code_editor::{CodeDocument, CodeSession};
use makepad_editor_panel::EditorPanelWidgetRefExt;
use makepad_score_live::model::ScoreDoc;
use makepad_score_live::{source, LiveConfig, LiveScore, Status};
use makepad_widgets::*;
use makepad_widgets::value_input::ValueInputWidgetRefExt;
use tabs::{PaneTabs, TabsAction};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    let Pane = RoundedView{width: Fill height: Fill flow: Down spacing: 0 padding: 0 show_bg: true
        draw_bg +: {color: stage.panel border_color: stage.separator border_size: 1 border_radius: 6}}
    let PaneHead = View{width: Fill height: 38 flow: Right spacing: 6 padding: Inset{left: 10 right: 8} align: Align{y: 0.5}}
    let Rule = SolidView{width: Fill height: 1 draw_bg +: {color: stage.separator}}
    let Meta = Label{width: Fit text: "" draw_text +: {color: stage.muted text_style: theme.font_regular{font_size: 8}}}
    let Tool = StageButton{width: Fit height: 24 margin: 0 padding: Inset{left: 9 right: 9} spacing: 6
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}}
    let GlyphTool = StageButton{width: 26 height: 24 margin: 0 padding: 0 align: Align{x: 0.5 y: 0.5}
        draw_text +: {text_style: theme.font_code{font_size: 8.5}}}
    let MenuRow = StageButton{width: Fill height: 28 margin: 0 padding: Inset{left: 10 right: 10} align: Align{x: 0.0 y: 0.5}
        draw_bg +: {color: stage.menu color_hover: stage.control_hover border_size: 0.0}
        draw_text +: {text_style: theme.font_regular{font_size: 9}}}
    let MenuRule = SolidView{width: Fill height: 1 margin: Inset{top: 4 bottom: 4} draw_bg +: {color: stage.separator}}
    let Well = RoundedView{width: Fill height: Fit show_bg: true
        draw_bg +: {color: stage.well border_color: stage.line border_size: 1.0 border_radius: 3}}
    let WellInput = TextInput{width: Fill height: Fit margin: 0 padding: Inset{left: 7 right: 7 top: 5 bottom: 5}
        draw_bg +: {color: #0000 color_hover: #0000 color_focus: #0000 border_size: 0.0}
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}}

    let DeskSplit = Splitter{size: 8 draw_bg +: {color_bg: stage.bg color: stage.border_hi color_hover: stage.accent color_drag: stage.accent splitter_pad: 3 bar_size: 28}}

    // The caption's controls: flat icons that light while on (`lit_flat`).
    let CapIcon = StageIconButton{width: 28 height: 24 margin: 0 icon_walk: Walk{width: 13 height: Fit}
        draw_bg +: {color: #0000 color_focus: #0000 color_hover: stage.control_hover color_down: stage.control_down border_size: 0.0 border_radius: 4.0}
        draw_icon +: {color: stage.secondary}}
    let CapDivider = SolidView{width: 1 height: 14 margin: Inset{left: 6 right: 6} draw_bg +: {color: stage.separator}}

    // The window's caption (app-drawn on every platform, `main.rs` hangs
    // it in the window's caption bar): the score's name and state, its
    // transport, the view toggles, save and the menu. The window buttons
    // keep their place (the traffic lights' room at the left on macOS and
    // under a macOS-style WM, the buttons at the right on Windows and Linux);
    // the empty bar, the name and the state drag the window.
    mod.widgets.SynthCaption = View{width: Fill height: Fill flow: Right spacing: 0 padding: Inset{left: 8 right: 6} align: Align{y: 0.5}
        // The document. The two sides share the bar equally, so the
        // transport stays centred whatever the name and state say.
        View{width: Fill height: Fill flow: Right spacing: 7 align: Align{y: 0.5} clip_x: true
            Icon{icon_walk: Walk{width: 12 height: Fit} draw_icon +: {color: stage.faint svg: crate_resource("self:resources/icons/note.svg")}}
            score_name := Label{width: Fit text: "" draw_text +: {color: stage.text_hi text_style: theme.font_bold{font_size: 10}}}
            View{width: Fit height: Fit flow: Right spacing: 5 margin: Inset{left: 2} align: Align{y: 0.5}
                score_dot := RoundedView{visible: false width: 6 height: 6 show_bg: true draw_bg +: {color: stage.amber border_radius: 3.0}}
                score_state := Meta{}}
        }
        // The transport: play/pause, stop, where it is and its tempo.
        // Seeking and loops are the tracker's.
        score_transport := View{width: Fit height: Fill flow: Right spacing: 1 margin: Inset{left: 12 right: 12} align: Align{y: 0.5}
            Tip{text: "Play the score (Space)"
                score_play := CapIcon{icon_walk: Walk{width: 10 height: Fit} draw_icon +: {svg: crate_resource("self:resources/icons/play.svg")}}}
            Tip{text: "Pause: the transport holds its position (Space)"
                score_pause := CapIcon{visible: false icon_walk: Walk{width: 10 height: Fit} draw_icon +: {svg: crate_resource("self:resources/icons/pause.svg")}}}
            Tip{text: "Stop and return to the first bar"
                score_stop := CapIcon{icon_walk: Walk{width: 9 height: Fit} draw_icon +: {svg: crate_resource("self:resources/icons/stop.svg")}}}
            RoundedView{width: Fit height: 22 flow: Right spacing: 8 margin: Inset{left: 6} padding: Inset{left: 10 right: 2} align: Align{y: 0.5} show_bg: true
                draw_bg +: {color: stage.well border_color: stage.line_faint border_size: 1 border_radius: 4}
                score_pos := Label{width: 30 text: "1.1" draw_text +: {color: stage.text_hi text_style: theme.font_code{font_size: 9}}}
                score_time := Label{width: Fit text: "0:00" draw_text +: {color: stage.muted text_style: theme.font_code{font_size: 8}}}
                SolidView{width: 1 height: 12 draw_bg +: {color: stage.separator}}
                Tip{text: "Tempo: drag sideways to change it, click to type (writes the score's clock)"
                    score_bpm := ValueInput{width: 64 height: 18 min: 20 max: 400 step: 0.5 precision: 0 suffix: " bpm"
                        arrow_color: stage.faint
                        draw_bg +: {color: #0000 border_color: #0000}
                        draw_text +: {color: stage.secondary text_style: theme.font_code{font_size: 8 line_spacing: 1.0}}
                        text_input +: {draw_bg +: {border_size: 0.0} draw_text +: {text_style: theme.font_code{font_size: 8}}}}}
            }
        }
        // The view toggles, save and the menu.
        View{width: Fill height: Fill flow: Right spacing: 1 align: Align{x: 1.0 y: 0.5}
            score_fold_code_tip := Tip{text: ""
                score_fold_code := CapIcon{draw_icon +: {svg: crate_resource("self:resources/icons/fold.svg")}}}
            score_fold_lens_tip := Tip{text: ""
                score_fold_lens := CapIcon{draw_icon +: {svg: crate_resource("self:resources/icons/lens.svg")}}}
            score_side_tip := Tip{text: ""
                score_side := CapIcon{draw_icon +: {svg: crate_resource("self:resources/icons/side.svg")}}}
            CapDivider{}
            score_save_tip := Tip{text: "Save the score into the library (⌘S)"
                score_save := CapIcon{icon_walk: Walk{width: 12 height: Fit} draw_icon +: {svg: crate_resource("self:resources/icons/save.svg")}}}
            Tip{text: "New score, open, export, the song library and resources"
                score_more := CapIcon{icon_walk: Walk{width: 12 height: Fit} draw_icon +: {svg: crate_resource("self:resources/icons/more.svg")}}}
        }
    }

    // The desk: the code (with its lenses) and the side panel's tabs over
    // the tracker. Every bar drags; the side panel and the tracker fold.
    mod.widgets.SynthDesk = View{width: Fill height: Fill flow: Overlay
      View{width: Fill height: Fill flow: Down spacing: 8 padding: 8
        tracker_split := DeskSplit{width: Fill height: Fill axis: SplitterAxis.Vertical align: SplitterAlign.FromB(300) fold: SplitterCollapse.B min_horizontal: 240 max_horizontal: 160
          a: View{width: Fill height: Fill
            side_split := DeskSplit{width: Fill height: Fill axis: SplitterAxis.Horizontal align: SplitterAlign.FromB(380) fold: SplitterCollapse.B min_vertical: 420 max_vertical: 300
              a: Pane{
                // A score plays General MIDI programs the machine does not have.
                score_gm := View{visible: false width: Fill height: Fit flow: Right spacing: 8 padding: Inset{left: 12 right: 8 top: 6 bottom: 6} align: Align{y: 0.5}
                    show_bg: true draw_bg +: {color: stage.well}
                    Label{width: Fill height: Fit text: "This score plays General MIDI instruments (a @soundfont with no src), and the GM SoundFont is not installed: they are silent until it is."
                        draw_text +: {color: stage.text_hi text_style: theme.font_regular{font_size: 9}}}
                    Tip{text: "Open Resources to download FluidR3 GM (MIT, 148 MB)" score_gm_install := Tool{text: "Install the GM SoundFont…"}}
                }
                score_code := mod.widgets.ScoreEditor{}
              }
              // The side panel: the AI chat, the library, the editor panel.
              b: Pane{
                PaneHead{
                    Tip{text: "AI: ask it to write or change the score · Library: scores, samples and batches · Panel: the score's own synthesiser panel"
                        side_tabs := mod.widgets.PaneTabs{labels: ["AI" "Library" "Panel"]}}
                    View{width: Fill height: 1}
                    side_panel_tools := View{visible: false width: Fit height: Fit flow: Right spacing: 4 align: Align{y: 0.5}
                        Tip{text: "Fold the code down to what the editor panel edits (again: unfold)" score_focus := GlyphTool{width: 30 text: "{•}"}}
                        Tip{text: "Have the AI make the editor panel's tweaks" score_make_tool := Tool{text: "Make"}}}
                    side_lib_tools := View{visible: false width: Fit height: Fit flow: Right spacing: 4 align: Align{y: 0.5}
                        Tip{text: "Copy audio files and SoundFonts into the samples a score can play" lib_add := Tool{visible: false text: "Add…"}}}
                }
                Rule{}
                side_chat := View{width: Fill height: Fill
                    chat_panel := AiChatPanel{width: Fill height: Fill}}
                side_library := View{visible: false width: Fill height: Fill flow: Down
                    View{width: Fill height: Fit padding: Inset{left: 10 right: 10 top: 8 bottom: 4}
                        Tip{text: "Scores: click one to open it (drafts not saved first) · Samples: audio a score plays by name (double-click puts one at the cursor) · Batch: songs written at once"
                            lib_lanes := mod.widgets.PaneTabs{labels: ["Scores" "Samples" "Batch"]}}}
                    lib_browse := View{width: Fill height: Fill flow: Down
                        View{width: Fill height: Fit padding: Inset{left: 10 right: 10 top: 4 bottom: 8}
                            Well{lib_search := WellInput{empty_text: "Search scores"}}}
                        Rule{}
                        ScrollYView{width: Fill height: Fill padding: Inset{left: 6 right: 6 top: 4}
                            lib_list := mod.widgets.MusicSongs{empty_text: "Nothing here yet."}}
                    }
                    score_batch := View{visible: false width: Fill height: Fill flow: Down
                        ScrollYView{width: Fill height: Fill flow: Down spacing: 8 padding: Inset{left: 10 right: 10 top: 4 bottom: 10}
                            Well{batch_prompt := WellInput{empty_text: "Describe the songs: a mood, a scene, an idea"}}
                            Tip{text: "Style markers the songs follow (click to pick)"
                                batch_markers := mod.widgets.MusicChips{}}
                            View{width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5}
                                Tip{text: "How long each song is" batch_length := DropDown{width: 72 height: 24 margin: 0 labels: ["15 s" "30 s" "1 min" "2 min"]
                                    draw_text +: {text_style: theme.font_regular{font_size: 8}}}}
                                View{width: Fill height: 1}
                                Tip{text: "Rewrite the composers' learned rules from the songs you rated" batch_learn := Tool{text: "Learn"}}
                                Tip{text: "Five songs written at once by the chat's model, rendered as each arrives" batch_go := Tool{text: "Generate"}}
                            }
                            Rule{}
                            batch_songs := mod.widgets.MusicSongs{}
                            batch_rating := View{visible: false width: Fill height: Fit flow: Down spacing: 6
                                Rule{}
                                batch_card := Meta{width: Fill text: ""}
                                Tip{text: "The selected song's aspects: click once for liked, twice for disliked, again to clear (unrated says nothing)"
                                    batch_aspects := mod.widgets.MusicChips{}}
                                Well{batch_comment := WellInput{empty_text: "What you liked or did not, about this song"}}
                            }
                            View{width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5}
                                batch_note := Meta{width: Fill text: ""}
                                batch_stop := Tool{visible: false text: "Stop"}
                            }
                        }
                    }
                }
                side_editor := View{visible: false width: Fill height: Fill
                    score_panel := mod.widgets.EditorPanel{}}
              }
            }
          }
          // The tracker across the bottom, full width.
          b: Pane{score_tracker := mod.widgets.ScoreTracker{}}
        }
      }
        // The document menu, hung under ⋯.
        score_menu := View{visible: false width: Fill height: Fill align: Align{x: 1.0 y: 0.0} padding: Inset{top: 4 right: 8}
            score_menu_box := RoundedView{width: 280 height: Fit flow: Down padding: 4 show_bg: true
                draw_bg +: {color: stage.menu border_color: stage.border_hi border_size: 1.0 border_radius: 5.0}
                score_new := MenuRow{text: "New score"}
                score_save_as := MenuRow{text: "Save as…"}
                score_open_file := MenuRow{text: "Open a score file…"}
                score_export := MenuRow{text: "Export to WAV…"}
                MenuRule{}
                score_songs := MenuRow{text: "Song library…"}
                score_open_music := MenuRow{text: "Open a music file…"}
                score_resources := MenuRow{text: "Resources…"}
                score_about := MenuRow{text: "About Synth…"}
                MenuRule{}
                score_make := MenuRow{text: "Make the editor panel's tweaks (AI)"}
                score_menu_focus := MenuRow{text: "Fold code to the panel"}
                score_preview_main := MenuRow{text: "Previews on the main output  ✓"}
            }
        }
        resources_panel := mod.widgets.SynthResourcesPanel{}
        about_panel := mod.widgets.SynthAboutPanel{}
        song_library_panel := mod.widgets.SynthSongLibraryPanel{}
    }
}

pub fn register(vm: &mut ScriptVm) {
    code::script_mod(vm);
    tabs::script_mod(vm);
    tracker::script_mod(vm);
    batch::script_mod(vm);
    script_mod(vm);
}

/// The side panel's tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    #[default]
    Chat,
    Library,
    Editor,
}

impl Side {
    const ALL: [Side; 3] = [Side::Chat, Side::Library, Side::Editor];
}

/// How far ahead of the output the composer schedules, in seconds: more
/// than a UI frame and a device buffer, so a loop never starts late.
const LOOKAHEAD: f64 = 0.15;
/// Tempo changes closer together than this (seconds) are one undo step.
const BPM_JOIN: f64 = 0.8;
/// The pause after typing before the text is applied.
const SETTLE: f64 = 0.35;

/// An audition's render: (what it auditions, the clip and when, or why not).
type Audition = (String, Result<(Vec<f32>, Vec<f32>, f32, Option<f64>), String>);

/// The score the desk plays: its live engine, source and views.
pub struct Desk {
    pub engine: Engine,
    pub live: Option<LiveScore>,
    /// A score waiting to be handed to the slot.
    module: Option<ScoreDoc>,
    /// The source the desk holds (what the editor shows).
    pub source: String,
    /// Changes to the source, counted.
    pub revision: u64,
    /// The source last sent to the composer.
    sent: String,
    /// While a melody variant auditions: the score with that part played
    /// instead, what the composer plays (the document stays as it is); any
    /// change to the document ends it.
    pub variant_live: Option<String>,
    /// The source the composer plays now.
    played: String,
    pub doc: Option<ScoreDoc>,
    pub status: Status,
    /// The code view's text changed; applied after a short pause.
    typed: bool,
    timer: Timer,
    /// The desk's next turn (every frame while it plays).
    pub frame: NextFrame,
    /// A slow turn while nothing plays (the device's rate, the slot).
    idle: Timer,
    started: bool,
    /// The revision the code view's lenses and marks were made for (0:
    /// make them again).
    code_shown: u64,
    /// The loops' cycles the code view was last given.
    loops_shown: Vec<(u64, u64)>,
    /// The Code view's folds (per score).
    folds: Folds,
    /// The caption's document menu is open.
    menu: bool,
    /// The position's arrangement (built per revision).
    arrangement: transport::Arrangement,
    arranged: u64,
    /// The range of the arrangement looping (the tracker's loop range).
    looping: Option<(f64, f64)>,
    /// The tempo the transport was last given, and shown.
    bpm_written: Option<f64>,
    bpm_shown: Option<f64>,
    /// When the caption's tempo was last written (a scrub joins one undo step).
    bpm_typed_at: f64,
    /// The open score as a document.
    pub document: Document,
    /// The last splice gesture (a drag's steps are one undo group).
    gesture: u64,
    /// The score's editor panel (`panel.rs`).
    pub panel: panel::ScorePanel,
    /// The voice tree's data and the melody variants (`vary.rs`).
    pub vary: vary::VaryDesk,
    /// The last AI variants: how many, in seconds.
    pub vary_ai_made: Option<(usize, f64)>,
    /// The batch (`batch.rs`).
    pub batch: batch::BatchDesk,
    /// The tracker (`tracker_desk.rs`).
    tracker: tracker_desk::TrackerDesk,
    /// Previews (the instrument switch, the keys) play on the main output
    /// (off: only a headphone cue hears them).
    preview_main: bool,
    /// The picker's audition: the choice under it (key, since when), the
    /// one rendering or played, and the render's answer.
    audition: Option<(String, f64)>,
    auditioned: Option<String>,
    audition_rx: Option<std::sync::mpsc::Receiver<Audition>>,
    /// The code view's instrument picker: the node and the choice under
    /// its selection (auditioned like the panel's switch).
    code_preview: Option<(String, makepad_score_live::instruments::Choice)>,
    /// Background work of the tools (listening, rendering, loading samples).
    pub jobs: run::Jobs,
    /// The library tab.
    lib: lib_pane::LibPane,
    /// Samples and SoundFonts asked for by name (an arrival prepares the
    /// score again so its samplers find them).
    samples_asked: std::collections::HashSet<String>,
    /// The save button was last shown for a saved, unchanged score.
    save_clean: Option<bool>,
    /// Whether the GM SoundFont is installed (asked once, again when
    /// Resources installs or removes it).
    gm_known: Option<bool>,
    /// The side panel shows, on this tab.
    side_open: bool,
    side: Side,
}

impl Default for Desk {
    fn default() -> Self {
        Desk {
            engine: Engine::default(), live: None, module: None, source: String::new(), revision: 0, sent: String::new(), variant_live: None, played: String::new(),
            doc: None, status: Status::default(), typed: false, timer: Timer::default(), frame: NextFrame::default(), idle: Timer::default(),
            started: false, code_shown: 0, loops_shown: Vec::new(), folds: Folds::default(), menu: false, arrangement: transport::Arrangement::default(), arranged: 0, looping: None,
            bpm_written: None, bpm_shown: None, bpm_typed_at: f64::NEG_INFINITY, document: Document::default(), gesture: 0, panel: panel::ScorePanel::default(), vary: vary::VaryDesk::default(),
            vary_ai_made: None, batch: batch::BatchDesk::default(), tracker: Default::default(), preview_main: true, audition: None, auditioned: None, audition_rx: None, code_preview: None,
            jobs: run::Jobs::default(), lib: lib_pane::LibPane::default(), samples_asked: Default::default(), save_clean: None, gm_known: None,
            side_open: true, side: Side::Chat,
        }
    }
}

/// A pane toggle's face: lit (`accent_wash`, accent ink) while on.
fn lit(cx: &mut Cx, ui: &WidgetRef, id: &[LiveId], on: bool) {
    let p = makepad_look::palette::get(cx);
    let (face, hover, ink) = if on { (p.accent_wash, p.accent_wash, p.accent) } else { (p.control, p.control_hover, p.label) };
    let (face, hover, ink) = (Vec4f::from_u32(face), Vec4f::from_u32(hover), Vec4f::from_u32(ink));
    let mut button = ui.button(cx, id);
    script_apply_eval!(cx, button, {
        draw_bg +: {color: #(face) color_focus: #(face) color_hover: #(hover)}
        draw_icon +: {color: #(ink) color_focus: #(ink) color_hover: #(ink)}
        draw_text +: {color: #(ink) color_focus: #(ink) color_hover: #(ink)}
    });
    button.redraw(cx);
}

/// A caption toggle's face: lit (`accent_wash`, accent ink) while on, flat
/// (the caption's own ground, a face only under the pointer) while off.
fn lit_flat(cx: &mut Cx, ui: &WidgetRef, id: &[LiveId], on: bool) {
    let p = makepad_look::palette::get(cx);
    let (face, hover, ink) = if on { (p.accent_wash, p.accent_wash, p.accent) } else { (0, p.control_hover, p.secondary) };
    let (face, hover, ink) = (Vec4f::from_u32(face), Vec4f::from_u32(hover), Vec4f::from_u32(ink));
    let mut button = ui.button(cx, id);
    script_apply_eval!(cx, button, {
        draw_bg +: {color: #(face) color_focus: #(face) color_hover: #(hover)}
        draw_icon +: {color: #(ink) color_focus: #(ink) color_hover: #(ink)}
    });
    button.redraw(cx);
}

/// Set a `Tip`'s text when it changed.
fn set_tip(cx: &mut Cx, ui: &WidgetRef, id: &[LiveId], text: &str) {
    if let Some(mut tip) = ui.widget(cx, id).borrow_mut::<makepad_widgets::tip::Tip>() {
        if tip.text != text {
            tip.text = text.to_string();
        }
    }
}

/// The node a score's tempo is written to: its `Clock{}`.
fn clock_node(src: &str) -> Option<String> {
    source::outline(src).nodes.into_iter().find(|n| n.type_name == "Clock").map(|n| n.name)
}

/// The blank score a new document starts from.
fn blank() -> String {
    makepad_score_live::templates::template("blank").unwrap_or_default().to_string()
}

impl Desk {
    /// The first start: the document open at the last stop, else a blank one.
    pub fn start(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        if self.started {
            return;
        }
        self.started = true;
        self.ensure_live();
        let (document, source) = Document::last().filter(|(_, s)| !s.trim().is_empty()).unwrap_or_else(|| (Document::new("Untitled"), blank()));
        self.open_document(cx, ui, document, source);
        self.show_side(cx, ui, true, self.side);
        self.show_folds(cx, ui);
        if let Some(mut editor) = ui.widget(cx, ids!(score_code)).borrow_mut::<ScoreEditor>() {
            editor.set_meters(Some(self.engine.meters.clone()));
        }
        self.idle = cx.start_interval(0.25);
        self.frame = cx.new_next_frame();
    }

    /// Show or fold the side panel, on `side`.
    fn show_side(&mut self, cx: &mut Cx, ui: &WidgetRef, open: bool, side: Side) {
        self.side_open = open;
        self.side = side;
        ui.splitter(cx, ids!(side_split)).set_collapse(cx, if open { SplitterCollapse::None } else { SplitterCollapse::B });
        for (id, shown) in [(ids!(side_chat), Side::Chat), (ids!(side_library), Side::Library), (ids!(side_editor), Side::Editor)] {
            ui.view(cx, id).set_visible(cx, side == shown);
        }
        ui.view(cx, ids!(side_panel_tools)).set_visible(cx, side == Side::Editor);
        ui.view(cx, ids!(side_lib_tools)).set_visible(cx, side == Side::Library);
        if let Some(mut tabs) = ui.widget(cx, ids!(side_tabs)).borrow_mut::<PaneTabs>() {
            tabs.set_selected(cx, Side::ALL.iter().position(|s| *s == side).unwrap_or(0));
        }
        lit_flat(cx, ui, ids!(score_side), open);
        set_tip(cx, ui, ids!(score_side_tip), if open { "Hide the side panel: the AI chat, the library and the editor panel" } else { "Show the side panel: the AI chat, the library and the editor panel" });
        ui.redraw(cx);
    }

    /// The caption's fold-all toggles, as the folds are: the code toggle
    /// lit while every block's code is folded, the lens toggle while the
    /// lenses show.
    fn show_folds(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        lit_flat(cx, ui, ids!(score_fold_code), self.folds.code_all);
        lit_flat(cx, ui, ids!(score_fold_lens), !self.folds.lens_all);
        let code = if self.folds.code_all {
            "Unfold the code: every block shows its code again"
        } else {
            "Fold the code: every block shows only its lens, a rack of the code's own controls"
        };
        let lens = if self.folds.lens_all { "Show the lenses under each block's first line" } else { "Hide the lenses: only the code shows" };
        set_tip(cx, ui, ids!(score_fold_code_tip), code);
        set_tip(cx, ui, ids!(score_fold_lens_tip), lens);
    }

    /// Fold the code as the folds say, and (while the panel's focus is on)
    /// down to what the editor panel edits. Only what changed moves.
    pub(super) fn apply_folds(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        let outline = source::outline(&self.source);
        let mut target = folds::folded_lines(&self.source, &outline, &self.folds);
        if self.panel.focus {
            let keep = panel::ScorePanel::focus_lines(&self.source);
            let count = self.source.lines().count().max(1);
            target.extend((0..count).filter(|l| keep.binary_search(l).is_err()));
            target.sort_unstable();
            target.dedup();
        }
        if let Some(mut editor) = ui.widget(cx, ids!(score_code)).borrow_mut::<ScoreEditor>() {
            editor.fold_to(cx, &target);
        }
    }

    /// The folds of the score labelled `label` (loaded when it changes).
    fn folds_for(&mut self, label: &str) {
        let key = Folds::key_of(label);
        if key != self.folds.key() {
            self.folds = Folds::load(&key);
            self.code_shown = 0;
        }
    }

    /// A node's copy, named `<node>_2` (the next free), after the score's
    /// nodes and listed in it.
    fn duplicate(&mut self, cx: &mut Cx, ui: &WidgetRef, node: &str) {
        let outline = source::outline(&self.source);
        let Some(ctor) = outline.node(node) else { return };
        let text = &self.source[ctor.span.start..ctor.span.end];
        let Some(body) = text.find('=').map(|eq| text[eq + 1..].trim().to_string()) else { return };
        let base = node.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches('_');
        let name = (2..).map(|i| format!("{base}_{i}")).find(|n| outline.node(n).is_none()).unwrap_or_default();
        match source::insert_node(&self.source, &name, &body) {
            Ok(next) => self.set_source(cx, ui, next),
            Err(why) => ui.label(cx, ids!(score_state)).set_text(cx, &why),
        }
    }

    pub fn handle_actions(&mut self, cx: &mut Cx, ui: &WidgetRef, actions: &Actions) {
        let tabs = ui.widget(cx, ids!(side_tabs));
        for action in actions.filter_widget_actions_cast::<TabsAction>(tabs.widget_uid()) {
            if let TabsAction::Selected(index) = action {
                self.show_side(cx, ui, true, Side::ALL[index.min(2)]);
            }
        }
        if ui.button(cx, ids!(score_play)).clicked(actions) {
            self.transport(true);
            self.frame = cx.new_next_frame();
        }
        if ui.button(cx, ids!(score_pause)).clicked(actions) {
            self.transport(false);
        }
        if ui.button(cx, ids!(score_stop)).clicked(actions) {
            self.transport(false);
            self.seek(0.0, 0.0);
        }
        // The caption's tempo: written to the score's clock, a scrub's steps
        // one undo step, each sounding as it lands.
        if let Some(bpm) = ui.widget(cx, ids!(score_bpm)).as_value_input().changed(actions) {
            self.write_bpm(cx, ui, bpm);
        }
        if ui.button(cx, ids!(score_more)).clicked(actions) {
            self.set_menu(cx, ui, !self.menu);
        }
        if ui.button(cx, ids!(score_side)).clicked(actions) {
            let open = !self.side_open;
            self.show_side(cx, ui, open, self.side);
        }
        if ui.button(cx, ids!(score_fold_code)).clicked(actions) {
            let on = !self.folds.code_all;
            self.folds.set_all(true, on);
            self.folds_changed(cx, ui);
        }
        if ui.button(cx, ids!(score_fold_lens)).clicked(actions) {
            let on = !self.folds.lens_all;
            self.folds.set_all(false, on);
            self.folds_changed(cx, ui);
        }
        let picked = [ids!(score_new), ids!(score_save_as), ids!(score_open_file), ids!(score_export), ids!(score_songs), ids!(score_open_music), ids!(score_resources), ids!(score_about), ids!(score_make), ids!(score_menu_focus), ids!(score_preview_main)].into_iter().any(|id| ui.button(cx, id).clicked(actions));
        if picked {
            self.set_menu(cx, ui, false);
        }
        if ui.button(cx, ids!(score_new)).clicked(actions) {
            self.open_document(cx, ui, Document::new("Untitled"), blank());
        }
        if ui.button(cx, ids!(score_make)).clicked(actions) || ui.button(cx, ids!(score_make_tool)).clicked(actions) {
            self.show_side(cx, ui, true, Side::Editor);
            self.make_editor(cx);
        }
        // Fold the code down to what the panel edits, or back.
        if ui.button(cx, ids!(score_focus)).clicked(actions) || ui.button(cx, ids!(score_menu_focus)).clicked(actions) {
            self.panel.focus = !self.panel.focus;
            self.focus_code(cx, ui);
        }
        if ui.button(cx, ids!(score_preview_main)).clicked(actions) {
            self.preview_main = !self.preview_main;
            let on = self.preview_main;
            ui.button(cx, ids!(score_preview_main)).set_text(cx, if on { "Previews on the main output  ✓" } else { "Previews on the main output" });
        }
        // A hand in the panel: its edits land on the next turn.
        let panel = ui.widget(cx, ids!(score_panel)).as_editor_panel();
        if panel.touched(actions) {
            self.frame = cx.new_next_frame();
        }
        if panel.make_clicked(actions) {
            self.make_editor(cx);
        }
        if ui.widget(cx, ids!(score_panel)).button(cx, ids!(p_stop)).clicked(actions) {
            self.stop_writers(cx, ui);
        }
        self.handle_batch_actions(cx, ui, actions);
        self.handle_library_actions(cx, ui, actions);
        self.handle_tracker_actions(cx, ui, actions);
        // Typing: applied after a pause.
        let code = ui.widget(cx, ids!(score_code)).widget(cx, ids!(code));
        if actions.find_widget_action(code.widget_uid()).is_some_and(|action| matches!(action.cast::<CodeViewAction>(), CodeViewAction::Changed)) {
            self.typed = true;
            cx.stop_timer(self.timer);
            self.timer = cx.start_timeout(SETTLE);
        }
        let editor = ui.widget(cx, ids!(score_code));
        for action in actions.filter_widget_actions_cast::<ScoreEditorAction>(editor.widget_uid()) {
            match action {
                ScoreEditorAction::Splice { start, end, text, gesture, done } => {
                    // A drag's steps are one undo step; its lenses are made
                    // again as it starts and when it ends.
                    let continues = gesture == self.gesture;
                    self.gesture = gesture;
                    self.splice_joined(cx, ui, start, end, &text, continues);
                    if done || !continues {
                        self.code_shown = 0;
                    }
                }
                ScoreEditorAction::Replace { start, end, text } => {
                    // The editor's text as it is now, then the switch.
                    let now = editor.borrow::<ScoreEditor>().map(|e| e.text(cx)).unwrap_or_else(|| self.source.clone());
                    if end <= now.len() && start <= end && now.is_char_boundary(start) && now.is_char_boundary(end) {
                        let next = source::apply(&now, &source::Splice { start, end, text });
                        self.set_source(cx, ui, next);
                    }
                }
                ScoreEditorAction::Duplicate(node) => self.duplicate(cx, ui, &node),
                ScoreEditorAction::Fold { node, code } => {
                    self.folds.toggle(&node, code);
                    self.folds_changed(cx, ui);
                }
                ScoreEditorAction::Status(text) => ui.label(cx, ids!(score_state)).set_text(cx, &text),
                ScoreEditorAction::None => {}
            }
        }
    }

    /// Write `bpm` into the score's `Clock{}`; changes within a moment of
    /// each other (a scrub) join one undo step. A score without a clock
    /// keeps its tempo, and the field shows it again.
    fn write_bpm(&mut self, cx: &mut Cx, ui: &WidgetRef, bpm: f64) {
        let now = cx.seconds_since_app_start();
        let splice = clock_node(&self.source).and_then(|node| source::set_field(&self.source, &node, "bpm", &source::fmt_num(bpm, 1)).ok());
        match splice {
            Some(s) => {
                let joined = now - self.bpm_typed_at < BPM_JOIN;
                self.bpm_typed_at = now;
                self.splice_joined(cx, ui, s.start, s.end, &s.text, joined);
            }
            // The field shows the score's tempo again on the next turn.
            None => self.bpm_shown = None,
        }
    }

    /// The folds moved: the lenses, the code's folds and the toggles follow.
    fn folds_changed(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        self.code_shown = 0;
        self.show_folds(cx, ui);
        self.apply_folds(cx, ui);
        self.frame = cx.new_next_frame();
    }

    /// A splice from a gesture: a step of the same drag joins its undo step.
    fn splice_joined(&mut self, cx: &mut Cx, ui: &WidgetRef, start: usize, end: usize, text: &str, joined: bool) {
        if end > self.source.len() || start > end || !self.source.is_char_boundary(start) || !self.source.is_char_boundary(end) {
            return;
        }
        let next = source::apply(&self.source, &source::Splice { start, end, text: text.to_string() });
        let code = ui.widget(cx, ids!(score_code)).widget(cx, ids!(code));
        if let Some(mut view) = code.borrow_mut::<CodeView>() {
            if let Some(session) = view.session.as_mut() {
                let to_pos = |at: usize| {
                    let before = &self.source[..at];
                    let line = before.matches('\n').count();
                    let col = at - before.rfind('\n').map_or(0, |nl| nl + 1);
                    makepad_code_editor::text::Position { line_index: line, byte_index: col }
                };
                // As given: typing's insert would autoindent the splice.
                if joined {
                    session.document().replace_range_joined(to_pos(start), to_pos(end), text.into());
                } else {
                    session.document().replace_range(to_pos(start), to_pos(end), text.into());
                }
                session.handle_changes();
            }
            view.redraw(cx);
        }
        self.source = next;
        self.revision += 1;
        self.send();
        self.frame = cx.new_next_frame();
    }

    /// A splice (the panel's release, a library item), applied to the
    /// editor's text in place (so the caret, scroll and undo stay the
    /// editor's) and to the source, as its own undo step.
    pub fn splice(&mut self, cx: &mut Cx, ui: &WidgetRef, start: usize, end: usize, text: &str) {
        self.splice_joined(cx, ui, start, end, text, false);
    }

    /// The code view's marks: the problems at their lines, and the values
    /// the panel holds at the lines they will be written to.
    fn marks(&self) -> Marks {
        let mut marks = Marks::default();
        marks.chips = self.status.diags.iter().map(|d| (d.line.saturating_sub(1) as usize, d.message.clone())).collect();
        for l in &self.status.loops {
            if let Some(error) = &l.error {
                marks.chips.push((error.line.saturating_sub(1) as usize, format!("{}: {}", l.name, error.message)));
            }
        }
        for (path, value) in self.panel.held() {
            let line = match score_doc::resolve(&self.source, path) {
                Ok(site) => source::line_col(&self.source, site.range().start).0 - 1,
                // Not written yet: at its node's line.
                Err(_) => match source::outline(&self.source).node(path.split('.').next().unwrap_or("")) {
                    Some(node) => source::line_col(&self.source, node.span.start).0 - 1,
                    None => continue,
                },
            };
            marks.readouts.push((line, format!("= {value}  ({path}, held)")));
        }
        marks
    }

    /// The code view: its lenses and marks once per change, what plays
    /// every turn (the loops' notes only when a cycle turns).
    fn pump_code(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        let editor_ref = ui.widget(cx, ids!(score_code));
        let Some(mut editor) = editor_ref.borrow_mut::<ScoreEditor>() else { return };
        if self.code_shown != self.revision {
            self.code_shown = self.revision;
            if let Some(doc) = &self.doc {
                let p = makepad_look::palette::get(cx);
                // The lenses follow the last score that loaded; a source
                // that does not load keeps them where they were.
                let outline = source::outline(&self.source);
                if doc.source == self.source {
                    let lenses = code::lens::build(doc, &self.source, &outline, &self.folds, &p);
                    editor.set_lenses(cx, &self.source, lenses);
                }
                // What the completions offer beside the built-ins: the
                // samples, SoundFonts and singers this score names.
                let named = |list: Vec<makepad_score_live::model::AssetRef>| list.into_iter().map(|a| (a.id.chars().take(12).collect::<String>(), a)).collect();
                editor.set_available(makepad_score_live::instruments::Available { fonts: named(doc.files()), samples: named(doc.samples()), singers: named(doc.singers()), mine: Vec::new() });
            }
            editor.set_marks(cx, self.marks());
            drop(editor);
            self.apply_folds(cx, ui);
        } else {
            if !self.panel.held().is_empty() {
                editor.set_marks(cx, self.marks());
            }
            drop(editor);
        }
        let Some(mut editor) = editor_ref.borrow_mut::<ScoreEditor>() else { return };
        let status = &self.status;
        editor.set_clock(cx, status.playing, status.beat, status.section.as_deref());
        let cycles: Vec<(u64, u64)> = status.loops.iter().map(|l| (l.key, l.cycle)).collect();
        if cycles != self.loops_shown {
            self.loops_shown = cycles;
            let loops = status.loops.iter().map(|l| LoopLive {
                key: l.key, cycle: l.cycle, start: l.start, len: l.len, notes: l.notes.clone(), durs: l.durs.clone(), lyrics: l.lyrics.clone(), resting: l.resting,
                error: l.error.as_ref().map(|e| e.message.clone()),
            }).collect();
            editor.set_loops(cx, loops);
        }
        self.code_preview = editor.picker_choice();
    }

    /// The desk's turn: status in, the code view refreshed, jobs polled.
    /// Cheap when nothing changed.
    pub fn pump(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        self.jobs.poll();
        if let Some(message) = self.jobs.finished_message() {
            ui.label(cx, ids!(score_state)).set_text(cx, &message);
        }
        if self.jobs.take_loaded() > 0 {
            // Prepared again, its samplers find what arrived.
            if let Err(diags) = audio::slot().refresh() {
                log!("score: {}", diags.first().map(|d| d.message.as_str()).unwrap_or(""));
            }
        }
        self.pump_batch(cx, ui);
        self.pump_library(cx, ui);
        self.pump_gm(cx, ui);
        self.pump_audition(cx);
        self.pump_audio();
        let Some(live) = &self.live else { return };
        let status = live.status();
        let changed = status.revision != self.status.revision || status.diags != self.status.diags;
        self.status = status;
        if changed {
            self.code_shown = 0;
        }
        self.pump_panel(cx, ui);
        self.pump_caption(cx, ui);
        self.pump_transport(cx, ui);
        self.pump_code(cx, ui);
        self.pump_tracker(cx, ui);
        if self.status.playing || self.engine.playing() || self.jobs.busy() {
            self.frame = cx.new_next_frame();
        }
    }

    /// The caption's document: the title, the state, the save button.
    fn pump_caption(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        let p = makepad_look::palette::get(cx);
        let edited = self.document.edited(&self.source);
        let (dot, state, tip) = match (&self.document.path, edited) {
            _ if !self.status.diags.is_empty() => {
                let n = self.status.diags.len();
                (Some(p.danger), format!("{n} problem{}", if n == 1 { "" } else { "s" }), "The newest change does not load; the last good version plays (the problems are marked in the code)".to_string())
            }
            (None, _) if self.source == blank() => (None, "New".to_string(), "A new score".to_string()),
            (None, true) => (Some(p.amber), "Not saved".to_string(), "A draft that was never saved: it is kept as you work".to_string()),
            (None, false) => (None, "Not saved".to_string(), "Not in the library".to_string()),
            (Some(_), true) => (Some(p.amber), "Edited".to_string(), "Changed since it was saved".to_string()),
            (Some(_), false) => (None, "Saved".to_string(), "As saved in the library".to_string()),
        };
        let title = if self.document.title.is_empty() { "Untitled".to_string() } else { self.document.title.clone() };
        const TITLE_CHARS: usize = 32;
        let shown = if title.chars().count() > TITLE_CHARS { format!("{}…", title.chars().take(TITLE_CHARS - 1).collect::<String>()) } else { title.clone() };
        if ui.label(cx, ids!(score_name)).text() != shown {
            ui.label(cx, ids!(score_name)).set_text(cx, &shown);
        }
        if ui.label(cx, ids!(score_state)).text() != state {
            ui.label(cx, ids!(score_state)).set_text(cx, &state);
        }
        // The state's explanation rides on save (the name and the state are
        // the caption's drag area, so they carry no tip of their own).
        let key = if matches!(cx.os_type(), OsType::Macos | OsType::Ios(_)) { "⌘S" } else { "Ctrl+S" };
        set_tip(cx, ui, ids!(score_save_tip), &format!("Save the score into the library ({key}) · {state}: {tip}"));
        // The window's own title (the Dock, the task switcher, the WM's
        // window list) names the score.
        ui.window(cx, ids!(main_window)).set_title(cx, &format!("{title} · Makepad Synth"));
        let clean = self.document.path.is_some() && !edited;
        if self.save_clean != Some(clean) {
            self.save_clean = Some(clean);
            ui.button(cx, ids!(score_save)).set_enabled(cx, !clean);
        }
        if ui.view(cx, ids!(score_dot)).visible() != dot.is_some() {
            ui.view(cx, ids!(score_dot)).set_visible(cx, dot.is_some());
        }
        if let Some(dot) = dot {
            let color = Vec4f::from_u32(dot);
            let mut view = ui.widget(cx, ids!(score_dot));
            script_apply_eval!(cx, view, {draw_bg +: {color: #(color)}});
        }
    }

    /// The caption's transport: play/pause, the position and the tempo.
    fn pump_transport(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        if let Some(doc) = &self.doc {
            if self.arranged != self.revision {
                self.arranged = self.revision;
                let next = if doc.arrange.is_empty() {
                    // The longest cycle the loops play (as played, else as
                    // written), at least a bar.
                    let played = self.status.loops.iter().map(|l| l.len).fold(0.0, f64::max);
                    let written = doc.loops.iter().filter_map(|l| l.beats).fold(0.0, f64::max);
                    let beats = if played > 0.0 { played } else if written > 0.0 { written } else { 4.0 * doc.beats_per_bar() };
                    transport::Arrangement::cycle(doc, beats)
                } else {
                    transport::Arrangement::of(doc)
                };
                if next != self.arrangement {
                    self.arrangement = next;
                    // A loop of the old arrangement means nothing in the new one.
                    if self.looping.take().is_some() {
                        self.set_loop(None);
                    }
                }
            }
        }
        let playing = self.status.playing;
        if ui.button(cx, ids!(score_play)).visible() == playing {
            ui.button(cx, ids!(score_play)).set_visible(cx, !playing);
            ui.button(cx, ids!(score_pause)).set_visible(cx, playing);
            lit_flat(cx, ui, ids!(score_pause), true);
        }
        let a = &self.arrangement;
        let at = self.song_beat().unwrap_or(self.status.beat);
        let pos = transport::bar_beat(at, a.beats_per_bar.max(1.0));
        let time = format!("{} / {}", transport::clock(a.seconds_at(at)), transport::clock(a.seconds_at(a.beats())));
        if ui.label(cx, ids!(score_pos)).text() != pos {
            ui.label(cx, ids!(score_pos)).set_text(cx, &pos);
        }
        if ui.label(cx, ids!(score_time)).text() != time {
            ui.label(cx, ids!(score_time)).set_text(cx, &time);
        }
        // The tempo playing, else the score's own.
        let bpm = if self.status.playing && self.status.bpm > 0.0 { self.status.bpm } else { self.bpm_written.unwrap_or(self.status.bpm.max(1.0)) };
        if self.bpm_shown.is_none_or(|b| (b - bpm).abs() > 0.05) {
            self.bpm_shown = Some(bpm);
            let field = ui.widget(cx, ids!(score_bpm));
            if let Some(mut input) = field.borrow_mut::<makepad_widgets::value_input::ValueInput>() {
                // A whole tempo shows whole; one with a fraction shows it.
                input.precision = if (bpm - bpm.round()).abs() > 0.05 { 1.0 } else { 0.0 };
            }
            field.as_value_input().set_value(cx, bpm);
        }
    }
    /// The composer, feeding the app's slot.
    fn ensure_live(&mut self) {
        if self.live.is_some() {
            return;
        }
        self.sent.clear();
        self.played.clear();
        self.live = Some(LiveScore::new_on(LiveConfig { instance: audio::SLOT.into(), lookahead: LOOKAHEAD, clock: self.engine.clock() }, audio::TRANSPORT));
    }

    /// Resources installed or removed the GM SoundFont: asked again.
    pub fn gm_changed(&mut self) {
        self.gm_known = None;
        self.samples_asked.remove(makepad_score_live::gm_font::FONT);
    }

    /// The General MIDI SoundFont: loaded once installed for a score that
    /// plays it; until then the desk says so.
    fn pump_gm(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        use makepad_score_live::gm_font;
        let wants = self.doc.as_ref().is_some_and(ScoreDoc::plays_gm_font);
        let have = *self.gm_known.get_or_insert_with(gm_font::installed);
        if wants && have && !gm_font::registered() && self.samples_asked.insert(gm_font::FONT.to_string()) {
            self.jobs.spawn(|| gm_font::ensure().map(|()| makepad_strict_json::obj(vec![("loaded", makepad_strict_json::Value::Int(1))])));
        }
        let missing = wants && !have;
        if ui.view(cx, ids!(score_gm)).visible() != missing {
            ui.view(cx, ids!(score_gm)).set_visible(cx, missing);
        }
    }

    /// Save the open score into the library (its file, or a new one `name`).
    pub fn save(&mut self, name: Option<&str>) -> Result<std::path::PathBuf, String> {
        let source = self.source.clone();
        self.document.save(&source, name)
    }

    /// `document` open in the desk with `source`.
    pub fn open_document(&mut self, cx: &mut Cx, ui: &WidgetRef, document: Document, source: String) {
        self.document = document;
        self.lib.dirty = true;
        self.set_source(cx, ui, source);
        self.document.keep(&self.source);
    }

    /// Hand the source to the composer and the slot when it changed.
    pub fn send(&mut self) {
        // A change to the document ends an audition.
        if self.sent != self.source {
            self.variant_live = None;
        }
        let plays = self.variant_live.as_ref().unwrap_or(&self.source);
        if let Some(live) = &self.live {
            if self.played != *plays {
                live.set_source(plays.clone());
                self.played = plays.clone();
            }
        }
        if self.sent == self.source {
            return;
        }
        self.sent = self.source.clone();
        if let Ok(doc) = ScoreDoc::load(&self.source) {
            if !doc.label.is_empty() {
                self.document.title = doc.label.clone();
            }
            // The code view's folds follow the score (by its label).
            self.folds_for(&doc.label.clone());
            // The samples and SoundFonts it names, loaded once each.
            let missing: Vec<_> = makepad_score_live::render::missing_samples(&doc).into_iter().chain(makepad_score_live::render::missing_files(&doc))
                .filter(|asset| self.samples_asked.insert(makepad_score_live::player::sample_name(asset))).collect();
            if !missing.is_empty() {
                self.jobs.spawn(move || run::load_samples(missing));
            }
            self.module = Some(doc.clone());
            self.panel.stats.modules_sent += 1;
            if self.panel.runtime_holding() {
                self.panel.stats.modules_while_held += 1;
            }
            self.doc = Some(doc);
        }
        // The document's draft follows every change.
        self.document.keep(&self.source);
    }

    /// Replace the source (a template, an import, the AI, a lens): the code
    /// view follows as one undoable change.
    pub fn set_source(&mut self, cx: &mut Cx, ui: &WidgetRef, source: String) {
        if source == self.source {
            return;
        }
        self.source = source;
        self.revision += 1;
        self.typed = false;
        let code = ui.widget(cx, ids!(score_code)).widget(cx, ids!(code));
        if let Some(mut view) = code.borrow_mut::<CodeView>() {
            match view.session.as_mut() {
                Some(session) if session.document().as_text().to_string() != self.source => {
                    session.document().replace(self.source.as_str().into());
                    session.handle_changes();
                }
                Some(_) => {}
                None => {
                    let prepared = CodeDocument::prepare_for_path("score.splash", self.source.as_str().into());
                    view.session = Some(CodeSession::new(CodeDocument::from_prepared(prepared)));
                }
            }
            view.redraw(cx);
        }
        self.code_shown = 0;
        self.send();
        self.frame = cx.new_next_frame();
    }

    /// Play or stop the transport at the score's tempo.
    pub fn transport(&mut self, play: bool) {
        if play {
            if let Some(doc) = &self.doc {
                self.engine.set_bpm(makepad_score_live::render::transport_bpm(doc));
            }
        }
        self.engine.set_playing(play);
    }

    /// Play the arrangement from `beat` from the next `quant` boundary
    /// (stopped: the next play starts there).
    pub fn seek(&mut self, beat: f64, quant: f64) {
        if let Some(live) = &self.live {
            live.seek(beat, quant);
        }
    }

    /// Loop `range` of the arrangement (beats), or play on.
    fn set_loop(&mut self, range: Option<(f64, f64)>) {
        if let Some(live) = &self.live {
            live.set_loop(range);
        }
    }

    /// Open or close the caption's ⋯ menu.
    fn set_menu(&mut self, cx: &mut Cx, ui: &WidgetRef, open: bool) {
        self.menu = open;
        ui.view(cx, ids!(score_menu)).set_visible(cx, open);
        lit_flat(cx, ui, ids!(score_more), open);
        ui.redraw(cx);
    }

    pub fn handle_event(&mut self, cx: &mut Cx, ui: &WidgetRef, event: &Event) {
        match event {
            // ⌘S saves the score anywhere in the window.
            Event::KeyDown(ke) if ke.key_code == KeyCode::KeyS && (ke.modifiers.logo || ke.modifiers.control) && !ke.modifiers.shift => self.save_now(cx, ui),
            Event::KeyDown(ke) if ke.key_code == KeyCode::Escape && self.menu => self.set_menu(cx, ui, false),
            // Space plays or pauses the score, unless the code, a text field
            // or the panel's instrument search has the keys.
            Event::KeyDown(ke)
                if ke.key_code == KeyCode::Space
                    && !ke.is_repeat
                    && !(ke.modifiers.logo || ke.modifiers.control || ke.modifiers.alt || ke.modifiers.shift)
                    && !cx.text_input_has_focus()
                    && !makepad_synth_panel::host::searching() =>
            {
                let play = !self.status.playing;
                self.transport(play);
                self.frame = cx.new_next_frame();
            }
            // A press anywhere but the menu and ⋯ closes it.
            Event::MouseDown(me) if self.menu => {
                let menu = ui.widget(cx, ids!(score_menu_box)).area().rect(cx);
                let more = ui.widget(cx, ids!(score_more)).area().rect(cx);
                if !menu.contains(me.abs) && !more.contains(me.abs) {
                    self.set_menu(cx, ui, false);
                }
            }
            _ => {}
        }
        if self.timer.is_event(event).is_some() && self.typed {
            self.typed = false;
            let text = ui.widget(cx, ids!(score_code)).borrow::<ScoreEditor>().map(|editor| editor.text(cx));
            if let Some(text) = text {
                if text != self.source {
                    self.source = text;
                    self.revision += 1;
                    self.send();
                }
            }
        }
        if self.frame.is_event(event).is_some() || self.idle.is_event(event).is_some() || self.batch.timer.is_event(event).is_some() {
            self.pump(cx, ui);
        }
    }

    /// Between the desk and the audio: the score into the slot, the
    /// transport's tempo, the meters out.
    fn pump_audio(&mut self) {
        self.engine.pump();
        let slot = audio::slot();
        if let Some(doc) = self.module.take() {
            if let Err(diags) = slot.set_doc(doc) {
                log!("score: {}", diags.first().map(|d| d.message.as_str()).unwrap_or(""));
            }
        }
        if let Err(diags) = slot.pump() {
            log!("score: {}", diags.first().map(|d| d.message.as_str()).unwrap_or(""));
        }
        if let Some(doc) = &self.doc {
            // The score's tempo is the transport's (its tempo map rides on
            // the transport's feed from the composer).
            let bpm = makepad_score_live::render::transport_bpm(doc);
            if self.bpm_written.is_none_or(|w| (w - bpm).abs() > 1e-6) {
                self.bpm_written = Some(bpm);
                self.engine.set_bpm(bpm);
            }
            self.engine.read_meters(doc);
        }
    }

    /// Where the output is in the open score's arrangement (in its cycle,
    /// for a score of loops).
    pub fn song_beat(&self) -> Option<f64> {
        let a = &self.arrangement;
        if a.bands.is_empty() {
            return None;
        }
        let song = self.status.song_beat.unwrap_or_else(|| a.fold(self.status.beat));
        Some(if a.cycle { a.fold(song) } else { song })
    }

    /// The instrument switch's audition: a short phrase on the choice the
    /// panel's switch rests on, on the preview bus; and a key held on the
    /// panel's keyboard, the instrument as it is now.
    fn pump_audition(&mut self, cx: &mut Cx) {
        use makepad_score_live::audition;
        let held: Vec<makepad_score_live::held::Held> = self.panel.held().iter().filter_map(|(path, value)| {
            let (node, field) = makepad_score_live::held::node_field(path)?;
            Some(makepad_score_live::held::Held { at: 0.0, node: node.to_string(), field: field.to_string(), value: value.parse().ok()? })
        }).collect();
        // A key: the node's own sound on that note (with what the panel holds).
        let key = self.panel.key.take().and_then(|(node, note)| audition::key(&self.source, &node, note).map(|choice| (node, choice)));
        if let Some((node, choice)) = key {
            self.audition = None;
            self.panel.preview = None;
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let rendered = audition::render_key(&choice, &held, 48_000.0);
                let _ = tx.send((format!("key:{node}"), rendered.map(|(l, r, rate)| (l, r, rate, None))));
            });
            self.audition_rx = Some(rx);
            self.auditioned = Some("key".into());
        }
        // The panel's switch, else the code view's picker.
        let (choice, node) = match self.panel.preview.clone().or_else(|| self.code_preview.clone()) {
            Some((node, choice)) => (Some(choice), Some(node)),
            None => (None, None),
        };
        // Previews are heard on the headphone cue, or on the main output.
        let route = if audition::bus::has_cue() { audition::bus::Route::Cue } else if self.preview_main { audition::bus::Route::Main } else { audition::bus::Route::Off };
        audition::bus::set_route(route);
        let now = Cx::time_now();
        match &choice {
            Some(c) if route != audition::bus::Route::Off => {
                if self.audition.as_ref().is_none_or(|(k, _)| *k != c.key()) {
                    self.audition = Some((c.key(), now));
                    audition::bus::stop();
                }
            }
            _ => {
                if self.auditioned.as_deref().is_some_and(|a| a != "key") && self.auditioned.take().is_some() {
                    audition::bus::stop();
                }
                self.audition = None;
                if self.audition_rx.is_none() {
                    return;
                }
            }
        }
        audition::bus::set_gain(1.0);
        // Rested on for 150 ms: the choice in place, rendered off the UI
        // thread. Playing: the whole mix with it from a bar ahead, started
        // on that bar; stopped: its own loops, soloed.
        if let (Some(c), Some(node), Some((key, since))) = (&choice, &node, self.audition.clone()) {
            if now - since > 0.15 && self.auditioned.as_deref() != Some(key.as_str()) && self.audition_rx.is_none() {
                self.auditioned = Some(key.clone());
                let status = &self.status;
                let bar = self.doc.as_ref().map_or(4.0, |d| d.beats_per_bar());
                let bpm = status.bpm.max(20.0);
                let ahead = status.song_beat.filter(|_| status.playing).map(|song| {
                    let mut target = (song / bar).ceil() * bar;
                    if (target - song) * 60.0 / bpm < 0.35 {
                        target += bar;
                    }
                    (target, now + (target - song) * 60.0 / bpm)
                });
                let seconds = (2.0 * bar * 60.0 / bpm).min(8.0);
                let (source, node, c) = (self.source.clone(), node.clone(), c.clone());
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let rendered = match audition::in_place(&source, &node, &c, ahead.map(|(song, _)| song)) {
                        Some((candidate, start)) => audition::render_in_place(&candidate, start, seconds, 48_000.0),
                        None => audition::render_phrase(&c, 48_000.0),
                    };
                    let _ = tx.send((key, rendered.map(|(l, r, rate)| (l, r, rate, ahead.map(|(_, at)| at)))));
                });
                self.audition_rx = Some(rx);
            }
        }
        if let Some(rx) = &self.audition_rx {
            match rx.try_recv() {
                Ok((key, result)) => {
                    self.audition_rx = None;
                    let current = key.starts_with("key:") || self.audition.as_ref().is_some_and(|(k, _)| *k == key);
                    match result {
                        Ok((l, r, rate, at)) if current => audition::bus::play_after(&l, &r, rate, at.map_or(0.0, |at| at - Cx::time_now())),
                        Ok(_) => {}
                        Err(error) => log!("audition: {error}"),
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(_) => self.audition_rx = None,
            }
        }
        let waiting = self.audition.as_ref().is_some_and(|(k, _)| self.auditioned.as_deref() != Some(k.as_str()));
        if self.audition_rx.is_some() || waiting {
            self.frame = cx.new_next_frame();
        }
    }
}

