//! The Arrange view: the score's arrangement as a timeline. A ruler of
//! bars and beats, the sections as labelled blocks (repeats bracketed),
//! a chord per bar, then a lane per loop (its role and instrument, mute,
//! solo and level) whose clips are the sections it plays in, drawn with
//! their notes: a piano roll fitted to the lane's range, or a hit per drum
//! row. Every gesture is an edit of the score's source (a Splice):
//! - a clip: click selects, drag moves it to another section, alt-drag
//!   copies it, Delete (or its ×) takes the loop out of that section; a
//!   click on an empty cell adds the loop there;
//! - a section: drag its right edge to resize (whole bars), drag it to
//!   reorder the arrangement (alt-drag plays it once more there), Split /
//!   Copy / Delete (S, ⌘D, Delete) when selected: a split falls at the bar
//!   clicked in it;
//! - the ruler seeks (by bar); ⌘/ctrl-scroll or a pinch zooms, scroll and
//!   shift-scroll move.

use super::arrange_model::{self, ArrangeModel, ClipNote, Lane, Role};
use makepad_score_player::Meters;
use makepad_widgets::badge::advance;
use makepad_widgets::*;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    set_type_default() do #(DrawArrangeBlock::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let w = self.rect_size.x
            let h = self.rect_size.y
            let p = self.pos * self.rect_size
            if self.shape > 2.5 {
                // A dashed rounded outline (an empty cell under the pointer).
                sdf.box(1., 1., w - 2., h - 2., self.radius)
                sdf.fill_keep(self.fill)
                let dash = step(0.5, fract((p.x + p.y) / 8.))
                sdf.stroke(vec4(self.edge.xyz, self.edge.w * (1. - dash)), 1.)
                return sdf.result
            }
            if self.shape > 1.5 {
                // A pentagon pointing down (the playhead's handle).
                sdf.move_to(0.5, 0.5)
                sdf.line_to(w - 0.5, 0.5)
                sdf.line_to(w - 0.5, h * 0.55)
                sdf.line_to(w * 0.5, h - 0.5)
                sdf.line_to(0.5, h * 0.55)
                sdf.close_path()
                sdf.fill(self.fill)
                return sdf.result
            }
            if self.shape > 0.5 {
                // A clip: its body, a brighter top band, the rim.
                sdf.box(0.5, 0.5, w - 1., h - 1., self.radius)
                let band = 1. - step(13., p.y)
                sdf.fill_keep(mix(self.fill, vec4(self.edge.xyz, self.fill.w * 1.8), band * 0.6))
                sdf.stroke(self.edge, self.stroke)
                return sdf.result
            }
            sdf.box(0.5, 0.5, w - 1., h - 1., self.radius)
            sdf.fill_keep(self.fill)
            sdf.stroke(self.edge, self.stroke)
            return sdf.result
        }
    }

    mod.widgets.ScoreArrangeBase = #(ScoreArrange::register_widget(vm))
    mod.widgets.ScoreArrange = set_type_default() do mod.widgets.ScoreArrangeBase{
        width: Fill height: Fill
        draw_bg +: {color: stage.wave_bg}
        draw_rect +: {color: stage.wave_rim}
        draw_text +: {color: stage.wave_label text_style: theme.font_regular{font_size: 7.5}}
        draw_bold +: {color: stage.glass_ink text_style: theme.font_bold{font_size: 8}}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawArrangeBlock {
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
    /// 0 a rounded box, 1 a clip, 2 a pentagon, 3 a dashed box.
    #[live]
    pub shape: f32,
}

#[derive(Clone, Debug, Default)]
pub enum ScoreArrangeAction {
    #[default]
    None,
    Splice { start: usize, end: usize, text: String },
    /// The ruler, pressed or dragged: play from this beat of the song.
    Seek(f64),
    /// An edit refused, or what a gesture does, for the status line.
    Status(String),
    /// A voice's lane or clip pressed: the voice is selected (the panel
    /// shows its timeline and its instrument's controls).
    Voice(String),
}

/// Where the music is: the song beat (None: off the arrangement).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ArrangeLive {
    pub song_beat: Option<f64>,
    pub playing: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Selected {
    None,
    /// A clip: the section's place in the arrangement, the loop.
    Clip(usize, String),
    /// A section, and the bar a split falls after.
    Section(usize, f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Flag {
    Mute,
    Solo,
}

#[derive(Clone, Debug, PartialEq)]
enum Target {
    None,
    Ruler,
    /// A section block: its index, the bar under the pointer in it, and
    /// whether the pointer is on its right edge.
    Strip(usize, f64, bool),
    /// The selected section's buttons: split, copy, delete.
    Button(usize),
    Chords,
    Header(usize, Option<Flag>),
    /// A cell: the section's index, the lane's, whether the loop plays.
    Cell(usize, usize, bool),
    /// The selected clip's ×.
    Close(usize, usize),
}

#[derive(Clone, Debug)]
struct Drag {
    target: Target,
    from: Vec2d,
    at: Vec2d,
    moved: bool,
    alt: bool,
}

const HEAD: f64 = 184.0;
const RULER: f64 = 22.0;
const STRIP: f64 = 32.0;
const CHORDS: f64 = 20.0;
const TOP: f64 = RULER + STRIP + CHORDS + 2.0;
const MELODIC: f64 = 50.0;
const EDGE_GRAB: f64 = 6.0;
const MIN_PX_PER_BEAT: f64 = 1.5;
const MAX_PX_PER_BEAT: f64 = 400.0;
const BUTTONS: [&str; 3] = ["Split", "Copy", "Delete"];

#[derive(Script, ScriptHook, Widget)]
pub struct ScoreArrange {
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
    draw_block: DrawArrangeBlock,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_bold: DrawText,
    #[rust]
    model: ArrangeModel,
    #[rust]
    live: ArrangeLive,
    #[rust]
    meters: Option<Arc<dyn Meters>>,
    #[rust]
    rect: Rect,
    #[rust]
    tick: NextFrame,
    #[rust]
    px_per_beat: f64,
    /// The beat at the timeline's left edge, and the lanes' scroll (px).
    #[rust]
    scroll: f64,
    #[rust]
    scroll_y: f64,
    /// The arrangement length the view was last fitted to.
    #[rust]
    fitted: Option<f64>,
    #[rust]
    followed: Option<f64>,
    #[rust(Selected::None)]
    selected: Selected,
    #[rust]
    drag: Option<Drag>,
    #[rust]
    hover: Option<Vec2d>,
}

fn color(value: u32, alpha: f32) -> Vec4f {
    let mut c = Vec4f::from_u32(value);
    c.w *= alpha;
    c
}

fn role_color(role: Role, p: &makepad_look::palette::Palette) -> u32 {
    match role {
        Role::Drums => p.wave_drums,
        Role::Bass => p.wave_bass,
        Role::Keys => p.info,
        Role::Pad => p.key,
        Role::Arp => p.teal,
        Role::Lead => p.accent,
        Role::Vocals => p.wave_vocals,
        Role::Other => p.wave_other,
    }
}

/// A lane's height: a melodic lane a fixed roll, a drum lane its rows.
fn lane_height(lane: &Lane) -> f64 {
    if lane.drums { 16.0 + (lane.rows.len().max(1) as f64 * 8.0).clamp(16.0, 48.0) } else { MELODIC }
}

/// Bars between ruler labels: labels at least 36 px apart.
fn label_every(px_per_bar: f64) -> usize {
    [1, 2, 4, 8, 16, 32, 64].into_iter().find(|n| *n as f64 * px_per_bar >= 36.0).unwrap_or(128)
}

impl ScoreArrange {
    pub fn set_model(&mut self, cx: &mut Cx, model: ArrangeModel) {
        if model != self.model {
            self.model = model;
            self.redraw(cx);
        }
    }

    pub fn set_live(&mut self, cx: &mut Cx, live: ArrangeLive) {
        if live != self.live {
            self.live = live;
            self.follow();
            self.redraw(cx);
        }
    }

    pub fn set_meters(&mut self, meters: Arc<dyn Meters>) {
        self.meters = Some(meters);
    }

    fn x0(&self) -> f64 {
        self.rect.pos.x + HEAD
    }

    fn x1(&self) -> f64 {
        self.rect.pos.x + self.rect.size.x
    }

    fn x_of(&self, beat: f64) -> f64 {
        self.x0() + (beat - self.scroll) * self.px_per_beat
    }

    fn beat_of(&self, x: f64) -> f64 {
        self.scroll + (x - self.x0()) / self.px_per_beat.max(1e-6)
    }

    fn bar(&self) -> f64 {
        self.model.beats_per_bar.max(1.0)
    }

    fn lanes_top(&self) -> f64 {
        self.rect.pos.y + TOP
    }

    /// Each lane's top and height, scrolled.
    fn lane_rows(&self) -> Vec<(f64, f64)> {
        let mut y = self.lanes_top() - self.scroll_y;
        self.model
            .lanes
            .iter()
            .map(|lane| {
                let h = lane_height(lane);
                let row = (y, h);
                y += h;
                row
            })
            .collect()
    }

    fn lanes_height(&self) -> f64 {
        self.model.lanes.iter().map(lane_height).sum()
    }

    fn clamp_scroll(&mut self) {
        let width = (self.x1() - self.x0()).max(1.0) / self.px_per_beat.max(1e-6);
        self.scroll = self.scroll.clamp(0.0, (self.model.total - width * 0.5).max(0.0));
        let room = (self.rect.size.y - TOP).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, (self.lanes_height() - room + 8.0).max(0.0));
    }

    fn fit(&mut self) {
        // Two bars of room after the end: a section there grows into it.
        let width = (self.rect.size.x - HEAD - 8.0).max(100.0);
        self.px_per_beat = (width / (self.model.total + 2.0 * self.bar()).max(1.0)).clamp(MIN_PX_PER_BEAT, MAX_PX_PER_BEAT);
        self.scroll = 0.0;
        self.fitted = Some(self.model.total);
    }

    fn zoom_about(&mut self, px_per_beat: f64, x: f64) {
        let anchor = self.beat_of(x);
        self.px_per_beat = px_per_beat.clamp(MIN_PX_PER_BEAT, MAX_PX_PER_BEAT);
        self.scroll = anchor - (x - self.x0()) / self.px_per_beat;
        self.clamp_scroll();
    }

    /// While playing, page when the playhead reaches 85 % of the width.
    fn follow(&mut self) {
        let Some(beat) = self.live.song_beat.filter(|_| self.live.playing) else { return };
        if self.followed == Some(beat) || self.drag.is_some() || self.px_per_beat <= 0.0 {
            return;
        }
        self.followed = Some(beat);
        let x = self.x_of(beat);
        let width = self.x1() - self.x0();
        if x > self.x0() + width * 0.85 || x < self.x0() {
            self.scroll = beat - 0.1 * width / self.px_per_beat;
            self.clamp_scroll();
        }
    }

    /// The rectangles of the selected section's buttons (in the header
    /// column, on the section row).
    fn buttons(&self) -> Vec<Rect> {
        let y = self.rect.pos.y + RULER + 8.0;
        let mut x = self.rect.pos.x + 8.0;
        BUTTONS
            .iter()
            .map(|label| {
                let w = 10.0 + label.len() as f64 * 5.4;
                let r = Rect { pos: dvec2(x, y), size: dvec2(w, 18.0) };
                x += w + 4.0;
                r
            })
            .collect()
    }

    /// A header's mute and solo toggles.
    fn flags(&self, top: f64) -> [(Flag, Rect); 2] {
        let x = self.rect.pos.x + HEAD - 48.0;
        [(Flag::Mute, Rect { pos: dvec2(x, top + 6.0), size: dvec2(16.0, 14.0) }), (Flag::Solo, Rect { pos: dvec2(x + 19.0, top + 6.0), size: dvec2(16.0, 14.0) })]
    }

    /// A clip's rectangle: a section in a lane.
    fn clip_rect(&self, section: usize, top: f64, h: f64) -> Rect {
        let s = &self.model.sections[section];
        let (a, b) = (self.x_of(s.start), self.x_of(s.start + s.len));
        Rect { pos: dvec2(a + 1.0, top + 2.0), size: dvec2((b - a - 2.0).max(1.0), h - 4.0) }
    }

    fn close_rect(clip: Rect) -> Rect {
        Rect { pos: dvec2(clip.pos.x + clip.size.x - 13.0, clip.pos.y + 1.0), size: dvec2(12.0, 11.0) }
    }

    /// Where a note sits in its clip: a melodic lane's roll (the lane's
    /// range over the clip below its label), a drum lane's row.
    fn note_rect(&self, lane: &Lane, clip: Rect, start: f64, n: &ClipNote) -> Rect {
        let top = clip.pos.y + 13.0;
        let h = (clip.pos.y + clip.size.y - 2.0 - top).max(4.0);
        let x = self.x_of(start + n.beat);
        if lane.drums {
            let rows = lane.rows.len().max(1);
            let row = lane.rows.iter().position(|r| *r == n.pitch).unwrap_or(0);
            let rh = h / rows as f64;
            let w = (self.px_per_beat * 0.18).clamp(2.0, 5.0);
            Rect { pos: dvec2(x, top + row as f64 * rh + rh * 0.12), size: dvec2(w, (rh * 0.76).max(2.0)) }
        } else {
            let span = (lane.high - lane.low + 1.0).max(1.0) as f64;
            let rh = h / span;
            let y = top + (lane.high as f64 - n.pitch as f64) * rh;
            let w = (n.dur as f64 * self.px_per_beat - 1.0).max(2.0);
            Rect { pos: dvec2(x, y), size: dvec2(w, (rh - 0.5).max(1.5)) }
        }
    }

    fn target(&self, at: Vec2d) -> Target {
        let r = self.rect;
        if !r.contains(at) {
            return Target::None;
        }
        let y = at.y - r.pos.y;
        if at.x < self.x0() {
            if (RULER..RULER + STRIP).contains(&y) {
                if let Selected::Section(..) = self.selected {
                    if let Some(k) = self.buttons().iter().position(|b| b.contains(at)) {
                        return Target::Button(k);
                    }
                }
                return Target::None;
            }
            if at.y < self.lanes_top() {
                return Target::None;
            }
            for (i, (top, h)) in self.lane_rows().into_iter().enumerate() {
                if at.y >= top && at.y < top + h {
                    let flag = self.flags(top).into_iter().find(|(_, rect)| rect.contains(at)).map(|(f, _)| f);
                    return Target::Header(i, flag);
                }
            }
            return Target::None;
        }
        if y < RULER {
            return Target::Ruler;
        }
        let beat = self.beat_of(at.x);
        if y < RULER + STRIP {
            // The nearest right edge within the grab, else the section.
            for (i, s) in self.model.sections.iter().enumerate() {
                if (self.x_of(s.start + s.len) - at.x).abs() <= EDGE_GRAB {
                    return Target::Strip(i, s.bars, true);
                }
            }
            return match self.model.section_at(beat) {
                Some(i) => {
                    let s = &self.model.sections[i];
                    Target::Strip(i, ((beat - s.start) / self.bar()).round(), false)
                }
                None => Target::None,
            };
        }
        if at.y < self.lanes_top() {
            return Target::Chords;
        }
        let Some(section) = self.model.section_at(beat) else { return Target::None };
        for (i, (top, h)) in self.lane_rows().into_iter().enumerate() {
            if at.y >= top && at.y < top + h {
                let lane = &self.model.lanes[i];
                let on = self.model.sections[section].loops.contains(&lane.name);
                if on && self.selected == Selected::Clip(section, lane.name.clone()) && Self::close_rect(self.clip_rect(section, top, h)).contains(at) {
                    return Target::Close(section, i);
                }
                return Target::Cell(section, i, on);
            }
        }
        Target::None
    }

    /// The source edited, as the one change to it; a refusal goes to the
    /// status line.
    fn edit(&mut self, cx: &mut Cx, result: Result<String, String>, done: &str) {
        match result {
            Ok(next) if next != self.model.source => {
                let splice = arrange_model::splice_of(&self.model.source, &next);
                cx.widget_action(self.uid, ScoreArrangeAction::Splice { start: splice.start, end: splice.end, text: splice.text });
                cx.widget_action(self.uid, ScoreArrangeAction::Status(done.to_string()));
            }
            Ok(_) => {}
            Err(error) => cx.widget_action(self.uid, ScoreArrangeAction::Status(error)),
        }
    }

    fn section_button(&mut self, cx: &mut Cx, k: usize) {
        let Selected::Section(index, at) = self.selected.clone() else { return };
        let Some(section) = self.model.sections.get(index).cloned() else { return };
        let src = self.model.source.clone();
        match k {
            0 => {
                let at = if at >= 1.0 && at <= section.bars - 1.0 { at } else { (section.bars / 2.0).floor() };
                let result = arrange_model::split_section(&src, &section.name, at);
                self.edit(cx, result, &format!("split {} after bar {}", section.name, at));
            }
            1 => {
                let result = arrange_model::duplicate_section(&src, index);
                self.selected = Selected::Section(index + 1, at);
                self.edit(cx, result, &format!("copied {}", section.name));
            }
            _ => {
                let result = arrange_model::delete_section(&src, index);
                self.selected = Selected::None;
                self.edit(cx, result, &format!("{} out of the arrangement", section.name));
            }
        }
    }

    /// Where a section dragged to `x` lands: its index once moved.
    fn drop_index(&self, from: usize, x: f64) -> usize {
        let beat = self.beat_of(x);
        self.model.sections.iter().enumerate().filter(|(i, s)| *i != from && s.start + s.len * 0.5 < beat).count()
    }

    fn finish_drag(&mut self, cx: &mut Cx, drag: Drag) {
        let src = self.model.source.clone();
        let bar = self.bar();
        match drag.target {
            Target::Strip(index, bars, true) => {
                let Some(section) = self.model.sections.get(index).cloned() else { return };
                let next = (bars + (drag.at.x - drag.from.x) / (self.px_per_beat * bar)).round().max(1.0);
                if drag.moved && (next - section.bars).abs() > 1e-9 {
                    let result = arrange_model::set_bars(&src, &section.name, next);
                    self.edit(cx, result, &format!("{} is {} bars", section.name, next));
                }
            }
            Target::Strip(index, _, false) if drag.moved => {
                let to = self.drop_index(index, drag.at.x);
                let name = self.model.sections.get(index).map(|s| s.name.clone()).unwrap_or_default();
                if drag.alt {
                    let to = self.model.sections.iter().filter(|s| s.start + s.len * 0.5 < self.beat_of(drag.at.x)).count();
                    self.selected = Selected::Section(to, 0.0);
                    let result = arrange_model::repeat_section(&src, index, to);
                    self.edit(cx, result, &format!("{name} plays again"));
                } else if to != index {
                    self.selected = Selected::Section(to, 0.0);
                    let result = arrange_model::move_section(&src, index, to);
                    self.edit(cx, result, &format!("moved {name}"));
                }
            }
            Target::Cell(section, lane, true) if drag.moved => {
                let Some(to) = self.model.section_at(self.beat_of(drag.at.x)) else { return };
                let (from, to_name) = (self.model.sections[section].name.clone(), self.model.sections[to].name.clone());
                let name = self.model.lanes[lane].name.clone();
                if from != to_name {
                    self.selected = Selected::Clip(to, name.clone());
                    let result = arrange_model::move_clip(&src, &from, &to_name, &name, drag.alt);
                    self.edit(cx, result, &format!("{name} {} {to_name}", if drag.alt { "copied to" } else { "moved to" }));
                }
            }
            Target::Cell(section, lane, false) if !drag.moved => {
                let (section, name) = (self.model.sections[section].name.clone(), self.model.lanes[lane].name.clone());
                let result = arrange_model::add_clip(&src, &section, &name);
                self.edit(cx, result, &format!("{name} plays in {section}"));
            }
            Target::Close(section, lane) if !drag.moved => self.delete_clip(cx, section, lane),
            Target::Header(lane, Some(flag)) if !drag.moved => {
                let lane = self.model.lanes[lane].clone();
                let (key, on) = match flag {
                    Flag::Mute => ("mute", !lane.mute),
                    Flag::Solo => ("solo", !lane.solo),
                };
                let result = arrange_model::set_flag(&src, &lane.name, key, on);
                self.edit(cx, result, &format!("{} {key} {}", lane.name, if on { "on" } else { "off" }));
            }
            Target::Button(k) if !drag.moved => self.section_button(cx, k),
            _ => {}
        }
    }

    fn delete_clip(&mut self, cx: &mut Cx, section: usize, lane: usize) {
        let (section, name) = (self.model.sections[section].name.clone(), self.model.lanes[lane].name.clone());
        self.selected = Selected::None;
        let result = arrange_model::remove_clip(&self.model.source.clone(), &section, &name);
        self.edit(cx, result, &format!("{name} out of {section}"));
    }

    fn seek(&mut self, cx: &mut Cx, x: f64) {
        let bar = self.bar();
        let beat = ((self.beat_of(x) / bar).floor() * bar).clamp(0.0, (self.model.total - bar).max(0.0));
        cx.widget_action(self.uid, ScoreArrangeAction::Seek(beat));
    }

    /// What the pointer is over, for its bubble.
    fn hover_text(&self, at: Vec2d) -> Option<String> {
        match self.target(at) {
            Target::Strip(i, _, true) => Some(format!("{} · {} bars: drag to resize", self.model.sections[i].name, self.model.sections[i].bars)),
            Target::Strip(i, bar, false) => {
                let s = &self.model.sections[i];
                let repeat = if s.run.1 > 1 { format!(" · {} of {}", s.run.0 + 1, s.run.1) } else if s.again { " · again".into() } else { String::new() };
                Some(format!("{} · {} bars{repeat} · bar {}", s.name, s.bars, bar as usize + 1))
            }
            Target::Button(k) => Some(["Split it after the bar clicked (S)", "Copy it, played right after (⌘D)", "Take it out of the arrangement (Delete)"][k].into()),
            Target::Header(_, Some(Flag::Mute)) => Some("Mute".into()),
            Target::Header(_, Some(Flag::Solo)) => Some("Solo".into()),
            Target::Close(..) => Some("Take the loop out of this section".into()),
            Target::Chords => {
                let bar = (self.beat_of(at.x) / self.bar()).floor() as usize;
                self.model.chords.iter().find(|(first, len, _)| bar >= *first && bar < first + len).map(|(_, _, name)| format!("{name} · bar {}", bar + 1))
            }
            Target::Cell(section, lane, on) => {
                let l = &self.model.lanes[lane];
                let s = &self.model.sections[section];
                if !on {
                    return Some(format!("Click: {} plays in {}", l.name, s.name));
                }
                let (top, h) = self.lane_rows()[lane];
                let clip = self.clip_rect(section, top, h);
                let notes = self.model.notes.get(&s.name, s.occurrence, &l.name);
                let note = notes.iter().rev().find(|n| {
                    let r = self.note_rect(l, clip, s.start, n);
                    Rect { pos: r.pos - dvec2(1.0, 1.0), size: r.size + dvec2(2.0, 2.0) }.contains(at)
                });
                Some(match note {
                    Some(n) => format!("{} · vel {:.2} · bar {}", arrange_model::pitch_name(n.pitch, n.hit), n.vel, ((s.start + n.beat) / self.bar()).floor() as usize + 1),
                    None => format!("{} in {}", l.name, s.name),
                })
            }
            _ => None,
        }
    }

    fn rect_in(&mut self, cx: &mut Cx2d, r: Rect, clip: Rect, c: Vec4f) {
        let a = dvec2(r.pos.x.max(clip.pos.x), r.pos.y.max(clip.pos.y));
        let b = dvec2((r.pos.x + r.size.x).min(clip.pos.x + clip.size.x), (r.pos.y + r.size.y).min(clip.pos.y + clip.size.y));
        if b.x > a.x && b.y > a.y {
            self.draw_rect.color = c;
            self.draw_rect.draw_abs(cx, Rect { pos: a, size: b - a });
        }
    }

    fn block(&mut self, cx: &mut Cx2d, r: Rect, shape: f32, fill: Vec4f, edge: Vec4f, stroke: f32, radius: f32) {
        if r.size.x > 0.0 && r.size.y > 0.0 {
            self.draw_block.fill = fill;
            self.draw_block.edge = edge;
            self.draw_block.stroke = stroke;
            self.draw_block.radius = radius;
            self.draw_block.shape = shape;
            self.draw_block.draw_abs(cx, r);
        }
    }

    /// Text clipped to `width` with an ellipsis; its width drawn.
    fn text(&mut self, cx: &mut Cx2d, bold: bool, pos: Vec2d, text: &str, width: f64, c: Vec4f) -> f64 {
        let draw = if bold { &mut self.draw_bold } else { &mut self.draw_text };
        if width < 10.0 {
            return 0.0;
        }
        let fitted = if advance(draw, cx, text) <= width {
            text.to_string()
        } else {
            let chars: Vec<char> = text.chars().collect();
            let (mut lo, mut hi) = (0usize, chars.len());
            while lo < hi {
                let mid = (lo + hi).div_ceil(2);
                let candidate = format!("{}…", chars[..mid].iter().collect::<String>());
                if advance(draw, cx, &candidate) <= width { lo = mid } else { hi = mid - 1 }
            }
            if lo == 0 {
                return 0.0;
            }
            format!("{}…", chars[..lo].iter().collect::<String>())
        };
        draw.color = c;
        draw.draw_abs(cx, pos, &fitted);
        advance(draw, cx, &fitted)
    }
}

impl Widget for ScoreArrange {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.tick.is_event(event).is_some() {
            self.redraw(cx);
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                self.hover = Some(fe.abs);
                let cursor = match self.target(fe.abs) {
                    Target::Strip(_, _, true) => MouseCursor::EwResize,
                    Target::Ruler => MouseCursor::Text,
                    Target::Strip(..) | Target::Cell(_, _, true) => MouseCursor::Grab,
                    Target::Cell(..) | Target::Button(_) | Target::Header(_, Some(_)) | Target::Close(..) => MouseCursor::Hand,
                    _ => MouseCursor::Default,
                };
                cx.set_cursor(cursor);
                self.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.hover = None;
                self.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                let target = self.target(fe.abs);
                match &target {
                    Target::Ruler => self.seek(cx, fe.abs.x),
                    Target::Strip(i, bar, edge) => {
                        let bar = if *edge { (self.model.sections[*i].bars / 2.0).floor() } else { *bar };
                        self.selected = Selected::Section(*i, bar);
                    }
                    Target::Cell(section, lane, true) => self.selected = Selected::Clip(*section, self.model.lanes[*lane].name.clone()),
                    Target::Cell(..) | Target::Chords => self.selected = Selected::None,
                    _ => {}
                }
                let voice = match &target {
                    Target::Cell(_, lane, _) | Target::Header(lane, None) => self.model.lanes.get(*lane).map(|l| l.name.clone()),
                    _ => None,
                };
                if let Some(voice) = voice {
                    cx.widget_action(self.uid, ScoreArrangeAction::Voice(voice));
                }
                self.drag = Some(Drag { target, from: fe.abs, at: fe.abs, moved: false, alt: fe.modifiers.alt });
                self.redraw(cx);
            }
            Hit::FingerMove(fe) => {
                let Some(drag) = &mut self.drag else { return };
                drag.at = fe.abs;
                drag.alt = fe.modifiers.alt;
                if (fe.abs - drag.from).length() > 4.0 {
                    drag.moved = true;
                }
                if drag.target == Target::Ruler {
                    self.seek(cx, fe.abs.x);
                }
                self.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                if let Some(mut drag) = self.drag.take() {
                    drag.at = fe.abs;
                    drag.alt = fe.modifiers.alt || drag.alt;
                    self.finish_drag(cx, drag);
                }
                self.redraw(cx);
            }
            Hit::FingerScroll(fe) => {
                if fe.modifiers.control || fe.modifiers.logo {
                    let step = if fe.scroll.y.abs() >= fe.scroll.x.abs() { fe.scroll.y } else { fe.scroll.x };
                    self.zoom_about(self.px_per_beat * (-step * 0.01).exp(), fe.abs.x);
                } else if fe.scroll.x.abs() > fe.scroll.y.abs() || fe.modifiers.shift {
                    let step = if fe.scroll.x.abs() > fe.scroll.y.abs() { fe.scroll.x } else { fe.scroll.y };
                    self.scroll += step / self.px_per_beat;
                    self.clamp_scroll();
                } else {
                    self.scroll_y += fe.scroll.y;
                    self.clamp_scroll();
                }
                self.redraw(cx);
            }
            Hit::FingerPinch(fe) => {
                self.zoom_about(self.px_per_beat * fe.scale, fe.abs.x);
                self.redraw(cx);
            }
            Hit::KeyDown(ke) => {
                let command = ke.modifiers.logo || ke.modifiers.control;
                match (&self.selected.clone(), ke.key_code) {
                    (Selected::Clip(section, name), KeyCode::Delete | KeyCode::Backspace) => {
                        if let Some(lane) = self.model.lanes.iter().position(|l| &l.name == name) {
                            if *section < self.model.sections.len() {
                                self.delete_clip(cx, *section, lane);
                            }
                        }
                    }
                    (Selected::Section(..), KeyCode::Delete | KeyCode::Backspace) => self.section_button(cx, 2),
                    (Selected::Section(..), KeyCode::KeyD) if command => self.section_button(cx, 1),
                    (Selected::Section(..), KeyCode::KeyS) if !command => self.section_button(cx, 0),
                    (_, KeyCode::Escape) => self.selected = Selected::None,
                    (_, KeyCode::Key0) if command => self.fit(),
                    _ => {}
                }
                self.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.rect = rect;
        let p = makepad_look::palette::get(cx);
        self.draw_bg.color = color(p.wave_bg, 1.0);
        self.draw_bg.draw_abs(cx, rect);
        let m = self.model.clone();
        if m.sections.is_empty() {
            self.text(cx, false, rect.pos + dvec2(16.0, 16.0), "No arrangement: every loop plays. Add sections (Section{bars loops}) and list them in Score{arrange: [...]}.", rect.size.x - 32.0, color(p.wave_label, 1.0));
            return DrawStep::done();
        }
        if self.fitted != Some(m.total) || self.px_per_beat <= 0.0 {
            self.fit();
        }
        self.clamp_scroll();
        // A selection the source no longer has goes.
        match &self.selected {
            Selected::Section(i, _) if *i >= m.sections.len() => self.selected = Selected::None,
            Selected::Clip(i, name) if *i >= m.sections.len() || !m.sections[*i].loops.contains(name) => self.selected = Selected::None,
            _ => {}
        }
        let bar = self.bar();
        let (x0, x1) = (self.x0(), self.x1());
        let ppbar = self.px_per_beat * bar;
        let top = rect.pos.y;
        let lanes_top = self.lanes_top();
        let bottom = rect.pos.y + rect.size.y;
        let timeline = Rect { pos: dvec2(x0, top), size: dvec2((x1 - x0).max(0.0), rect.size.y) };
        let lanes_area = Rect { pos: dvec2(x0, lanes_top), size: dvec2((x1 - x0).max(0.0), (bottom - lanes_top).max(0.0)) };
        let rows = self.lane_rows();
        let current = self.live.song_beat.filter(|_| self.live.playing).and_then(|b| m.section_at(b));
        let any_solo = m.lanes.iter().any(|l| l.solo);
        let drag = self.drag.clone().filter(|d| d.moved);

        // Layer 0: grounds, bands and grid.
        self.rect_in(cx, Rect { pos: dvec2(x0, top), size: dvec2(x1 - x0, RULER) }, timeline, color(p.wave_ruler_bg, 1.0));
        self.rect_in(cx, Rect { pos: dvec2(x0, top + RULER), size: dvec2(x1 - x0, STRIP + CHORDS) }, timeline, color(p.panel, 0.55));
        for (i, s) in m.sections.iter().enumerate() {
            let (a, b) = (self.x_of(s.start), self.x_of(s.start + s.len));
            if i % 2 == 1 {
                self.rect_in(cx, Rect { pos: dvec2(a, lanes_top), size: dvec2(b - a, bottom - lanes_top) }, lanes_area, color(p.row_alt, 1.0));
            }
            if current == Some(i) {
                self.rect_in(cx, Rect { pos: dvec2(a, lanes_top), size: dvec2(b - a, bottom - lanes_top) }, lanes_area, color(p.accent, 0.035));
            }
        }
        for (i, (y, h)) in rows.iter().enumerate() {
            if *y > bottom || y + h < lanes_top {
                continue;
            }
            if i % 2 == 0 {
                self.rect_in(cx, Rect { pos: dvec2(x0, *y), size: dvec2(x1 - x0, *h) }, lanes_area, color(p.ghost_face, 0.5));
            }
            self.rect_in(cx, Rect { pos: dvec2(x0, y + h - 1.0), size: dvec2(x1 - x0, 1.0) }, lanes_area, color(p.line_faint, 1.0));
        }
        let bars_total = (m.total / bar).ceil() as i64;
        let first_bar = (self.scroll / bar).floor().max(0.0) as i64;
        let last_bar = ((self.beat_of(x1)) / bar).ceil() as i64;
        let every = label_every(ppbar);
        let beats = bar.round().max(1.0) as i64;
        for b in first_bar..=last_bar.min(bars_total) {
            let x = self.x_of(b as f64 * bar);
            if x < x0 - 1.0 || x > x1 {
                continue;
            }
            let labelled = b as usize % every == 0;
            self.rect_in(cx, Rect { pos: dvec2(x, top + if labelled { 9.0 } else { RULER - 7.0 }), size: dvec2(1.0, if labelled { RULER - 9.0 } else { 6.0 }) }, timeline, color(p.wave_label, if labelled { 0.8 } else { 0.4 }));
            self.rect_in(cx, Rect { pos: dvec2(x, lanes_top), size: dvec2(1.0, bottom - lanes_top) }, lanes_area, color(p.wave_grid, if labelled { 0.9 } else { 0.5 }));
            if self.px_per_beat >= 10.0 && b < bars_total {
                for k in 1..beats {
                    let bx = self.x_of(b as f64 * bar + k as f64);
                    self.rect_in(cx, Rect { pos: dvec2(bx, top + RULER - 4.0), size: dvec2(1.0, 3.0) }, timeline, color(p.wave_muted, 0.5));
                    self.rect_in(cx, Rect { pos: dvec2(bx, lanes_top), size: dvec2(1.0, bottom - lanes_top) }, lanes_area, color(p.wave_grid, 0.25));
                }
            }
        }
        // Section boundaries through everything below the ruler.
        for s in &m.sections {
            let x = self.x_of(s.start + s.len);
            self.rect_in(cx, Rect { pos: dvec2(x - 0.5, top + RULER), size: dvec2(1.0, rect.size.y - RULER) }, timeline, color(p.line, 1.0));
        }
        // The header column.
        self.rect_in(cx, Rect { pos: rect.pos, size: dvec2(HEAD, rect.size.y) }, rect, color(p.panel, 1.0));
        self.rect_in(cx, Rect { pos: dvec2(rect.pos.x + HEAD - 1.0, top), size: dvec2(1.0, rect.size.y) }, rect, color(p.line, 1.0));
        self.rect_in(cx, Rect { pos: dvec2(rect.pos.x, lanes_top - 1.0), size: dvec2(rect.size.x, 1.0) }, rect, color(p.line, 1.0));
        let head_area = Rect { pos: dvec2(rect.pos.x, lanes_top), size: dvec2(HEAD, (bottom - lanes_top).max(0.0)) };
        for (i, (y, h)) in rows.iter().enumerate() {
            let lane = &m.lanes[i];
            self.rect_in(cx, Rect { pos: dvec2(rect.pos.x, *y + 1.0), size: dvec2(3.0, h - 2.0) }, head_area, color(role_color(lane.role, &p), if lane.mute { 0.35 } else { 1.0 }));
            self.rect_in(cx, Rect { pos: dvec2(rect.pos.x, y + h - 1.0), size: dvec2(HEAD, 1.0) }, head_area, color(p.line_faint, 1.0));
        }

        // Layer 1: section blocks, clips, toggles.
        let strip_y = top + RULER;
        for (i, s) in m.sections.iter().enumerate() {
            let (a, b) = (self.x_of(s.start), self.x_of(s.start + s.len));
            if b < x0 || a > x1 {
                continue;
            }
            let selected = matches!(self.selected, Selected::Section(k, _) if k == i);
            let (fill, edge) = if selected {
                (color(p.accent_wash, 1.0), color(p.accent, 1.0))
            } else if current == Some(i) {
                (color(p.accent_wash, 0.7), color(p.accent, 0.5))
            } else {
                (color(if i % 2 == 0 { p.raised } else { p.control }, 1.0), color(p.line, 1.0))
            };
            let (ca, cb) = (a.max(x0) + 1.0, b.min(x1) - 1.0);
            self.block(cx, Rect { pos: dvec2(ca, strip_y + 9.0), size: dvec2(cb - ca, STRIP - 11.0) }, 0.0, fill, edge, if selected { 1.5 } else { 1.0 }, 3.0);
        }
        for (li, (y, h)) in rows.iter().enumerate() {
            if *y > bottom || y + h < lanes_top {
                continue;
            }
            let lane = &m.lanes[li];
            let hue = role_color(lane.role, &p);
            let dim = if lane.mute || (any_solo && !lane.solo) { 0.4 } else { 1.0 };
            for (si, s) in m.sections.iter().enumerate() {
                let clip = self.clip_rect(si, *y, *h);
                if clip.pos.x > x1 || clip.pos.x + clip.size.x < x0 {
                    continue;
                }
                let on = s.loops.contains(&lane.name);
                let hovered = self.drag.is_none() && self.hover.is_some_and(|at| self.target(at) == Target::Cell(si, li, on));
                let clipped = clip_to(clip, lanes_area);
                if on {
                    let selected = self.selected == Selected::Clip(si, lane.name.clone());
                    let edge = if selected { color(p.text_hi, 1.0) } else { color(hue, 0.75 * dim) };
                    let fill = color(hue, if hovered { 0.24 } else { 0.16 } * dim);
                    self.block(cx, clipped, 1.0, fill, edge, if selected { 1.5 } else { 1.0 }, 3.0);
                } else if hovered {
                    self.block(cx, clipped, 3.0, color(hue, 0.06), color(hue, 0.6), 1.0, 3.0);
                }
            }
        }
        for (i, (y, _)) in rows.iter().enumerate() {
            if *y < lanes_top - 4.0 || *y > bottom - 20.0 {
                continue;
            }
            let lane = &m.lanes[i];
            for (flag, r) in self.flags(*y) {
                let on = match flag {
                    Flag::Mute => lane.mute,
                    Flag::Solo => lane.solo,
                };
                let lit = match flag {
                    Flag::Mute => p.amber,
                    Flag::Solo => p.live,
                };
                let (fill, edge) = if on { (color(lit, 0.9), color(lit, 1.0)) } else { (color(p.control, 1.0), color(p.line, 1.0)) };
                self.block(cx, r, 0.0, fill, edge, 1.0, 2.5);
            }
        }
        if let Selected::Section(..) = self.selected {
            let hover = self.hover;
            for (k, r) in self.buttons().into_iter().enumerate() {
                let over = hover.is_some_and(|h| r.contains(h));
                let danger = k == 2;
                let (fill, edge) = if over { (color(if danger { p.danger_wash } else { p.control_hover }, 1.0), color(if danger { p.danger } else { p.accent }, 1.0)) } else { (color(p.control, 1.0), color(p.line, 1.0)) };
                self.block(cx, r, 0.0, fill, edge, 1.0, 3.0);
            }
        }

        // Layer 2: notes and meters.
        self.draw_rect.new_draw_call(cx);
        for (li, (y, h)) in rows.iter().enumerate() {
            if *y > bottom || y + h < lanes_top {
                continue;
            }
            let lane = m.lanes[li].clone();
            let hue = role_color(lane.role, &p);
            let dim = if lane.mute || (any_solo && !lane.solo) { 0.4 } else { 1.0 };
            for (si, s) in m.sections.iter().enumerate() {
                if !s.loops.contains(&lane.name) {
                    continue;
                }
                let clip = self.clip_rect(si, *y, *h);
                if clip.pos.x > x1 || clip.pos.x + clip.size.x < x0 {
                    continue;
                }
                let inner = clip_to(Rect { pos: clip.pos + dvec2(1.0, 0.0), size: clip.size - dvec2(2.0, 0.0) }, clip_to(clip, lanes_area));
                // A drum lane's rows as faint guides.
                if lane.drums && lane.rows.len() > 1 {
                    let rh = (clip.size.y - 15.0) / lane.rows.len() as f64;
                    for r in 1..lane.rows.len() {
                        self.rect_in(cx, Rect { pos: dvec2(clip.pos.x, clip.pos.y + 13.0 + r as f64 * rh), size: dvec2(clip.size.x, 1.0) }, inner, color(hue, 0.12));
                    }
                }
                for n in m.notes.get(&s.name, s.occurrence, &lane.name) {
                    let r = self.note_rect(&lane, clip, s.start, n);
                    if r.pos.x > x1 || r.pos.x + r.size.x < x0 {
                        continue;
                    }
                    let alpha = (0.45 + 0.55 * n.vel.clamp(0.0, 1.0)) * dim;
                    self.rect_in(cx, r, inner, color(hue, alpha));
                }
            }
            // The level while playing.
            if self.live.playing {
                if let Some((peak, _)) = self.meters.as_ref().and_then(|m| m.level(lane.key)) {
                    let db = 20.0 * peak.max(1e-6).log10();
                    let level = ((db + 48.0) / 48.0).clamp(0.0, 1.0) as f64;
                    let mh = h - 10.0;
                    let mx = rect.pos.x + HEAD - 8.0;
                    self.rect_in(cx, Rect { pos: dvec2(mx, y + 5.0), size: dvec2(3.0, mh) }, head_area, color(p.well, 1.0));
                    let lit = if db > -3.0 { p.amber } else { p.live };
                    self.rect_in(cx, Rect { pos: dvec2(mx, y + 5.0 + mh * (1.0 - level)), size: dvec2(3.0, mh * level) }, head_area, color(lit, 0.9));
                }
            }
        }

        // Layer 3: labels.
        self.draw_text.new_draw_call(cx);
        let mut label_end = f64::MIN;
        for b in first_bar..=last_bar.min(bars_total) {
            let x = self.x_of(b as f64 * bar);
            if b as usize % every != 0 || x < x0 - 1.0 || x + 4.0 < label_end || b >= bars_total {
                continue;
            }
            let w = self.text(cx, false, dvec2(x + 4.0, top + 4.0), &format!("{}", b + 1), 40.0, color(p.wave_label, 0.9));
            label_end = x + 4.0 + w + 6.0;
        }
        for (i, s) in m.sections.iter().enumerate() {
            let (a, b) = (self.x_of(s.start).max(x0), self.x_of(s.start + s.len).min(x1));
            if b - a < 14.0 {
                continue;
            }
            let selected = matches!(self.selected, Selected::Section(k, _) if k == i) || current == Some(i);
            let w = self.text(cx, true, dvec2(a + 6.0, strip_y + 14.0), &s.name, b - a - 12.0, color(if selected { p.text_hi } else { p.glass_ink }, 1.0));
            let detail = if s.again && s.run.1 == 1 { format!("{} bars · again", s.bars) } else { format!("{} bars", s.bars) };
            self.text(cx, false, dvec2(a + 12.0 + w, strip_y + 14.5), &detail, b - a - 18.0 - w, color(p.wave_muted, 1.0));
        }
        // Repeat brackets: a run of the same section, ×N over it.
        for s in m.sections.iter().filter(|s| s.run.0 == 0 && s.run.1 > 1) {
            let end = m.sections[s.index + s.run.1 - 1].start + m.sections[s.index + s.run.1 - 1].len;
            let (a, b) = (self.x_of(s.start) + 2.0, self.x_of(end) - 2.0);
            self.rect_in(cx, Rect { pos: dvec2(a, strip_y + 3.0), size: dvec2(b - a, 1.0) }, timeline, color(p.accent_soft, 0.9));
            self.rect_in(cx, Rect { pos: dvec2(a, strip_y + 3.0), size: dvec2(1.0, 5.0) }, timeline, color(p.accent_soft, 0.9));
            self.rect_in(cx, Rect { pos: dvec2(b - 1.0, strip_y + 3.0), size: dvec2(1.0, 5.0) }, timeline, color(p.accent_soft, 0.9));
            let label = format!("×{}", s.run.1);
            let lw = advance(&self.draw_text, cx, &label);
            let lx = ((a + b) * 0.5 - lw * 0.5).clamp(x0 + 2.0, x1 - lw - 2.0);
            self.rect_in(cx, Rect { pos: dvec2(lx - 3.0, strip_y), size: dvec2(lw + 6.0, 8.0) }, timeline, color(p.panel, 1.0));
            self.text(cx, false, dvec2(lx, strip_y - 1.5), &label, lw + 2.0, color(p.accent_soft, 1.0));
        }
        for (first, len, name) in &m.chords {
            let (a, b) = (self.x_of(*first as f64 * bar), self.x_of((first + len) as f64 * bar));
            let a = a.max(x0);
            if b - a < 12.0 || a > x1 {
                continue;
            }
            self.text(cx, true, dvec2(a + 4.0, strip_y + STRIP + 5.0), name, b - a - 6.0, color(p.key, 1.0));
        }
        self.text(cx, false, dvec2(rect.pos.x + 10.0, top + 5.0), &format!("{} bars · {} lanes", (m.total / bar).round(), m.lanes.len()), HEAD - 20.0, color(p.wave_muted, 1.0));
        if let Selected::Section(..) = self.selected {
            for (k, r) in self.buttons().into_iter().enumerate() {
                self.text(cx, false, dvec2(r.pos.x + 5.0, r.pos.y + 4.0), BUTTONS[k], r.size.x - 6.0, color(if k == 2 { p.danger } else { p.label }, 1.0));
            }
        } else {
            self.text(cx, false, dvec2(rect.pos.x + 10.0, strip_y + 14.0), "SECTIONS", HEAD - 20.0, color(p.wave_muted, 1.0));
        }
        self.text(cx, false, dvec2(rect.pos.x + 10.0, strip_y + STRIP + 5.0), "CHORDS", HEAD - 20.0, color(p.wave_muted, 1.0));
        for (i, (y, h)) in rows.iter().enumerate() {
            if *y < lanes_top - 4.0 || *y > bottom - 14.0 {
                continue;
            }
            let lane = &m.lanes[i];
            let ink = if lane.mute { color(p.wave_muted, 1.0) } else { color(p.glass_ink, 1.0) };
            self.text(cx, true, dvec2(rect.pos.x + 10.0, y + 6.0), &lane.title, HEAD - 66.0, ink);
            if *h >= 34.0 {
                self.text(cx, false, dvec2(rect.pos.x + 10.0, y + 21.0), &lane.name, HEAD - 66.0, color(p.wave_muted, 1.0));
            }
            for (flag, r) in self.flags(*y) {
                let on = match flag {
                    Flag::Mute => lane.mute,
                    Flag::Solo => lane.solo,
                };
                let ink = if on { color(p.on_accent, 1.0) } else { color(p.wave_muted, 1.0) };
                self.text(cx, true, dvec2(r.pos.x + 4.5, r.pos.y + 2.0), if flag == Flag::Mute { "M" } else { "S" }, 12.0, ink);
            }
            // Clip labels, pinned to the left edge while scrolled.
            for (si, s) in m.sections.iter().enumerate() {
                if !s.loops.contains(&lane.name) {
                    continue;
                }
                let clip = self.clip_rect(si, *y, *h);
                let (a, b) = (clip.pos.x.max(x0), (clip.pos.x + clip.size.x).min(x1));
                if b - a < 24.0 || clip.pos.y < lanes_top {
                    continue;
                }
                let selected = self.selected == Selected::Clip(si, lane.name.clone());
                let room = if selected { b - a - 22.0 } else { b - a - 8.0 };
                self.text(cx, false, dvec2(a + 4.0, clip.pos.y + 1.5), &s.name, room, color(p.text_hi, 0.85));
            }
        }

        // Layer 4: what is dragged, the selection's extras, the playhead.
        self.draw_rect.new_draw_call(cx);
        self.draw_block.new_draw_call(cx);
        if let Selected::Section(i, at) = self.selected {
            if let Some(s) = m.sections.get(i) {
                if at >= 1.0 && at <= s.bars - 1.0 {
                    let x = self.x_of(s.start + at * bar);
                    self.rect_in(cx, Rect { pos: dvec2(x - 0.5, strip_y + 9.0), size: dvec2(1.5, STRIP - 11.0) }, timeline, color(p.accent, 1.0));
                    self.rect_in(cx, Rect { pos: dvec2(x - 0.5, lanes_top), size: dvec2(1.0, bottom - lanes_top) }, lanes_area, color(p.accent, 0.35));
                }
            }
        }
        if let Selected::Clip(si, name) = self.selected.clone() {
            if let Some(li) = m.lanes.iter().position(|l| l.name == name) {
                let (y, h) = rows[li];
                let close = Self::close_rect(self.clip_rect(si, y, h));
                if close.pos.x > x0 && close.pos.y > lanes_top {
                    self.block(cx, close, 0.0, color(p.danger_wash, 1.0), color(p.danger, 1.0), 1.0, 2.0);
                }
            }
        }
        if let Some(d) = &drag {
            match d.target {
                Target::Strip(i, bars, true) => {
                    let s = &m.sections[i];
                    let next = (bars + (d.at.x - d.from.x) / ppbar).round().max(1.0);
                    let x = self.x_of(s.start + next * bar);
                    self.rect_in(cx, Rect { pos: dvec2(x - 1.0, strip_y), size: dvec2(2.0, rect.size.y - RULER) }, timeline, color(p.accent, 1.0));
                    self.block(cx, Rect { pos: dvec2(self.x_of(s.start).max(x0), strip_y + 9.0), size: dvec2(x - self.x_of(s.start).max(x0), STRIP - 11.0) }, 3.0, color(p.accent, 0.12), color(p.accent, 1.0), 1.0, 3.0);
                }
                Target::Strip(i, _, false) => {
                    let s = &m.sections[i];
                    let w = s.len * self.px_per_beat;
                    let x = self.x_of(s.start) + d.at.x - d.from.x;
                    self.block(cx, Rect { pos: dvec2(x, strip_y + 9.0), size: dvec2(w, STRIP - 11.0) }, 0.0, color(p.accent_wash, 0.85), color(p.accent, 1.0), 1.5, 3.0);
                    // Where it lands.
                    let to = if d.alt { m.sections.iter().filter(|s| s.start + s.len * 0.5 < self.beat_of(d.at.x)).count() } else { self.drop_index(i, d.at.x) };
                    let others: Vec<&arrange_model::Section> = m.sections.iter().filter(|o| d.alt || o.index != i).collect();
                    let beat = if to == 0 { 0.0 } else { others.get(to - 1).map_or(m.total, |o| o.start + o.len) };
                    let lx = self.x_of(beat);
                    self.rect_in(cx, Rect { pos: dvec2(lx - 1.0, strip_y + 4.0), size: dvec2(2.0, STRIP - 4.0) }, timeline, color(p.accent, 1.0));
                }
                Target::Cell(si, li, true) => {
                    let (y, h) = rows[li];
                    if let Some(to) = m.section_at(self.beat_of(d.at.x)) {
                        let ghost = self.clip_rect(to, y, h);
                        let hue = role_color(m.lanes[li].role, &p);
                        self.block(cx, clip_to(ghost, lanes_area), 3.0, color(hue, 0.18), color(hue, 1.0), 1.5, 3.0);
                    }
                    let _ = si;
                }
                _ => {}
            }
        }
        if let Some(beat) = self.live.song_beat {
            let x = self.x_of(beat);
            if x >= x0 - 7.0 && x <= x1 + 7.0 {
                self.rect_in(cx, Rect { pos: dvec2(x - 0.5, top + 10.0), size: dvec2(1.0, rect.size.y - 10.0) }, Rect { pos: dvec2(x0 - 1.0, top), size: dvec2(x1 - x0 + 2.0, rect.size.y) }, color(p.accent, 1.0));
                self.block(cx, Rect { pos: dvec2(x - 6.5, top + 1.0), size: dvec2(13.0, 14.0) }, 2.0, color(p.accent, 1.0), color(p.accent, 1.0), 0.0, 0.0);
            }
        }
        // The hover bubble.
        let bubble = if self.drag.is_none() { self.hover.and_then(|at| self.hover_text(at).map(|t| (at, t))) } else { None };
        if let Some((at, text)) = &bubble {
            let w = advance(&self.draw_text, cx, text) + 12.0;
            let pos = dvec2((at.x + 12.0).min(x1 - w - 2.0).max(rect.pos.x + 2.0), (at.y + 16.0).min(bottom - 18.0));
            self.block(cx, Rect { pos, size: dvec2(w, 16.0) }, 0.0, color(p.raised, 0.96), color(p.edge, 1.0), 1.0, 3.0);
        }
        // Layer 5: the bubble's text, the playhead's position.
        self.draw_text.new_draw_call(cx);
        if let Some((at, text)) = bubble {
            let w = advance(&self.draw_text, cx, &text) + 12.0;
            let pos = dvec2((at.x + 12.0).min(x1 - w - 2.0).max(rect.pos.x + 2.0), (at.y + 16.0).min(bottom - 18.0));
            self.text(cx, false, pos + dvec2(6.0, 3.0), &text, w, color(p.label, 1.0));
        }
        if self.live.playing || self.drag.is_some() {
            self.tick = cx.new_next_frame();
        }
        DrawStep::done()
    }
}

/// `r` cut to `to`.
fn clip_to(r: Rect, to: Rect) -> Rect {
    let a = dvec2(r.pos.x.max(to.pos.x), r.pos.y.max(to.pos.y));
    let b = dvec2((r.pos.x + r.size.x).min(to.pos.x + to.size.x), (r.pos.y + r.size.y).min(to.pos.y + to.size.y));
    Rect { pos: a, size: dvec2((b.x - a.x).max(0.0), (b.y - a.y).max(0.0)) }
}
