//! The SCORE desk: one score, live-coded music (makepad-score-live), seen
//! through four lenses: its own editor panel (a synthesiser UI written into
//! the score and run beside the code, `panel.rs`), the code (`editor.rs`),
//! the arrangement (`arrange.rs`) and the batch of songs written at once
//! (`batch.rs`), under a header with its transport (`transport.rs`).
//! One source of truth: the desk's source. Typing, the panel's controls,
//! the arrangement's gestures and the AI all change it; each change that
//! loads plays at once (loops switch at their next cycle), and one that
//! does not keeps the last good score playing with its problems shown where
//! they are. A control under a hand sounds before it is written: it lands
//! in the playing graph, and its release is written.
//!
//! The score plays through the app's audio root (`crate::audio`): the
//! composer (`LiveScore`) on the engine's clock feeds the slot's player.

mod arrange;
mod arrange_model;
mod batch;
mod chat;
mod editor;
mod lib_pane;
pub mod panel;
pub mod run;
pub mod score_doc;
mod transport;
mod vary;

use crate::audio::{self, Engine};
use crate::library::Document;
use editor::{EditorModel, ScoreEditor, ScoreEditorAction};
use makepad_code_editor::code_view::{CodeView, CodeViewAction};
use makepad_code_editor::{CodeDocument, CodeSession};
use makepad_editor_panel::EditorPanelWidgetRefExt;
use makepad_score_live::model::ScoreDoc;
use makepad_score_live::{source, LiveConfig, LiveScore, Status};
use makepad_widgets::*;
use transport::DrawScrub;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    let Pane = RoundedView{width: Fill height: Fill flow: Down spacing: 0 padding: 0 show_bg: true
        draw_bg +: {color: stage.panel border_color: stage.separator border_size: 1 border_radius: 5}}
    let PaneHead = View{width: Fill height: 34 flow: Right spacing: 6 padding: Inset{left: 12 right: 8} align: Align{y: 0.5}}
    let Rule = SolidView{width: Fill height: 1 draw_bg +: {color: stage.separator}}
    let Caps = Label{width: Fit text: "" draw_text +: {color: stage.secondary text_style: theme.font_bold{font_size: 7.5}}}
    let Meta = Label{width: Fit text: "" draw_text +: {color: stage.muted text_style: theme.font_regular{font_size: 8}}}
    let Tool = StageButton{width: Fit height: 22 margin: 0 padding: Inset{left: 8 right: 8} spacing: 6
        draw_text +: {text_style: theme.font_regular{font_size: 9}}}
    let IconTool = StageIconButton{margin: 0}
    let GlyphTool = StageButton{width: 24 height: 22 margin: 0 padding: 0 align: Align{x: 0.5 y: 0.5}
        draw_text +: {text_style: theme.font_code{font_size: 8.5}}}
    let Divider = SolidView{width: 1 height: 14 margin: Inset{left: 2 right: 2} draw_bg +: {color: stage.separator}}
    let MenuRow = StageButton{width: Fill height: 28 margin: 0 padding: Inset{left: 10 right: 10} align: Align{x: 0.0 y: 0.5}
        draw_bg +: {color: stage.menu color_hover: stage.control_hover border_size: 0.0}
        draw_text +: {text_style: theme.font_regular{font_size: 9}}}
    let MenuRule = SolidView{width: Fill height: 1 margin: Inset{top: 4 bottom: 4} draw_bg +: {color: stage.separator}}
    let Well = RoundedView{width: Fill height: Fit show_bg: true
        draw_bg +: {color: stage.well border_color: stage.line border_size: 1.0 border_radius: 2.5}}
    let WellInput = TextInput{width: Fill height: Fit margin: 0 padding: Inset{left: 6 right: 6 top: 4 bottom: 4}
        draw_bg +: {color: #0000 color_hover: #0000 color_focus: #0000 border_size: 0.0}
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}}

    mod.widgets.ScoreTabsBase = #(ScoreTabs::register_widget(vm))
    mod.widgets.ScoreTabs = set_type_default() do mod.widgets.ScoreTabsBase{
        width: 150 height: 22
        draw_text +: {color: stage.label text_style: theme.font_regular{font_size: 9}}
        draw_on +: {color: stage.text_hi text_style: theme.font_bold{font_size: 9}}
        icon_code +: {svg: crate_resource("self:resources/icons/text.svg")}
        icon_arrange +: {svg: crate_resource("self:resources/icons/timeline.svg")}
    }

    let DeskSplit = Splitter{size: 8 draw_bg +: {color_bg: stage.bg color: stage.border_hi color_hover: stage.accent color_drag: stage.accent splitter_pad: 3 bar_size: 24}}

    // The desk: the score's editor panel wide on the left, the code (or
    // the arrangement) in the middle, the batch on the right. Every bar
    // drags; the side panes fold.
    mod.widgets.SynthDesk = View{width: Fill height: Fill flow: Overlay
      View{width: Fill height: Fill flow: Down spacing: 8 padding: 8
        score_head := RoundedView{width: Fill height: 36 flow: Right spacing: 6 padding: Inset{left: 12 right: 8} align: Align{y: 0.5} show_bg: true
            draw_bg +: {color: stage.panel border_color: stage.separator border_size: 1 border_radius: 5}
            score_glyph := View{width: Fit height: Fit Icon{icon_walk: Walk{width: 13 height: Fit} draw_icon +: {color: stage.faint svg: crate_resource("self:resources/icons/note.svg")}}}
            score_name_tip := Tip{text: ""
                score_name := Label{width: Fit text: "" draw_text +: {color: stage.text_hi text_style: theme.font_bold{font_size: 10.5}}}}
            score_state_tip := Tip{text: "" View{width: Fit height: 18 flow: Right spacing: 6 margin: Inset{bottom: 3} align: Align{y: 0.5}
                score_dot := RoundedView{visible: false width: 8 height: 8 show_bg: true draw_bg +: {color: stage.amber border_radius: 4.0}}
                score_state := Meta{}}}
            // The score's transport: play/pause, stop, where it is, and
            // its timeline filling the free width (transport.rs).
            score_transport := View{width: Fill height: 22 flow: Right spacing: 2 margin: Inset{left: 8 right: 8} align: Align{y: 0.5}
                Tip{text: "Play the score (Space)"
                    score_play := IconTool{icon_walk: Walk{width: 10 height: Fit} draw_icon +: {svg: crate_resource("self:resources/icons/play.svg")}}}
                Tip{text: "Pause: the transport holds its position"
                    score_pause := IconTool{visible: false icon_walk: Walk{width: 10 height: Fit} draw_icon +: {svg: crate_resource("self:resources/icons/pause.svg")}}}
                Tip{text: "Stop and return to the first bar"
                    score_stop := IconTool{icon_walk: Walk{width: 9 height: Fit} draw_icon +: {svg: crate_resource("self:resources/icons/stop.svg")}}}
                score_position := Tip{text: "Where the score is: bar.beat, and the time on its tempo map of its length once through (of the cycle, for a score of loops)"
                    View{width: Fit height: 22 flow: Right spacing: 6 margin: Inset{left: 8 right: 6} align: Align{y: 0.5}
                        score_pos := Label{width: 30 margin: Inset{bottom: 2} text: "1.1" draw_text +: {color: stage.text_hi text_style: theme.font_bold{font_size: 9.5}}}
                        score_time := Label{width: 62 text: "0:00" draw_text +: {color: stage.muted text_style: theme.font_regular{font_size: 8}}}}}
                score_scrub_tip := Tip{text: "The arrangement: click or drag to move the playhead to a bar (from the next bar line); edit sections in Arrange"
                    score_scrub := mod.widgets.ScoreScrubber{width: Fill}}
            }
            Tip{text: "Code: the score as written · Arrange: its sections in time"
                score_tabs := mod.widgets.ScoreTabs{labels: ["Code" "Arrange"]}}
            score_folds := View{width: Fit height: Fit flow: Right spacing: 4 align: Align{y: 0.5}
                Tip{text: "Fold the code down to what the editor panel edits (again: unfold)" score_focus := GlyphTool{width: 30 text: "{•}"}}}
            Divider{margin: 0}
            score_save_tip := Tip{text: "Save the score into the library (⌘S)"
                score_save := IconTool{icon_walk: Walk{width: 11 height: Fit} draw_icon +: {svg: crate_resource("self:resources/icons/save.svg")}}}
            Tip{text: "The chat: ask the AI to write or change the score (its tools play and listen to it)" score_chat := Tool{text: "AI"}}
            Tip{text: "New score, the editor panel's tweaks, previews"
                score_more := IconTool{icon_walk: Walk{width: 11 height: Fit} draw_icon +: {svg: crate_resource("self:resources/icons/more.svg")}}}
        }
        chat_split := DeskSplit{width: Fill height: Fill axis: SplitterAxis.Horizontal align: SplitterAlign.FromB(380) fold: SplitterCollapse.B min_vertical: 360 max_vertical: 300
        a: View{width: Fill height: Fill
        s_split := DeskSplit{width: Fill height: Fill axis: SplitterAxis.Horizontal align: SplitterAlign.FromA(520) fold: SplitterCollapse.A min_vertical: 320 max_vertical: 420
          a: Pane{
            s_split_lib := DeskSplit{width: Fill height: Fill axis: SplitterAxis.Vertical align: SplitterAlign.FromB(240) fold: SplitterCollapse.B min_horizontal: 160 max_horizontal: 120
              // The score's editor panel: the voice tree, the selected
              // voice's timeline and variants, each instrument's tweaks.
              a: View{width: Fill height: Fill score_panel := mod.widgets.EditorPanel{}}
              // The library: the scores, and the samples a score plays.
              b: View{width: Fill height: Fill flow: Down
                PaneHead{
                    Caps{text: "LIBRARY"}
                    View{width: Fill height: 1}
                    Tip{text: "Copy audio files and SoundFonts into the samples a score can play" lib_add := Tool{visible: false text: "Add…"}}
                    Tip{text: "The scores: click one to open it; drafts not saved come first" lib_lane_scores := Tool{text: "Scores"}}
                    Tip{text: "Audio and SoundFonts a score plays by name: double-click puts one at the code's cursor" lib_lane_samples := Tool{text: "Samples"}}
                }
                Rule{}
                View{width: Fill height: Fit padding: Inset{left: 8 right: 8 top: 6 bottom: 6}
                    Well{lib_search := WellInput{empty_text: "Search scores"}}}
                Rule{}
                ScrollYView{width: Fill height: Fill padding: Inset{left: 4 right: 4 top: 4}
                    lib_list := mod.widgets.MusicSongs{empty_text: "Nothing here yet."}}
              }
            }
          }
          b: View{width: Fill height: Fill s_split_batch := DeskSplit{width: Fill height: Fill axis: SplitterAxis.Horizontal align: SplitterAlign.FromB(360) fold: SplitterCollapse.B min_vertical: 360 max_vertical: 240
            a: Pane{
                // A score plays General MIDI programs the machine does not have.
                score_gm := View{visible: false width: Fill height: Fit flow: Right spacing: 8 padding: Inset{left: 12 right: 8 top: 5 bottom: 5} align: Align{y: 0.5}
                    show_bg: true draw_bg +: {color: stage.well}
                    Label{width: Fill height: Fit text: "This score plays General MIDI instruments (a @soundfont with no src), and the GM SoundFont is not installed: they are silent until it is."
                        draw_text +: {color: stage.text_hi text_style: theme.font_regular{font_size: 9}}}
                    Tip{text: "Open Resources to download FluidR3 GM (MIT, 148 MB)" score_gm_install := Tool{text: "Install the GM SoundFont…"}}
                }
                score_code := mod.widgets.ScoreEditor{}
                score_arrange_box := View{width: Fill height: Fill visible: false
                    score_arrange := mod.widgets.ScoreArrange{}}
            }
            b: Pane{
                score_batch := View{width: Fill height: Fill flow: Down
                    PaneHead{
                        Caps{text: "BATCH"}
                        View{width: Fill height: 1}
                        Tip{text: "Rewrite the composers' learned rules from the songs you rated" batch_learn := Tool{text: "Learn"}}
                    }
                    Rule{}
                    ScrollYView{width: Fill height: Fill flow: Down spacing: 6 padding: Inset{left: 8 right: 8 top: 8 bottom: 8}
                        Well{batch_prompt := WellInput{empty_text: "Describe the songs: a mood, a scene, an idea"}}
                        Tip{text: "Style markers the songs follow (click to pick)"
                            batch_markers := mod.widgets.MusicChips{}}
                        View{width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5}
                            Tip{text: "How long each song is" batch_length := DropDown{width: 72 height: 22 margin: 0 labels: ["15 s" "30 s" "1 min" "2 min"]
                                draw_text +: {text_style: theme.font_regular{font_size: 8}}}}
                            View{width: Fill height: 1}
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
          }}
        }
        }
        // The chat: the model the person picked, with the score's tools.
        b: Pane{
            chat_panel := AiChatPanel{width: Fill height: Fill}
        }
        }
      }
        // The document menu, hung under ⋯.
        score_menu := View{visible: false width: Fill height: Fill align: Align{x: 1.0 y: 0.0} padding: Inset{top: 40 right: 8}
            score_menu_box := RoundedView{width: 280 height: Fit flow: Down padding: 4 show_bg: true
                draw_bg +: {color: stage.menu border_color: stage.border_hi border_size: 1.0 border_radius: 4.0}
                score_new := MenuRow{text: "New score"}
                score_save_as := MenuRow{text: "Save as…"}
                score_open_file := MenuRow{text: "Open a score file…"}
                score_export := MenuRow{text: "Export to WAV…"}
                MenuRule{}
                score_songs := MenuRow{text: "Song library…"}
                score_open_music := MenuRow{text: "Open a music file…"}
                score_resources := MenuRow{text: "Resources…"}
                MenuRule{}
                score_make := MenuRow{text: "Make the editor panel's tweaks (AI)"}
                score_menu_focus := MenuRow{text: "Fold code to the panel"}
                score_preview_main := MenuRow{text: "Previews on the main output  ✓"}
            }
        }
        resources_panel := mod.widgets.SynthResourcesPanel{}
        song_library_panel := mod.widgets.SynthSongLibraryPanel{}
    }
}

pub fn register(vm: &mut ScriptVm) {
    editor::script_mod(vm);
    arrange::script_mod(vm);
    batch::script_mod(vm);
    transport::script_mod(vm);
    script_mod(vm);
}

// ------------------------------------------------------------ view tabs

#[derive(Clone, Debug, Default)]
pub enum TabsAction {
    #[default]
    None,
    Selected(usize),
}

/// How much of each view segment shows: icon and label, the active one's
/// label only, or icons only (their names in the tooltip).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TabsDensity {
    #[default]
    Full,
    Compact,
    Icons,
}

/// The views as a segmented control: a well, the active segment in the one
/// picked look (`Palette::pick`), hover lifting the others; each segment
/// an icon and its name, narrowing as the header needs room.
#[derive(Script, ScriptHook, Widget)]
pub struct ScoreTabs {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawScrub,
    #[live]
    draw_box: DrawScrub,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_on: DrawText,
    #[live]
    icon_code: DrawSvg,
    #[live]
    icon_arrange: DrawSvg,
    #[live]
    labels: Vec<String>,
    #[rust]
    selected: usize,
    #[rust]
    hover: Option<usize>,
    #[rust]
    density: TabsDensity,
    #[rust]
    segs: Vec<Rect>,
    #[rust]
    area: Area,
}

/// A segment's padding either side, its icon and the gap to its name.
const SEG_PAD: f64 = 9.0;
const SEG_ICON: f64 = 11.0;
const SEG_GAP: f64 = 5.0;
/// An icon-only segment.
const SEG_ICON_ONLY: f64 = 28.0;

impl ScoreTabs {
    pub fn set_selected(&mut self, cx: &mut Cx, index: usize) {
        if self.selected != index {
            self.selected = index;
            self.redraw(cx);
        }
    }

    pub fn set_density(&mut self, cx: &mut Cx, density: TabsDensity) {
        if self.density != density {
            self.density = density;
            self.redraw(cx);
        }
    }

    /// The segment at `dx` from its left edge.
    fn at(&self, dx: f64) -> Option<usize> {
        self.segs.iter().position(|r| dx >= r.pos.x && dx < r.pos.x + r.size.x)
    }

    fn labelled(&self, i: usize) -> bool {
        match self.density {
            TabsDensity::Full => true,
            TabsDensity::Compact => i == self.selected,
            TabsDensity::Icons => false,
        }
    }
}

impl Widget for ScoreTabs {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // Where it is now: the header's Fill moves it after it was walked,
        // so the segments are kept relative to its left edge.
        let left = self.area.rect(cx).pos.x;
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                cx.set_cursor(MouseCursor::Hand);
                let hover = self.at(fe.abs.x - left);
                if hover != self.hover {
                    self.hover = hover;
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hover = None;
                self.redraw(cx);
            }
            Hit::FingerUp(fe) if fe.is_over && !fe.cancelled => {
                if let Some(index) = self.at(fe.abs.x - left) {
                    self.selected = index;
                    self.redraw(cx);
                    cx.widget_action(self.uid, TabsAction::Selected(index));
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let p = makepad_look::palette::get(cx);
        // Its width is its segments'.
        let labels = self.labels.clone();
        let widths: Vec<f64> = labels.iter().enumerate().map(|(i, label)| {
            if self.labelled(i) {
                let text = if i == self.selected { &self.draw_on } else { &self.draw_text };
                SEG_PAD * 2.0 + SEG_ICON + SEG_GAP + makepad_widgets::badge::advance(text, cx, label)
            } else {
                SEG_ICON_ONLY
            }
        }).collect();
        let total: f64 = widths.iter().sum::<f64>() + 2.0;
        let rect = cx.walk_turtle(Walk { width: Size::Fixed(total), ..walk });
        self.draw_bg.shape = 0.0;
        self.draw_bg.radius = 2.5;
        self.draw_bg.fill = Vec4f::from_u32(p.well);
        self.draw_bg.edge = Vec4f::from_u32(p.line);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        self.draw_box.shape = 0.0;
        self.segs.clear();
        let mut x = rect.pos.x + 1.0;
        for (i, label) in labels.iter().enumerate() {
            let seg = Rect { pos: dvec2(x, rect.pos.y + 1.0), size: dvec2(widths[i], rect.size.y - 2.0) };
            x += widths[i];
            self.segs.push(Rect { pos: dvec2(seg.pos.x - rect.pos.x, 0.0), size: seg.size });
            let active = i == self.selected;
            if active || self.hover == Some(i) {
                self.draw_box.radius = 2.0;
                let look = p.pick(active, true, p.well);
                self.draw_box.fill = Vec4f::from_u32(look.fill);
                self.draw_box.edge = Vec4f::from_u32(if active { look.edge } else { look.fill });
                self.draw_box.draw_abs(cx, seg);
            }
            let ink = Vec4f::from_u32(if active { p.accent } else if self.hover == Some(i) { p.text_hi } else { p.label });
            let icon_x = if self.labelled(i) { seg.pos.x + SEG_PAD } else { seg.pos.x + (seg.size.x - SEG_ICON) * 0.5 };
            let icon = match i {
                0 => &mut self.icon_code,
                _ => &mut self.icon_arrange,
            };
            icon.color = ink;
            icon.draw_abs(cx, Rect { pos: dvec2(icon_x, seg.pos.y + (seg.size.y - SEG_ICON) * 0.5), size: dvec2(SEG_ICON, SEG_ICON) });
            if self.labelled(i) {
                let text = if active { &mut self.draw_on } else { &mut self.draw_text };
                text.draw_abs(cx, dvec2(icon_x + SEG_ICON + SEG_GAP, seg.pos.y + (seg.size.y - 12.0) * 0.5), label);
            }
        }
        DrawStep::done()
    }
}

// ------------------------------------------------------------ the desk

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Code,
    Arrange,
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::Code, Mode::Arrange];
}

/// How far ahead of the output the composer schedules, in seconds: more
/// than a UI frame and a device buffer, so a loop never starts late.
const LOOKAHEAD: f64 = 0.15;
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
    pub mode: Mode,
    /// The code view's text changed; applied after a short pause.
    typed: bool,
    timer: Timer,
    /// The desk's next turn (every frame while it plays).
    pub frame: NextFrame,
    /// A slow turn while nothing plays (the device's rate, the slot).
    idle: Timer,
    started: bool,
    /// The revision the code view's marks were made for.
    code_shown: u64,
    /// The header's document menu is open; the header's collapse level.
    menu: bool,
    collapse: u8,
    /// The playback bar: the arrangement (built per revision) and the
    /// range of it looping.
    arrangement: transport::Arrangement,
    arranged: u64,
    looping: Option<(f64, f64)>,
    /// The tempo the transport was last given.
    bpm_written: Option<f64>,
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
    /// The Arrange view: what each section plays (the arrangement played
    /// through off the UI thread) for a revision, the job making the next,
    /// and the (revision, notes' revision) its model was built for.
    arrange_notes: (u64, arrange_model::ClipNotes),
    arrange_rx: Option<(u64, std::sync::mpsc::Receiver<arrange_model::ClipNotes>)>,
    arrange_built: (u64, u64),
    /// Previews (the instrument switch, the keys) play on the main output
    /// (off: only a headphone cue hears them).
    preview_main: bool,
    /// The picker's audition: the choice under it (key, since when), the
    /// one rendering or played, and the render's answer.
    audition: Option<(String, f64)>,
    auditioned: Option<String>,
    audition_rx: Option<std::sync::mpsc::Receiver<Audition>>,
    /// Background work of the tools (listening, rendering, loading samples).
    pub jobs: run::Jobs,
    /// The library pane.
    lib: lib_pane::LibPane,
    /// Samples and SoundFonts asked for by name (an arrival prepares the
    /// score again so its samplers find them).
    samples_asked: std::collections::HashSet<String>,
    /// The save button was last shown for a saved, unchanged score.
    save_clean: Option<bool>,
    /// Whether the GM SoundFont is installed (asked once, again when
    /// Resources installs or removes it).
    gm_known: Option<bool>,
    /// The chat column shows.
    chat_open: bool,
}

impl Default for Desk {
    fn default() -> Self {
        Desk {
            engine: Engine::default(), live: None, module: None, source: String::new(), revision: 0, sent: String::new(), variant_live: None, played: String::new(),
            doc: None, status: Status::default(), mode: Mode::Code, typed: false, timer: Timer::default(), frame: NextFrame::default(), idle: Timer::default(),
            started: false, code_shown: 0, menu: false, collapse: 0, arrangement: transport::Arrangement::default(), arranged: 0, looping: None, bpm_written: None,
            document: Document::default(), gesture: 0, panel: panel::ScorePanel::default(), vary: vary::VaryDesk::default(), vary_ai_made: None,
            batch: batch::BatchDesk::default(), arrange_notes: (0, Default::default()), arrange_rx: None, arrange_built: (0, 0), preview_main: true,
            audition: None, auditioned: None, audition_rx: None, jobs: run::Jobs::default(), chat_open: false, lib: lib_pane::LibPane::default(), samples_asked: Default::default(), save_clean: None, gm_known: None,
        }
    }
}

/// A header toggle's face: lit (`accent_wash`, accent ink) while on.
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
        self.set_mode(cx, ui, self.mode);
        self.show_chat(cx, ui, false);
        self.idle = cx.start_interval(0.25);
        self.frame = cx.new_next_frame();
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

    /// Show or fold the chat column.
    fn show_chat(&mut self, cx: &mut Cx, ui: &WidgetRef, open: bool) {
        self.chat_open = open;
        ui.splitter(cx, ids!(chat_split)).set_collapse(cx, if open { SplitterCollapse::None } else { SplitterCollapse::B });
        lit(cx, ui, ids!(score_chat), open);
        ui.redraw(cx);
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

    /// A splice (a scrubbed number, the panel's release, a switch), applied
    /// to the editor's text in place (so the caret, scroll and undo stay the
    /// editor's) and to the source.
    pub fn splice(&mut self, cx: &mut Cx, ui: &WidgetRef, start: usize, end: usize, text: &str) {
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
                session.document().replace_range(to_pos(start), to_pos(end), text.into());
                session.handle_changes();
            }
            view.redraw(cx);
        }
        self.source = next;
        self.revision += 1;
        self.send();
        self.frame = cx.new_next_frame();
    }

    /// The code view's marks: the problems at their lines, and the values
    /// the panel holds at the lines they will be written to.
    fn model(&self) -> EditorModel {
        let mut model = EditorModel { source: self.source.clone(), ..EditorModel::default() };
        model.chips = self.status.diags.iter().map(|d| (d.line.saturating_sub(1) as usize, d.message.clone())).collect();
        for l in &self.status.loops {
            if let Some(error) = &l.error {
                model.chips.push((error.line.saturating_sub(1) as usize, format!("{}: {}", l.name, error.message)));
            }
        }
        for (path, value) in self.panel.held() {
            let line = match score_doc::resolve(&self.source, path) {
                Ok(site) => source::line_col(&self.source, site.range.start).0 - 1,
                // Not written yet: at its node's line.
                Err(_) => match source::outline(&self.source).node(path.split('.').next().unwrap_or("")) {
                    Some(node) => source::line_col(&self.source, node.span.start).0 - 1,
                    None => continue,
                },
            };
            model.readouts.push((line, format!("= {value}  ({path}, held)")));
        }
        model
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

    fn set_loop(&mut self, range: Option<(f64, f64)>) {
        if let Some(live) = &self.live {
            live.set_loop(range);
        }
    }

    fn set_mode(&mut self, cx: &mut Cx, ui: &WidgetRef, mode: Mode) {
        self.mode = mode;
        for (id, shown) in [(live_id!(score_code), Mode::Code), (live_id!(score_arrange_box), Mode::Arrange)] {
            ui.widget(cx, &[id]).set_visible(cx, mode == shown);
        }
        if let Some(mut tabs) = ui.widget(cx, ids!(score_tabs)).borrow_mut::<ScoreTabs>() {
            tabs.set_selected(cx, Mode::ALL.iter().position(|m| *m == mode).unwrap_or(0));
        }
        self.code_shown = 0;
        self.frame = cx.new_next_frame();
        ui.redraw(cx);
    }

    pub fn handle_actions(&mut self, cx: &mut Cx, ui: &WidgetRef, actions: &Actions) {
        let tabs = ui.widget(cx, ids!(score_tabs));
        for action in actions.filter_widget_actions_cast::<TabsAction>(tabs.widget_uid()) {
            if let TabsAction::Selected(index) = action {
                self.set_mode(cx, ui, Mode::ALL[index.min(1)]);
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
        let scrub = ui.widget(cx, ids!(score_scrub));
        for action in actions.filter_widget_actions_cast::<transport::ScrubAction>(scrub.widget_uid()) {
            if let transport::ScrubAction::Seek(beat) = action {
                let quant = if self.status.playing { -1.0 } else { 0.0 };
                self.seek(beat, quant);
            }
        }
        if ui.button(cx, ids!(score_more)).clicked(actions) {
            self.set_menu(cx, ui, !self.menu);
        }
        if ui.button(cx, ids!(score_chat)).clicked(actions) {
            let open = !self.chat_open;
            self.show_chat(cx, ui, open);
        }
        let picked = [ids!(score_new), ids!(score_save_as), ids!(score_open_file), ids!(score_export), ids!(score_songs), ids!(score_open_music), ids!(score_resources), ids!(score_make), ids!(score_menu_focus), ids!(score_preview_main)].into_iter().any(|id| ui.button(cx, id).clicked(actions));
        if picked {
            self.set_menu(cx, ui, false);
        }
        if ui.button(cx, ids!(score_new)).clicked(actions) {
            self.open_document(cx, ui, Document::new("Untitled"), blank());
        }
        if ui.button(cx, ids!(score_make)).clicked(actions) {
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
        // Typing: applied after a pause.
        let code = ui.widget(cx, ids!(score_code)).widget(cx, ids!(code));
        if actions.find_widget_action(code.widget_uid()).is_some_and(|action| matches!(action.cast::<CodeViewAction>(), CodeViewAction::Changed)) {
            self.typed = true;
            cx.stop_timer(self.timer);
            self.timer = cx.start_timeout(SETTLE);
        }
        let editor = ui.widget(cx, ids!(score_code));
        for action in actions.filter_widget_actions_cast::<ScoreEditorAction>(editor.widget_uid()) {
            if let ScoreEditorAction::Splice { start, end, text, gesture, done } = action {
                // The marks are made again as a scrub starts and when it ends.
                let starts = gesture != self.gesture;
                self.gesture = gesture;
                self.splice(cx, ui, start, end, &text);
                if done || starts {
                    self.code_shown = 0;
                }
            }
        }
        let arrange = ui.widget(cx, ids!(score_arrange));
        for action in actions.filter_widget_actions_cast::<arrange::ScoreArrangeAction>(arrange.widget_uid()) {
            match action {
                arrange::ScoreArrangeAction::Splice { start, end, text } => {
                    let next = source::apply(&self.source, &source::Splice { start, end, text });
                    self.set_source(cx, ui, next);
                }
                arrange::ScoreArrangeAction::Seek(beat) => {
                    let quant = if self.status.playing { -1.0 } else { 0.0 };
                    self.seek(beat, quant);
                }
                arrange::ScoreArrangeAction::Status(text) => ui.label(cx, ids!(score_state)).set_text(cx, &text),
                arrange::ScoreArrangeAction::Voice(name) => {
                    makepad_synth_panel::host::select(&name);
                    makepad_editor_panel::changed();
                    ui.widget(cx, ids!(score_panel)).redraw(cx);
                }
                arrange::ScoreArrangeAction::None => {}
            }
        }
    }

    /// Open or close the header's ⋯ menu.
    fn set_menu(&mut self, cx: &mut Cx, ui: &WidgetRef, open: bool) {
        self.menu = open;
        ui.view(cx, ids!(score_menu)).set_visible(cx, open);
        lit(cx, ui, ids!(score_more), open);
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

    /// The desk's turn: status in, lenses refreshed, jobs polled. Cheap
    /// when nothing changed.
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
        self.pump_header(cx, ui);
        self.pump_transport(cx, ui);
        match self.mode {
            Mode::Code => {
                // The marks (problems, held values) when they change.
                if self.code_shown != self.revision || !self.panel.held().is_empty() {
                    self.code_shown = self.revision;
                    let model = self.model();
                    // What the completions offer beside the built-ins: the
                    // samples, SoundFonts and singers this score names.
                    let available = self.doc.as_ref().map(|doc| {
                        let named = |list: Vec<makepad_score_live::model::AssetRef>| list.into_iter().map(|a| (a.id.chars().take(12).collect::<String>(), a)).collect();
                        makepad_score_live::instruments::Available { fonts: named(doc.files()), samples: named(doc.samples()), singers: named(doc.singers()), mine: Vec::new() }
                    }).unwrap_or_default();
                    if let Some(mut editor) = ui.widget(cx, ids!(score_code)).borrow_mut::<ScoreEditor>() {
                        editor.set_available(available);
                        editor.set_model(cx, model);
                    }
                }
            }
            Mode::Arrange => {
                // What each section plays, again when the source changes.
                if let Some((revision, rx)) = &self.arrange_rx {
                    if let Ok(notes) = rx.try_recv() {
                        self.arrange_notes = (*revision, notes);
                        self.arrange_rx = None;
                    }
                }
                if self.arrange_notes.0 != self.revision && self.arrange_rx.as_ref().is_none_or(|(r, _)| *r != self.revision) {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let source = self.source.clone();
                    std::thread::spawn(move || {
                        let _ = tx.send(arrange_model::clip_notes(&source));
                    });
                    self.arrange_rx = Some((self.revision, rx));
                }
                if self.arrange_rx.is_some() {
                    self.frame = cx.new_next_frame();
                }
                let built = (self.revision, self.arrange_notes.0);
                // A score that does not load keeps the last model.
                let model = self.doc.as_ref().filter(|_| self.arrange_built != built).map(|doc| arrange_model::build(doc, &self.source, self.arrange_notes.1.clone()));
                if model.is_some() {
                    self.arrange_built = built;
                }
                let live = arrange::ArrangeLive { song_beat: self.song_beat(), playing: self.status.playing };
                if let Some(mut view) = ui.widget(cx, ids!(score_arrange)).borrow_mut::<arrange::ScoreArrange>() {
                    if let Some(model) = model {
                        view.set_meters(self.engine.meters.clone());
                        view.set_model(cx, model);
                    }
                    view.set_live(cx, live);
                }
            }
        }
        if self.status.playing || self.engine.playing() || self.jobs.busy() {
            self.frame = cx.new_next_frame();
        }
    }

    /// The header: the title, the state, the views, collapsing as it narrows.
    fn pump_header(&mut self, cx: &mut Cx, ui: &WidgetRef) {
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
        let width = ui.widget(cx, ids!(score_head)).area().rect(cx).size.x;
        let fit = transport::header_fit(width, &title, &state);
        let shown = if fit.title_chars < title.chars().count() { format!("{}…", title.chars().take(fit.title_chars.saturating_sub(1)).collect::<String>()) } else { title.clone() };
        if ui.label(cx, ids!(score_name)).text() != shown {
            ui.label(cx, ids!(score_name)).set_text(cx, &shown);
        }
        if let Some(mut name_tip) = ui.widget(cx, ids!(score_name_tip)).borrow_mut::<makepad_widgets::tip::Tip>() {
            name_tip.text = title;
        }
        let label = if fit.level >= 2 { String::new() } else { state };
        if ui.label(cx, ids!(score_state)).text() != label {
            ui.label(cx, ids!(score_state)).set_text(cx, &label);
        }
        if let Some(mut state_tip) = ui.widget(cx, ids!(score_state_tip)).borrow_mut::<makepad_widgets::tip::Tip>() {
            state_tip.text = tip;
        }
        let clean = self.document.path.is_some() && !edited;
        if self.save_clean != Some(clean) {
            self.save_clean = Some(clean);
            ui.button(cx, ids!(score_save)).set_enabled(cx, !clean);
        }
        ui.view(cx, ids!(score_dot)).set_visible(cx, dot.is_some());
        if let Some(dot) = dot {
            let color = Vec4f::from_u32(dot);
            let mut view = ui.widget(cx, ids!(score_dot));
            script_apply_eval!(cx, view, {draw_bg +: {color: #(color)}});
        }
        // What gives way first (`header_fit`): the folds, the state's text,
        // the views' labels, the position; the title and the scrubber last.
        let level = fit.level;
        if level != self.collapse {
            self.collapse = level;
            ui.view(cx, ids!(score_folds)).set_visible(cx, level < 1);
            ui.widget(cx, ids!(score_position)).set_visible(cx, level < 5);
            ui.view(cx, ids!(score_glyph)).set_visible(cx, level < 5);
            let density = match level { 0..=2 => TabsDensity::Full, 3 => TabsDensity::Compact, _ => TabsDensity::Icons };
            if let Some(mut tabs) = ui.widget(cx, ids!(score_tabs)).borrow_mut::<ScoreTabs>() {
                tabs.set_density(cx, density);
            }
            ui.redraw(cx);
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

    /// The header's transport: play/pause, the position, the scrubber (the
    /// arrangement, or the longest loop's cycle for a score of loops).
    fn pump_transport(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        if let Some(doc) = &self.doc {
            let next = if self.arranged == self.revision && !self.arrangement.cycle {
                None
            } else if doc.arrange.is_empty() {
                // The longest cycle the loops play (as played, else as
                // written), at least a bar.
                let played = self.status.loops.iter().map(|l| l.len).fold(0.0, f64::max);
                let written = doc.loops.iter().filter_map(|l| l.beats).fold(0.0, f64::max);
                let beats = if played > 0.0 { played } else if written > 0.0 { written } else { 4.0 * doc.beats_per_bar() };
                Some(transport::Arrangement::cycle(doc, beats))
            } else {
                Some(transport::Arrangement::of(doc))
            };
            self.arranged = self.revision;
            if let Some(next) = next.filter(|next| *next != self.arrangement) {
                let cycle = next.cycle;
                self.arrangement = next;
                // A loop of the old arrangement means nothing in the new one.
                if self.looping.take().is_some() {
                    self.set_loop(None);
                }
                let tip = if cycle {
                    "The longest loop's cycle, bar by bar: drag or click to move to a bar (from the next bar line)"
                } else {
                    "The arrangement: click or drag to move the playhead to a bar (from the next bar line); edit sections in Arrange"
                };
                if let Some(mut scrub_tip) = ui.widget(cx, ids!(score_scrub_tip)).borrow_mut::<makepad_widgets::tip::Tip>() {
                    scrub_tip.text = tip.to_string();
                }
            }
        }
        let playing = self.status.playing;
        if ui.button(cx, ids!(score_play)).visible() == playing {
            ui.button(cx, ids!(score_play)).set_visible(cx, !playing);
            ui.button(cx, ids!(score_pause)).set_visible(cx, playing);
            lit(cx, ui, ids!(score_pause), true);
        }
        let a = &self.arrangement;
        let song = self.song_beat();
        let at = song.unwrap_or(self.status.beat);
        let pos = transport::bar_beat(at, a.beats_per_bar.max(1.0));
        let time = format!("{} / {}", transport::clock(a.seconds_at(at)), transport::clock(a.seconds_at(a.beats())));
        if ui.label(cx, ids!(score_pos)).text() != pos {
            ui.label(cx, ids!(score_pos)).set_text(cx, &pos);
        }
        if ui.label(cx, ids!(score_time)).text() != time {
            ui.label(cx, ids!(score_time)).set_text(cx, &time);
        }
        let looping = if a.cycle { None } else { self.status.looping.or(self.looping) };
        let arrangement = self.arrangement.clone();
        if let Some(mut scrubber) = ui.widget(cx, ids!(score_scrub)).borrow_mut::<transport::ScoreScrubber>() {
            scrubber.set_model(cx, &arrangement, song, looping);
        }
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
        let (choice, node) = match self.panel.preview.clone() {
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
