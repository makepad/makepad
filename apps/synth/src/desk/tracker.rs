//! The tracker: the score's loops as channels on one time axis, each a
//! column of tracker cells or a piano roll, under a header with its live
//! waveform, mix and instrument. One model and one layout in (time,
//! channel) space; the orientation is the transform to the screen:
//! - vertical (the default): time flows down, channels are columns; while
//!   it plays the rows glide under the playhead fixed at the centre;
//! - horizontal: time to the right, channels are tracks; the play line
//!   moves across and the view pages when it reaches an edge.
//!
//! Every edit is a splice of the score's source (`tracker_model`), one
//! undo step per gesture; a control under a hand (level, pan, a send)
//! sounds held in the graph and is written on release. The keys:
//! arrows move (shift extends the selection), Tab/shift-Tab the channel,
//! PageUp/PageDown a bar; the two piano rows (Z S X … and Q 2 W …) enter
//! notes at the octave ([ ]) and advance by the step ({ }); 1 or ` is a
//! note-off, Delete clears, Backspace clears the row above; digits on the
//! velocity field set it; ⌘C/⌘X/⌘V copy, cut and paste, ⌘↑/⌘↓ transpose
//! (shift: an octave), ⌘L loops the selection, ⌘Z/⌘⇧Z undo and redo,
//! alt-←/→ the previous/next section, ⌘ or ctrl-scroll the rows per beat;
//! F2 the channel's settings, F3 the orientation, F4 tracker ⇄ roll.

use super::tracker_model::{self as tm, Channel, EvKind, Form, Src, TrackModel, View, FX_LANE};
use makepad_score_live::instruments::Choice;
use makepad_score_live::node::Slot;
use makepad_score_live::source::Splice;
use makepad_score_player::Meters;
use makepad_widgets::badge::advance;
use makepad_widgets::*;
use std::collections::HashMap;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    set_type_default() do #(DrawTrackBlock::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let w = self.rect_size.x
            let h = self.rect_size.y
            if self.shape > 2.5 {
                // A knob (as the lenses draw them): its track, the value's
                // arc from 7 o'clock round to 5 (from the top for a
                // bipolar one), a pointer, and the meter as an outer ring.
                let c = vec2(w * 0.5, h * 0.5)
                let r = min(w, h) * 0.5 - 2.0
                let a0 = 0.75 * 3.1415926
                let span = 1.5 * 3.1415926
                if self.meter > 0.002 {
                    sdf.arc_round_caps(c.x, c.y, r + 0.8, a0, a0 + span * min(self.meter, 1.0), 1.3)
                    sdf.fill(self.edge)
                }
                sdf.circle(c.x, c.y, r - 4.5)
                sdf.fill(self.body)
                sdf.arc_round_caps(c.x, c.y, r - 1.5, a0, a0 + span, 2.4)
                sdf.fill(self.body * vec4(1.6, 1.6, 1.6, 1.0))
                let from = mix(0.0, 0.5, self.bipolar)
                let lo = min(from, self.value)
                let hi = max(from, self.value)
                sdf.arc_round_caps(c.x, c.y, r - 1.5, a0 + span * lo, a0 + span * max(hi, lo + 0.001), 2.4)
                sdf.fill(self.fill)
                let a = a0 + span * self.value
                sdf.move_to(c.x + cos(a) * (r * 0.15), c.y + sin(a) * (r * 0.15))
                sdf.line_to(c.x + cos(a) * (r - 6.0), c.y + sin(a) * (r - 6.0))
                sdf.stroke(vec4(0.95, 0.95, 0.95, 1.0), 1.6)
                return sdf.result
            }
            if self.shape > 1.5 {
                // A triangle pointing along time (the playhead's mark).
                sdf.move_to(0.5, 0.5)
                sdf.line_to(w - 0.5, h * 0.5)
                sdf.line_to(0.5, h - 0.5)
                sdf.close_path()
                sdf.fill(self.fill)
                return sdf.result
            }
            if self.shape > 0.5 {
                // A note: its body, the start brighter.
                sdf.box(0.5, 0.5, w - 1., h - 1., self.radius)
                sdf.fill_keep(self.fill)
                sdf.stroke(self.edge, self.stroke)
                return sdf.result
            }
            sdf.box(0.5, 0.5, w - 1., h - 1., self.radius)
            sdf.fill_keep(self.fill)
            sdf.stroke(self.edge, self.stroke)
            return sdf.result
        }
    }

    mod.widgets.ScoreTrackerBase = #(ScoreTracker::register_widget(vm))
    mod.widgets.ScoreTracker = set_type_default() do mod.widgets.ScoreTrackerBase{
        width: Fill height: Fill
        draw_bg +: {color: stage.wave_bg}
        draw_rect +: {color: stage.wave_rim}
        draw_cell +: {color: stage.wave_ink text_style: theme.font_code{font_size: 8}}
        draw_text +: {color: stage.wave_label text_style: theme.font_regular{font_size: 7.5}}
        draw_bold +: {color: stage.glass_ink text_style: theme.font_bold{font_size: 8}}
        pop_rect +: {color: stage.menu}
        pop_text +: {color: stage.text text_style: theme.font_regular{font_size: 8.5}}
        pop_bold +: {color: stage.text_hi text_style: theme.font_bold{font_size: 8.5}}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTrackBlock {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub fill: Vec4f,
    #[live]
    pub edge: Vec4f,
    #[live]
    pub stroke: f32,
    #[live]
    pub radius: f32,
    /// 0 a box, 1 a note, 2 a triangle, 3 a knob.
    #[live]
    pub shape: f32,
    /// A knob's body colour, value (0..1), meter (0..1) and whether it
    /// turns both ways from the top (pan).
    #[live]
    pub body: Vec4f,
    #[live]
    pub value: f32,
    #[live]
    pub meter: f32,
    #[live]
    pub bipolar: f32,
}

#[derive(Clone, Debug, Default)]
pub enum TrackerAction {
    #[default]
    None,
    /// One gesture's change of the source (normalized splices).
    Edit { splices: Vec<Splice>, status: String },
    /// Play from this song beat.
    Seek(f64),
    /// Loop this range of song beats (None: stop looping).
    Loop(Option<(f64, f64)>),
    /// A loop's control held live (`amp`, `pan`) while a hand is on it.
    Hold { channel: String, field: &'static str, value: f32 },
    /// Audition this choice in place of instrument `inst` (None: stop).
    Preview(Option<(String, Choice)>),
    /// The settings opened (the desk hands over the instrument catalogue).
    Settings,
    Undo,
    Redo,
    Status(String),
    /// A channel pressed: its voice selected in the panel.
    Voice(String),
}

/// Where the music is, from the desk each turn: the output beat (the
/// audio clock's), the song beat minus it, the tempo, the loop.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrackerLive {
    pub playing: bool,
    pub beat: f64,
    pub song_off: Option<f64>,
    pub bpm: f64,
    pub looping: Option<(f64, f64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Orient {
    #[default]
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
struct Cursor {
    row: i64,
    ch: usize,
    lane: u8,
    /// 0 the note, 1 the velocity.
    field: u8,
    pitch: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Sel {
    r0: i64,
    r1: i64,
    c0: usize,
    c1: usize,
}

#[derive(Clone, Copy, Debug, Default)]
struct Geo {
    c0: f64,
    size: f64,
    view: View,
    lane: f64,
    key: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum HeadHit {
    Name,
    Gear,
    Inst,
    Mute,
    Solo,
    View,
    Level,
    Pan,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tool {
    PatPrev,
    PatNext,
    Orient,
    Follow,
    OctDown,
    OctUp,
    StepDown,
    StepUp,
    LpbDown,
    LpbUp,
    Chord,
    Loop,
}

#[derive(Clone, Debug)]
enum Drag {
    Select { anchor: (i64, usize) },
    Note { ch: usize, ev: usize, at: Vec2d, now: Vec2d, resize: bool },
    Fader { ch: usize, field: &'static str, value: f32, from: Vec2d, base: f32 },
    Ruler { from: f64, to: f64 },
    PopFader { what: PopFader, value: f32 },
}

#[derive(Clone, Debug, PartialEq)]
enum PopFader {
    Level,
    Pan,
    Send(String),
}

#[derive(Clone, Debug)]
struct ClipItem {
    ch_off: usize,
    beat: f64,
    pitch: f32,
    dur: f32,
    vel: f32,
    kind: EvKind,
}

/// A choice of the settings' instrument list.
#[derive(Clone, Debug)]
enum Pick {
    Score(String),
    Lib(usize),
}

#[derive(Clone, Debug, PartialEq)]
enum PopHit {
    Search,
    Name,
    Item(usize),
    Level,
    Pan,
    Send(String),
    View(Option<View>),
    Color(Option<&'static str>),
    /// Split the hit pattern into this many slots.
    Slots(usize),
    Close,
}

#[derive(Clone, Debug, Default)]
struct Popup {
    ch: String,
    search: String,
    name: Option<String>,
    /// The list shown: (what, its text).
    list: Vec<(Pick, String)>,
    scroll: f64,
    hover: Option<usize>,
    rect: Rect,
    list_rect: Rect,
    hits: Vec<(PopHit, Rect)>,
}

/// The chords the roll stamps.
const CHORDS: [(&str, &[f32]); 6] = [("note", &[0.0]), ("maj", &[0.0, 4.0, 7.0]), ("min", &[0.0, 3.0, 7.0]), ("7", &[0.0, 4.0, 7.0, 10.0]), ("m7", &[0.0, 3.0, 7.0, 10.0]), ("5th", &[0.0, 7.0])];
/// The colours a track can take: (atom, palette entry).
const COLORS: [&str; 8] = ["red", "amber", "green", "teal", "blue", "violet", "pink", "grey"];

const TOOLBAR: f64 = 26.0;
const HEAD_V: f64 = 104.0;
const HEAD_H: f64 = 264.0;
const RULER_V: f64 = 44.0;
const RULER_H: f64 = 18.0;
const ROW_V: f64 = 15.0;
const ROW_H: f64 = 12.0;
const WAVE: f64 = 9.0;
const LANE_H: f64 = 16.0;
const MIN_COL: f64 = 124.0;
const MIN_ROLL: f64 = 168.0;
const MIN_TRACK: f64 = 62.0;
const POP_W: f64 = 300.0;
const SCOPE_W: usize = 48;
/// A header knob's size, and how far a drag turns it end to end.
const KNOB: f64 = 24.0;
const KNOB_DRAG: f64 = 150.0;

#[derive(Script, ScriptHook, Widget)]
pub struct ScoreTracker {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_rect: DrawColor,
    #[live]
    draw_block: DrawTrackBlock,
    #[live]
    draw_cell: DrawText,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_bold: DrawText,
    #[live]
    pop_rect: DrawColor,
    #[live]
    pop_block: DrawTrackBlock,
    #[live]
    pop_text: DrawText,
    #[live]
    pop_bold: DrawText,
    #[rust]
    model: TrackModel,
    /// The source the model is of (what its splices are against).
    #[rust]
    source: String,
    /// Edits are possible (the source loads and is the model's).
    #[rust]
    editable: bool,
    #[rust]
    why: String,
    #[rust]
    orient: Orient,
    #[rust(4)]
    lpb: u32,
    #[rust(4)]
    octave: i32,
    #[rust(1)]
    step: i64,
    #[rust(true)]
    follow: bool,
    #[rust]
    chord: usize,
    #[rust]
    views: HashMap<String, View>,
    #[rust]
    geo: Vec<Geo>,
    #[rust]
    geo_dirty: bool,
    #[rust]
    total_c: f64,
    /// The channel axis's length the layout was made for.
    #[rust]
    laid_view: f64,
    /// The time axis placed once (the start at the centre).
    #[rust]
    placed: bool,
    #[rust]
    scroll_t: f64,
    #[rust]
    scroll_c: f64,
    /// Where the time axis eases to (stopped: the cursor kept in view).
    #[rust]
    target_t: Option<f64>,
    #[rust]
    cursor: Cursor,
    #[rust]
    sel: Option<Sel>,
    #[rust]
    drag: Option<Drag>,
    #[rust]
    hover: Option<Vec2d>,
    #[rust]
    clip: Vec<ClipItem>,
    #[rust]
    vel_digit: Option<(i64, usize, u32)>,
    #[rust]
    live: TrackerLive,
    /// The output beat, smoothed between the audio clock's updates: the
    /// last update, when it came, and the beat shown.
    #[rust]
    raw: (f64, f64),
    #[rust]
    base: (f64, f64),
    /// The song beat shown, and how far the vertical view has unfolded
    /// the loop or cycle it repeats.
    #[rust]
    song: Option<f64>,
    #[rust]
    unfold: f64,
    #[rust]
    last_song: Option<f64>,
    #[rust]
    popup: Option<Popup>,
    #[rust]
    catalog: Vec<Choice>,
    #[rust]
    meters: Option<Arc<dyn Meters>>,
    #[rust]
    slot: Option<Arc<Slot>>,
    #[rust]
    spans: (u64, Vec<Option<u16>>),
    #[rust]
    scope_buf: Vec<[f32; 4]>,
    /// Per channel: the header's live waveform (min, max per column) and
    /// the played envelope along the song (32 buckets a beat).
    #[rust]
    scopes: Vec<[[f32; 2]; SCOPE_W]>,
    #[rust]
    hist: Vec<Vec<[f32; 2]>>,
    #[rust]
    hist_at: Vec<i64>,
    #[rust]
    labels: Vec<String>,
    /// Each channel's level and pan as read under its knobs, and the one
    /// under a drag.
    #[rust]
    readouts: Vec<[String; 2]>,
    #[rust]
    drag_text: String,
    /// The settings' channel's slot count, as read.
    #[rust]
    slots_text: String,
    #[rust]
    texts: [String; 6],
    #[rust]
    tools: Vec<(Tool, Rect)>,
    #[rust]
    rect: Rect,
    #[rust]
    grid: Rect,
    #[rust]
    heads: Rect,
    #[rust]
    ruler: Rect,
    #[rust]
    cw: f64,
    #[rust]
    tick: NextFrame,
    #[rust]
    area: Area,
}

fn rgba(value: u32, alpha: f32) -> Vec4f {
    let mut c = Vec4f::from_u32(value);
    c.w *= alpha;
    c
}

fn level_text(amp: f32) -> String {
    format!("{:.0}%", amp * 100.0)
}

fn pan_text(pan: f32) -> String {
    format!("{pan:.2}")
}

/// The two piano rows: a key's semitone from the octave's C.
fn piano(key: KeyCode) -> Option<i32> {
    use KeyCode::*;
    Some(match key {
        KeyZ => 0, KeyS => 1, KeyX => 2, KeyD => 3, KeyC => 4, KeyV => 5, KeyG => 6, KeyB => 7, KeyH => 8, KeyN => 9, KeyJ => 10, KeyM => 11,
        Comma => 12, KeyL => 13, Period => 14, Semicolon => 15, Slash => 16,
        KeyQ => 12, Key2 => 13, KeyW => 14, Key3 => 15, KeyE => 16, KeyR => 17, Key5 => 18, KeyT => 19, Key6 => 20, KeyY => 21, Key7 => 22, KeyU => 23,
        KeyI => 24, Key9 => 25, KeyO => 26, Key0 => 27, KeyP => 28,
        _ => return None,
    })
}

/// A key's character for the search and name fields.
fn key_char(key: KeyCode, shift: bool) -> Option<char> {
    use KeyCode::*;
    let c = match key {
        KeyA => 'a', KeyB => 'b', KeyC => 'c', KeyD => 'd', KeyE => 'e', KeyF => 'f', KeyG => 'g', KeyH => 'h', KeyI => 'i', KeyJ => 'j', KeyK => 'k', KeyL => 'l', KeyM => 'm',
        KeyN => 'n', KeyO => 'o', KeyP => 'p', KeyQ => 'q', KeyR => 'r', KeyS => 's', KeyT => 't', KeyU => 'u', KeyV => 'v', KeyW => 'w', KeyX => 'x', KeyY => 'y', KeyZ => 'z',
        Key0 => '0', Key1 => '1', Key2 => '2', Key3 => '3', Key4 => '4', Key5 => '5', Key6 => '6', Key7 => '7', Key8 => '8', Key9 => '9',
        Space => ' ', Minus => if shift { '_' } else { '-' }, Period => '.',
        _ => return None,
    };
    Some(if shift { c.to_ascii_uppercase() } else { c })
}

fn digit(key: KeyCode) -> Option<u32> {
    use KeyCode::*;
    Some(match key {
        Key0 | Numpad0 => 0, Key1 | Numpad1 => 1, Key2 | Numpad2 => 2, Key3 | Numpad3 => 3, Key4 | Numpad4 => 4,
        Key5 | Numpad5 => 5, Key6 | Numpad6 => 6, Key7 | Numpad7 => 7, Key8 | Numpad8 => 8, Key9 | Numpad9 => 9,
        _ => return None,
    })
}

impl ScoreTracker {
    // ------------------------------------------------------------ input from the desk

    /// A new model: `editable` when it is the source's (else `why` not).
    pub fn set_model(&mut self, cx: &mut Cx, model: TrackModel, editable: bool, why: &str) {
        if model != self.model {
            let same_shape = model.channels.len() == self.model.channels.len() && model.song_len == self.model.song_len;
            self.model = model;
            self.geo_dirty = true;
            if !same_shape {
                self.reset_hist();
            }
            self.rebuild_labels();
            self.readouts = self.model.channels.iter().map(|c| [level_text(c.amp), pan_text(c.pan)]).collect();
            let n = self.model.channels.len();
            self.cursor.ch = self.cursor.ch.min(n.saturating_sub(1));
            self.cursor.row = self.cursor.row.clamp(0, self.rows().saturating_sub(1).max(0));
            if let Some(p) = &self.popup {
                if self.model.channel(&p.ch).is_none() {
                    self.popup = None;
                }
            }
        }
        self.editable = editable;
        if self.why != why {
            self.why = why.to_string();
        }
        self.redraw(cx);
    }

    pub fn set_editable(&mut self, cx: &mut Cx, editable: bool, why: &str) {
        if self.editable != editable || self.why != why {
            self.editable = editable;
            self.why = why.to_string();
            self.redraw(cx);
        }
    }

    pub fn set_live(&mut self, cx: &mut Cx, live: TrackerLive) {
        let was = self.live.playing;
        self.live = live;
        let now = Cx::time_now();
        if (live.beat - self.raw.0).abs() > 1e-9 || live.playing != was {
            self.raw = (live.beat, now);
            let predicted = self.base.0 + (now - self.base.1) * live.bpm.max(1.0) / 60.0;
            let err = live.beat - predicted;
            self.base = if !live.playing || !was || err.abs() > 0.5 { (live.beat, now) } else { (predicted + err * 0.15, now) };
        }
        if live.playing || was {
            self.redraw(cx);
        }
    }

    pub fn set_meters(&mut self, meters: Arc<dyn Meters>, slot: Arc<Slot>) {
        self.meters = Some(meters);
        self.slot = Some(slot);
    }

    pub fn set_catalog(&mut self, cx: &mut Cx, catalog: Vec<Choice>) {
        self.catalog = catalog;
        self.filter_popup();
        self.redraw(cx);
    }

    /// The settings are open, for this channel.
    pub fn settings_open(&self) -> Option<&str> {
        self.popup.as_ref().map(|p| p.ch.as_str())
    }

    // ------------------------------------------------------------ geometry

    fn vertical(&self) -> bool {
        self.orient == Orient::Vertical
    }

    fn row_px(&self) -> f64 {
        if self.vertical() { ROW_V } else { ROW_H }
    }

    fn rb(&self) -> f64 {
        1.0 / self.lpb.max(1) as f64
    }

    /// Rows of the timeline (the song, or the loops' cycle).
    fn rows(&self) -> i64 {
        (self.model.song_len.max(self.model.bar) * self.lpb as f64).ceil() as i64
    }

    fn view_t(&self) -> f64 {
        if self.vertical() { self.grid.size.y } else { self.grid.size.x }
    }

    fn view_c(&self) -> f64 {
        if self.vertical() { self.grid.size.x } else { self.grid.size.y }
    }

    fn view_of(&self, c: &Channel) -> View {
        self.views.get(&c.name).copied().unwrap_or(c.auto_view)
    }

    fn layout_channels(&mut self) {
        let vertical = self.vertical();
        let cw = self.cw.max(4.0);
        let mut c0 = 0.0;
        self.geo.clear();
        for c in &self.model.channels {
            let view = self.views.get(&c.name).copied().unwrap_or(c.auto_view);
            let keys = (c.hi - c.lo + 1.0).max(12.0) as f64;
            let fx = c.has_fx as u8 as f64;
            let g = if vertical {
                match view {
                    View::Tracker => {
                        let lane = cw * 7.0 + 4.0;
                        Geo { c0, size: (WAVE + c.lanes as f64 * lane + fx * (cw * 4.0 + 4.0) + 2.0).max(MIN_COL), view, lane, key: 0.0 }
                    }
                    View::Roll => {
                        let key = 7.0;
                        Geo { c0, size: (WAVE + keys * key + 2.0).max(MIN_ROLL), view, lane: 0.0, key }
                    }
                }
            } else {
                match view {
                    View::Tracker => Geo { c0, size: (WAVE + c.lanes as f64 * LANE_H + fx * 12.0 + 2.0).max(MIN_TRACK), view, lane: LANE_H, key: 0.0 },
                    View::Roll => {
                        let body = (keys * 4.0).clamp(MIN_TRACK, 240.0);
                        Geo { c0, size: WAVE + body + 2.0, view, lane: 0.0, key: body / keys }
                    }
                }
            };
            c0 += g.size;
            self.geo.push(g);
        }
        // Channels fill the width (the height, horizontal): the spare goes
        // to the rolls three to one.
        let view = self.view_c();
        if c0 < view && !self.geo.is_empty() {
            let weight = |g: &Geo| if g.view == View::Roll { 3.0 } else { 1.0 };
            let total: f64 = self.geo.iter().map(weight).sum();
            let extra = view - c0;
            let mut at = 0.0;
            for (i, g) in self.geo.iter_mut().enumerate() {
                let c = &self.model.channels[i];
                g.size += extra * weight(g) / total;
                g.c0 = at;
                at += g.size;
                let fx = c.has_fx as u8 as f64;
                let keys = (c.hi - c.lo + 1.0).max(12.0) as f64;
                match (g.view, vertical) {
                    (View::Roll, _) => g.key = (g.size - WAVE - 2.0) / keys,
                    (View::Tracker, true) => g.lane = ((g.size - WAVE - 2.0 - fx * (cw * 4.0 + 4.0)) / c.lanes.max(1) as f64).min(cw * 11.0).max(g.lane),
                    (View::Tracker, false) => g.lane = ((g.size - WAVE - 2.0 - fx * 12.0) / c.lanes.max(1) as f64).min(28.0).max(g.lane),
                }
            }
            c0 = at;
        }
        self.laid_view = view;
        self.total_c = c0;
        self.geo_dirty = false;
    }

    /// Content (t, c) to the screen.
    fn tc(&self, t0: f64, t1: f64, c0: f64, c1: f64) -> Rect {
        let (t0, t1) = (t0 - self.scroll_t, t1 - self.scroll_t);
        let (c0, c1) = (c0 - self.scroll_c, c1 - self.scroll_c);
        if self.vertical() {
            Rect { pos: dvec2(self.grid.pos.x + c0, self.grid.pos.y + t0), size: dvec2(c1 - c0, t1 - t0) }
        } else {
            Rect { pos: dvec2(self.grid.pos.x + t0, self.grid.pos.y + c0), size: dvec2(t1 - t0, c1 - c0) }
        }
    }

    /// The screen to content (t, c).
    fn at(&self, abs: Vec2d) -> (f64, f64) {
        let d = abs - self.grid.pos;
        if self.vertical() { (d.y + self.scroll_t, d.x + self.scroll_c) } else { (d.x + self.scroll_t, d.y + self.scroll_c) }
    }

    fn head_rect(&self, i: usize) -> Rect {
        let g = self.geo[i];
        if self.vertical() {
            Rect { pos: dvec2(self.grid.pos.x + g.c0 - self.scroll_c, self.heads.pos.y), size: dvec2(g.size, self.heads.size.y) }
        } else {
            Rect { pos: dvec2(self.heads.pos.x, self.grid.pos.y + g.c0 - self.scroll_c), size: dvec2(self.heads.size.x, g.size) }
        }
    }

    /// The channel at content c.
    fn channel_at(&self, c: f64) -> Option<usize> {
        self.geo.iter().position(|g| c >= g.c0 && c < g.c0 + g.size)
    }

    /// A pitch's c within its channel (the roll).
    fn pitch_c(&self, ch: &Channel, g: &Geo, pitch: f32) -> f64 {
        if self.vertical() { g.c0 + WAVE + (pitch - ch.lo) as f64 * g.key } else { g.c0 + WAVE + (ch.hi - pitch) as f64 * g.key }
    }

    fn pitch_at(&self, ch: &Channel, g: &Geo, c: f64) -> f32 {
        let k = ((c - g.c0 - WAVE) / g.key.max(1.0)).floor() as f32;
        if self.vertical() { (ch.lo + k).clamp(ch.lo, ch.hi) } else { (ch.hi - k).clamp(ch.lo, ch.hi) }
    }

    /// The song beat a content row shows: the cycle repeats; while a loop
    /// plays the vertical view runs on through it.
    fn content_beat(&self, row: i64) -> Option<f64> {
        let b = row as f64 * self.rb();
        let len = self.model.song_len.max(self.model.bar);
        if self.model.cycle {
            return Some(b.rem_euclid(len));
        }
        if self.vertical() && self.live.playing {
            if let Some((lo, hi)) = self.live.looping.filter(|(lo, hi)| hi > lo) {
                if b >= hi {
                    return Some(lo + (b - lo).rem_euclid(hi - lo));
                }
            }
        }
        (b >= 0.0 && b < len - 1e-9).then_some(b)
    }

    /// A content row as the timeline's row (0..rows).
    fn home_row(&self, row: i64) -> Option<i64> {
        self.content_beat(row).map(|b| (b * self.lpb as f64).round() as i64)
    }

    /// The pattern (section) playing at a song beat, and where that
    /// occurrence starts.
    fn pattern_at(&self, song: f64) -> Option<(usize, f64)> {
        if self.model.cycle {
            return (!self.model.patterns.is_empty()).then_some((0, 0.0));
        }
        self.model.patterns.iter().enumerate().find_map(|(i, p)| p.starts.iter().find(|s| song >= **s - 1e-9 && song < **s + p.len - 1e-9).map(|s| (i, *s)))
    }

    fn rebuild_labels(&mut self) {
        // Row numbers within their section (bar.beat at each beat).
        let rows = self.rows().max(0) as usize;
        self.labels.clear();
        let bar = self.model.bar.max(1.0);
        let lpb = self.lpb.max(1) as f64;
        for r in 0..rows.min(1 << 16) {
            let beat = r as f64 / lpb;
            let start = self.pattern_at(beat).map_or(0.0, |(_, s)| s);
            let local = ((beat - start) * lpb).round() as i64;
            let label = if r as f64 % lpb == 0.0 {
                let b = beat - start;
                format!("{}.{}", (b / bar).floor() as i64 + 1, (b % bar).floor() as i64 + 1)
            } else {
                format!("{local:02}")
            };
            self.labels.push(label);
        }
        self.texts[0] = format!("Oct {}", self.octave);
        self.texts[1] = format!("Step {}", self.step);
        self.texts[2] = format!("{} rows/beat", self.lpb);
        self.texts[3] = CHORDS[self.chord].0.to_string();
    }

    fn reset_hist(&mut self) {
        let n = self.model.channels.len();
        let len = ((self.model.song_len.max(self.model.bar)) * 32.0).ceil() as usize;
        self.hist = (0..n).map(|_| vec![[0.0, 0.0]; len.min(1 << 18)]).collect();
        self.hist_at = vec![-1; n];
        self.scopes = vec![[[0.0; 2]; SCOPE_W]; n];
        self.spans = (0, Vec::new());
    }

    // ------------------------------------------------------------ live state, once a frame

    /// The song beat shown now: the audio clock's beat run on at the tempo
    /// between its updates.
    fn song_now(&mut self) -> Option<f64> {
        let off = self.live.song_off?;
        let beat = if self.live.playing { self.base.0 + (Cx::time_now() - self.base.1) * self.live.bpm.max(1.0) / 60.0 } else { self.live.beat };
        Some(beat + off)
    }

    fn pump_live(&mut self) {
        let song = self.song_now();
        // The vertical view runs on through a loop or the cycle (no jump
        // back as it repeats): a step back by its length unfolds.
        let period = if self.model.cycle { Some(self.model.song_len.max(self.model.bar)) } else { self.live.looping.map(|(lo, hi)| hi - lo) };
        match (song, self.last_song, period) {
            (Some(s), Some(prev), Some(p)) if self.live.playing && s < prev - 0.5 && ((prev - s) - p).abs() < 1.0 => self.unfold += p,
            (Some(s), Some(prev), _) if (s - prev).abs() > 1.0 || !self.live.playing => self.unfold = 0.0,
            _ => {}
        }
        if !self.live.playing {
            self.unfold = 0.0;
        }
        self.last_song = song;
        self.song = song;
        self.pump_scopes(song);
    }

    /// The header waveforms and the played envelope, read from the audio
    /// thread's published passes (no allocation once the spans resolve).
    fn pump_scopes(&mut self, song: Option<f64>) {
        let Some(slot) = self.slot.clone() else { return };
        let generation = slot.playing();
        if generation == 0 {
            return;
        }
        if self.spans.0 != generation || self.spans.1.len() != self.model.channels.len() {
            let names: Vec<String> = self.model.channels.iter().map(|c| format!("{}/{}", crate::audio::SLOT, c.name)).collect();
            self.spans = slot.scope_spans(&names).unwrap_or((generation, vec![None; names.len()]));
            if self.scope_buf.len() != makepad_score_live::player::SCOPE_BINS {
                self.scope_buf = vec![[0.0; 4]; makepad_score_live::player::SCOPE_BINS];
            }
        }
        if self.scopes.len() != self.model.channels.len() {
            self.reset_hist();
            return;
        }
        let len = self.model.song_len.max(self.model.bar);
        for i in 0..self.model.channels.len() {
            let Some(span) = self.spans.1.get(i).copied().flatten() else { continue };
            if !slot.scope_read(self.spans.0, span, &mut self.scope_buf) {
                self.spans.0 = 0;
                return;
            }
            let bins = self.scope_buf.len();
            let (mut lo, mut hi) = (0.0f32, 0.0f32);
            for (k, out) in self.scopes[i].iter_mut().enumerate() {
                let (a, b) = (k * bins / SCOPE_W, ((k + 1) * bins / SCOPE_W).max(k * bins / SCOPE_W + 1));
                let (mut mn, mut mx) = (0.0f32, 0.0f32);
                for bin in &self.scope_buf[a..b.min(bins)] {
                    mn = mn.min(bin[0].min(bin[2]));
                    mx = mx.max(bin[1].max(bin[3]));
                }
                *out = [mn, mx];
                lo = lo.min(mn);
                hi = hi.max(mx);
            }
            // The envelope where the song is.
            if let (Some(s), true) = (song, self.live.playing) {
                let s = if self.model.cycle { s.rem_euclid(len) } else { s };
                let at = (s * 32.0).floor() as i64;
                let hist = &mut self.hist[i];
                if at >= 0 && (at as usize) < hist.len() {
                    let from = if self.hist_at[i] >= 0 && self.hist_at[i] < at && at - self.hist_at[i] < 64 { self.hist_at[i] + 1 } else { at };
                    for k in from..=at {
                        hist[k as usize] = [lo, hi];
                    }
                    self.hist_at[i] = at;
                }
            }
        }
    }

    /// The time axis this frame: playing, the vertical view keeps the
    /// playhead at its centre, the horizontal one pages; stopped, it eases
    /// to keep the cursor in view.
    fn scroll_frame(&mut self, cx: &mut Cx2d) {
        let row_px = self.row_px();
        let view = self.view_t();
        let px_beat = row_px * self.lpb as f64;
        if let (true, Some(song), true) = (self.live.playing, self.song, self.follow) {
            let play_t = (song + self.unfold) * px_beat;
            if self.vertical() {
                self.scroll_t = play_t - view * 0.5;
                self.target_t = None;
                return;
            }
            let margin = (view * 0.08).min(60.0);
            let shown = play_t - self.scroll_t;
            if shown < 0.0 || shown > view - margin {
                self.target_t = Some((play_t - margin).max(0.0));
            }
        }
        if let Some(target) = self.target_t {
            let d = target - self.scroll_t;
            if d.abs() < 0.5 {
                self.scroll_t = target;
                self.target_t = None;
            } else {
                // A short ease: about 120 ms to settle.
                self.scroll_t += d * 0.35;
                self.tick = cx.new_next_frame();
            }
        }
    }

    /// Ease the time axis to keep `row` in view.
    fn reveal(&mut self, row: i64) {
        let row_px = self.row_px();
        let view = self.view_t();
        let t = row as f64 * row_px;
        let cur = self.target_t.unwrap_or(self.scroll_t);
        let margin = (row_px * 3.0).min(view * 0.3);
        let target = if self.vertical() {
            // The fixed time position is the centre: the cursor's row sits there.
            let _ = margin;
            t + row_px * 0.5 - view * 0.5
        } else if t < cur + margin {
            (t - margin).max(0.0)
        } else if t > cur + view - margin - row_px {
            t - view + margin + row_px
        } else {
            cur
        };
        if (target - cur).abs() > 0.5 {
            self.target_t = Some(target);
        }
        // The channel axis: the cursor's channel in view.
        if let Some(g) = self.geo.get(self.cursor.ch) {
            let vc = self.view_c();
            if g.c0 < self.scroll_c {
                self.scroll_c = g.c0;
            } else if g.c0 + g.size > self.scroll_c + vc {
                self.scroll_c = (g.c0 + g.size - vc).min(g.c0);
            }
        }
    }

    // ------------------------------------------------------------ cells

    /// The events of channel `ch` in the timeline row `row` (lane `lane`;
    /// None: any), first first.
    fn events_at(&self, ch: usize, row: i64, lane: Option<u8>) -> Vec<usize> {
        let c = &self.model.channels[ch];
        let Some(song) = self.content_beat(row) else { return Vec::new() };
        let l = c.local(song);
        c.range(l, l + self.rb()).filter(|&i| lane.is_none_or(|lane| c.events[i].lane == lane)).collect()
    }

    /// The events of `ch` between two timeline rows, deduplicated (rows
    /// past a cycle map onto the same events).
    fn events_between(&self, ch: usize, r0: i64, r1: i64) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        for r in r0..=r1 {
            for i in self.events_at(ch, r, None) {
                if !out.contains(&i) {
                    out.push(i);
                }
            }
        }
        out
    }

    fn status(&self, cx: &mut Cx, text: &str) {
        cx.widget_action(self.uid, TrackerAction::Status(text.to_string()));
    }

    fn emit(&mut self, cx: &mut Cx, splices: Vec<Splice>, status: &str) {
        if !self.editable {
            let why = if self.why.is_empty() { "The tracker is read only right now".to_string() } else { self.why.clone() };
            self.status(cx, &why);
            return;
        }
        // A value written as it already is changes nothing (no undo step).
        let src = &self.source;
        let splices: Vec<Splice> = splices.into_iter().filter(|s| src.get(s.start..s.end) != Some(s.text.as_str())).collect();
        if splices.is_empty() {
            return;
        }
        match tm::normalize(splices) {
            Ok(splices) => cx.widget_action(self.uid, TrackerAction::Edit { splices, status: status.to_string() }),
            Err(e) => self.status(cx, &e),
        }
    }

    /// The channel's part can be written here, else say why.
    fn writable(&self, cx: &mut Cx, ch: usize) -> bool {
        let c = &self.model.channels[ch];
        if let Form::Fixed(why) = c.form {
            self.status(cx, &format!("{} {why}: read only here (edit it in the code)", c.label));
            return false;
        }
        true
    }

    /// The loop-local beat of a timeline row in a channel.
    fn local_of(&self, ch: usize, row: i64) -> Option<f64> {
        self.content_beat(row).map(|b| self.model.channels[ch].local(b))
    }

    /// The note list a lane writes into: the list its notes are in, else
    /// the first.
    fn list_of_lane(&self, ch: usize, lane: u8) -> usize {
        let c = &self.model.channels[ch];
        c.events.iter().find_map(|e| match e.src {
            Src::Entry(s) if e.lane == lane => Some(s.list as usize),
            _ => None,
        }).unwrap_or(0)
    }

    /// A note entered at the cursor (with the roll's chord): on a hit
    /// part a hit, on a note list the note at that row replaced or new.
    fn enter_note(&mut self, cx: &mut Cx, pitch: f32, from_key: bool) {
        let ch = self.cursor.ch;
        if ch >= self.model.channels.len() || !self.writable(cx, ch) {
            return;
        }
        let row = self.cursor.row;
        let Some(local) = self.local_of(ch, row) else { return };
        let c = &self.model.channels[ch];
        let view = self.view_of(c);
        let mut splices = Vec::new();
        match &c.form {
            Form::Hits { step, .. } => {
                let s = local / step;
                if (s - s.round()).abs() > 1e-6 {
                    self.status(cx, &format!("{}: no step on this row (its steps are {} beats)", c.label, makepad_score_live::source::fmt_num(*step, 3)));
                    return;
                }
                if let Some(sp) = tm::set_step(c, s.round() as usize, b'x') {
                    splices.push(sp);
                }
            }
            Form::Notes { .. } => {
                let lane = if view == View::Roll { None } else { Some(self.cursor.lane) };
                let here: Vec<usize> = self.events_at(ch, row, lane).into_iter().filter(|&i| matches!(c.events[i].kind, EvKind::Note | EvKind::Hit)).collect();
                let chord: &[f32] = if view == View::Roll { CHORDS[self.chord].1 } else { &[0.0] };
                // Its length: the step, cut at the lane's next note.
                let rb = self.rb();
                let mut dur = self.step.max(1) as f64 * rb;
                if let Some(next) = c.events.iter().find(|e| e.lane == self.cursor.lane && e.beat > local + 1e-9 && matches!(e.kind, EvKind::Note | EvKind::Hit)) {
                    dur = dur.min(next.beat - local);
                }
                if view != View::Roll && !here.is_empty() {
                    let e = &c.events[here[0]];
                    splices.extend(tm::change_event(c, e, e.beat, pitch, e.dur as f64, e.vel));
                } else {
                    // A note entered in a lane cuts the one sounding there (a
                    // tracker lane plays one note at a time).
                    if view != View::Roll {
                        let lane = self.cursor.lane;
                        if let Some(e) = c.events.iter().filter(|e| e.lane == lane && e.kind == EvKind::Note && e.beat < local - 1e-9 && e.beat + e.dur as f64 > local + 1e-9).last() {
                            splices.extend(tm::change_event(c, e, e.beat, e.pitch, local - e.beat, e.vel));
                        }
                    }
                    let list = self.list_of_lane(ch, self.cursor.lane);
                    for d in chord {
                        let p = (pitch + d).clamp(0.0, 127.0);
                        if c.events.iter().any(|e| (e.beat - local).abs() < 1e-6 && (e.pitch - p).abs() < 0.5) {
                            continue;
                        }
                        if let Some(sp) = tm::insert_entry(c, list, tm::entry_text(local, p, dur, 0.8, c.drums), local) {
                            splices.push(sp);
                        }
                    }
                }
            }
            Form::Fixed(_) => return,
        }
        self.emit(cx, splices, "note written");
        self.cursor.pitch = pitch;
        // The keys step on; a pointer stays where it pressed.
        if from_key {
            self.advance(1);
        }
    }

    /// A press on an empty cell adds there: one click on a hit pattern
    /// (a step sequencer), a double click on a note part.
    fn adds_here(&self, ch: usize, row: i64, taps: u32) -> bool {
        let c = &self.model.channels[ch];
        let lane = if self.view_of(c) == View::Roll { None } else { Some(self.cursor.lane) };
        let empty = self.events_at(ch, row, lane).is_empty();
        empty && match c.form {
            Form::Hits { .. } => taps <= 1,
            Form::Notes { .. } => taps >= 2,
            Form::Fixed(_) => false,
        }
    }

    /// The pitch a pointer adds: the last one entered, else the octave's C.
    fn pointer_pitch(&self) -> f32 {
        if self.cursor.pitch >= 1.0 { self.cursor.pitch } else { ((self.octave + 1) * 12) as f32 }
    }

    fn advance(&mut self, rows: i64) {
        let n = self.rows().max(1);
        self.cursor.row = (self.cursor.row + rows * self.step.max(0)).rem_euclid(n);
        let row = self.cursor.row;
        self.reveal(row);
    }

    /// A note-off at the cursor: the note sounding in the lane ends here.
    fn note_off(&mut self, cx: &mut Cx) {
        let ch = self.cursor.ch;
        if !self.writable(cx, ch) {
            return;
        }
        let Some(local) = self.local_of(ch, self.cursor.row) else { return };
        let c = &self.model.channels[ch];
        let sounding = c.events.iter().filter(|e| e.lane == self.cursor.lane && e.kind == EvKind::Note && e.beat < local - 1e-9).last();
        match sounding {
            Some(e) => {
                let splices = tm::change_event(c, e, e.beat, e.pitch, local - e.beat, e.vel);
                self.emit(cx, splices, "note-off");
                self.advance(1);
            }
            None => self.status(cx, "No note sounds in this lane to end here"),
        }
    }

    /// Clear the cell at the cursor.
    fn clear_cell(&mut self, cx: &mut Cx, row: i64) {
        let ch = self.cursor.ch;
        if !self.writable(cx, ch) {
            return;
        }
        let c = &self.model.channels[ch];
        let lane = if self.view_of(c) == View::Roll { None } else { Some(self.cursor.lane) };
        let mut splices = Vec::new();
        for i in self.events_at(ch, row, lane) {
            splices.extend(self.delete_splice(ch, i));
        }
        self.emit(cx, splices, "cleared");
    }

    fn delete_splice(&self, ch: usize, i: usize) -> Option<Splice> {
        let c = &self.model.channels[ch];
        match c.events[i].src {
            Src::Step(s) => tm::set_step(c, s, b'.'),
            Src::Entry(e) => tm::delete_entry("", c, &e),
            Src::None => None,
        }
    }

    /// A velocity digit at the cursor: the first sets tens, a second the units.
    fn enter_vel(&mut self, cx: &mut Cx, d: u32) {
        let ch = self.cursor.ch;
        if !self.writable(cx, ch) {
            return;
        }
        let row = self.cursor.row;
        let value = match self.vel_digit {
            Some((r, c, tens)) if r == row && c == ch => {
                self.vel_digit = None;
                (tens * 10 + d) as f32 / 99.0
            }
            _ => {
                self.vel_digit = Some((row, ch, d));
                (d * 10) as f32 / 99.0
            }
        }.clamp(0.0, 1.0);
        let c = &self.model.channels[ch];
        let lane = Some(self.cursor.lane);
        let mut splices = Vec::new();
        for i in self.events_at(ch, row, lane) {
            let e = &c.events[i];
            if matches!(e.kind, EvKind::Note | EvKind::Hit) {
                // A hit's digit is its character (1-9); 0 rests.
                let v = if matches!(c.form, Form::Hits { .. }) { if d == 0 { 0.0 } else { d as f32 / 10.0 } } else { value };
                if v <= 0.0 {
                    splices.extend(self.delete_splice(ch, i));
                } else {
                    splices.extend(tm::change_event(c, e, e.beat, e.pitch, e.dur as f64, v));
                }
            }
        }
        let hits = matches!(c.form, Form::Hits { .. });
        if splices.is_empty() {
            self.status(cx, "No note on this row to set the velocity of");
        }
        self.emit(cx, splices, "velocity");
        if self.vel_digit.is_none() || hits {
            self.vel_digit = None;
            self.advance(1);
        }
    }

    fn sel_or_cursor(&self) -> Sel {
        self.sel.unwrap_or(Sel { r0: self.cursor.row, r1: self.cursor.row, c0: self.cursor.ch, c1: self.cursor.ch })
    }

    fn copy(&mut self) {
        let s = self.sel_or_cursor();
        self.clip.clear();
        let base = s.r0 as f64 * self.rb();
        for ch in s.c0..=s.c1.min(self.model.channels.len().saturating_sub(1)) {
            for i in self.events_between(ch, s.r0, s.r1) {
                let e = &self.model.channels[ch].events[i];
                // Its offset from the selection's top, in this cycle.
                let row = (s.r0..=s.r1).find(|r| self.events_at(ch, *r, None).contains(&i)).unwrap_or(s.r0);
                let start = self.local_of(ch, row).unwrap_or(0.0);
                let beat = row as f64 * self.rb() - base + (e.beat - start).max(0.0);
                self.clip.push(ClipItem { ch_off: ch - s.c0, beat, pitch: e.pitch, dur: e.dur, vel: e.vel, kind: e.kind });
            }
        }
    }

    /// Clear the selection (cut, Delete with a selection).
    fn clear_sel(&mut self, cx: &mut Cx, status: &str) {
        let s = self.sel_or_cursor();
        let mut splices = Vec::new();
        for ch in s.c0..=s.c1.min(self.model.channels.len().saturating_sub(1)) {
            if !self.model.channels[ch].form.editable() {
                continue;
            }
            for i in self.events_between(ch, s.r0, s.r1) {
                splices.extend(self.delete_splice(ch, i));
            }
        }
        self.emit(cx, splices, status);
    }

    fn paste(&mut self, cx: &mut Cx) {
        if self.clip.is_empty() {
            return;
        }
        let rows = (self.clip.iter().map(|c| c.beat).fold(0.0, f64::max) * self.lpb as f64).floor() as i64;
        let (r0, c0) = (self.cursor.row, self.cursor.ch);
        let mut splices = Vec::new();
        let span = self.clip.iter().map(|c| c.ch_off).max().unwrap_or(0);
        for off in 0..=span {
            let ch = c0 + off;
            if ch >= self.model.channels.len() || !self.model.channels[ch].form.editable() {
                continue;
            }
            // What is there goes; the clip's notes come in. A hit pattern
            // gets its final character per step; a note list replaces an
            // entry in place where a pasted note lands on its beat.
            let c = &self.model.channels[ch];
            let Some(start) = self.local_of(ch, r0) else { continue };
            let gone = self.events_between(ch, r0, r0 + rows);
            let mut items: Vec<&ClipItem> = self.clip.iter().filter(|i| i.ch_off == off).collect();
            items.sort_by(|a, b| a.beat.total_cmp(&b.beat));
            let len = c.len.max(self.rb());
            match &c.form {
                Form::Hits { step, chars, .. } => {
                    let mut want: Vec<(usize, u8)> = gone.iter().filter_map(|&i| match c.events[i].src {
                        Src::Step(s) => Some((s, b'.')),
                        _ => None,
                    }).collect();
                    for item in items {
                        let s = (start + item.beat).rem_euclid(len) / step;
                        if (s - s.round()).abs() < 1e-6 && (s.round() as usize) < chars.len() {
                            want.retain(|(k, _)| *k != s.round() as usize);
                            want.push((s.round() as usize, tm::hit_char(item.vel)));
                        }
                    }
                    for (k, ch_byte) in want {
                        splices.extend(tm::set_step(c, k, ch_byte));
                    }
                }
                Form::Notes { .. } => {
                    let mut dels: Vec<(f64, tm::EntrySrc)> = gone.iter().filter_map(|&i| match c.events[i].src {
                        Src::Entry(e) => Some((c.events[i].beat, e)),
                        _ => None,
                    }).collect();
                    for item in items {
                        let local = (start + item.beat).rem_euclid(len);
                        let text = match item.kind {
                            EvKind::Program(p) => format!("[{} @program {p}]", makepad_score_live::source::fmt_num(local, 4)),
                            EvKind::Pedal => format!("[{} @pedal {}]", makepad_score_live::source::fmt_num(local, 4), makepad_score_live::source::fmt_num(item.vel as f64, 2)),
                            _ => tm::entry_text(local, item.pitch, item.dur as f64, item.vel, c.drums),
                        };
                        match dels.iter().position(|(b, _)| (b - local).abs() < 1e-9) {
                            Some(k) => {
                                let (_, e) = dels.remove(k);
                                splices.push(Splice { start: e.start, end: e.end, text });
                            }
                            None => splices.extend(tm::insert_entry(c, 0, text, local)),
                        }
                    }
                    for (_, e) in dels {
                        splices.extend(tm::delete_entry("", c, &e));
                    }
                }
                Form::Fixed(_) => {}
            }
        }
        self.emit(cx, splices, "pasted");
    }

    fn transpose(&mut self, cx: &mut Cx, by: f32) {
        let s = self.sel_or_cursor();
        let mut splices = Vec::new();
        for ch in s.c0..=s.c1.min(self.model.channels.len().saturating_sub(1)) {
            let c = &self.model.channels[ch];
            if !matches!(c.form, Form::Notes { .. }) {
                continue;
            }
            for i in self.events_between(ch, s.r0, s.r1) {
                let e = &c.events[i];
                if e.kind == EvKind::Note {
                    splices.extend(tm::change_event(c, e, e.beat, (e.pitch + by).clamp(0.0, 127.0), e.dur as f64, e.vel));
                }
            }
        }
        if splices.is_empty() {
            self.status(cx, "No notes to transpose here (a hit pattern plays one note: set it with note:)");
        }
        self.emit(cx, splices, "transposed");
    }

    // ------------------------------------------------------------ mix and settings

    fn write_field(&mut self, cx: &mut Cx, ch: usize, key: &str, value: String, status: &str) {
        let Some(c) = self.model.channels.get(ch) else { return };
        // The source's own bytes: the model's spans are of it.
        match makepad_score_live::source::set_field(&self.source_hint(), &c.name, key, &value) {
            Ok(sp) => self.emit(cx, vec![sp], status),
            Err(e) => self.status(cx, &e),
        }
    }

    /// The source the model was built from (kept for field writes).
    fn source_hint(&self) -> String {
        self.source.clone()
    }

    fn open_settings(&mut self, cx: &mut Cx, ch: usize) {
        let Some(name) = self.model.channels.get(ch).map(|c| c.name.clone()) else { return };
        self.popup = Some(Popup { ch: name.clone(), ..Popup::default() });
        self.filter_popup();
        cx.widget_action(self.uid, TrackerAction::Settings);
        cx.widget_action(self.uid, TrackerAction::Voice(name));
    }

    fn filter_popup(&mut self) {
        let Some(p) = &mut self.popup else { return };
        let q = p.search.to_lowercase();
        let hit = |s: &str| q.is_empty() || s.to_lowercase().contains(&q);
        p.list.clear();
        for inst in &self.model.insts {
            let text = format!("{inst}  ·  this score");
            if hit(&text) {
                p.list.push((Pick::Score(inst.clone()), text));
            }
        }
        for (i, choice) in self.catalog.iter().enumerate() {
            if hit(&choice.label) || hit(choice.group) {
                p.list.push((Pick::Lib(i), format!("{}  ·  {}", choice.label, choice.group)));
            }
        }
        p.scroll = p.scroll.min((p.list.len() as f64 * 18.0 - 100.0).max(0.0));
    }

    /// Pick an instrument for the settings' channel: one of the score's
    /// (`play:` written), or a library one (the instrument node turned
    /// into it when only this loop plays it, else a new node for it).
    fn pick(&mut self, cx: &mut Cx, index: usize) {
        let Some(p) = &self.popup else { return };
        let Some(ch) = self.model.channel(&p.ch) else { return };
        let Some((pick, _)) = p.list.get(index).cloned() else { return };
        let c = self.model.channels[ch].clone();
        let src = self.source.clone();
        match pick {
            Pick::Score(inst) => self.write_field(cx, ch, "play", inst, "instrument picked"),
            Pick::Lib(i) => {
                let Some(choice) = self.catalog.get(i).cloned() else { return };
                let shared = c.play.as_ref().is_none_or(|inst| self.model.channels.iter().filter(|o| o.play.as_ref() == Some(inst)).count() > 1);
                match (&c.play, shared) {
                    (Some(inst), false) => match makepad_score_live::instruments::switch(&src, inst, &choice) {
                        Ok(sp) => self.emit(cx, vec![sp], &format!("{inst} is now {}", choice.label)),
                        Err(e) => self.status(cx, &e),
                    },
                    _ => {
                        // A node of its own: `let <loop>_inst = …`, listed in the Score, played.
                        let outline = makepad_score_live::source::outline(&src);
                        let Some(root) = &outline.root else { return };
                        let mut name = format!("{}_inst", c.name);
                        let mut k = 2;
                        while outline.node(&name).is_some() {
                            name = format!("{}_inst{k}", c.name);
                            k += 1;
                        }
                        let fields: Vec<String> = choice.fields.iter().map(|(k, v)| format!("{k}: {v}")).collect();
                        let node = Splice { start: root.span.start, end: root.span.start, text: format!("let {name} = {}{{{}}}\n", choice.type_name, fields.join(" ")) };
                        let close = root.body.end - 1;
                        let listed = Splice { start: close, end: close, text: format!(" {name}") };
                        match makepad_score_live::source::set_field(&src, &c.name, "play", &name) {
                            Ok(play) => self.emit(cx, vec![node, play, listed], &format!("{} plays {}", c.label, choice.label)),
                            Err(e) => self.status(cx, &e),
                        }
                    }
                }
            }
        }
        cx.widget_action(self.uid, TrackerAction::Preview(None));
    }

    fn preview(&mut self, cx: &mut Cx, index: Option<usize>) {
        let Some(p) = &self.popup else { return };
        let Some(ch) = self.model.channel(&p.ch) else { return };
        let inst = self.model.channels[ch].play.clone();
        let choice = index.and_then(|i| p.list.get(i)).and_then(|(pick, _)| match pick {
            Pick::Lib(i) => self.catalog.get(*i).cloned(),
            Pick::Score(_) => None,
        });
        let action = match (inst, choice) {
            (Some(inst), Some(choice)) => Some((inst, choice)),
            _ => None,
        };
        cx.widget_action(self.uid, TrackerAction::Preview(action));
    }
}

/// The source the model was built from; the desk sets it with the model.
impl ScoreTracker {
    pub fn set_source(&mut self, source: &str) {
        if self.source != source {
            self.source = source.to_string();
        }
    }
}

// ------------------------------------------------------------ drawing

fn fill(d: &mut DrawColor, cx: &mut Cx2d, r: Rect, c: Vec4f) {
    if r.size.x > 0.0 && r.size.y > 0.0 {
        d.color = c;
        d.draw_abs(cx, r);
    }
}

#[allow(clippy::too_many_arguments)]
fn block(d: &mut DrawTrackBlock, cx: &mut Cx2d, r: Rect, shape: f32, f: Vec4f, edge: Vec4f, stroke: f32, radius: f32) {
    if r.size.x > 0.0 && r.size.y > 0.0 {
        d.fill = f;
        d.edge = edge;
        d.stroke = stroke;
        d.radius = radius;
        d.shape = shape;
        d.draw_abs(cx, r);
    }
}

#[allow(clippy::too_many_arguments)]
fn knob(d: &mut DrawTrackBlock, cx: &mut Cx2d, r: Rect, value: f32, meter: f32, bipolar: bool, fill: Vec4f, ring: Vec4f, body: Vec4f) {
    d.shape = 3.0;
    d.value = value;
    d.meter = meter;
    d.bipolar = bipolar as u8 as f32;
    d.fill = fill;
    d.edge = ring;
    d.body = body;
    d.draw_abs(cx, r);
    d.meter = 0.0;
}

fn put(d: &mut DrawText, cx: &mut Cx2d, pos: Vec2d, s: &str, c: Vec4f) {
    if !s.is_empty() {
        d.color = c;
        d.draw_abs(cx, pos, s);
    }
}

/// `s` cut to `n` characters (no allocation).
fn first_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((at, _)) => &s[..at],
        None => s,
    }
}

/// A header's parts.
struct HeadRects {
    name: Rect,
    gear: Rect,
    inst: Rect,
    mute: Rect,
    solo: Rect,
    view: Rect,
    tag: Vec2d,
    /// The knobs, and where their name and value read.
    level: Rect,
    pan: Rect,
    level_text: Vec2d,
    pan_text: Vec2d,
    wave: Rect,
}

fn head_rects(r: Rect, vertical: bool) -> HeadRects {
    let (x, y, w, h) = (r.pos.x, r.pos.y, r.size.x, r.size.y);
    let rr = |x: f64, y: f64, w: f64, h: f64| Rect { pos: dvec2(x, y), size: dvec2(w.max(0.0), h.max(0.0)) };
    if vertical {
        HeadRects {
            name: rr(x + 7.0, y + 4.0, w - 30.0, 14.0),
            gear: rr(x + w - 22.0, y + 4.0, 16.0, 14.0),
            inst: rr(x + 7.0, y + 19.0, w - 12.0, 13.0),
            mute: rr(x + 7.0, y + 35.0, 17.0, 15.0),
            solo: rr(x + 27.0, y + 35.0, 17.0, 15.0),
            view: rr(x + 47.0, y + 35.0, 30.0, 15.0),
            tag: dvec2(x + 81.0, y + 37.0),
            wave: rr(x + 5.0, y + 53.0, w - 10.0, 20.0),
            level: rr(x + 6.0, y + 76.0, KNOB, KNOB),
            pan: rr(x + 6.0 + (w - 12.0) * 0.5, y + 76.0, KNOB, KNOB),
            level_text: dvec2(x + 6.0 + KNOB + 3.0, y + 78.0),
            pan_text: dvec2(x + 6.0 + (w - 12.0) * 0.5 + KNOB + 3.0, y + 78.0),
        }
    } else {
        let right = x + 140.0;
        HeadRects {
            name: rr(x + 8.0, y + 4.0, 104.0, 14.0),
            gear: rr(x + 116.0, y + 4.0, 16.0, 14.0),
            inst: rr(x + 8.0, y + 19.0, 126.0, 13.0),
            mute: rr(x + 8.0, y + 35.0, 17.0, 15.0),
            solo: rr(x + 28.0, y + 35.0, 17.0, 15.0),
            view: rr(x + 48.0, y + 35.0, 30.0, 15.0),
            tag: dvec2(x + 82.0, y + 37.0),
            wave: rr(right, y + 4.0, w - 148.0, (h - KNOB - 10.0).max(8.0)),
            level: rr(right, y + h - KNOB - 3.0, KNOB, KNOB),
            pan: rr(right + (w - 148.0) * 0.5, y + h - KNOB - 3.0, KNOB, KNOB),
            level_text: dvec2(right + KNOB + 3.0, y + h - KNOB - 1.0),
            pan_text: dvec2(right + (w - 148.0) * 0.5 + KNOB + 3.0, y + h - KNOB - 1.0),
        }
    }
}

impl ScoreTracker {
    fn color_of(&self, p: &makepad_look::palette::Palette, ch: usize) -> u32 {
        self.color_named(p, self.model.channels[ch].color.as_deref(), ch)
    }

    /// A channel's colour as `atom` would make it (None: its own default).
    fn color_named(&self, p: &makepad_look::palette::Palette, atom: Option<&str>, ch: usize) -> u32 {
        let named = |a: &str| match a {
            "red" => Some(p.danger),
            "amber" => Some(p.amber),
            "green" => Some(p.found),
            "teal" => Some(p.teal),
            "blue" => Some(p.info),
            "violet" => Some(p.key),
            "pink" => Some(p.wave_vocals),
            "grey" => Some(p.muted),
            _ => None,
        };
        atom.and_then(named).unwrap_or_else(|| {
            let c = &self.model.channels[ch];
            if c.drums { p.wave_drums } else { [p.accent, p.teal, p.amber, p.info, p.wave_vocals, p.key, p.wave_bass, p.found][ch % 8] }
        })
    }

    fn regions(&mut self, rect: Rect) {
        let (x, y, w, h) = (rect.pos.x, rect.pos.y, rect.size.x, rect.size.y);
        let r = |x: f64, y: f64, w: f64, h: f64| Rect { pos: dvec2(x, y), size: dvec2(w.max(0.0), h.max(0.0)) };
        if self.vertical() {
            self.heads = r(x + RULER_V, y + TOOLBAR, w - RULER_V, HEAD_V);
            self.ruler = r(x, y + TOOLBAR + HEAD_V, RULER_V, h - TOOLBAR - HEAD_V);
            self.grid = r(x + RULER_V, y + TOOLBAR + HEAD_V, w - RULER_V, h - TOOLBAR - HEAD_V);
        } else {
            self.ruler = r(x + HEAD_H, y + TOOLBAR, w - HEAD_H, RULER_H);
            self.heads = r(x, y + TOOLBAR + RULER_H, HEAD_H, h - TOOLBAR - RULER_H);
            self.grid = r(x + HEAD_H, y + TOOLBAR + RULER_H, w - HEAD_H, h - TOOLBAR - RULER_H);
        }
    }

    fn clamp_scroll(&mut self) {
        let max_c = (self.total_c - self.view_c()).max(0.0);
        self.scroll_c = self.scroll_c.clamp(0.0, max_c);
        if self.live.playing && self.follow {
            return;
        }
        let len = self.rows() as f64 * self.row_px();
        let view = self.view_t();
        let (lo, hi) = if self.vertical() { (-view * 0.5, (len - view * 0.5).max(-view * 0.5)) } else { (0.0, (len - view * 0.8).max(0.0)) };
        if !self.model.cycle {
            self.scroll_t = self.scroll_t.clamp(lo, hi);
        }
    }

    fn visible_channels(&self) -> std::ops::Range<usize> {
        let (a, b) = (self.scroll_c, self.scroll_c + self.view_c());
        let first = self.geo.iter().position(|g| g.c0 + g.size > a).unwrap_or(self.geo.len());
        let last = self.geo.iter().rposition(|g| g.c0 < b).map_or(first, |i| i + 1);
        first..last.max(first)
    }

    fn draw_grid(&mut self, cx: &mut Cx2d, p: &makepad_look::palette::Palette) {
        let grid = self.grid;
        cx.push_clip_rect(grid);
        let row_px = self.row_px();
        let rb = self.rb();
        let lpb = self.lpb.max(1) as i64;
        let bar_rows = ((self.model.bar * lpb as f64).round() as i64).max(1);
        let px_beat = row_px * lpb as f64;
        let first = (self.scroll_t / row_px).floor() as i64;
        let last = ((self.scroll_t + self.view_t()) / row_px).ceil() as i64;
        let chans = self.visible_channels();
        let any_solo = self.model.channels.iter().any(|c| c.solo);
        let vertical = self.vertical();
        let cw = self.cw;
        let total_c = self.total_c;
        let (t_lo, t_hi) = (first as f64 * row_px, (last + 1) as f64 * row_px);
        // Columns first: grounds, rolls' black keys, rules between channels.
        for ch in chans.clone() {
            let g = self.geo[ch];
            let c = &self.model.channels[ch];
            let ground = self.tc(t_lo, t_hi, g.c0, g.c0 + g.size);
            fill(&mut self.draw_rect, cx, ground, rgba(if ch % 2 == 0 { p.wave_bg } else { p.panel }, 1.0));
            if g.view == View::Roll {
                let mut k = c.lo;
                while k <= c.hi {
                    let pc = k.rem_euclid(12.0) as i32;
                    if matches!(pc, 1 | 3 | 6 | 8 | 10) {
                        let x0 = self.pitch_c(c, &g, k);
                        let (a, b) = if vertical { (x0, x0 + g.key) } else { (x0, x0 + g.key) };
                        let r = self.tc(t_lo, t_hi, a, b);
                        fill(&mut self.draw_rect, cx, r, rgba(p.bg, 0.45));
                    } else if pc == 0 {
                        let x0 = self.pitch_c(c, &g, k);
                        let edge = if vertical { x0 } else { x0 + g.key - 1.0 };
                        let r = self.tc(t_lo, t_hi, edge, edge + 1.0);
                        fill(&mut self.draw_rect, cx, r, rgba(p.line_faint, 1.0));
                    }
                    k += 1.0;
                }
            }
            let strip = self.tc(t_lo, t_hi, g.c0, g.c0 + WAVE);
            fill(&mut self.draw_rect, cx, strip, rgba(p.bg, 0.5));
            let rule = self.tc(t_lo, t_hi, g.c0 + g.size - 1.0, g.c0 + g.size);
            fill(&mut self.draw_rect, cx, rule, rgba(p.line, 1.0));
        }
        let sel = self.sel;
        let cursor = self.cursor;
        let focused = cx.has_key_focus(self.area);
        for r in first..=last {
            let t0 = r as f64 * row_px;
            let Some(song) = self.content_beat(r) else {
                let dead = self.tc(t0, t0 + row_px, 0.0, total_c);
                fill(&mut self.draw_rect, cx, dead, rgba(p.bg, 0.8));
                continue;
            };
            let home = (song * lpb as f64).round() as i64;
            let pattern = self.pattern_at(song);
            if home % lpb == 0 {
                let band = self.tc(t0, t0 + row_px, 0.0, total_c);
                fill(&mut self.draw_rect, cx, band, rgba(p.row_alt, 0.9));
            }
            let starts_pattern = pattern.is_some_and(|(_, s)| (s - song).abs() < 1e-9) && !self.model.cycle;
            if home % bar_rows == 0 || starts_pattern {
                let line = self.tc(t0, t0 + 1.0, 0.0, total_c);
                fill(&mut self.draw_rect, cx, line, rgba(if starts_pattern { p.accent } else { p.wave_grid }, if starts_pattern { 0.6 } else { 1.0 }));
            }
            if let Some(s) = sel.filter(|s| home >= s.r0 && home <= s.r1) {
                if let (Some(a), Some(b)) = (self.geo.get(s.c0), self.geo.get(s.c1)) {
                    let band = self.tc(t0, t0 + row_px, a.c0, b.c0 + b.size);
                    fill(&mut self.draw_rect, cx, band, rgba(p.accent, 0.14));
                }
            }
            for ch in chans.clone() {
                let g = self.geo[ch];
                let c = &self.model.channels[ch];
                let plays = match pattern {
                    Some((pi, _)) => self.model.patterns[pi].loops.iter().any(|n| n == &c.name),
                    None => self.model.cycle,
                };
                let muted = c.mute || (any_solo && !c.solo);
                let col = self.color_of(p, ch);
                let dim = if !plays || muted { 0.35 } else { 1.0 };
                if !plays {
                    let rest = self.tc(t0, t0 + row_px, g.c0 + WAVE, g.c0 + g.size - 1.0);
                    fill(&mut self.draw_rect, cx, rest, rgba(p.bg, 0.4));
                    // Where a rest starts (or the view's top): it says so.
                    let l = c.local(song);
                    if (starts_pattern || r == first) && vertical && c.range(l, l + rb).is_empty() {
                        let at = rest.pos + dvec2(4.0, 2.0);
                        put(&mut self.draw_text, cx, at, "rests in", rgba(p.wave_label, 0.8));
                        if let Some((pi, _)) = pattern {
                            put(&mut self.draw_text, cx, at + dvec2(40.0, 0.0), &self.model.patterns[pi].name, rgba(p.wave_label, 0.8));
                        }
                    }
                }
                // The played envelope, 3 bars a row.
                if let Some(hist) = self.hist.get(ch) {
                    for k in 0..3 {
                        let (b0, b1) = (song + rb * k as f64 / 3.0, song + rb * (k + 1) as f64 / 3.0);
                        let (i0, i1) = ((b0 * 32.0).floor().max(0.0) as usize, ((b1 * 32.0).ceil() as usize).max((b0 * 32.0).floor() as usize + 1));
                        let (mut lo, mut hi) = (0.0f32, 0.0f32);
                        for v in hist.get(i0..i1.min(hist.len())).unwrap_or(&[]) {
                            lo = lo.min(v[0]);
                            hi = hi.max(v[1]);
                        }
                        let amp = (hi - lo).min(2.0) as f64 * 0.5;
                        if amp > 0.002 {
                            let mid = g.c0 + WAVE * 0.5;
                            let half = (amp * WAVE * 0.5).max(0.5);
                            let tt = t0 + row_px * k as f64 / 3.0;
                            let r = self.tc(tt, tt + row_px / 3.0, mid - half, mid + half);
                            fill(&mut self.draw_rect, cx, r, rgba(col, 0.75 * dim as f32));
                        }
                    }
                }
                let l0 = c.local(song);
                let l1 = l0 + rb;
                // What sounds in this row: from before it (the longest note back) to its end.
                let back = c.events.partition_point(|e| e.beat < l0 - 64.0);
                let end = c.events.partition_point(|e| e.beat < l1 - 1e-9);
                let editable = c.form.editable();
                let ink = rgba(if editable { p.wave_ink } else { p.wave_muted }, dim as f32);
                match (g.view, vertical) {
                    (View::Tracker, true) => {
                        let mut started = 0u16;
                        let mut more = 0u16;
                        for i in c.range(l0, l1) {
                            let e = &c.events[i];
                            let lane_x = if e.lane == FX_LANE { g.c0 + WAVE + c.lanes as f64 * g.lane } else { g.c0 + WAVE + e.lane.min(15) as f64 * g.lane };
                            let bit = 1u16 << (e.lane.min(15));
                            if started & bit != 0 {
                                more |= bit;
                                continue;
                            }
                            started |= bit;
                            let r = self.tc(t0, t0 + row_px, lane_x, lane_x + g.lane);
                            let off_grid = ((e.beat - l0) * lpb as f64 - ((e.beat - l0) * lpb as f64).round()).abs() > 1e-3 || e.beat > l0 + 1e-6;
                            let text_c = if e.kind == EvKind::Note || e.kind == EvKind::Hit { rgba(col, dim as f32).max_with(ink) } else { rgba(p.info, dim as f32) };
                            put(&mut self.draw_cell, cx, r.pos + dvec2(2.0, 2.0), &e.text, text_c);
                            put(&mut self.draw_cell, cx, r.pos + dvec2(2.0 + cw * 4.0, 2.0), &e.vel_text, rgba(p.wave_label, dim as f32));
                            if off_grid {
                                let m = Rect { pos: r.pos + dvec2(1.0, 1.0), size: dvec2(2.0, 2.0) };
                                fill(&mut self.draw_rect, cx, m, rgba(p.amber, 0.9));
                            }
                        }
                        // Notes sounding on (a line in their lane) and ending here (===).
                        for e in &c.events[back..end] {
                            if e.kind != EvKind::Note || e.lane == FX_LANE {
                                continue;
                            }
                            let bit = 1u16 << e.lane.min(15);
                            let e_end = e.beat + e.dur as f64;
                            let lane_x = g.c0 + WAVE + e.lane.min(15) as f64 * g.lane;
                            if e.beat < l0 - 1e-9 && e_end > l1 - 1e-9 && started & bit == 0 {
                                let r = self.tc(t0, t0 + row_px, lane_x + 4.0, lane_x + 5.5);
                                fill(&mut self.draw_rect, cx, r, rgba(col, 0.35 * dim as f32));
                            } else if e_end > l0 + 1e-9 && e_end <= l1 + 1e-9 && e_end > e.beat + rb * 0.5 && started & bit == 0 && e.beat < l0 - 1e-9 {
                                let r = self.tc(t0, t0 + row_px, lane_x, lane_x + g.lane);
                                put(&mut self.draw_cell, cx, r.pos + dvec2(2.0, 2.0), "===", rgba(p.wave_label, 0.6 * dim as f32));
                            }
                        }
                        if more != 0 {
                            let r = self.tc(t0, t0 + row_px, g.c0 + g.size - 8.0, g.c0 + g.size - 2.0);
                            put(&mut self.draw_cell, cx, r.pos + dvec2(0.0, 2.0), "+", rgba(p.amber, 1.0));
                        }
                    }
                    (View::Tracker, false) | (View::Roll, _) => {
                        for e in &c.events[back..end] {
                            let e_end = e.beat + (e.dur as f64).max(rb * 0.6);
                            if e_end <= l0 + 1e-9 {
                                continue;
                            }
                            let (s0, s1) = (e.beat.max(l0), e_end.min(l1));
                            if s1 <= s0 {
                                continue;
                            }
                            let ta = t0 + (s0 - l0) * px_beat;
                            let tb = t0 + (s1 - l0) * px_beat;
                            let (ca, cb) = if e.lane == FX_LANE || !matches!(e.kind, EvKind::Note | EvKind::Hit) {
                                (g.c0 + 1.0, g.c0 + WAVE - 1.0)
                            } else if g.view == View::Roll {
                                let pc = self.pitch_c(c, &g, e.pitch);
                                (pc + 0.5, pc + g.key - 0.5)
                            } else {
                                let lx = g.c0 + WAVE + e.lane.min(15) as f64 * g.lane;
                                (lx + 1.0, lx + g.lane - 1.0)
                            };
                            let r = self.tc(ta, tb, ca, cb);
                            let a = (0.35 + 0.65 * e.vel) * dim as f32;
                            let starts = e.beat >= l0 - 1e-9;
                            fill(&mut self.draw_rect, cx, r, rgba(col, if editable { a } else { a * 0.6 }));
                            if starts {
                                let cap = if vertical { Rect { pos: r.pos, size: dvec2(r.size.x, 2.0_f64.min(r.size.y)) } } else { Rect { pos: r.pos, size: dvec2(2.0_f64.min(r.size.x), r.size.y) } };
                                fill(&mut self.draw_rect, cx, cap, rgba(p.text_hi, 0.8 * dim as f32));
                                // Its name when the whole note has room for it.
                                if !vertical && g.view == View::Tracker && (e.dur as f64).max(rb * 0.6) * px_beat >= cw * 3.0 + 5.0 {
                                    put(&mut self.draw_cell, cx, r.pos + dvec2(3.0, 2.0), &e.text, rgba(p.bg, 0.9));
                                }
                            }
                        }
                    }
                }
                // The cursor's cell.
                if home == cursor.row && ch == cursor.ch {
                    let (a, b) = match g.view {
                        View::Tracker if vertical => {
                            let lx = if cursor.lane == FX_LANE { g.c0 + WAVE + c.lanes as f64 * g.lane } else { g.c0 + WAVE + cursor.lane as f64 * g.lane };
                            if cursor.field == 1 { (lx + 1.0 + cw * 4.0, lx + 2.0 + cw * 6.0 + 1.0) } else { (lx + 1.0, lx + 2.0 + cw * 3.0 + 1.0) }
                        }
                        View::Tracker => {
                            let lx = g.c0 + WAVE + cursor.lane.min(15) as f64 * g.lane;
                            (lx, lx + g.lane)
                        }
                        View::Roll => {
                            let pc = self.pitch_c(c, &g, cursor.pitch.clamp(c.lo, c.hi));
                            (pc, pc + g.key)
                        }
                    };
                    let r = self.tc(t0, t0 + row_px, a, b);
                    block(&mut self.draw_block, cx, r, 0.0, rgba(p.accent, 0.22), rgba(p.accent, if focused { 1.0 } else { 0.5 }), 1.0, 1.5);
                }
            }
        }
        // A note dragged: where it lands.
        if let Some(Drag::Note { ch, ev, at, now, resize }) = self.drag.clone() {
            if let Some(d) = self.dragged(ch, ev, at, now, resize) {
                let g = self.geo[ch];
                let c = &self.model.channels[ch];
                let e = &c.events[ev];
                let (ca, cb) = if g.view == View::Roll {
                    let pc = self.pitch_c(c, &g, d.pitch);
                    (pc, pc + g.key)
                } else {
                    let lx = g.c0 + WAVE + e.lane.min(15) as f64 * g.lane;
                    (lx, lx + g.lane)
                };
                let r = self.tc(d.t, d.t + d.dur.max(rb * 0.5) * px_beat, ca, cb);
                block(&mut self.draw_block, cx, r, 1.0, rgba(p.accent, 0.35), rgba(p.accent, 1.0), 1.0, 1.0);
            }
        }
        // The loop range and the playhead.
        if let Some((lo, hi)) = self.live.looping.filter(|_| !self.model.cycle) {
            for b in [lo, hi] {
                let t = b * px_beat;
                let r = self.tc(t - 1.0, t + 1.0, 0.0, total_c);
                fill(&mut self.draw_rect, cx, r, rgba(p.amber, 0.7));
            }
        }
        if let Some(song) = self.song {
            let u = if vertical { song + self.unfold } else if self.model.cycle { song.rem_euclid(self.model.song_len.max(self.model.bar)) } else { song };
            let t = u * px_beat;
            let r = self.tc(t, t + 2.0, 0.0, total_c.max(self.view_c() + self.scroll_c));
            fill(&mut self.draw_rect, cx, r, rgba(p.accent, if self.live.playing { 0.95 } else { 0.5 }));
        }
        cx.pop_clip_rect();
    }

    fn draw_ruler(&mut self, cx: &mut Cx2d, p: &makepad_look::palette::Palette) {
        let ruler = self.ruler;
        fill(&mut self.draw_rect, cx, ruler, rgba(p.wave_ruler_bg, 1.0));
        cx.push_clip_rect(ruler);
        let row_px = self.row_px();
        let lpb = self.lpb.max(1) as i64;
        let first = (self.scroll_t / row_px).floor() as i64;
        let last = ((self.scroll_t + self.view_t()) / row_px).ceil() as i64;
        let vertical = self.vertical();
        let every = if vertical { 1 } else { lpb.max(1) };
        for r in first..=last {
            let Some(home) = self.home_row(r) else { continue };
            if home % every != 0 {
                continue;
            }
            let t = r as f64 * row_px - self.scroll_t;
            let pos = if vertical { dvec2(ruler.pos.x + 4.0, self.grid.pos.y + t + 2.0) } else { dvec2(self.grid.pos.x + t + 2.0, ruler.pos.y + 3.0) };
            let beat = home % lpb == 0;
            if let Some(label) = self.labels.get(home as usize) {
                put(&mut self.draw_cell, cx, pos, label, rgba(if beat { p.wave_ink } else { p.wave_label }, if beat { 1.0 } else { 0.7 }));
            }
            // A section's name where it starts.
            if !self.model.cycle && !vertical {
                let song = home as f64 / lpb as f64;
                if let Some((pi, s)) = self.pattern_at(song) {
                    if (s - song).abs() < 1e-9 {
                        let name = &self.model.patterns[pi].name;
                        put(&mut self.draw_bold, cx, pos + dvec2(28.0, 0.0), name, rgba(p.accent, 1.0));
                    }
                }
            } else if !self.model.cycle && vertical && beat {
                let song = home as f64 / lpb as f64;
                if let Some((pi, s)) = self.pattern_at(song) {
                    if (s - song).abs() < 1e-9 {
                        let name = first_chars(&self.model.patterns[pi].name, 5);
                        let r = Rect { pos: dvec2(ruler.pos.x, self.grid.pos.y + t), size: dvec2(3.0, row_px) };
                        fill(&mut self.draw_rect, cx, r, rgba(p.accent, 1.0));
                        let _ = name;
                    }
                }
            }
        }
        // The fixed time position: the view's centre (vertical).
        if vertical {
            let y = self.grid.pos.y + self.view_t() * 0.5;
            let mark = Rect { pos: dvec2(ruler.pos.x + ruler.size.x - 7.0, y - 5.0), size: dvec2(7.0, 10.0) };
            block(&mut self.draw_block, cx, mark, 2.0, rgba(p.accent, 1.0), rgba(p.accent, 0.0), 0.0, 0.0);
        }
        cx.pop_clip_rect();
    }

    fn draw_heads(&mut self, cx: &mut Cx2d, p: &makepad_look::palette::Palette) {
        let heads = self.heads;
        fill(&mut self.draw_rect, cx, heads, rgba(p.panel, 1.0));
        cx.push_clip_rect(heads);
        let vertical = self.vertical();
        let any_solo = self.model.channels.iter().any(|c| c.solo);
        let meters = self.meters.clone();
        let cw = self.cw;
        let wave_from = self.content_beat(self.cursor.row).unwrap_or(0.0);
        for ch in self.visible_channels() {
            let r = self.head_rect(ch);
            let h = head_rects(r, vertical);
            let col = self.color_of(p, ch);
            let c = &self.model.channels[ch];
            let view = self.views.get(&c.name).copied().unwrap_or(c.auto_view);
            let live_dim = c.mute || (any_solo && !c.solo);
            let bg = Rect { pos: r.pos + dvec2(1.0, 1.0), size: r.size - dvec2(2.0, 2.0) };
            let picked = ch == self.cursor.ch;
            block(&mut self.draw_block, cx, bg, 0.0, rgba(if picked { p.control_hover } else { p.control }, 1.0), rgba(if picked { p.accent } else { p.line_faint }, 1.0), 1.0, 3.0);
            let strip = if vertical { Rect { pos: bg.pos, size: dvec2(bg.size.x, 3.0) } } else { Rect { pos: bg.pos, size: dvec2(3.0, bg.size.y) } };
            fill(&mut self.draw_rect, cx, strip, rgba(col, 1.0));
            let n = ((h.name.size.x / (cw * 1.15)).floor() as usize).max(1);
            put(&mut self.draw_bold, cx, h.name.pos, first_chars(&c.label, n), rgba(p.text_hi, if live_dim { 0.5 } else { 1.0 }));
            // The settings: three dots.
            for k in 0..3 {
                let d = Rect { pos: h.gear.pos + dvec2(2.0 + k as f64 * 4.5, 6.0), size: dvec2(3.0, 3.0) };
                block(&mut self.draw_block, cx, d, 0.0, rgba(p.label, 1.0), rgba(p.label, 0.0), 0.0, 1.5);
            }
            let n = ((h.inst.size.x / (cw * 1.05)).floor() as usize).max(1);
            put(&mut self.draw_text, cx, h.inst.pos, first_chars(&c.inst_text, n), rgba(p.secondary, 1.0));
            for (r, on, text, lit) in [(h.mute, c.mute, "M", p.amber), (h.solo, c.solo, "S", p.accent)] {
                block(&mut self.draw_block, cx, r, 0.0, rgba(if on { lit } else { p.well }, if on { 0.9 } else { 1.0 }), rgba(p.line, 1.0), 1.0, 2.0);
                put(&mut self.draw_cell, cx, r.pos + dvec2(5.0, 2.0), text, rgba(if on { p.bg } else { p.label }, 1.0));
            }
            block(&mut self.draw_block, cx, h.view, 0.0, rgba(p.well, 1.0), rgba(p.line, 1.0), 1.0, 2.0);
            put(&mut self.draw_cell, cx, h.view.pos + dvec2(3.0, 2.0), if view == View::Roll { "ROLL" } else { "TRK" }, rgba(p.label, 1.0));
            let tag = match c.form {
                Form::Hits { .. } => "hits",
                Form::Notes { .. } => "notes",
                Form::Fixed(_) => "code",
            };
            if h.tag.x + cw * 4.0 < r.pos.x + r.size.x {
                put(&mut self.draw_text, cx, h.tag, tag, rgba(if c.form.editable() { p.muted } else { p.amber }, 1.0));
            }
            // Level and pan: knobs as the lenses have them, the meter a ring
            // round the level, each named with its value.
            let amp = match &self.drag {
                Some(Drag::Fader { ch: d, field: "amp", value, .. }) if *d == ch => *value,
                _ => c.amp,
            };
            let pan = match &self.drag {
                Some(Drag::Fader { ch: d, field: "pan", value, .. }) if *d == ch => *value,
                _ => c.pan,
            };
            let peak = meters.as_ref().and_then(|m| m.level(c.key)).map_or(0.0, |l| l.0);
            let ring = rgba(if peak > 0.99 { p.danger } else { p.text_hi }, 0.85);
            knob(&mut self.draw_block, cx, h.level, (amp / 2.0).clamp(0.0, 1.0), if c.mute { 0.0 } else { peak }, false, rgba(col, 1.0), ring, rgba(p.well, 1.0));
            knob(&mut self.draw_block, cx, h.pan, ((pan + 1.0) / 2.0).clamp(0.0, 1.0), 0.0, true, rgba(col, 1.0), ring, rgba(p.well, 1.0));
            put(&mut self.draw_text, cx, h.level_text, "LEVEL", rgba(p.secondary, 1.0));
            put(&mut self.draw_text, cx, h.pan_text, "PAN", rgba(p.secondary, 1.0));
            let dragged = matches!(&self.drag, Some(Drag::Fader { ch: d, .. }) if *d == ch);
            let (lv, pv) = match (self.readouts.get(ch), dragged, &self.drag) {
                (_, true, Some(Drag::Fader { field: "amp", .. })) => (self.drag_text.as_str(), self.readouts.get(ch).map_or("", |r| r[1].as_str())),
                (_, true, Some(Drag::Fader { field: "pan", .. })) => (self.readouts.get(ch).map_or("", |r| r[0].as_str()), self.drag_text.as_str()),
                (Some(r), _, _) => (r[0].as_str(), r[1].as_str()),
                _ => ("", ""),
            };
            put(&mut self.draw_cell, cx, h.level_text + dvec2(0.0, 10.0), lv, rgba(p.text, 1.0));
            put(&mut self.draw_cell, cx, h.pan_text + dvec2(0.0, 10.0), pv, rgba(p.text, 1.0));
            // The waveform: live while it plays (min/max per column of the
            // audio thread's last pass), else what it played here (the
            // envelope recorded at the cursor's bar).
            block(&mut self.draw_block, cx, h.wave, 0.0, rgba(p.bg, 0.7), rgba(p.line_faint, 1.0), 1.0, 2.0);
            let w = (h.wave.size.x - 2.0) / SCOPE_W as f64;
            let mid = h.wave.pos.y + h.wave.size.y * 0.5;
            let half = h.wave.size.y * 0.5 - 1.5;
            fill(&mut self.draw_rect, cx, Rect { pos: dvec2(h.wave.pos.x + 1.0, mid - 0.5), size: dvec2(h.wave.size.x - 2.0, 1.0) }, rgba(col, 0.35));
            let alpha = if live_dim { 0.35 } else { 0.9 };
            let playing = self.live.playing;
            let bar = self.model.bar.max(1.0);
            let base = (wave_from / bar).floor() * bar;
            let hist = self.hist.get(ch);
            let mut peak = 0.25f32;
            let at = |k: usize| -> [f32; 2] {
                if playing {
                    self.scopes.get(ch).map_or([0.0; 2], |s| s[k])
                } else {
                    let b = ((base + bar * k as f64 / SCOPE_W as f64) * 32.0) as usize;
                    hist.and_then(|h| h.get(b)).copied().unwrap_or([0.0; 2])
                }
            };
            for k in 0..SCOPE_W {
                let v = at(k);
                peak = peak.max(v[1].abs()).max(v[0].abs());
            }
            for k in 0..SCOPE_W {
                let v = at(k);
                let (lo, hi) = ((v[0] / peak).clamp(-1.0, 0.0) as f64, (v[1] / peak).clamp(0.0, 1.0) as f64);
                if hi - lo < 0.01 {
                    continue;
                }
                let r = Rect { pos: dvec2(h.wave.pos.x + 1.0 + k as f64 * w, mid - hi * half), size: dvec2((w - 0.5).max(1.0), ((hi - lo) * half).max(1.0)) };
                fill(&mut self.draw_rect, cx, r, rgba(col, alpha));
            }
        }
        cx.pop_clip_rect();
    }

    fn draw_toolbar(&mut self, cx: &mut Cx2d, p: &makepad_look::palette::Palette) {
        let r = self.rect;
        let bar = Rect { pos: r.pos, size: dvec2(r.size.x, TOOLBAR) };
        fill(&mut self.draw_rect, cx, bar, rgba(p.panel, 1.0));
        fill(&mut self.draw_rect, cx, Rect { pos: dvec2(bar.pos.x, bar.pos.y + TOOLBAR - 1.0), size: dvec2(bar.size.x, 1.0) }, rgba(p.separator, 1.0));
        self.tools.clear();
        let cw = self.cw;
        let mut x = bar.pos.x + 8.0;
        let y = bar.pos.y + 4.0;
        put(&mut self.draw_bold, cx, dvec2(x, y + 2.0), "TRACKER", rgba(p.secondary, 1.0));
        x += 64.0;
        let shown = self.pattern_at(self.content_beat((self.cursor.row).max(0)).unwrap_or(0.0)).map(|(i, _)| i);
        let pattern_name: &str = shown.and_then(|i| self.model.patterns.get(i)).map_or("", |p| p.name.as_str());
        let looping = self.live.looping.is_some();
        let orient = if self.vertical() { "Vertical" } else { "Horizontal" };
        let items: [(Tool, &str, bool); 12] = [
            (Tool::PatPrev, "<", false),
            (Tool::PatNext, ">", false),
            (Tool::Orient, orient, false),
            (Tool::Follow, "Follow", self.follow),
            (Tool::OctDown, "-", false),
            (Tool::OctUp, "+", false),
            (Tool::StepDown, "-", false),
            (Tool::StepUp, "+", false),
            (Tool::LpbDown, "-", false),
            (Tool::LpbUp, "+", false),
            (Tool::Chord, "", false),
            (Tool::Loop, "Loop", looping),
        ];
        let texts = &self.texts;
        for (k, (tool, text, on)) in items.iter().enumerate() {
            // Labels between the steppers.
            let label: Option<&str> = match k {
                1 => Some(pattern_name),
                4 => Some(texts[0].as_str()),
                6 => Some(texts[1].as_str()),
                8 => Some(texts[2].as_str()),
                _ => None,
            };
            if let (Some(label), true) = (label, k != 1) {
                put(&mut self.draw_cell, cx, dvec2(x, y + 3.0), label, rgba(p.label, 1.0));
                x += label.chars().count() as f64 * cw + 6.0;
            }
            let text: &str = if *tool == Tool::Chord { texts[3].as_str() } else { text };
            let w = text.chars().count().max(1) as f64 * cw + 12.0;
            let rr = Rect { pos: dvec2(x, y), size: dvec2(w, TOOLBAR - 8.0) };
            let hot = self.hover.is_some_and(|h| rr.contains(h));
            let look = p.pick(*on, hot, p.control);
            block(&mut self.draw_block, cx, rr, 0.0, rgba(look.fill, 1.0), rgba(look.edge, 1.0), 1.0, 2.5);
            put(&mut self.draw_cell, cx, rr.pos + dvec2(6.0, 3.0), text, rgba(if *on { p.accent } else { p.text }, 1.0));
            self.tools.push((*tool, rr));
            x += w + 3.0;
            if k == 0 {
                // The pattern's name between < and >.
                let n = pattern_name.chars().count().min(14);
                put(&mut self.draw_bold, cx, dvec2(x + 2.0, y + 2.0), first_chars(pattern_name, 14), rgba(p.text_hi, 1.0));
                x += n as f64 * cw * 1.1 + 8.0;
            }
            if matches!(k, 1 | 3 | 5 | 7 | 9) {
                x += 8.0;
            }
        }
        if !self.editable && !self.why.is_empty() {
            let room = ((r.pos.x + r.size.x - x - 12.0) / (cw * 1.1)).floor().max(0.0) as usize;
            put(&mut self.draw_text, cx, dvec2(x + 8.0, y + 3.0), first_chars(&self.why, room), rgba(p.amber, 1.0));
        }
    }

    fn draw_popup(&mut self, cx: &mut Cx2d, p: &makepad_look::palette::Palette) {
        let Some(mut pop) = self.popup.take() else { return };
        let Some(ch) = self.model.channel(&pop.ch) else { return };
        if let Form::Hits { chars, .. } = &self.model.channels[ch].form {
            use std::fmt::Write;
            self.slots_text.clear();
            let _ = write!(self.slots_text, "{} slots", chars.len());
        }
        let c = &self.model.channels[ch];
        // Beside its header, in the widget.
        let head = self.head_rect(ch);
        let h = (self.rect.size.y - 12.0).min(500.0).max(200.0);
        let mut x = if self.vertical() { head.pos.x } else { head.pos.x + head.size.x + 6.0 };
        x = x.min(self.rect.pos.x + self.rect.size.x - POP_W - 6.0).max(self.rect.pos.x + 6.0);
        let y = (if self.vertical() { self.heads.pos.y + 6.0 } else { head.pos.y }).min(self.rect.pos.y + self.rect.size.y - h - 6.0).max(self.rect.pos.y + 6.0);
        let r = Rect { pos: dvec2(x, y), size: dvec2(POP_W, h) };
        pop.rect = r;
        pop.hits.clear();
        self.pop_rect.new_draw_call(cx);
        self.pop_block.new_draw_call(cx);
        self.pop_text.new_draw_call(cx);
        self.pop_bold.new_draw_call(cx);
        block(&mut self.pop_block, cx, r, 0.0, rgba(p.menu, 1.0), rgba(p.border_hi, 1.0), 1.0, 5.0);
        let (x0, w) = (x + 12.0, POP_W - 24.0);
        let mut yy = y + 10.0;
        // The name, editable.
        let name_r = Rect { pos: dvec2(x0 - 4.0, yy - 2.0), size: dvec2(w - 24.0, 20.0) };
        block(&mut self.pop_block, cx, name_r, 0.0, rgba(if pop.name.is_some() { p.well } else { p.menu }, 1.0), rgba(if pop.name.is_some() { p.accent } else { p.menu }, 1.0), 1.0, 2.0);
        let name_text = pop.name.as_deref().unwrap_or(&c.label);
        put(&mut self.pop_bold, cx, dvec2(x0, yy + 1.0), name_text, rgba(p.text_hi, 1.0));
        pop.hits.push((PopHit::Name, name_r));
        let close = Rect { pos: dvec2(x + POP_W - 28.0, yy - 2.0), size: dvec2(18.0, 18.0) };
        put(&mut self.pop_text, cx, close.pos + dvec2(5.0, 2.0), "x", rgba(p.label, 1.0));
        pop.hits.push((PopHit::Close, close));
        yy += 26.0;
        put(&mut self.pop_text, cx, dvec2(x0, yy), "INSTRUMENT", rgba(p.secondary, 1.0));
        put(&mut self.pop_text, cx, dvec2(x0 + 80.0, yy), first_chars(&c.inst_text, 30), rgba(p.muted, 1.0));
        yy += 16.0;
        let search = Rect { pos: dvec2(x0 - 4.0, yy), size: dvec2(w + 8.0, 20.0) };
        let searching = pop.name.is_none();
        block(&mut self.pop_block, cx, search, 0.0, rgba(p.well, 1.0), rgba(if searching { p.accent } else { p.line }, 1.0), 1.0, 2.5);
        if pop.search.is_empty() {
            put(&mut self.pop_text, cx, search.pos + dvec2(6.0, 3.0), "Search instruments (type)", rgba(p.muted, 1.0));
        } else {
            put(&mut self.pop_text, cx, search.pos + dvec2(6.0, 3.0), &pop.search, rgba(p.text_hi, 1.0));
        }
        pop.hits.push((PopHit::Search, search));
        yy += 26.0;
        let list_h = (h - (yy - y) - 170.0).max(60.0);
        let list = Rect { pos: dvec2(x0 - 4.0, yy), size: dvec2(w + 8.0, list_h) };
        pop.list_rect = list;
        fill(&mut self.pop_rect, cx, list, rgba(p.well, 0.6));
        cx.push_clip_rect(list);
        let first = (pop.scroll / 18.0).floor() as usize;
        let rows = (list_h / 18.0).ceil() as usize + 1;
        for i in first..(first + rows).min(pop.list.len()) {
            let ry = list.pos.y + i as f64 * 18.0 - pop.scroll;
            let row = Rect { pos: dvec2(list.pos.x, ry), size: dvec2(list.size.x, 18.0) };
            let current = matches!(&pop.list[i].0, Pick::Score(n) if Some(n) == c.play.as_ref());
            if pop.hover == Some(i) || current {
                fill(&mut self.pop_rect, cx, row, rgba(if current { p.accent_wash } else { p.control_hover }, 1.0));
            }
            let n = ((list.size.x - 12.0) / 5.6).floor() as usize;
            put(&mut self.pop_text, cx, row.pos + dvec2(6.0, 3.0), first_chars(&pop.list[i].1, n), rgba(if current { p.accent } else { p.text }, 1.0));
            pop.hits.push((PopHit::Item(i), row));
        }
        cx.pop_clip_rect();
        if pop.list.is_empty() {
            put(&mut self.pop_text, cx, list.pos + dvec2(6.0, 4.0), if self.catalog.is_empty() { "Loading the library…" } else { "Nothing matches" }, rgba(p.muted, 1.0));
        }
        yy += list_h + 10.0;
        // Mix: level, pan, sends.
        put(&mut self.pop_text, cx, dvec2(x0, yy), "MIX", rgba(p.secondary, 1.0));
        yy += 16.0;
        let dragging = |what: &PopFader| match &self.drag {
            Some(Drag::PopFader { what: w, value }) if w == what => Some(*value),
            _ => None,
        };
        let mut faders: Vec<(PopHit, &str, f32, f32)> = vec![(PopHit::Level, "level", dragging(&PopFader::Level).unwrap_or(c.amp) / 2.0, c.amp), (PopHit::Pan, "pan", (dragging(&PopFader::Pan).unwrap_or(c.pan) + 1.0) / 2.0, c.pan)];
        for fx in self.model.fx.iter().take(4) {
            let amount = c.sends.iter().find(|(n, _)| n == fx).map_or(0.0, |(_, a)| *a);
            let v = dragging(&PopFader::Send(fx.clone())).unwrap_or(amount);
            faders.push((PopHit::Send(fx.clone()), fx.as_str(), v / 2.0, v));
        }
        for (hit, label, frac, _) in faders {
            put(&mut self.pop_text, cx, dvec2(x0, yy), first_chars(label, 10), rgba(p.label, 1.0));
            let fr = Rect { pos: dvec2(x0 + 70.0, yy + 2.0), size: dvec2(w - 70.0, 10.0) };
            block(&mut self.pop_block, cx, fr, 0.0, rgba(p.well, 1.0), rgba(p.line, 1.0), 1.0, 2.0);
            fill(&mut self.pop_rect, cx, Rect { pos: fr.pos + dvec2(1.0, 1.0), size: dvec2((fr.size.x - 2.0) * frac.clamp(0.0, 1.0) as f64, 8.0) }, rgba(p.accent, 0.55));
            pop.hits.push((hit, fr));
            yy += 16.0;
        }
        yy += 6.0;
        // A hit pattern's slots: fewer or more over the same length.
        if let Form::Hits { chars, .. } = &c.form {
            let n = chars.len();
            put(&mut self.pop_text, cx, dvec2(x0, yy + 2.0), "SLOTS", rgba(p.secondary, 1.0));
            let fewer = tm::SLOTS.iter().rev().find(|s| **s < n).copied();
            let more = tm::SLOTS.iter().find(|s| **s > n).copied();
            let mut bx = x0 + 70.0;
            for (label, to) in [("÷", fewer), ("×", more)] {
                let br = Rect { pos: dvec2(bx, yy), size: dvec2(24.0, 18.0) };
                block(&mut self.pop_block, cx, br, 0.0, rgba(p.control, if to.is_some() { 1.0 } else { 0.4 }), rgba(p.line, 1.0), 1.0, 2.0);
                put(&mut self.pop_text, cx, br.pos + dvec2(8.0, 2.0), label, rgba(p.text, 1.0));
                if let Some(to) = to {
                    pop.hits.push((PopHit::Slots(to), br));
                }
                bx += 28.0;
            }
            put(&mut self.pop_text, cx, dvec2(bx + 4.0, yy + 2.0), &self.slots_text, rgba(p.text, 1.0));
            yy += 26.0;
        }
        // The view and the colour.
        put(&mut self.pop_text, cx, dvec2(x0, yy + 2.0), "VIEW", rgba(p.secondary, 1.0));
        let chosen = self.views.get(&c.name).copied();
        let mut bx = x0 + 70.0;
        for (label, v) in [("Auto", None), ("Tracker", Some(View::Tracker)), ("Roll", Some(View::Roll))] {
            let br = Rect { pos: dvec2(bx, yy), size: dvec2(56.0, 18.0) };
            let on = chosen == v;
            let look = p.pick(on, false, p.control);
            block(&mut self.pop_block, cx, br, 0.0, rgba(look.fill, 1.0), rgba(look.edge, 1.0), 1.0, 2.0);
            put(&mut self.pop_text, cx, br.pos + dvec2(8.0, 2.0), label, rgba(if on { p.accent } else { p.text }, 1.0));
            pop.hits.push((PopHit::View(v), br));
            bx += 60.0;
        }
        yy += 26.0;
        put(&mut self.pop_text, cx, dvec2(x0, yy + 2.0), "COLOUR", rgba(p.secondary, 1.0));
        let mut bx = x0 + 70.0;
        for (k, name) in std::iter::once(None).chain(COLORS.iter().map(|c| Some(*c))).enumerate() {
            let sw = Rect { pos: dvec2(bx, yy), size: dvec2(16.0, 16.0) };
            let value = match name {
                Some(n) => self.color_named(p, Some(n), ch),
                None => p.control,
            };
            let on = self.model.channels[ch].color.as_deref() == name;
            block(&mut self.pop_block, cx, sw, 0.0, rgba(value, 1.0), rgba(if on { p.text_hi } else { p.line }, 1.0), if on { 2.0 } else { 1.0 }, 3.0);
            pop.hits.push((PopHit::Color(name), sw));
            bx += 20.0;
            let _ = k;
        }
        self.popup = Some(pop);
    }
}

trait MaxWith {
    fn max_with(self, other: Vec4f) -> Vec4f;
}

impl MaxWith for Vec4f {
    /// The brighter of a channel's colour and the ink (cells stay legible).
    fn max_with(self, other: Vec4f) -> Vec4f {
        let l = |c: &Vec4f| c.x * 0.3 + c.y * 0.59 + c.z * 0.11;
        if l(&self) >= l(&other) * 0.8 { self } else { Vec4f { x: (self.x + other.x) * 0.5, y: (self.y + other.y) * 0.5, z: (self.z + other.z) * 0.5, w: self.w } }
    }
}

/// A note under a drag: where it lands.
struct Dragged {
    beat: f64,
    pitch: f32,
    dur: f64,
    /// Its start on the time axis (content px).
    t: f64,
}

impl ScoreTracker {
    fn dragged(&self, ch: usize, ev: usize, at: Vec2d, now: Vec2d, resize: bool) -> Option<Dragged> {
        let c = self.model.channels.get(ch)?;
        let e = c.events.get(ev)?;
        let g = self.geo.get(ch)?;
        let row_px = self.row_px();
        let rb = self.rb();
        let px_beat = row_px * self.lpb as f64;
        let (ta, ca) = self.at(at);
        let (tn, cn) = self.at(now);
        let rows = ((tn - ta) / row_px).round();
        let d = rows * rb;
        // The event's start where it was pressed (its row's repeat).
        let press_row = (ta / row_px).floor() as i64;
        let local = self.local_of(ch, press_row)?;
        let t0 = press_row as f64 * row_px + (e.beat - local) * px_beat;
        if resize {
            return Some(Dragged { beat: e.beat, pitch: e.pitch, dur: (e.dur as f64 + d).max(rb * 0.25), t: t0 });
        }
        let pitch = if g.view == View::Roll { (e.pitch + self.pitch_at(c, g, cn) - self.pitch_at(c, g, ca)).clamp(0.0, 127.0) } else { e.pitch };
        let len = if c.absolute { f64::MAX } else { c.len.max(rb) };
        Some(Dragged { beat: (e.beat + d).rem_euclid(len), pitch, dur: e.dur as f64, t: t0 + d * px_beat })
    }

    /// The note under the pointer in a roll or a horizontal lane, and
    /// whether the pointer is on its end.
    fn note_under(&self, ch: usize, abs: Vec2d) -> Option<(usize, bool)> {
        let (t, cpos) = self.at(abs);
        let row_px = self.row_px();
        let row = (t / row_px).floor() as i64;
        let local = self.local_of(ch, row)?;
        let px_beat = row_px * self.lpb as f64;
        let beat = local + (t - row as f64 * row_px) / px_beat;
        let c = &self.model.channels[ch];
        let g = &self.geo[ch];
        let rb = self.rb();
        let back = c.events.partition_point(|e| e.beat < beat - 64.0);
        let end = c.events.partition_point(|e| e.beat <= beat + 1e-9);
        let pitch = (g.view == View::Roll).then(|| self.pitch_at(c, g, cpos));
        let lane = ((cpos - g.c0 - WAVE) / g.lane.max(1.0)).floor().max(0.0) as u8;
        (back..end).rev().find(|&i| {
            let e = &c.events[i];
            let e_end = e.beat + (e.dur as f64).max(rb * 0.6);
            matches!(e.kind, EvKind::Note | EvKind::Hit) && beat < e_end && match pitch {
                Some(p) => (e.pitch - p).abs() < 0.5,
                None => e.lane == lane,
            }
        }).map(|i| {
            let e = &c.events[i];
            let e_end = e.beat + (e.dur as f64).max(rb * 0.6);
            (i, (e_end - beat) * px_beat < 5.0)
        })
    }

    fn head_hit(&self, abs: Vec2d) -> Option<(usize, HeadHit)> {
        if !self.heads.contains(abs) {
            return None;
        }
        let vertical = self.vertical();
        self.visible_channels().find_map(|ch| {
            let r = self.head_rect(ch);
            if !r.contains(abs) {
                return None;
            }
            let h = head_rects(r, vertical);
            let hit = [(h.gear, HeadHit::Gear), (h.inst, HeadHit::Inst), (h.mute, HeadHit::Mute), (h.solo, HeadHit::Solo), (h.view, HeadHit::View), (h.level, HeadHit::Level), (h.pan, HeadHit::Pan)]
                .into_iter()
                .find(|(r, _)| Rect { pos: r.pos - dvec2(1.0, 2.0), size: r.size + dvec2(2.0, 4.0) }.contains(abs))
                .map_or(HeadHit::Name, |(_, k)| k);
            Some((ch, hit))
        })
    }

    fn fader_value(r: Rect, abs: Vec2d) -> f32 {
        (((abs.x - r.pos.x) / r.size.x.max(1.0)).clamp(0.0, 1.0)) as f32
    }

    fn tool(&mut self, cx: &mut Cx, tool: Tool) {
        let n = self.model.patterns.len();
        match tool {
            Tool::PatPrev | Tool::PatNext => {
                if n == 0 {
                    return;
                }
                let at = self.content_beat(self.cursor.row).unwrap_or(0.0);
                let now = self.pattern_at(at).map_or(0, |(i, _)| i);
                let next = if tool == Tool::PatNext { (now + 1) % n } else { (now + n - 1) % n };
                self.goto_pattern(cx, next);
            }
            Tool::Orient => {
                self.orient = if self.vertical() { Orient::Horizontal } else { Orient::Vertical };
                self.geo_dirty = true;
                self.scroll_c = 0.0;
                self.placed = false;
                self.target_t = None;
            }
            Tool::Follow => self.follow = !self.follow,
            Tool::OctDown => self.octave = (self.octave - 1).max(0),
            Tool::OctUp => self.octave = (self.octave + 1).min(8),
            Tool::StepDown => self.step = (self.step - 1).max(0),
            Tool::StepUp => self.step = (self.step + 1).min(16),
            Tool::LpbDown | Tool::LpbUp => {
                let lpbs = [1u32, 2, 3, 4, 6, 8, 12, 16, 24, 32];
                let at = lpbs.iter().position(|l| *l == self.lpb).unwrap_or(3);
                let next = if tool == Tool::LpbUp { (at + 1).min(lpbs.len() - 1) } else { at.saturating_sub(1) };
                self.set_lpb(lpbs[next]);
            }
            Tool::Chord => self.chord = (self.chord + 1) % CHORDS.len(),
            Tool::Loop => self.toggle_loop(cx),
        }
        self.rebuild_labels();
    }

    fn set_lpb(&mut self, lpb: u32) {
        if lpb == self.lpb {
            return;
        }
        let beat = self.cursor.row as f64 * self.rb();
        let center = (self.scroll_t + self.view_t() * 0.5) / (self.row_px() * self.lpb as f64);
        self.lpb = lpb;
        self.cursor.row = (beat * lpb as f64).round() as i64;
        self.sel = None;
        self.scroll_t = center * self.row_px() * lpb as f64 - self.view_t() * 0.5;
        self.rebuild_labels();
    }

    fn goto_pattern(&mut self, cx: &mut Cx, index: usize) {
        let Some((first, name)) = self.model.patterns.get(index).map(|p| (p.start, p.name.clone())) else { return };
        let start = first.unwrap_or(0.0);
        self.cursor.row = (start * self.lpb as f64).round() as i64;
        let row = self.cursor.row;
        self.reveal(row);
        if !self.vertical() {
            self.target_t = Some((row as f64 * self.row_px() - 20.0).max(0.0));
        } else {
            self.target_t = Some(row as f64 * self.row_px() - 30.0);
        }
        if first.is_none() {
            self.status(cx, &format!("{name} is not in the arrangement"));
        } else if !self.live.playing {
            cx.widget_action(self.uid, TrackerAction::Seek(start));
        }
    }

    fn toggle_loop(&mut self, cx: &mut Cx) {
        if self.live.looping.is_some() {
            cx.widget_action(self.uid, TrackerAction::Loop(None));
            return;
        }
        let rb = self.rb();
        let range = match self.sel {
            Some(s) => (s.r0 as f64 * rb, (s.r1 + 1) as f64 * rb),
            None => {
                let at = self.content_beat(self.cursor.row).unwrap_or(0.0);
                match self.pattern_at(at) {
                    Some((i, start)) if !self.model.cycle => (start, start + self.model.patterns[i].len),
                    _ => (0.0, self.model.song_len.max(self.model.bar)),
                }
            }
        };
        cx.widget_action(self.uid, TrackerAction::Loop(Some(range)));
    }

    fn popup_hit(&self, abs: Vec2d) -> Option<PopHit> {
        let p = self.popup.as_ref()?;
        p.hits.iter().rev().find(|(h, r)| r.contains(abs) && (!matches!(h, PopHit::Item(_)) || p.list_rect.contains(abs))).map(|(h, _)| h.clone())
    }

    fn close_popup(&mut self, cx: &mut Cx) {
        if self.popup.take().is_some() {
            cx.widget_action(self.uid, TrackerAction::Preview(None));
        }
    }

    fn pop_fader_value(&self, what: &PopFader, frac: f32) -> f32 {
        match what {
            PopFader::Level => frac * 2.0,
            PopFader::Pan => frac * 2.0 - 1.0,
            PopFader::Send(_) => frac * 2.0,
        }
    }

    fn popup_down(&mut self, cx: &mut Cx, abs: Vec2d) {
        let Some(hit) = self.popup_hit(abs) else { return };
        let ch = self.popup.as_ref().and_then(|p| self.model.channel(&p.ch));
        let Some(ch) = ch else { return };
        match hit {
            PopHit::Close => self.close_popup(cx),
            PopHit::Search => {
                if let Some(p) = &mut self.popup {
                    p.name = None;
                }
            }
            PopHit::Name => {
                let label = self.model.channels[ch].label.clone();
                if let Some(p) = &mut self.popup {
                    p.name.get_or_insert(label);
                }
            }
            PopHit::Item(i) => self.pick(cx, i),
            PopHit::Level | PopHit::Pan | PopHit::Send(_) => {
                let what = match hit {
                    PopHit::Level => PopFader::Level,
                    PopHit::Pan => PopFader::Pan,
                    PopHit::Send(fx) => PopFader::Send(fx),
                    _ => return,
                };
                let r = self.popup.as_ref().and_then(|p| p.hits.iter().find(|(h, _)| *h == match &what {
                    PopFader::Level => PopHit::Level,
                    PopFader::Pan => PopHit::Pan,
                    PopFader::Send(fx) => PopHit::Send(fx.clone()),
                }).map(|(_, r)| *r));
                let Some(r) = r else { return };
                let value = self.pop_fader_value(&what, Self::fader_value(r, abs));
                self.hold_pop(cx, ch, &what, value);
                self.drag = Some(Drag::PopFader { what, value });
            }
            PopHit::View(v) => {
                let name = self.model.channels[ch].name.clone();
                match v {
                    Some(v) => {
                        self.views.insert(name, v);
                    }
                    None => {
                        self.views.remove(&name);
                    }
                }
                self.geo_dirty = true;
            }
            PopHit::Slots(slots) => {
                let name = self.model.channels[ch].name.clone();
                match tm::resample_hits(&self.source, &name, slots) {
                    Ok(sp) => self.emit(cx, vec![sp], "slots"),
                    Err(e) => self.status(cx, &e),
                }
            }
            PopHit::Color(c) => match c {
                Some(c) => self.write_field(cx, ch, "color", format!("@{c}"), "colour"),
                None => {
                    let name = self.model.channels[ch].name.clone();
                    if self.model.channels[ch].color.is_some() {
                        match makepad_score_live::source::remove_field(&self.source, &name, "color") {
                            Ok(sp) => self.emit(cx, vec![sp], "colour"),
                            Err(e) => self.status(cx, &e),
                        }
                    }
                }
            },
        }
    }

    fn hold_pop(&mut self, cx: &mut Cx, ch: usize, what: &PopFader, value: f32) {
        let name = self.model.channels[ch].name.clone();
        match what {
            PopFader::Level => cx.widget_action(self.uid, TrackerAction::Hold { channel: name, field: "amp", value }),
            PopFader::Pan => cx.widget_action(self.uid, TrackerAction::Hold { channel: name, field: "pan", value }),
            // A send is not a control: it is heard when written.
            PopFader::Send(_) => {}
        }
    }

    fn release_pop(&mut self, cx: &mut Cx, what: PopFader, value: f32) {
        let Some(ch) = self.popup.as_ref().and_then(|p| self.model.channel(&p.ch)) else { return };
        let num = |v: f32| makepad_score_live::source::fmt_num(v as f64, 2);
        match what {
            PopFader::Level => self.write_field(cx, ch, "amp", num(value), "level"),
            PopFader::Pan => self.write_field(cx, ch, "pan", num(value), "pan"),
            PopFader::Send(fx) => {
                let text = tm::sends_text(&self.model.channels[ch].sends, &fx, value);
                self.write_field(cx, ch, "sends", text, "send");
            }
        }
    }

    fn popup_key(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        let ch = self.popup.as_ref().and_then(|p| self.model.channel(&p.ch));
        let Some(p) = &mut self.popup else { return };
        match ke.key_code {
            KeyCode::Escape => {
                if p.name.is_some() {
                    p.name = None;
                } else {
                    self.close_popup(cx);
                }
            }
            KeyCode::ReturnKey => {
                if let Some(name) = p.name.take() {
                    if let Some(ch) = ch {
                        let text = format!("{:?}", name.trim());
                        self.write_field(cx, ch, "label", text, "renamed");
                    }
                } else if let Some(i) = p.hover.or((!p.list.is_empty()).then_some(0)) {
                    self.pick(cx, i);
                }
            }
            KeyCode::Backspace => {
                match &mut p.name {
                    Some(name) => {
                        name.pop();
                    }
                    None => {
                        p.search.pop();
                        self.filter_popup();
                    }
                }
            }
            KeyCode::ArrowDown | KeyCode::ArrowUp => {
                let n = p.list.len();
                if n > 0 {
                    let at = p.hover.map_or(0, |h| if ke.key_code == KeyCode::ArrowDown { (h + 1).min(n - 1) } else { h.saturating_sub(1) });
                    p.hover = Some(at);
                    let view = p.list_rect.size.y;
                    let y = at as f64 * 18.0;
                    if y < p.scroll {
                        p.scroll = y;
                    } else if y + 18.0 > p.scroll + view {
                        p.scroll = y + 18.0 - view;
                    }
                    self.preview(cx, Some(at));
                }
            }
            code => {
                if ke.modifiers.logo || ke.modifiers.control {
                    return;
                }
                if let Some(c) = key_char(code, ke.modifiers.shift) {
                    match &mut p.name {
                        Some(name) => name.push(c),
                        None => {
                            p.search.push(c);
                            p.hover = None;
                            p.scroll = 0.0;
                            self.filter_popup();
                        }
                    }
                }
            }
        }
    }

    /// Move the cursor along the channel axis: fields and lanes, keys in
    /// a roll, then the next channel.
    fn move_c(&mut self, by: i32) {
        let n = self.model.channels.len();
        if n == 0 {
            return;
        }
        let ch = self.cursor.ch.min(n - 1);
        let c = &self.model.channels[ch];
        let view = self.view_of(c);
        if view == View::Roll {
            let up = if self.vertical() { by > 0 } else { by < 0 };
            let next = self.cursor.pitch.clamp(c.lo, c.hi) + if up { 1.0 } else { -1.0 };
            if next >= c.lo && next <= c.hi {
                self.cursor.pitch = next;
                return;
            }
        } else if self.vertical() {
            // note → velocity → next lane … → effect column.
            let lanes = c.lanes;
            let fx = c.has_fx;
            let mut slot = if self.cursor.lane == FX_LANE { lanes as i32 * 2 } else { self.cursor.lane as i32 * 2 + self.cursor.field as i32 };
            let max = lanes as i32 * 2 - 1 + fx as i32;
            slot += by;
            if slot >= 0 && slot <= max {
                if fx && slot == lanes as i32 * 2 {
                    self.cursor.lane = FX_LANE;
                    self.cursor.field = 0;
                } else {
                    self.cursor.lane = (slot / 2) as u8;
                    self.cursor.field = (slot % 2) as u8;
                }
                return;
            }
        } else {
            let lanes = c.lanes as i32;
            let next = self.cursor.lane as i32 + by;
            if self.cursor.lane != FX_LANE && next >= 0 && next < lanes {
                self.cursor.lane = next as u8;
                return;
            }
        }
        // The neighbour channel, at its near edge.
        let next = (ch as i32 + by.signum()).clamp(0, n as i32 - 1) as usize;
        if next != ch {
            self.cursor.ch = next;
            let c = &self.model.channels[next];
            self.cursor.lane = if by > 0 { 0 } else { c.lanes.saturating_sub(1) };
            self.cursor.field = 0;
            self.cursor.pitch = if (self.cursor.pitch - c.lo) >= 0.0 && self.cursor.pitch <= c.hi { self.cursor.pitch } else { ((c.lo + c.hi) * 0.5).round() };
        }
    }

    fn key(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        if self.popup.is_some() {
            self.popup_key(cx, ke);
            return;
        }
        let cmd = ke.modifiers.logo || ke.modifiers.control;
        let shift = ke.modifiers.shift;
        let rows = self.rows().max(1);
        let vertical = self.vertical();
        let n = self.model.channels.len();
        if n == 0 {
            return;
        }
        let before = (self.cursor.row, self.cursor.ch);
        let time_key = |k: KeyCode| -> Option<i64> {
            match (k, vertical) {
                (KeyCode::ArrowUp, true) | (KeyCode::ArrowLeft, false) => Some(-1),
                (KeyCode::ArrowDown, true) | (KeyCode::ArrowRight, false) => Some(1),
                _ => None,
            }
        };
        let chan_key = |k: KeyCode| -> Option<i32> {
            match (k, vertical) {
                (KeyCode::ArrowLeft, true) | (KeyCode::ArrowUp, false) => Some(-1),
                (KeyCode::ArrowRight, true) | (KeyCode::ArrowDown, false) => Some(1),
                _ => None,
            }
        };
        let bar_rows = ((self.model.bar * self.lpb as f64).round() as i64).max(1);
        match ke.key_code {
            KeyCode::ArrowLeft | KeyCode::ArrowRight if ke.modifiers.alt => {
                self.tool(cx, if ke.key_code == KeyCode::ArrowRight { Tool::PatNext } else { Tool::PatPrev });
                return;
            }
            KeyCode::ArrowUp | KeyCode::ArrowDown if cmd => {
                let up = ke.key_code == KeyCode::ArrowUp;
                self.transpose(cx, if up { 1.0 } else { -1.0 } * if shift { 12.0 } else { 1.0 });
                return;
            }
            k if time_key(k).is_some() => {
                self.cursor.row = (self.cursor.row + time_key(k).unwrap_or(0)).rem_euclid(rows);
            }
            k if chan_key(k).is_some() => self.move_c(chan_key(k).unwrap_or(0)),
            KeyCode::Tab => {
                let next = if shift { self.cursor.ch.saturating_sub(1) } else { (self.cursor.ch + 1).min(n - 1) };
                self.cursor.ch = next;
                self.cursor.lane = 0;
                self.cursor.field = 0;
            }
            KeyCode::PageUp => self.cursor.row = (self.cursor.row - bar_rows).max(0),
            KeyCode::PageDown => self.cursor.row = (self.cursor.row + bar_rows).min(rows - 1),
            KeyCode::Home => self.cursor.row = 0,
            KeyCode::End => self.cursor.row = rows - 1,
            KeyCode::Escape => {
                self.sel = None;
                return;
            }
            KeyCode::F2 => {
                let ch = self.cursor.ch;
                self.open_settings(cx, ch);
                return;
            }
            KeyCode::F3 => {
                self.tool(cx, Tool::Orient);
                return;
            }
            KeyCode::F4 => {
                let c = &self.model.channels[self.cursor.ch.min(n - 1)];
                let next = if self.view_of(c) == View::Roll { View::Tracker } else { View::Roll };
                let name = c.name.clone();
                self.views.insert(name, next);
                self.geo_dirty = true;
                return;
            }
            KeyCode::KeyC if cmd => {
                self.copy();
                self.status(cx, &format!("{} copied", self.clip.len()));
                return;
            }
            KeyCode::KeyX if cmd => {
                self.copy();
                self.clear_sel(cx, "cut");
                return;
            }
            KeyCode::KeyV if cmd => {
                self.paste(cx);
                return;
            }
            KeyCode::KeyA if cmd => {
                self.sel = Some(Sel { r0: 0, r1: rows - 1, c0: self.cursor.ch, c1: self.cursor.ch });
                return;
            }
            KeyCode::KeyL if cmd => {
                self.toggle_loop(cx);
                return;
            }
            KeyCode::KeyZ if cmd => {
                cx.widget_action(self.uid, if shift { TrackerAction::Redo } else { TrackerAction::Undo });
                return;
            }
            KeyCode::LBracket | KeyCode::RBracket => {
                let up = ke.key_code == KeyCode::RBracket;
                if shift {
                    self.tool(cx, if up { Tool::StepUp } else { Tool::StepDown });
                } else {
                    self.tool(cx, if up { Tool::OctUp } else { Tool::OctDown });
                }
                return;
            }
            KeyCode::Delete => {
                if self.sel.is_some() {
                    self.clear_sel(cx, "cleared");
                } else {
                    let row = self.cursor.row;
                    self.clear_cell(cx, row);
                    self.advance(1);
                }
                return;
            }
            KeyCode::Backspace => {
                self.cursor.row = (self.cursor.row - 1).rem_euclid(rows);
                let row = self.cursor.row;
                self.clear_cell(cx, row);
                self.reveal(row);
                return;
            }
            _ if cmd || ke.modifiers.alt => return,
            code => {
                let on_vel = self.cursor.field == 1 && self.view_of(&self.model.channels[self.cursor.ch.min(n - 1)]) == View::Tracker && vertical;
                if on_vel {
                    if let Some(d) = digit(code) {
                        self.enter_vel(cx, d);
                        return;
                    }
                    // A note key on the velocity field enters the note.
                    self.cursor.field = 0;
                }
                if matches!(code, KeyCode::Key1 | KeyCode::Backtick) {
                    self.note_off(cx);
                    return;
                }
                if let Some(k) = piano(code) {
                    if !ke.is_repeat {
                        let pitch = ((self.octave + 1) * 12 + k).clamp(0, 127) as f32;
                        self.enter_note(cx, pitch, true);
                    }
                }
                return;
            }
        }
        // A move: shift extends the selection from where it was.
        if shift && matches!(ke.key_code, KeyCode::ArrowUp | KeyCode::ArrowDown | KeyCode::ArrowLeft | KeyCode::ArrowRight | KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End) {
            let anchor = match self.sel {
                Some(s) => (if s.r0 == before.0 { s.r1 } else { s.r0 }, if s.c0 == before.1 { s.c1 } else { s.c0 }),
                None => before,
            };
            let (r, c) = (self.cursor.row, self.cursor.ch);
            self.sel = Some(Sel { r0: anchor.0.min(r), r1: anchor.0.max(r), c0: anchor.1.min(c), c1: anchor.1.max(c) });
        } else if !shift {
            self.sel = None;
        }
        self.vel_digit = None;
        let row = self.cursor.row;
        self.reveal(row);
    }

    fn finger_down(&mut self, cx: &mut Cx, fe: &FingerDownEvent) {
        cx.set_key_focus(self.area);
        let abs = fe.abs;
        if self.popup.as_ref().is_some_and(|p| p.rect.contains(abs)) {
            self.popup_down(cx, abs);
            return;
        }
        if self.popup.is_some() {
            self.close_popup(cx);
            return;
        }
        if let Some((tool, _)) = self.tools.iter().find(|(_, r)| r.contains(abs)).copied() {
            self.tool(cx, tool);
            return;
        }
        if let Some((ch, hit)) = self.head_hit(abs) {
            let name = self.model.channels[ch].name.clone();
            self.cursor.ch = ch;
            let h = head_rects(self.head_rect(ch), self.vertical());
            match hit {
                HeadHit::Gear | HeadHit::Inst => self.open_settings(cx, ch),
                HeadHit::Mute | HeadHit::Solo => {
                    let (key, on) = if hit == HeadHit::Mute { ("mute", !self.model.channels[ch].mute) } else { ("solo", !self.model.channels[ch].solo) };
                    cx.widget_action(self.uid, TrackerAction::Hold { channel: name.clone(), field: key, value: on as u8 as f32 });
                    self.write_field(cx, ch, key, on.to_string(), key);
                }
                HeadHit::View => {
                    let next = if self.view_of(&self.model.channels[ch]) == View::Roll { View::Tracker } else { View::Roll };
                    self.views.insert(name, next);
                    self.geo_dirty = true;
                }
                HeadHit::Level | HeadHit::Pan => {
                    // A knob turns with a vertical drag from where it is.
                    let _ = h;
                    let (field, value) = if hit == HeadHit::Level { ("amp", self.model.channels[ch].amp) } else { ("pan", self.model.channels[ch].pan) };
                    self.drag_text.clear();
                    self.drag_text.push_str(&self.readouts.get(ch).map_or(String::new(), |r| r[(field == "pan") as usize].clone()));
                    self.drag = Some(Drag::Fader { ch, field, value, from: abs, base: value });
                }
                HeadHit::Name => cx.widget_action(self.uid, TrackerAction::Voice(name)),
            }
            return;
        }
        if self.ruler.contains(abs) {
            let (t, _) = self.at(abs);
            let row = (t / self.row_px()).floor() as i64;
            let beat = self.content_beat(row).unwrap_or(0.0);
            self.drag = Some(Drag::Ruler { from: beat, to: beat });
            return;
        }
        if !self.grid.contains(abs) {
            return;
        }
        let (t, c) = self.at(abs);
        let row = (t / self.row_px()).floor() as i64;
        let Some(home) = self.home_row(row) else { return };
        let Some(ch) = self.channel_at(c) else { return };
        let g = self.geo[ch];
        let view = g.view;
        let secondary = fe.mouse_button().is_some_and(|b| b.is_secondary()) || fe.modifiers.alt;
        self.cursor.ch = ch;
        self.cursor.row = home;
        self.vel_digit = None;
        let roll_like = view == View::Roll || !self.vertical();
        if roll_like {
            if view == View::Roll {
                let pitch = self.pitch_at(&self.model.channels[ch], &g, c);
                self.cursor.pitch = pitch;
            } else {
                self.cursor.lane = ((c - g.c0 - WAVE) / g.lane.max(1.0)).floor().clamp(0.0, (self.model.channels[ch].lanes.max(1) - 1) as f64) as u8;
            }
            match self.note_under(ch, abs) {
                Some((ev, _)) if secondary => {
                    if self.writable(cx, ch) {
                        let splices: Vec<Splice> = self.delete_splice(ch, ev).into_iter().collect();
                        self.emit(cx, splices, "note deleted");
                    }
                }
                Some((ev, resize)) => {
                    self.drag = Some(Drag::Note { ch, ev, at: abs, now: abs, resize });
                }
                None if view == View::Roll && !secondary && fe.tap_count <= 1 => {
                    // Draw a note (the chord) where pressed.
                    let pitch = self.cursor.pitch;
                    self.enter_note(cx, pitch, false);
                }
                None if view == View::Tracker && !secondary && self.adds_here(ch, home, fe.tap_count) => {
                    let pitch = self.pointer_pitch();
                    self.enter_note(cx, pitch, false);
                }
                None => {
                    self.sel = None;
                    self.drag = Some(Drag::Select { anchor: (home, ch) });
                }
            }
        } else {
            let lane_or_fx = ((c - g.c0 - WAVE) / g.lane.max(1.0)).floor();
            let lanes = self.model.channels[ch].lanes as f64;
            if lane_or_fx >= lanes && self.model.channels[ch].has_fx {
                self.cursor.lane = FX_LANE;
                self.cursor.field = 0;
            } else {
                let lane = lane_or_fx.clamp(0.0, lanes - 1.0);
                self.cursor.lane = lane as u8;
                let within = c - g.c0 - WAVE - lane * g.lane;
                // The velocity field is its two digits; the rest of the lane is the note.
                self.cursor.field = (within > self.cw * 4.0 && within < self.cw * 6.5 + 4.0) as u8;
            }
            if !fe.modifiers.shift && !secondary && self.cursor.lane != FX_LANE && self.adds_here(ch, home, fe.tap_count) {
                self.cursor.field = 0;
                let pitch = self.pointer_pitch();
                self.enter_note(cx, pitch, false);
            }
            if fe.modifiers.shift {
                let anchor = self.sel.map_or((home, ch), |s| (s.r0, s.c0));
                self.sel = Some(Sel { r0: anchor.0.min(home), r1: anchor.0.max(home), c0: anchor.1.min(ch), c1: anchor.1.max(ch) });
            } else {
                self.sel = None;
                self.drag = Some(Drag::Select { anchor: (home, ch) });
            }
        }
        cx.widget_action(self.uid, TrackerAction::Voice(self.model.channels[ch].name.clone()));
    }

    fn finger_move(&mut self, cx: &mut Cx, abs: Vec2d) {
        let Some(drag) = self.drag.clone() else { return };
        match drag {
            Drag::Select { anchor } => {
                let (t, c) = self.at(abs);
                let row = (t / self.row_px()).floor() as i64;
                let (Some(home), Some(ch)) = (self.home_row(row), self.channel_at(c).or(Some(self.cursor.ch))) else { return };
                if (home, ch) != anchor {
                    self.sel = Some(Sel { r0: anchor.0.min(home), r1: anchor.0.max(home), c0: anchor.1.min(ch), c1: anchor.1.max(ch) });
                    self.cursor.row = home;
                    self.cursor.ch = ch;
                }
            }
            Drag::Note { ch, ev, at, resize, .. } => self.drag = Some(Drag::Note { ch, ev, at, now: abs, resize }),
            Drag::Fader { ch, field, from, base, .. } => {
                use std::fmt::Write;
                let (lo, hi) = if field == "amp" { (0.0f32, 2.0f32) } else { (-1.0, 1.0) };
                let v = (base - ((abs.y - from.y) / KNOB_DRAG) as f32 * (hi - lo)).clamp(lo, hi);
                let v = (v * 100.0).round() / 100.0;
                self.drag_text.clear();
                let _ = if field == "amp" { write!(self.drag_text, "{:.0}%", v * 100.0) } else { write!(self.drag_text, "{v:.2}") };
                let name = self.model.channels[ch].name.clone();
                cx.widget_action(self.uid, TrackerAction::Hold { channel: name, field, value: v });
                self.drag = Some(Drag::Fader { ch, field, value: v, from, base });
            }
            Drag::Ruler { from, .. } => {
                let (t, _) = self.at(abs);
                let row = (t / self.row_px()).floor() as i64;
                let to = self.content_beat(row).unwrap_or(from);
                self.drag = Some(Drag::Ruler { from, to });
            }
            Drag::PopFader { what, .. } => {
                let key = match &what {
                    PopFader::Level => PopHit::Level,
                    PopFader::Pan => PopHit::Pan,
                    PopFader::Send(fx) => PopHit::Send(fx.clone()),
                };
                let r = self.popup.as_ref().and_then(|p| p.hits.iter().find(|(h, _)| *h == key).map(|(_, r)| *r));
                let (Some(r), Some(ch)) = (r, self.popup.as_ref().and_then(|p| self.model.channel(&p.ch))) else { return };
                let value = self.pop_fader_value(&what, Self::fader_value(r, abs));
                self.hold_pop(cx, ch, &what, value);
                self.drag = Some(Drag::PopFader { what, value });
            }
        }
    }

    fn finger_up(&mut self, cx: &mut Cx) {
        let Some(drag) = self.drag.take() else { return };
        match drag {
            Drag::Select { .. } => {}
            Drag::Note { ch, ev, at, now, resize } => {
                if (now - at).length() < 3.0 {
                    return;
                }
                if !self.writable(cx, ch) {
                    return;
                }
                let Some(d) = self.dragged(ch, ev, at, now, resize) else { return };
                let c = &self.model.channels[ch];
                let e = &c.events[ev];
                let splices = tm::change_event(c, e, d.beat, d.pitch, d.dur, e.vel);
                self.emit(cx, splices, if resize { "note resized" } else { "note moved" });
            }
            Drag::Fader { ch, field, value, .. } => {
                let v = makepad_score_live::source::fmt_num(value as f64, 2);
                self.write_field(cx, ch, field, v, field);
            }
            Drag::Ruler { from, to } => {
                if (to - from).abs() < self.rb() * 0.5 {
                    cx.widget_action(self.uid, TrackerAction::Seek(from));
                } else {
                    let (a, b) = (from.min(to), from.max(to) + self.rb());
                    cx.widget_action(self.uid, TrackerAction::Loop(Some((a, b))));
                }
            }
            Drag::PopFader { what, value } => self.release_pop(cx, what, value),
        }
    }
}

impl Widget for ScoreTracker {
    /// A grid: its name, and where the cursor is (channel, row, the cell).
    fn semantics(&self, _cx: &Cx, node: &mut makepad_widgets::makepad_platform::a11y_tree::Node) {
        use std::fmt::Write;
        node.role = makepad_widgets::makepad_platform::a11y_tree::Role::Grid;
        node.states = makepad_widgets::makepad_platform::a11y_tree::States::FOCUSABLE;
        node.name.clear();
        node.name.push_str("Tracker");
        node.value.clear();
        if let Some(c) = self.model.channels.get(self.cursor.ch) {
            let row = self.labels.get(self.cursor.row.max(0) as usize).map_or("", String::as_str);
            let _ = write!(node.value, "{} row {row}", c.label);
            if let Some(song) = self.content_beat(self.cursor.row) {
                let l = c.local(song);
                if let Some(e) = c.range(l, l + self.rb()).map(|i| &c.events[i]).find(|e| e.lane == self.cursor.lane) {
                    let _ = write!(node.value, ": {} {}", e.text, e.vel_text);
                }
            }
        }
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.tick.is_event(event).is_some() {
            self.redraw(cx);
        }
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                self.hover = Some(fe.abs);
                let item = self.popup_hit(fe.abs).and_then(|h| if let PopHit::Item(i) = h { Some(i) } else { None });
                if let Some(p) = &mut self.popup {
                    if p.rect.contains(fe.abs) && item != p.hover && item.is_some() {
                        p.hover = item;
                        self.preview(cx, item);
                    }
                }
                let cursor = if self.head_hit(fe.abs).is_some_and(|(_, h)| matches!(h, HeadHit::Level | HeadHit::Pan)) {
                    MouseCursor::EwResize
                } else if self.tools.iter().any(|(_, r)| r.contains(fe.abs)) || self.head_hit(fe.abs).is_some() || self.popup.is_some() {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Default
                };
                cx.set_cursor(cursor);
                self.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.hover = None;
                self.redraw(cx);
            }
            Hit::FingerDown(fe) => {
                self.finger_down(cx, &fe);
                self.redraw(cx);
            }
            Hit::FingerMove(fe) => {
                self.finger_move(cx, fe.abs);
                self.redraw(cx);
            }
            Hit::FingerUp(_) => {
                self.finger_up(cx);
                self.redraw(cx);
            }
            Hit::FingerScroll(fe) => {
                if let Some(p) = &mut self.popup {
                    if p.list_rect.contains(fe.abs) {
                        p.scroll = (p.scroll + fe.scroll.y).clamp(0.0, (p.list.len() as f64 * 18.0 - p.list_rect.size.y).max(0.0));
                        self.redraw(cx);
                        return;
                    }
                }
                if fe.modifiers.control || fe.modifiers.logo {
                    let step = if fe.scroll.y.abs() >= fe.scroll.x.abs() { fe.scroll.y } else { fe.scroll.x };
                    if step.abs() > 0.5 {
                        self.tool(cx, if step < 0.0 { Tool::LpbUp } else { Tool::LpbDown });
                    }
                } else {
                    let (mut dt, mut dc) = if self.vertical() { (fe.scroll.y, fe.scroll.x) } else { (fe.scroll.x, fe.scroll.y) };
                    if fe.modifiers.shift {
                        std::mem::swap(&mut dt, &mut dc);
                    }
                    self.scroll_t += dt;
                    self.scroll_c += dc;
                    self.target_t = None;
                }
                self.redraw(cx);
            }
            Hit::KeyDown(ke) => {
                self.key(cx, &ke);
                self.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.rect = rect;
        let p = makepad_look::palette::get(cx);
        if self.cw <= 0.0 {
            self.cw = advance(&self.draw_cell, cx, "0000000000") / 10.0;
            self.rebuild_labels();
        }
        self.draw_bg.color = rgba(p.wave_bg, 1.0);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        self.regions(rect);
        if self.geo_dirty || self.geo.len() != self.model.channels.len() || (self.view_c() - self.laid_view).abs() > 0.5 {
            self.layout_channels();
        }
        if !self.placed && self.view_t() > 0.0 {
            self.placed = true;
            let row = self.cursor.row;
            self.scroll_t = if self.vertical() { (row as f64 + 0.5) * self.row_px() - self.view_t() * 0.5 } else { 0.0 };
        }
        if self.model.channels.is_empty() {
            put(&mut self.draw_text, cx, rect.pos + dvec2(16.0, TOOLBAR + 16.0), "No loops yet: a Loop{…} in the code shows here as a channel.", rgba(p.wave_label, 1.0));
            self.draw_toolbar(cx, &p);
            return DrawStep::done();
        }
        self.pump_live();
        self.scroll_frame(cx);
        self.clamp_scroll();
        self.draw_grid(cx, &p);
        self.draw_ruler(cx, &p);
        self.draw_heads(cx, &p);
        self.draw_toolbar(cx, &p);
        self.draw_popup(cx, &p);
        DrawStep::done()
    }
}
