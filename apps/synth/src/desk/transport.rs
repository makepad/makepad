//! SCORE's header fit (what the score's one header row shows at a width)
//! and its playback bar in the caption: the open score's arrangement once
//! through as a slim timeline (sections, bar ticks, the playhead and the
//! loop), and the position as bar.beat and time. Drag seeks to a bar,
//! a click on a section loops it, shift-click loops a range of sections
//! (design: local/agent_state/edits/design/SYNTH-HEADER.md §3).

use makepad_score_live::render::{self, Timing};
use makepad_score_live::ScoreDoc;
use makepad_widgets::badge::advance;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    set_type_default() do #(DrawScrub::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let w = self.rect_size.x
            let h = self.rect_size.y
            if self.shape > 0.5 {
                // The playhead's cap: a small triangle pointing down.
                sdf.move_to(0., 0.) sdf.line_to(w, 0.) sdf.line_to(w * 0.5, h)
                sdf.close_path() sdf.fill(self.fill)
                return sdf.result
            }
            sdf.box(0.5, 0.5, w - 1., h - 1., self.radius)
            sdf.fill_keep(self.fill)
            sdf.stroke(self.edge, 1.)
            return sdf.result
        }
    }

    mod.widgets.ScoreScrubberBase = #(ScoreScrubber::register_widget(vm))
    mod.widgets.ScoreScrubber = set_type_default() do mod.widgets.ScoreScrubberBase{
        width: 240 height: 22
        draw_name +: {color: stage.muted text_style: theme.font_regular{font_size: 7.5}}
        draw_chip +: {color: stage.text_hi text_style: theme.font_bold{font_size: 7.5}}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawScrub {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub fill: Vec4f,
    #[live]
    pub edge: Vec4f,
    #[live]
    pub radius: f32,
    /// 0 a box, 1 the playhead's cap.
    #[live]
    pub shape: f32,
}

/// One section of the arrangement, in beats from its start.
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    pub name: String,
    pub start: f64,
    pub len: f64,
}

/// The arrangement once through: its sections and its tempo map.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arrangement {
    pub beats_per_bar: f64,
    pub bands: Vec<Band>,
    timing: Option<Timing>,
    /// A loop's cycle, not an arrangement: it repeats, and a click seeks
    /// (there are no sections to loop).
    pub cycle: bool,
}

impl Arrangement {
    /// A score of loops (no arrangement): its longest loop's cycle of
    /// `beats`, one segment per bar, repeating.
    pub fn cycle(doc: &ScoreDoc, beats: f64) -> Arrangement {
        let beats_per_bar = doc.beats_per_bar().max(1.0);
        let bars = (beats / beats_per_bar).ceil().max(1.0) as usize;
        let bands = (0..bars).map(|i| Band { name: format!("{}", i + 1), start: i as f64 * beats_per_bar, len: beats_per_bar }).collect();
        let timing = Timing { bpm: render::transport_bpm(doc), beats_per_bar, seconds: 0.0, sections: Vec::new(), tempo: Vec::new() };
        Arrangement { beats_per_bar, bands, timing: Some(timing), cycle: true }
    }

    pub fn of(doc: &ScoreDoc) -> Arrangement {
        let beats_per_bar = doc.beats_per_bar().max(1.0);
        let mut start = 0.0;
        let bands = doc.arrange.iter().filter_map(|name| doc.sections.iter().find(|s| &s.name == name)).map(|s| {
            let band = Band { name: s.name.clone(), start, len: s.bars * beats_per_bar };
            start += band.len;
            band
        }).collect();
        let timing = Timing { bpm: render::transport_bpm(doc), beats_per_bar, seconds: 0.0, sections: Vec::new(), tempo: render::arrangement_tempo(doc) };
        Arrangement { beats_per_bar, bands, timing: Some(timing), cycle: false }
    }

    /// Beats once through.
    pub fn beats(&self) -> f64 {
        self.bands.last().map_or(0.0, |b| b.start + b.len)
    }

    /// The band `beat` falls in (the last one past the end).
    pub fn band_at(&self, beat: f64) -> Option<usize> {
        if self.bands.is_empty() {
            return None;
        }
        Some(self.bands.iter().position(|b| beat < b.start + b.len).unwrap_or(self.bands.len() - 1))
    }

    /// Where a transport beat is in the arrangement when nothing jumped:
    /// once through, then the last section repeating (a live score never
    /// falls silent).
    pub fn fold(&self, beat: f64) -> f64 {
        let total = self.beats();
        if self.cycle && total > 0.0 {
            return beat.max(0.0) % total;
        }
        match self.bands.last() {
            Some(last) if beat >= total && last.len > 0.0 => last.start + (beat - total) % last.len,
            _ => beat.max(0.0),
        }
    }

    /// The whole bar at or before `beat`, in beats.
    pub fn snap(&self, beat: f64) -> f64 {
        let bar = self.beats_per_bar.max(1.0);
        ((beat / bar).floor() * bar).clamp(0.0, (self.beats() - bar).max(0.0))
    }

    /// Seconds from the start to `beat` on the score's tempo map (as the
    /// render places it).
    pub fn seconds_at(&self, beat: f64) -> f64 {
        self.timing.as_ref().map_or(0.0, |t| t.seconds_at(beat))
    }

}

/// `beat` (from the start) as bar.beat, both from 1.
pub fn bar_beat(beat: f64, beats_per_bar: f64) -> String {
    let bpb = beats_per_bar.max(1.0);
    let beat = beat.max(0.0);
    let bar = (beat / bpb).floor();
    format!("{}.{}", bar as u64 + 1, ((beat - bar * bpb).floor() as u64 + 1).min(bpb as u64))
}

/// Seconds as m:ss.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).floor() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// What the header shows at a width. Kept longest: the scrubber (260),
/// then the title (160), then the position text, then the views' labels;
/// the fold buttons (into ⋯) and the state's text (its dot stays) go
/// first. `level`: 1 folds in ⋯, 2 the state's dot only, 3 only the active
/// view labelled, 4 views as icons, 5 no position text (nor the note
/// glyph), 6 the title cut
/// (never under 80 px), 7 the scrubber under 260 (it is Fill: it gets the
/// rest).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeaderFit {
    pub level: u8,
    pub title_chars: usize,
}

/// Plex at the header's sizes, near enough to plan a row: the title (bold
/// 10.5) and the state (regular 8), per character.
const TITLE_PX: f64 = 6.8;
const META_PX: f64 = 4.9;
pub const SCRUB_MIN: f64 = 260.0;
const TITLE_KEEP: f64 = 160.0;
const TITLE_MIN: f64 = 80.0;

/// The header's row at `width` (see `HeaderFit`); the widths are the
/// header's own (spacing 6, padding 12/8, 24 px icon wells).
pub fn header_fit(width: f64, title: &str, state: &str) -> HeaderFit {
    // (+20: what the estimates miss, so the scrubber really gets 260.)
    const FIXED: f64 = 20.0 + 20.0 + 8.0 + (16.0 + 24.0 + 2.0 + 24.0 + 2.0) + (5.0 + 24.0 + 24.0 + 5.0 + 24.0);
    const VIEWS: [f64; 3] = [150.0, 100.0, 60.0];
    const FOLDS: f64 = 30.0 + 6.0;
    const POSITION: f64 = 114.0;
    let title_px = title.chars().count() as f64 * TITLE_PX;
    let state_px = state.chars().count() as f64 * META_PX + 6.0;
    // Everything but the title at a level, the scrubber at its minimum.
    let rest = |level: u8| {
        FIXED + 9.0 * 6.0 + SCRUB_MIN
            + VIEWS[match level { 0..=2 => 0, 3 => 1, _ => 2 }]
            + if level < 1 { FOLDS } else { 0.0 }
            + if level < 2 { state_px } else { 0.0 }
            + if level < 5 { POSITION } else { 0.0 }
            // The note glyph goes with the position.
            + if level < 5 { 13.0 + 6.0 } else { 0.0 }
    };
    let chars = |px: f64| ((px / TITLE_PX).floor().max(1.0) as usize).min(title.chars().count());
    for level in 0..=5 {
        if rest(level) + title_px.min(TITLE_KEEP) <= width {
            return HeaderFit { level, title_chars: chars(width - rest(level)) };
        }
    }
    let room = width - rest(5);
    if room >= TITLE_MIN {
        HeaderFit { level: 6, title_chars: chars(room) }
    } else {
        HeaderFit { level: 7, title_chars: chars(TITLE_MIN) }
    }
}

#[derive(Clone, Debug, Default)]
pub enum ScrubAction {
    #[default]
    None,
    /// Play from this beat of the arrangement (a whole bar).
    Seek(f64),
}

#[derive(Script, ScriptHook, Widget)]
pub struct ScoreScrubber {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    /// The well: drawn once, so its area is the widget's (hits, and the
    /// caption's client region).
    #[redraw]
    #[live]
    draw_bg: DrawScrub,
    #[live]
    draw_box: DrawScrub,
    #[live]
    draw_name: DrawText,
    #[live]
    draw_chip: DrawText,
    #[rust]
    arrangement: Arrangement,
    /// Where the output is in the arrangement.
    #[rust]
    at: Option<f64>,
    #[rust]
    looping: Option<(f64, f64)>,
    #[rust]
    hover: Option<usize>,
    /// The bar a press or drag is on.
    #[rust]
    ghost: Option<f64>,
    #[rust]
    rect: Rect,
    #[rust]
    area: Area,
}

impl ScoreScrubber {
    pub fn set_model(&mut self, cx: &mut Cx, arrangement: &Arrangement, at: Option<f64>, looping: Option<(f64, f64)>) {
        if *arrangement != self.arrangement {
            self.arrangement = arrangement.clone();
            self.redraw(cx);
        }
        let moved = match (at, self.at) {
            (Some(a), Some(b)) => (a - b).abs() * self.rect.size.x / self.arrangement.beats().max(1.0) > 0.25,
            (a, b) => a.is_some() != b.is_some(),
        };
        if moved || looping != self.looping {
            self.at = at;
            self.looping = looping;
            self.redraw(cx);
        }
    }

    fn inner(&self) -> (f64, f64) {
        (self.rect.pos.x + 2.0, (self.rect.size.x - 4.0).max(1.0))
    }

    fn beat_at(&self, x: f64) -> f64 {
        let (x0, w) = self.inner();
        ((x - x0) / w).clamp(0.0, 1.0) * self.arrangement.beats()
    }
}

impl Widget for ScoreScrubber {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.arrangement.bands.is_empty() {
            return;
        }
        // Where it is now (a Fill beside it can move it after it was walked).
        self.rect = self.area.rect(cx);
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                cx.set_cursor(MouseCursor::Hand);
                let hover = self.arrangement.band_at(self.beat_at(fe.abs.x));
                if hover != self.hover {
                    self.hover = hover;
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hover = None;
                self.redraw(cx);
            }
            // A time scrubber only: a press moves the
            // playhead (to its bar) at once and a drag scrubs. Sections are
            // edited in the Arrange view.
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                let bar = self.arrangement.snap(self.beat_at(fe.abs.x));
                self.ghost = Some(bar);
                self.redraw(cx);
                cx.widget_action(self.uid, ScrubAction::Seek(bar));
            }
            Hit::FingerMove(_) if self.ghost.is_none() => {}
            Hit::FingerMove(fe) => {
                let bar = self.arrangement.snap(self.beat_at(fe.abs.x));
                if self.ghost != Some(bar) {
                    self.ghost = Some(bar);
                    self.redraw(cx);
                    cx.widget_action(self.uid, ScrubAction::Seek(bar));
                }
            }
            Hit::FingerUp(_) => {
                if self.ghost.take().is_some() {
                    self.redraw(cx);
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let p = makepad_look::palette::get(cx);
        let rect = cx.walk_turtle(walk);
        self.rect = rect;
        self.draw_bg.shape = 0.0;
        self.draw_bg.radius = 2.5;
        self.draw_bg.fill = Vec4f::from_u32(p.well);
        self.draw_bg.edge = Vec4f::from_u32(p.line);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        self.draw_box.shape = 0.0;
        let a = self.arrangement.clone();
        let total = a.beats();
        if a.bands.is_empty() || total <= 0.0 {
            self.draw_name.color = Vec4f::from_u32(p.dim);
            self.draw_name.draw_abs(cx, dvec2(rect.pos.x + 8.0, rect.pos.y + 6.0), "No arrangement");
            return DrawStep::done();
        }
        let (x0, w) = self.inner();
        let px = w / total;
        let (top, h) = (rect.pos.y + 3.0, rect.size.y - 6.0);
        let current = self.at.and_then(|at| a.band_at(at));
        let bar_px = a.beats_per_bar * px;
        let no_edge = Vec4f::from_u32(0);
        for (i, band) in a.bands.iter().enumerate() {
            let x = x0 + band.start * px;
            let bw = (band.len * px - 1.0).max(1.0);
            let lit = current == Some(i) || self.hover == Some(i);
            self.draw_box.radius = 1.5;
            self.draw_box.fill = Vec4f::from_u32(if lit { p.control_hover } else { p.raised });
            self.draw_box.edge = no_edge;
            self.draw_box.draw_abs(cx, Rect { pos: dvec2(x, top), size: dvec2(bw, h) });
            // Bar ticks along the bottom, when they are far enough apart.
            if bar_px >= 4.0 {
                self.draw_box.radius = 0.0;
                self.draw_box.fill = Vec4f::from_u32(p.line_faint);
                let bars = (band.len / a.beats_per_bar).round() as usize;
                for k in 1..bars {
                    self.draw_box.draw_abs(cx, Rect { pos: dvec2(x + k as f64 * bar_px, top + h - 3.0), size: dvec2(1.0, 3.0) });
                }
            }
            let name_w = advance(&self.draw_name, cx, &band.name);
            if name_w + 10.0 < bw && self.ghost.is_none() {
                self.draw_name.color = Vec4f::from_u32(if current == Some(i) { p.text_hi } else if lit { p.label } else { p.muted });
                self.draw_name.draw_abs(cx, dvec2(x + 5.0, top + 2.5), &band.name);
            }
        }
        // The loop: a wash over its beats and a rule along the top.
        if let Some((from, to)) = self.looping {
            let (lx, lw) = (x0 + from * px, ((to - from) * px).max(2.0));
            let mut wash = Vec4f::from_u32(p.accent);
            wash.w = 0.14;
            self.draw_box.radius = 0.0;
            self.draw_box.fill = wash;
            self.draw_box.draw_abs(cx, Rect { pos: dvec2(lx, rect.pos.y + 1.0), size: dvec2(lw, rect.size.y - 2.0) });
            self.draw_box.fill = Vec4f::from_u32(p.accent);
            self.draw_box.draw_abs(cx, Rect { pos: dvec2(lx, rect.pos.y + 1.0), size: dvec2(lw, 2.0) });
        }
        // A drag's bar: a ghost line and its bar number.
        if let Some(bar) = self.ghost {
            let gx = x0 + bar * px;
            self.draw_box.radius = 0.0;
            self.draw_box.fill = Vec4f::from_u32(p.text_hi);
            self.draw_box.draw_abs(cx, Rect { pos: dvec2(gx - 1.0, rect.pos.y + 1.0), size: dvec2(2.0, rect.size.y - 2.0) });
            let text = format!("bar {}", (bar / a.beats_per_bar).round() as u64 + 1);
            let tw = advance(&self.draw_chip, cx, &text);
            let cx0 = if gx + tw + 12.0 < rect.pos.x + rect.size.x { gx + 3.0 } else { gx - tw - 11.0 };
            self.draw_box.radius = 2.0;
            self.draw_box.fill = Vec4f::from_u32(p.raised);
            self.draw_box.edge = Vec4f::from_u32(p.border_hi);
            self.draw_box.draw_abs(cx, Rect { pos: dvec2(cx0, rect.pos.y + 4.0), size: dvec2(tw + 8.0, rect.size.y - 8.0) });
            self.draw_chip.draw_abs(cx, dvec2(cx0 + 4.0, rect.pos.y + 6.5), &text);
        }
        if let Some(at) = self.at {
            let hx = x0 + at.clamp(0.0, total) * px;
            self.draw_box.radius = 0.0;
            self.draw_box.fill = Vec4f::from_u32(p.accent);
            self.draw_box.edge = no_edge;
            self.draw_box.draw_abs(cx, Rect { pos: dvec2(hx - 1.0, rect.pos.y + 1.0), size: dvec2(2.0, rect.size.y - 2.0) });
            self.draw_box.shape = 1.0;
            self.draw_box.draw_abs(cx, Rect { pos: dvec2(hx - 3.5, rect.pos.y + 1.0), size: dvec2(7.0, 4.0) });
            self.draw_box.shape = 0.0;
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIECE: &str = r#"use mod.score.*
let clock = Clock{bpm: 120 sig: [4 4]}
let kit = Drums{kit: @analog_hybrid}
let kick = Loop{play: kit hits: "x..."}
let intro = Section{bars: 4 loops: [kick]}
let drop = Section{bars: 8 loops: [kick] bpm: 60}
Score{label: "t" clock kit kick intro drop arrange: [intro drop intro]}
"#;

    fn piece() -> Arrangement {
        Arrangement::of(&ScoreDoc::load(PIECE).expect("the piece loads"))
    }

    #[test]
    fn bands_follow_the_arrangement() {
        let a = piece();
        let bands: Vec<(&str, f64, f64)> = a.bands.iter().map(|b| (b.name.as_str(), b.start, b.len)).collect();
        assert_eq!(bands, vec![("intro", 0.0, 16.0), ("drop", 16.0, 32.0), ("intro", 48.0, 16.0)]);
        assert_eq!(a.beats(), 64.0);
        assert_eq!(a.band_at(15.9), Some(0));
        assert_eq!(a.band_at(16.0), Some(1));
        assert_eq!(a.band_at(999.0), Some(2));
    }

    #[test]
    fn position_text_and_time() {
        let a = piece();
        assert_eq!(bar_beat(0.0, 4.0), "1.1");
        assert_eq!(bar_beat(18.5, 4.0), "5.3");
        // 16 beats at 120, then the drop at 60 bpm: 8 s + 2 beats = 10 s.
        assert!((a.seconds_at(16.0) - 8.0).abs() < 1e-6);
        assert!((a.seconds_at(18.0) - 10.0).abs() < 1e-6);
        assert_eq!(clock(70.9), "1:10");
    }

    #[test]
    fn seeking_snaps_to_whole_bars_inside_the_piece() {
        let a = piece();
        assert_eq!(a.snap(21.9), 20.0);
        assert_eq!(a.snap(-3.0), 0.0);
        assert_eq!(a.snap(500.0), 60.0);
        // Past the end the last section repeats.
        assert_eq!(a.fold(64.0 + 5.0), 48.0 + 5.0);
        assert_eq!(a.fold(10.0), 10.0);
    }

    #[test]
    fn the_header_keeps_the_scrubber_then_the_title_then_the_position() {
        let fit = |w: f64| header_fit(w, "Strobe Cathedral", "Edited");
        // Wide: everything, the title whole.
        assert_eq!(fit(1300.0), HeaderFit { level: 0, title_chars: 16 });
        // A narrow header: the scrubber (260) and the whole title keep;
        // the rest gave way.
        let pane = 760.0;
        let narrow = fit(pane);
        assert!(narrow.level <= 5 && narrow.title_chars == 16, "{narrow:?} at {pane}");
        // A long title keeps most of its 160 px even there (cut last).
        let long = header_fit(pane, "A very long score title that will not fit", "Edited");
        assert!(long.title_chars as f64 * 6.8 >= 130.0, "{long:?}");
        // Levels shed in order as the pane narrows.
        let mut last = 0;
        for w in (400..1300).rev().step_by(10) {
            let level = fit(w as f64).level;
            assert!(level >= last, "level went back at {w}");
            last = level;
        }
        assert_eq!(fit(300.0).level, 7);
    }

    #[test]
    fn a_score_of_loops_scrubs_its_longest_cycle() {
        let doc = ScoreDoc::load(PIECE).expect("the piece loads");
        let c = Arrangement::cycle(&doc, 14.0);
        assert!(c.cycle);
        assert_eq!(c.bands.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), vec!["1", "2", "3", "4"]);
        assert_eq!(c.beats(), 16.0);
        assert_eq!(c.fold(37.0), 5.0);
        assert!((c.seconds_at(8.0) - 4.0).abs() < 1e-6);
    }

}
