//! BATCH: songs written at once. The pane's form (or the chat's
//! `score_batch`: a prompt, style markers and a length make five songs at
//! once, `makepad-music-iter`) starts them; each is written by the model as a
//! whole Score and rendered as it arrives, so the first can be heard while
//! the rest come, and the pane (and the chat) says as each finishes. A song opens in the
//! desk like any score (it plays on the transport, its code shows). The
//! person rates aspects of each (up, down or not at all) and comments here
//! (selecting a song is the rating's target); `score_learn` has the model
//! work out why and rewrite MUSIC-AGENTS.md's learned rules, which the next
//! batch reads. Every batch, its ratings and the rule changes go to a log.

use super::tabs::DrawSeg;
use makepad_music_iter::batch::{self, Batch, Learned, Run, State, ASPECTS};
use makepad_music_iter::gen::{Request, TurnMonitor};
use super::Desk;
use crate::writer::AiWriter;
use makepad_strict_json::Value;
use makepad_widgets::*;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    mod.widgets.MusicChipsBase = #(MusicChips::register_widget(vm))
    mod.widgets.MusicChips = set_type_default() do mod.widgets.MusicChipsBase{
        width: Fill height: 20
        draw_text +: {color: stage.label text_style: theme.font_regular{font_size: 8}}
        draw_head +: {color: stage.muted text_style: theme.font_bold{font_size: 7}}
    }
    mod.widgets.MusicSongsBase = #(MusicSongs::register_widget(vm))
    mod.widgets.MusicSongs = set_type_default() do mod.widgets.MusicSongsBase{
        width: Fill height: 40
        draw_text +: {color: stage.muted text_style: theme.font_regular{font_size: 8}}
        draw_title +: {color: stage.text_hi text_style: theme.font_bold{font_size: 9}}
        empty_text: "No batch yet: describe the songs and Generate."
    }
}

/// Chip states: off, on (a marker; an aspect liked), down (an aspect
/// disliked).
pub const OFF: u8 = 0;
pub const ON: u8 = 1;
pub const DOWN: u8 = 2;

#[derive(Clone, Debug, Default)]
pub enum ChipsAction {
    #[default]
    None,
    /// Chip index, its new state.
    Set(usize, u8),
}

/// Toggle chips that wrap: style markers (groups with a heading each), or
/// aspects that cycle off → up → down.
#[derive(Script, ScriptHook, Widget)]
pub struct MusicChips {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawSeg,
    #[live]
    draw_box: DrawSeg,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_head: DrawText,
    /// (group heading, chip labels); one flat index over all chips.
    #[rust]
    groups: Vec<(String, Vec<String>)>,
    #[rust]
    states: Vec<u8>,
    /// Aspects: a press cycles through down too.
    #[rust]
    tri: bool,
    #[rust]
    rects: Vec<Rect>,
    #[rust]
    hover: Option<usize>,
    #[rust]
    width: f64,
    #[rust]
    height: f64,
    #[rust]
    area: Area,
}

const CHIP_H: f64 = 18.0;
const CHIP_GAP: f64 = 4.0;
const CHIP_PAD: f64 = 6.0;

impl MusicChips {
    pub fn set_chips(&mut self, cx: &mut Cx, groups: Vec<(String, Vec<String>)>, states: Vec<u8>, tri: bool) {
        if groups != self.groups || states != self.states || tri != self.tri {
            self.groups = groups;
            self.states = states;
            self.tri = tri;
            self.redraw(cx);
        }
    }

    fn label(&self, i: usize) -> String {
        let label = self.groups.iter().flat_map(|(_, c)| c).nth(i).cloned().unwrap_or_default();
        match self.states.get(i) {
            Some(&ON) if self.tri => format!("+ {label}"),
            Some(&DOWN) => format!("- {label}"),
            _ => label,
        }
    }

    /// Where every chip goes at `width`; the height it takes.
    fn layout(&self, cx: &mut Cx2d, width: f64) -> (Vec<(Option<(String, DVec2)>, Rect)>, f64) {
        let mut out = Vec::new();
        let (mut x, mut y) = (0.0, 0.0);
        let mut index = 0;
        for (head, chips) in &self.groups {
            let mut heading = None;
            if !head.is_empty() {
                let w = makepad_widgets::badge::advance(&self.draw_head, cx, head) + CHIP_GAP;
                if x > 0.0 && x + w > width {
                    x = 0.0;
                    y += CHIP_H + CHIP_GAP;
                }
                heading = Some((head.clone(), dvec2(x, y + 4.0)));
                x += w;
            }
            for _ in chips {
                let w = makepad_widgets::badge::advance(&self.draw_text, cx, &self.label(index)) + 2.0 * CHIP_PAD;
                if x > 0.0 && x + w > width {
                    x = 0.0;
                    y += CHIP_H + CHIP_GAP;
                }
                out.push((heading.take(), Rect { pos: dvec2(x, y), size: dvec2(w, CHIP_H) }));
                x += w + CHIP_GAP;
                index += 1;
            }
            // Each group starts on its own line.
            x = 0.0;
            y += CHIP_H + CHIP_GAP;
        }
        (out, (y - CHIP_GAP).max(CHIP_H))
    }
}

impl Widget for MusicChips {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let origin = self.area.rect(cx).pos;
        let at = |rects: &[Rect], p: DVec2| rects.iter().position(|r| r.contains(p - origin));
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hover = at(&self.rects, fe.abs);
                cx.set_cursor(if hover.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
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
                if let Some(i) = at(&self.rects, fe.abs) {
                    let state = self.states.get(i).copied().unwrap_or(OFF);
                    let next = match (state, self.tri) {
                        (OFF, _) => ON,
                        (ON, true) => DOWN,
                        _ => OFF,
                    };
                    if let Some(s) = self.states.get_mut(i) {
                        *s = next;
                    }
                    self.redraw(cx);
                    cx.widget_action(self.uid, ChipsAction::Set(i, next));
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let p = makepad_look::palette::get(cx);
        let width = if self.width > 0.0 { self.width } else { 330.0 };
        let (layout, height) = self.layout(cx, width);
        let rect = cx.walk_turtle(Walk { height: Size::Fixed(height), ..walk });
        // The width is known after the walk; a change lays out again.
        if (rect.size.x - self.width).abs() > 0.5 || (height - self.height).abs() > 0.5 {
            self.width = rect.size.x;
            self.height = height;
            cx.redraw_all();
        }
        self.draw_bg.fill = Vec4f::from_u32(p.panel);
        self.draw_bg.edge = Vec4f::from_u32(p.panel);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        self.rects.clear();
        for (i, (heading, r)) in layout.into_iter().enumerate() {
            if let Some((head, at)) = heading {
                self.draw_head.draw_abs(cx, rect.pos + at, &head);
            }
            let state = self.states.get(i).copied().unwrap_or(OFF);
            let chip = Rect { pos: rect.pos + r.pos, size: r.size };
            let (fill, ink) = match state {
                ON if self.tri => (p.accent_wash, p.found),
                ON => (p.accent_wash, p.accent),
                DOWN => (p.danger_wash, p.danger),
                _ if self.hover == Some(i) => (p.control_hover, p.text_hi),
                _ => (p.well, p.label),
            };
            self.draw_box.radius = 3.0;
            self.draw_box.fill = Vec4f::from_u32(fill);
            self.draw_box.edge = Vec4f::from_u32(if state == OFF { p.line } else { fill });
            self.draw_box.draw_abs(cx, chip);
            self.draw_text.color = Vec4f::from_u32(ink);
            let label = self.label(i);
            self.draw_text.draw_abs(cx, chip.pos + dvec2(CHIP_PAD, 3.0), &label);
            self.rects.push(r);
        }
        DrawStep::done()
    }
}

#[derive(Clone, Debug, Default)]
pub enum SongsAction {
    #[default]
    None,
    Selected(usize),
    /// A double-click: play it from the start (or stop it, playing).
    Play(usize),
}

/// One song row: its title line, what it is doing or its idea, and the
/// right-hand facts (length, card, ratings).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SongRow {
    pub title: String,
    pub detail: String,
    pub facts: String,
}

/// Rows (the batch's songs, the library's scores and samples), one row
/// each: a click picks one, a double-click plays or opens it.
#[derive(Script, ScriptHook, Widget)]
pub struct MusicSongs {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawSeg,
    #[live]
    draw_box: DrawSeg,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_title: DrawText,
    /// What it says with no rows.
    #[live]
    empty_text: String,
    #[rust]
    rows: Vec<SongRow>,
    #[rust]
    selected: Option<usize>,
    /// The row playing on the transport.
    #[rust]
    playing: Option<usize>,
    #[rust]
    hover: Option<usize>,
    #[rust]
    area: Area,
}

const ROW_H: f64 = 34.0;

impl MusicSongs {
    pub fn set_rows(&mut self, cx: &mut Cx, rows: Vec<SongRow>, selected: Option<usize>, playing: Option<usize>) {
        if rows != self.rows || selected != self.selected || playing != self.playing {
            self.rows = rows;
            self.selected = selected;
            self.playing = playing;
            self.redraw(cx);
        }
    }
}

impl Widget for MusicSongs {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let top = self.area.rect(cx).pos.y;
        let row = |y: f64, n: usize| {
            let i = ((y - top) / ROW_H).floor();
            (i >= 0.0 && (i as usize) < n).then_some(i as usize)
        };
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hover = row(fe.abs.y, self.rows.len());
                cx.set_cursor(if hover.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
                if hover != self.hover {
                    self.hover = hover;
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hover = None;
                self.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.tap_count == 2 => {
                if let Some(i) = row(fe.abs.y, self.rows.len()) {
                    cx.widget_action(self.uid, SongsAction::Play(i));
                }
            }
            Hit::FingerUp(fe) if fe.is_over && !fe.cancelled => {
                if let Some(i) = row(fe.abs.y, self.rows.len()) {
                    if self.selected != Some(i) {
                        self.selected = Some(i);
                        self.redraw(cx);
                        cx.widget_action(self.uid, SongsAction::Selected(i));
                    }
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let p = makepad_look::palette::get(cx);
        let height = (self.rows.len() as f64 * ROW_H).max(ROW_H);
        let rect = cx.walk_turtle(Walk { height: Size::Fixed(height), ..walk });
        self.draw_bg.fill = Vec4f::from_u32(p.panel);
        self.draw_bg.edge = Vec4f::from_u32(p.panel);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        if self.rows.is_empty() {
            self.draw_text.color = Vec4f::from_u32(p.muted);
            let empty = self.empty_text.clone();
            self.draw_text.draw_abs(cx, rect.pos + dvec2(8.0, 10.0), &empty);
        }
        for (i, row) in self.rows.iter().enumerate() {
            let r = Rect { pos: rect.pos + dvec2(0.0, i as f64 * ROW_H), size: dvec2(rect.size.x, ROW_H - 2.0) };
            if self.selected == Some(i) || self.hover == Some(i) {
                self.draw_box.radius = 3.0;
                self.draw_box.fill = Vec4f::from_u32(if self.selected == Some(i) { p.accent_wash } else { p.control_hover });
                self.draw_box.edge = self.draw_box.fill;
                self.draw_box.draw_abs(cx, r);
            }
            // The playing row: an accent bar down its left edge.
            if self.playing == Some(i) {
                self.draw_box.radius = 1.0;
                self.draw_box.fill = Vec4f::from_u32(p.accent);
                self.draw_box.edge = self.draw_box.fill;
                self.draw_box.draw_abs(cx, Rect { pos: r.pos + dvec2(1.0, 4.0), size: dvec2(3.0, r.size.y - 8.0) });
            }
            let facts_w = makepad_widgets::badge::advance(&self.draw_text, cx, &row.facts);
            // Long titles and details are cut to the row.
            let room = ((r.size.x - facts_w - 24.0) / 6.0).max(8.0) as usize;
            let cut = |s: &str, n: usize| if s.chars().count() > n { format!("{}…", s.chars().take(n.saturating_sub(1)).collect::<String>()) } else { s.to_string() };
            self.draw_title.draw_abs(cx, r.pos + dvec2(8.0, 3.0), &cut(&row.title, room));
            self.draw_text.color = Vec4f::from_u32(p.muted);
            self.draw_text.draw_abs(cx, r.pos + dvec2(8.0, 18.0), &cut(&row.detail, ((r.size.x - 16.0) / 5.0) as usize));
            self.draw_text.color = Vec4f::from_u32(p.label);
            self.draw_text.draw_abs(cx, r.pos + dvec2(r.size.x - facts_w - 8.0, 4.0), &row.facts);
        }
        DrawStep::done()
    }
}

// ------------------------------------------------------------ the mode

/// A card's sub-genre, tempo and key, for a row.
fn concept_line(card: &str) -> Option<String> {
    let field = |key: &str| card.lines().find_map(|l| l.strip_prefix(key)).map(|v| v.trim().to_string());
    Some(format!("{} · {} · {}", field("SUBGENRE:")?, field("TEMPO:")?, field("KEY:")?))
}

/// The default style markers; `chips.txt` beside the batches (one group
/// per line, `heading: a, b, c`) replaces them, and is written the first
/// time so the person can edit it.
const DEFAULT_CHIPS: &str = "genre: house, techno, synthwave, drum & bass, lo-fi hip hop, pop, cinematic, ambient, trance, future bass\n\
mood: bright, dark, melancholic, euphoric, tense, dreamy, playful\n\
energy: low energy, medium energy, high energy\n\
sound: analog synths, piano, strings, guitars, 808s, plucks, choir\n\
vocals: instrumental, sung vocal\n\
tempo: slow, mid-tempo, fast, half-time feel, swung\n";

pub const LENGTHS: [u32; 4] = [15, 30, 60, 120];

/// How many songs a Generate makes.
pub const SONGS: usize = 5;

/// The most songs one `score_batch` makes.
pub const MOST_SONGS: usize = 10;

/// What a batch is asked for: the Generate form's fields, and the
/// `score_batch` tool's arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct BatchSpec {
    pub prompt: String,
    pub markers: Vec<String>,
    pub seconds: u32,
    pub count: usize,
}

/// The tool's arguments as a request: `prompt` (required), `markers`
/// (style marker names), `seconds` (one of [`LENGTHS`], default 30), `count`
/// (1 to [`MOST_SONGS`], default [`SONGS`]).
pub fn batch_spec(args: &Value) -> Result<BatchSpec, String> {
    let prompt = args.get("prompt").and_then(Value::as_str).map(str::trim).unwrap_or("");
    if prompt.is_empty() {
        return Err("give the `prompt`: what the songs are about".into());
    }
    let markers = match args.get("markers") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Arr(list)) => list.iter().filter_map(Value::as_str).map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect(),
        Some(_) => return Err("`markers` is a list of style marker names".into()),
    };
    let whole = |key: &str| match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Int(n)) => Ok(Some(*n)),
        Some(Value::F64(n)) if n.fract() == 0.0 => Ok(Some(*n as i64)),
        Some(_) => Err(format!("`{key}` is a whole number")),
    };
    let seconds = match whole("seconds")? {
        None => LENGTHS[1],
        Some(n) if LENGTHS.iter().any(|l| i64::from(*l) == n) => n as u32,
        Some(n) => return Err(format!("`seconds` is one of {LENGTHS:?}, not {n}")),
    };
    let count = match whole("count")? {
        None => SONGS,
        Some(n) if (1..=MOST_SONGS as i64).contains(&n) => n as usize,
        Some(n) => return Err(format!("`count` is 1 to {MOST_SONGS}, not {n}")),
    };
    Ok(BatchSpec { prompt: prompt.to_string(), markers, seconds, count })
}

/// A rating word of `score_rate`: liked, disliked or clear.
pub fn rating_of(word: &str) -> Result<Option<bool>, String> {
    match word {
        "liked" => Ok(Some(true)),
        "disliked" => Ok(Some(false)),
        "clear" => Ok(None),
        other => Err(format!("`{other}` is not liked, disliked or clear")),
    }
}

fn state_word(state: &State) -> &'static str {
    match state {
        State::Waiting => "planning",
        State::Writing { .. } => "writing",
        State::Rendering => "rendering",
        State::Revising => "revising",
        State::Ready => "ready",
        State::Failed(_) => "failed",
    }
}

#[derive(Default)]
pub struct BatchDesk {
    pub batch: Option<Batch>,
    pub run: Option<Run>,
    /// The batch is in the log (Learn or the next Generate puts it there).
    pub logged: bool,
    pub selected: Option<usize>,
    /// chips.txt: the style markers by group (the set a batch card offers).
    pub chips: Vec<(String, Vec<String>)>,
    pub learning: Option<(Arc<AtomicBool>, Receiver<Result<Learned, String>>)>,
    monitor: TurnMonitor,
    not_responding: bool,
    pub note: String,
    /// The song double-clicked to play (it plays while the transport does).
    pub playing: Option<usize>,
    /// When it was started: the transport reports playing a moment later.
    pub play_started: Option<std::time::Instant>,
    pub timer: Timer,
    pub loaded: bool,
    /// Lines for the chat's transcript (a batch started, a song finished).
    pub told: Vec<String>,
    /// Songs whose end the chat was told.
    reported: Vec<bool>,
    /// The pane was filled once.
    pub shown: bool,
    /// The form's style markers (one state per chip of `chips`) and length.
    pub marker_states: Vec<u8>,
    pub length: usize,
}

impl BatchDesk {
    /// First use: the markers and the newest batch.
    pub fn load(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let path = batch::root().join("chips.txt");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            let _ = std::fs::create_dir_all(batch::root());
            let _ = std::fs::write(&path, DEFAULT_CHIPS);
            DEFAULT_CHIPS.to_string()
        });
        self.chips = text
            .lines()
            .filter_map(|l| {
                let (head, rest) = l.split_once(':')?;
                let chips: Vec<String> = rest.split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect();
                (!chips.is_empty()).then(|| (head.trim().to_string(), chips))
            })
            .collect();
        // The form starts with the newest batch's markers.
        self.batch = Batch::latest();
        let on = self.card_markers();
        self.marker_states = self.chips.iter().flat_map(|(_, chips)| chips.iter().map(|c| if on.contains(c) { ON } else { OFF }).collect::<Vec<_>>()).collect();
        self.length = 1;
        self.logged = true;
        if let Some(b) = &self.batch {
            self.note = format!("Batch b{:03}, from {}", b.id, b.dir.display());
        }
    }

    /// The batch card's starting markers: the newest batch's, else the first
    /// style of each group of chips.txt.
    pub fn card_markers(&self) -> Vec<String> {
        match &self.batch {
            Some(b) if !b.request.markers.is_empty() => b.request.markers.clone(),
            _ => self.chips.iter().filter_map(|(_, chips)| chips.first().cloned()).collect(),
        }
    }

    /// The style markers picked on the form.
    pub fn markers_on(&self) -> Vec<String> {
        self.chips.iter().flat_map(|(_, chips)| chips.iter()).zip(&self.marker_states).filter(|(_, s)| **s == ON).map(|(c, _)| c.clone()).collect()
    }

    /// A line for the desk's note and the chat's transcript.
    fn tell(&mut self, line: String) {
        self.note = line.clone();
        self.told.push(line);
    }

    pub fn busy(&self) -> bool {
        self.run.is_some() || self.learning.is_some()
    }

    /// Start a batch (`score_batch`).
    pub fn start(&mut self, spec: BatchSpec) -> Result<(), String> {
        if self.run.is_some() {
            return Err("A batch is still being written; Stop it first.".into());
        }
        if self.learning.is_some() {
            return Err("The model is still learning from the last batch.".into());
        }
        if let (Some(b), false) = (&self.batch, self.logged) {
            batch::append_log(b, None);
        }
        let count = spec.count;
        let request = Request { prompt: spec.prompt, markers: spec.markers, seconds: spec.seconds };
        let writer = AiWriter::default();
        let monitor = writer.monitor.clone();
        let label = makepad_music_iter::gen::Writer::label(&writer);
        match Batch::start(Arc::new(writer), request, count) {
            Ok((b, run)) => {
                self.monitor = monitor;
                self.not_responding = false;
                self.tell(format!("Batch b{:03}: the director plans {count} different songs, then {count} composers write them ({label})…", b.id));
                self.reported = vec![false; count];
                self.batch = Some(b);
                self.run = Some(run);
                self.logged = false;
                self.selected = None;
                Ok(())
            }
            Err(e) => Err(format!("Could not start: {e}")),
        }
    }

    pub fn stop(&mut self) {
        let busy = self.busy();
        if let Some(run) = self.run.take() {
            run.stop();
        }
        if let Some((cancel, _)) = self.learning.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.not_responding = false;
        if busy {
            self.tell("Stopped; the songs that finished stay.".into());
        }
    }

    /// Learn from the batch's ratings (`score_learn`):
    /// the model works on a thread; `poll` takes its answer. An empty
    /// error means there is nothing to say (no batch).
    pub fn start_learning(&mut self) -> Result<(), String> {
        let Some(b) = &self.batch else { return Err(String::new()) };
        if self.learning.is_some() {
            return Err("The model is already learning from the ratings.".into());
        }
        if self.run.is_some() {
            return Err("A batch is still being written; wait for it first.".into());
        }
        b.save_ratings();
        let Some(copy) = Batch::open(&b.dir) else { return Err("The batch could not be read back.".into()) };
        if copy.rated().is_empty() {
            return Err("Rate an aspect of a song (or comment on one) first.".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let flag = cancel.clone();
        let writer = AiWriter::default();
        self.monitor = writer.monitor.clone();
        self.not_responding = false;
        std::thread::spawn(move || {
            let _ = tx.send(batch::learn(&writer, &copy, &flag));
        });
        self.learning = Some((cancel, rx));
        self.tell("Learning: the model is reading the rated songs…".into());
        Ok(())
    }

    /// Progress in; true when the view should change.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        let mut finished = None;
        if let (Some(run), Some(b)) = (self.run.as_mut(), self.batch.as_mut()) {
            changed |= run.poll(b);
            self.reported.resize(b.songs.len(), false);
            for (i, song) in b.songs.iter().enumerate() {
                let line = match &song.state {
                    State::Ready => format!("Song {} is ready: {}{}.", i + 1, if song.title.is_empty() { "Untitled" } else { &song.title }, song.card.as_ref().map_or(String::new(), |c| format!(" (scorecard {:.0})", c.total()))),
                    State::Failed(why) => format!("Song {} failed: {why}", i + 1),
                    _ => continue,
                };
                if !std::mem::replace(&mut self.reported[i], true) {
                    self.told.push(line);
                }
            }
            if run.done() {
                self.run = None;
                let ready = b.songs.iter().filter(|e| e.state == State::Ready).count();
                finished = Some(format!("Batch b{:03}: {ready} of {} songs ready. Rate them in the results beside the code, then learn from the ratings.", b.id, b.songs.len()));
                changed = true;
            }
        }
        if let Some(line) = finished {
            self.tell(line);
        }
        if let Some((_, rx)) = &self.learning {
            if let Ok(result) = rx.try_recv() {
                self.learning = None;
                self.logged = true;
                let line = match result {
                    Ok(l) => format!("MUSIC-AGENTS.md: +{} rules, -{} ({} now). {}", l.added().len(), l.removed().len(), l.after.len(), l.changes.lines().next().unwrap_or("")),
                    Err(e) => format!("Learning failed: {e}"),
                };
                self.tell(line);
                changed = true;
            }
        }
        let quiet = self.busy() && self.monitor.not_responding();
        if self.not_responding != quiet {
            self.not_responding = quiet;
            changed = true;
        }
        changed
    }

    pub fn rows(&self) -> Vec<SongRow> {
        let Some(b) = &self.batch else { return Vec::new() };
        b.songs
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let ups = e.ratings.iter().filter(|r| **r == Some(true)).count();
                let downs = e.ratings.iter().filter(|r| **r == Some(false)).count();
                let rated = if ups + downs > 0 { format!(" · +{ups} -{downs}") } else { String::new() };
                match &e.state {
                    State::Ready => SongRow {
                        title: format!("{}  {}", i + 1, if e.title.is_empty() { "Untitled" } else { &e.title }),
                        detail: e.idea.clone(),
                        facts: format!("{}:{:02} · {}{rated}", e.seconds as u32 / 60, e.seconds as u32 % 60, e.card.as_ref().map_or("-".into(), |c| format!("{:.0}", c.total()))),
                    },
                    State::Failed(why) => SongRow { title: format!("{}  failed", i + 1), detail: why.clone(), facts: String::new() },
                    State::Waiting => SongRow { title: format!("{}  planning…", i + 1), detail: "the director is planning five different songs".into(), facts: String::new() },
                    State::Writing { turn } => SongRow { title: if e.title.is_empty() { format!("{}  writing…", i + 1) } else { format!("{}  {} (writing…)", i + 1, e.title) }, detail: if *turn > 1 { format!("fixing what did not play (turn {turn})") } else { concept_line(&e.concept).unwrap_or_else(|| "the model is writing the score".into()) }, facts: String::new() },
                    State::Revising => SongRow { title: if e.title.is_empty() { format!("{}  revising…", i + 1) } else { format!("{}  {} (revising…)", i + 1, e.title) }, detail: "a producer's notes on the first render: weak mix lines and form".into(), facts: String::new() },
                    State::Rendering => SongRow { title: format!("{}  rendering…", i + 1), detail: "mix, stems and scorecard".into(), facts: String::new() },
                }
            })
            .collect()
    }

    /// The selected song's aspect chips.
    pub fn aspects(&self) -> (Vec<(String, Vec<String>)>, Vec<u8>) {
        let groups = vec![(String::new(), ASPECTS.iter().map(|a| a.to_string()).collect())];
        let states = self
            .selected
            .and_then(|i| self.batch.as_ref()?.songs.get(i))
            .map_or(vec![OFF; ASPECTS.len()], |e| e.ratings.iter().map(|r| match r { Some(true) => ON, Some(false) => DOWN, None => OFF }).collect());
        (groups, states)
    }

    pub fn rate(&mut self, aspect: usize, state: u8) {
        if let Some(e) = self.selected.and_then(|i| self.batch.as_mut()?.songs.get_mut(i)) {
            if let Some(r) = e.ratings.get_mut(aspect) {
                *r = match state { ON => Some(true), DOWN => Some(false), _ => None };
            }
        }
        if let Some(b) = &self.batch {
            b.save_ratings();
        }
    }

    pub fn comment(&mut self, text: String) {
        if let Some(e) = self.selected.and_then(|i| self.batch.as_mut()?.songs.get_mut(i)) {
            e.feedback = text;
        }
        if let Some(b) = &self.batch {
            b.save_ratings();
        }
    }

    /// The selected song's card, short: the total and its weakest lines.
    pub fn card_line(&self) -> String {
        let Some(e) = self.selected.and_then(|i| self.batch.as_ref()?.songs.get(i)) else { return String::new() };
        let Some(card) = &e.card else { return String::new() };
        let weak: Vec<String> = card.weakest(3).iter().map(|m| format!("{} {:.0}", m.key, m.score)).collect();
        format!("Scorecard {:.0}{}", card.total(), if weak.is_empty() { String::new() } else { format!(" · weakest: {}", weak.join(", ")) })
    }
}

impl Desk {
    pub(super) fn handle_batch_actions(&mut self, cx: &mut Cx, ui: &WidgetRef, actions: &Actions) {
        // The form: markers, length, Generate, Learn.
        let markers = ui.widget(cx, ids!(batch_markers));
        for action in actions.filter_widget_actions_cast::<ChipsAction>(markers.widget_uid()) {
            if let ChipsAction::Set(i, state) = action {
                if let Some(s) = self.batch.marker_states.get_mut(i) {
                    *s = state;
                }
            }
        }
        if let Some(index) = ui.drop_down(cx, ids!(batch_length)).selected(actions) {
            self.batch.length = index.min(LENGTHS.len() - 1);
        }
        if ui.button(cx, ids!(batch_go)).clicked(actions) {
            let prompt = ui.text_input(cx, ids!(batch_prompt)).text();
            let spec = BatchSpec { prompt: prompt.trim().to_string(), markers: self.batch.markers_on(), seconds: LENGTHS[self.batch.length], count: SONGS };
            let started = if spec.prompt.is_empty() { Err("Describe the songs first.".to_string()) } else { self.batch.start(spec) };
            if let Err(why) = started {
                self.batch.note = why;
            }
            self.batch.timer = cx.start_timeout(0.25);
            self.show_batch(cx, ui);
        }
        if ui.button(cx, ids!(batch_learn)).clicked(actions) {
            if let Err(why) = self.batch.start_learning() {
                self.batch.note = if why.is_empty() { "There is no batch yet.".into() } else { why };
            }
            self.batch.timer = cx.start_timeout(0.25);
            self.show_batch(cx, ui);
        }
        if ui.button(cx, ids!(batch_stop)).clicked(actions) {
            self.batch.stop();
            self.show_batch(cx, ui);
        }
        if self.batch.batch.is_none() {
            return;
        }
        let aspects = ui.widget(cx, ids!(batch_aspects));
        for action in actions.filter_widget_actions_cast::<ChipsAction>(aspects.widget_uid()) {
            if let ChipsAction::Set(i, state) = action {
                self.batch.rate(i, state);
                self.show_batch(cx, ui);
            }
        }
        let songs = ui.widget(cx, ids!(batch_songs));
        for action in actions.filter_widget_actions_cast::<SongsAction>(songs.widget_uid()) {
            match action {
                SongsAction::Selected(i) => {
                    self.batch.selected = Some(i);
                    self.open_batch_song(cx, ui, i);
                    let comment = self.batch.batch.as_ref().and_then(|b| b.songs.get(i)).map(|e| e.feedback.clone()).unwrap_or_default();
                    ui.text_input(cx, ids!(batch_comment)).set_text(cx, &comment);
                    self.show_batch(cx, ui);
                }
                // Play it from the start on the transport; the playing
                // one stops.
                SongsAction::Play(i) => {
                    let playing = self.batch.playing == Some(i) && self.status.playing;
                    self.transport(false);
                    self.seek(0.0, 0.0);
                    if playing {
                        self.batch.playing = None;
                    } else if !self.open_batch_song(cx, ui, i) {
                        self.batch.note = match self.batch.batch.as_ref().and_then(|b| b.songs.get(i)).map(|e| &e.state) {
                            Some(State::Failed(why)) => format!("Song {} did not finish: {why}", i + 1),
                            _ => format!("Song {} is still being written; it plays once it is ready.", i + 1),
                        };
                    } else {
                        self.batch.selected = Some(i);
                        let comment = self.batch.batch.as_ref().and_then(|b| b.songs.get(i)).map(|e| e.feedback.clone()).unwrap_or_default();
                        ui.text_input(cx, ids!(batch_comment)).set_text(cx, &comment);
                        self.transport(true);
                        self.batch.playing = Some(i);
                        self.batch.play_started = Some(std::time::Instant::now());
                    }
                    self.show_batch(cx, ui);
                }
                SongsAction::None => {}
            }
        }
        if let Some(text) = ui.text_input(cx, ids!(batch_comment)).changed(actions) {
            self.batch.comment(text);
        }
    }

    /// A song of the batch in the desk, as its own document: it plays on
    /// the transport and its code shows, like any score.
    fn open_batch_song(&mut self, cx: &mut Cx, ui: &WidgetRef, i: usize) -> bool {
        let Some((id, title, source)) = self.batch.batch.as_ref().and_then(|b| {
            let e = b.songs.get(i).filter(|e| e.state == State::Ready)?;
            Some((batch_song_id(b.id, i), e.title.clone(), e.source.clone()))
        }) else {
            return false;
        };
        if self.document.id == id {
            return true;
        }
        self.open_document(cx, ui, crate::library::Document::unsaved(id, &title, &source), source);
        true
    }

    pub(super) fn pump_batch(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        // First look: the newest batch on disk is the results.
        if !self.batch.loaded {
            self.batch.load();
        }
        let mut changed = self.batch.poll();
        // The highlight follows the transport: stopped, or another score
        // opened, it goes.
        if let Some(i) = self.batch.playing {
            let id = self.batch.batch.as_ref().map(|b| batch_song_id(b.id, i));
            let settling = self.batch.play_started.is_some_and(|t| t.elapsed().as_secs_f64() < 1.5);
            if (!self.status.playing && !settling) || id.as_deref() != Some(self.document.id.as_str()) {
                self.batch.playing = None;
                changed = true;
            }
        }
        if changed || !self.batch.shown {
            self.show_batch(cx, ui);
        }
        if self.batch.busy() {
            self.batch.timer = cx.start_timeout(0.25);
        }
    }

    fn show_batch(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        self.batch.shown = true;
        let desk = &self.batch;
        if let Some(mut chips) = ui.widget(cx, ids!(batch_markers)).borrow_mut::<MusicChips>() {
            chips.set_chips(cx, desk.chips.clone(), desk.marker_states.clone(), false);
        }
        if let Some(mut songs) = ui.widget(cx, ids!(batch_songs)).borrow_mut::<MusicSongs>() {
            songs.set_rows(cx, desk.rows(), desk.selected, desk.playing);
        }
        let (groups, states) = desk.aspects();
        if let Some(mut aspects) = ui.widget(cx, ids!(batch_aspects)).borrow_mut::<MusicChips>() {
            aspects.set_chips(cx, groups, states, true);
        }
        ui.label(cx, ids!(batch_card)).set_text(cx, &desk.card_line());
        ui.label(cx, ids!(batch_note)).set_text(cx, if desk.not_responding { "not responding — Stop?" } else { &desk.note });
        ui.button(cx, ids!(batch_stop)).set_visible(cx, desk.busy());
        ui.view(cx, ids!(batch_rating)).set_visible(cx, desk.selected.is_some());
    }
}

/// A batch song's document id.
fn batch_song_id(batch: u32, song: usize) -> String {
    format!("music-b{batch:03}-s{}", song + 1)
}

impl Desk {
    /// The control API's `score_batch`, `score_batch_status`, `score_batch_stop`,
    /// `score_learn` and `score_rate`: the results' desk.
    pub fn batch_tool_call(&mut self, cx: &mut Cx, ui: &WidgetRef, name: &str, args: &Value) -> Result<Value, String> {
        self.batch.load();
        self.batch.poll();
        let result = match name {
            "score_batch" => {
                let spec = batch_spec(args)?;
                self.batch.start(spec.clone())?;
                let desk = &self.batch;
                let b = desk.batch.as_ref().ok_or("no batch")?;
                Ok(Value::Obj(vec![
                    ("batch".into(), Value::Str(format!("b{:03}", b.id))),
                    ("songs".into(), Value::Int(spec.count as i64)),
                    ("note".into(), Value::Str(desk.note.clone())),
                ]))
            }
            "score_learn" => {
                self.batch.start_learning().map_err(|e| if e.is_empty() { "there is no batch yet: make one with score_batch".to_string() } else { e })?;
                Ok(Value::Obj(vec![("learning".into(), Value::Bool(true)), ("note".into(), Value::Str(self.batch.note.clone()))]))
            }
            "score_rate" => self.score_rate_call(args),
            "score_batch_stop" => {
                let was = self.batch.busy();
                self.batch.stop();
                Ok(Value::Obj(vec![("stopped".into(), Value::Bool(was)), ("note".into(), Value::Str(if was { self.batch.note.clone() } else { "nothing is running".into() }))]))
            }
            _ => Ok(self.score_batch_status_json()),
        };
        self.batch.timer = cx.start_timeout(0.25);
        self.show_batch(cx, ui);
        result
    }

    fn score_rate_call(&mut self, args: &Value) -> Result<Value, String> {
        let desk = &mut self.batch;
        let b = desk.batch.as_mut().ok_or("there is no batch yet: make one with score_batch")?;
        let song = match args.get("song") {
            Some(Value::Int(n)) if *n >= 1 && (*n as usize) <= b.songs.len() => *n as usize - 1,
            _ => return Err(format!("`song` is 1 to {}", b.songs.len())),
        };
        let mut ratings = Vec::new();
        for (i, aspect) in ASPECTS.iter().enumerate() {
            if let Some(word) = args.get(aspect).and_then(Value::as_str) {
                ratings.push((i, rating_of(word)?));
            }
        }
        let comment = args.get("comment").and_then(Value::as_str);
        if ratings.is_empty() && comment.is_none() {
            return Err(format!("rate an aspect ({}) as liked, disliked or clear, or give a `comment`", ASPECTS.join(", ")));
        }
        let entry = &mut b.songs[song];
        for (i, rating) in ratings {
            entry.ratings[i] = rating;
        }
        if let Some(comment) = comment {
            entry.feedback = comment.to_string();
        }
        b.save_ratings();
        Ok(Value::Obj(vec![("song".into(), Value::Int(song as i64 + 1)), ("rated".into(), Value::Bool(b.songs[song].rated()))]))
    }

    fn score_batch_status_json(&self) -> Value {
        let desk = &self.batch;
        let mut out = vec![
            ("running".to_string(), Value::Bool(desk.run.is_some())),
            ("learning".to_string(), Value::Bool(desk.learning.is_some())),
            ("not_responding".to_string(), Value::Bool(desk.not_responding)),
            ("note".to_string(), Value::Str(desk.note.clone())),
        ];
        if let Some(b) = &desk.batch {
            out.push(("batch".into(), Value::Str(format!("b{:03}", b.id))));
            out.push(("prompt".into(), Value::Str(b.request.prompt.clone())));
            out.push(("markers".into(), Value::Arr(b.request.markers.iter().cloned().map(Value::Str).collect())));
            let songs = b.songs.iter().enumerate().map(|(i, e)| {
                let mut song = vec![
                    ("song".to_string(), Value::Int(i as i64 + 1)),
                    ("state".into(), Value::Str(state_word(&e.state).into())),
                    ("title".into(), Value::Str(e.title.clone())),
                    ("idea".into(), Value::Str(e.idea.clone())),
                    ("seconds".into(), Value::F64(e.seconds.round())),
                ];
                if let State::Failed(why) = &e.state {
                    song.push(("error".into(), Value::Str(why.clone())));
                }
                if let Some(card) = &e.card {
                    song.push(("score".into(), Value::F64(f64::from(card.total().round()))));
                }
                let rated: Vec<(String, Value)> = ASPECTS.iter().zip(e.ratings).filter_map(|(a, r)| r.map(|r| (a.to_string(), Value::Str(if r { "liked" } else { "disliked" }.into())))).collect();
                song.push(("ratings".into(), Value::Obj(rated)));
                song.push(("comment".into(), Value::Str(e.feedback.clone())));
                Value::Obj(song)
            });
            out.push(("songs".into(), Value::Arr(songs.collect())));
        } else {
            out.push(("songs".into(), Value::Arr(Vec::new())));
        }
        Value::Obj(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Value {
        makepad_strict_json::parse(text.as_bytes()).unwrap()
    }

    #[test]
    fn batch_spec_defaults_and_fields() {
        let spec = batch_spec(&args(r#"{"prompt": " night drive "}"#)).unwrap();
        assert_eq!(spec, BatchSpec { prompt: "night drive".into(), markers: vec![], seconds: 30, count: 5 });
        let spec = batch_spec(&args(r#"{"prompt": "x", "markers": ["house", " dark ", ""], "seconds": 60, "count": 3}"#)).unwrap();
        assert_eq!(spec, BatchSpec { prompt: "x".into(), markers: vec!["house".into(), "dark".into()], seconds: 60, count: 3 });
    }

    #[test]
    fn batch_spec_refuses_what_the_form_could_not_send() {
        assert!(batch_spec(&args(r#"{}"#)).unwrap_err().contains("prompt"));
        assert!(batch_spec(&args(r#"{"prompt": "x", "seconds": 45}"#)).unwrap_err().contains("seconds"));
        assert!(batch_spec(&args(r#"{"prompt": "x", "count": 0}"#)).unwrap_err().contains("count"));
        assert!(batch_spec(&args(r#"{"prompt": "x", "markers": "house"}"#)).unwrap_err().contains("markers"));
    }

    #[test]
    fn rating_words() {
        assert_eq!(rating_of("liked"), Ok(Some(true)));
        assert_eq!(rating_of("disliked"), Ok(Some(false)));
        assert_eq!(rating_of("clear"), Ok(None));
        assert!(rating_of("meh").is_err());
    }
}
