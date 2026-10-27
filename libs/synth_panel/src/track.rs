//! `SynthTrack{}`: the selected voice's timeline and its melody variants,
//! drawn by the voice tree unfolded under the voice whose "variations" was
//! pressed (where the panel lists it, it takes no room). The arrangement shows where the voice plays; its
//! part (one cycle) is a roll, where a drag picks the bars the next
//! variants change (none: the whole part). The variant buttons make them:
//! the procedural ones at once and seeded (variations, more syncopated,
//! simpler, darker, answer phrase, re-harmonise, busier, humanise), the AI
//! ones from the score's agent. Each variant is a card showing what it
//! changed (its new notes bright, the original's it dropped as ghosts):
//! play it in place in the song (play the original to compare), keep it,
//! apply it (one undo step), or pick it bar by bar under the cards and
//! apply the merge.

use crate::host::{self as synth, Card, Inline, Track, VaryAction};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SynthTrackBase = #(SynthTrack::register_widget(vm))
    /** The selected voice's timeline and melody variants (audition, keep, apply, merge). */
    mod.widgets.SynthTrack = set_type_default() do mod.widgets.SynthTrackBase{
        width: Fill height: Fit
        asks: "4 variations|more syncopated|darker and simpler|an answer phrase|re-harmonise it"
        draw_bg +: {color: #x00000000}
        draw_cell +: {color: #x00000000}
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_bold +: {text_style: theme.font_bold{font_size: 8.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
    }
}

fn color(value: u32, alpha: f32) -> Vec4f {
    let mut c = Vec4f::from_u32(value);
    c.w *= alpha;
    c
}

const ROLL: f64 = 64.0;
const CHIP: f64 = 20.0;
const CARD: f64 = 58.0;

#[derive(Clone, Debug, PartialEq)]
enum Hot {
    Make(String, bool),
    Play(usize),
    Keep(usize),
    Apply(usize),
    Pick(usize, usize),
    Merge,
    Clear,
    Roll,
}

#[derive(Script, ScriptHook, Widget)]
pub struct SynthTrack {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_cell: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_bold: DrawText,
    #[live]
    draw_small: DrawText,
    /// The AI asks offered, `|` between them.
    #[live]
    asks: String,
    #[rust]
    hots: Vec<(Rect, Hot)>,
    #[rust]
    roll: Rect,
    /// The bars a drag on the roll picked (from, to), while dragging.
    #[rust]
    drag: Option<(f64, f64)>,
    /// Bar by bar, which card the merge takes (0 the original).
    #[rust]
    picks: Vec<usize>,
    #[rust]
    picks_for: String,
    #[rust]
    next_frame: NextFrame,
    /// Drawn by the tree this frame, and so in the last (its events).
    #[rust]
    drawn: bool,
    #[rust]
    inline: bool,
}

impl SynthTrack {
    /// The selected voice, as long as its part (its phrase) once read.
    fn voice(&self) -> Option<Track> {
        let selected = synth::selected()?;
        let mut track = synth::tracks().into_iter().find(|t| t.name == selected)?;
        let beats = synth::variants(&track.name).beats;
        if beats > 0.0 {
            track.beats = beats;
        }
        Some(track)
    }

    fn bar_at(&self, x: f64, track: &Track) -> f64 {
        let t = ((x - self.roll.pos.x) / self.roll.size.x.max(1.0)).clamp(0.0, 1.0) * track.beats;
        (t / track.beats_per_bar.max(1.0)).floor()
    }

    fn draw_notes(&mut self, cx: &mut Cx2d, r: Rect, notes: &[(f64, f32, f32, f32, bool)], beats: f64, ink: u32) {
        let (lo, hi) = notes.iter().filter(|n| !n.4).fold((f32::MAX, f32::MIN), |(a, b), n| (a.min(n.1), b.max(n.1)));
        let span = if hi > lo { (hi - lo) as f64 } else { 12.0 };
        for n in notes {
            let nx = r.pos.x + n.0 / beats.max(1.0) * r.size.x;
            let nw = (n.2 as f64 / beats.max(1.0) * r.size.x).max(2.0);
            let ny = if n.4 { r.pos.y + r.size.y * 0.5 - 1.5 } else { r.pos.y + r.size.y - 4.0 - ((n.1 - lo) as f64 / span) * (r.size.y - 8.0) };
            self.draw_cell.color = color(ink, 0.35 + 0.65 * n.3.clamp(0.0, 1.0));
            self.draw_cell.draw_abs(cx, Rect { pos: dvec2(nx, ny), size: dvec2(nw, 3.0) });
        }
    }

    /// A card's notes against the original's: the notes it changed bright,
    /// those it keeps dim, the original's it dropped as faint ghosts. The
    /// number it changed.
    fn draw_diff(&mut self, cx: &mut Cx2d, r: Rect, notes: &[(f64, f32, f32, f32, bool)], original: &[(f64, f32, f32, f32, bool)], beats: f64) -> usize {
        let p = makepad_look::palette::get(cx);
        let (lo, hi) = notes.iter().chain(original).filter(|n| !n.4).fold((f32::MAX, f32::MIN), |(a, b), n| (a.min(n.1), b.max(n.1)));
        let span = if hi > lo { (hi - lo) as f64 } else { 12.0 };
        let same = |a: &(f64, f32, f32, f32, bool), b: &(f64, f32, f32, f32, bool)| (a.0 - b.0).abs() < 1e-3 && a.1 == b.1 && a.4 == b.4;
        let place = |n: &(f64, f32, f32, f32, bool)| {
            let nx = r.pos.x + n.0 / beats.max(1.0) * r.size.x;
            let nw = (n.2 as f64 / beats.max(1.0) * r.size.x).max(2.0);
            let ny = if n.4 { r.pos.y + r.size.y * 0.5 - 1.5 } else { r.pos.y + r.size.y - 4.0 - ((n.1 - lo) as f64 / span) * (r.size.y - 8.0) };
            Rect { pos: dvec2(nx, ny), size: dvec2(nw, 3.0) }
        };
        for o in original.iter().filter(|o| !notes.iter().any(|n| same(n, o))) {
            self.draw_cell.color = color(p.muted, 0.35);
            self.draw_cell.draw_abs(cx, place(o));
        }
        let mut changed = 0;
        for n in notes {
            let kept = original.iter().any(|o| same(n, o));
            if !kept {
                changed += 1;
            }
            self.draw_cell.color = if kept { color(p.info, 0.3) } else { color(p.accent, 0.6 + 0.4 * n.3.clamp(0.0, 1.0)) };
            self.draw_cell.draw_abs(cx, place(n));
        }
        changed
    }

    fn button(&mut self, cx: &mut Cx2d, x: f64, y: f64, text: &str, on: bool, hot: Hot) -> f64 {
        let p = makepad_look::palette::get(cx);
        let w = makepad_widgets::badge::advance(&self.draw_small, cx, text) + 12.0;
        let r = Rect { pos: dvec2(x, y), size: dvec2(w, CHIP - 4.0) };
        self.draw_cell.color = color(if on { p.accent_wash } else { p.control }, 1.0);
        self.draw_cell.draw_abs(cx, r);
        self.draw_small.color = color(if on { p.accent } else { p.text_hi }, 1.0);
        self.draw_small.draw_abs(cx, dvec2(x + 6.0, y + 3.0), text);
        self.hots.push((r, hot));
        w
    }
}

impl Widget for SynthTrack {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        synth::register_inline(Inline::Track, self.uid);
        if self.next_frame.is_event(event).is_some() {
            self.redraw(cx);
        }
        if !self.inline {
            return;
        }
        let Some(track) = self.voice() else { return };
        let hot_at = |abs: DVec2, hots: &[(Rect, Hot)]| hots.iter().find(|(r, _)| r.contains(abs)).map(|(_, h)| h.clone());
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::FingerDown(fe) if fe.is_primary_hit() && self.roll.contains(fe.abs) => {
                let b = self.bar_at(fe.abs.x, &track);
                self.drag = Some((b, b));
                self.redraw(cx);
            }
            Hit::FingerMove(fe) => {
                if let Some((a, _)) = self.drag {
                    self.drag = Some((a, self.bar_at(fe.abs.x, &track)));
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(fe) => {
                if let Some((a, b)) = self.drag.take() {
                    let (a, b) = (a.min(b), a.max(b));
                    // A tap on the roll: the whole part again.
                    let region = (fe.was_tap() && a == b).then_some(None).unwrap_or(Some((a * track.beats_per_bar, (b + 1.0) * track.beats_per_bar)));
                    synth::vary(&track.name, VaryAction::Region(region));
                    self.redraw(cx);
                    return;
                }
                if !(fe.is_over && fe.was_tap()) {
                    return;
                }
                let playing = synth::variants(&track.name).playing;
                let action = match hot_at(fe.abs, &self.hots) {
                    Some(Hot::Make(kind, ai)) => Some(VaryAction::Make { kind, ai, n: 4 }),
                    Some(Hot::Play(i)) => Some(VaryAction::Audition(if playing == Some(i) { None } else { Some(i) })),
                    Some(Hot::Keep(i)) => Some(VaryAction::Keep(i)),
                    Some(Hot::Apply(i)) => Some(VaryAction::Apply(i)),
                    Some(Hot::Pick(bar, card)) => {
                        if self.picks.len() <= bar {
                            self.picks.resize(bar + 1, 0);
                        }
                        self.picks[bar] = if self.picks[bar] == card { 0 } else { card };
                        None
                    }
                    Some(Hot::Merge) => Some(VaryAction::Merge(self.picks.clone())),
                    Some(Hot::Clear) => Some(VaryAction::Clear),
                    _ => None,
                };
                if let Some(action) = action {
                    // Applied or cleared: the cards change, the bar picks go.
                    if matches!(action, VaryAction::Apply(_) | VaryAction::Merge(_) | VaryAction::Clear) {
                        self.picks.clear();
                    }
                    synth::select_variations(&track.name);
                    synth::vary(&track.name, action);
                }
                self.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !synth::drawing_inline() {
            // The panel's own pass: the tree drew it (or not) already.
            synth::register_inline(Inline::Track, self.uid);
            self.inline = std::mem::take(&mut self.drawn);
            cx.walk_turtle(Walk { height: Size::Fixed(0.0), ..walk });
            return DrawStep::done();
        }
        let Some(track) = self.voice() else {
            cx.walk_turtle(Walk { height: Size::Fixed(0.0), ..walk });
            return DrawStep::done();
        };
        self.drawn = true;
        if self.picks_for != track.name {
            self.picks_for = track.name.clone();
            self.picks.clear();
        }
        let vars = synth::variants(&track.name);
        let p = makepad_look::palette::get(cx);
        let bars = (track.beats / track.beats_per_bar.max(1.0)).ceil().max(1.0) as usize;
        let asks: Vec<String> = self.asks.split('|').map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
        // Rows of chips: the procedural kinds, then the AI asks.
        let chip_rows = 2 + asks.len().div_ceil(3);
        let height = 22.0 + 14.0 + ROLL + 8.0 + chip_rows as f64 * CHIP + 18.0 + vars.cards.len().saturating_sub(1) as f64 * (CARD + 4.0) + 30.0;
        let rect = cx.walk_turtle(Walk { height: Size::Fixed(height), ..walk });
        self.draw_bg.color = color(p.well, 1.0);
        self.draw_bg.draw_abs(cx, rect);
        self.hots.clear();
        let (x0, x1) = (rect.pos.x + 8.0, rect.pos.x + rect.size.x - 8.0);
        let mut y = rect.pos.y + 5.0;
        self.draw_small.color = color(p.secondary, 1.0);
        self.draw_small.draw_abs(cx, dvec2(x0, y), &format!("TIMELINE  {}  ·  its part: {} bars", track.name, bars));
        y += 17.0;
        // The arrangement: where it plays.
        let song: f64 = track.sections.iter().map(|s| s.1).sum::<f64>().max(1.0);
        let mut sx = x0;
        for (_, beats, plays) in &track.sections {
            let w = beats / song * (x1 - x0);
            self.draw_cell.color = color(if *plays { p.accent } else { p.control }, if *plays { 0.7 } else { 1.0 });
            self.draw_cell.draw_abs(cx, Rect { pos: dvec2(sx, y), size: dvec2((w - 1.0).max(1.0), 8.0) });
            sx += w;
        }
        y += 14.0;
        // The part: one cycle, bars marked, the region shaded.
        self.roll = Rect { pos: dvec2(x0, y), size: dvec2(x1 - x0, ROLL) };
        self.draw_cell.color = color(p.panel, 1.0);
        self.draw_cell.draw_abs(cx, self.roll);
        let region = match self.drag {
            Some((a, b)) => Some((a.min(b) * track.beats_per_bar, (a.max(b) + 1.0) * track.beats_per_bar)),
            None => vars.region,
        };
        if let Some((a, b)) = region {
            let ra = self.roll.pos.x + a / track.beats * self.roll.size.x;
            let rb = self.roll.pos.x + b.min(track.beats) / track.beats * self.roll.size.x;
            self.draw_cell.color = color(p.accent_wash, 1.0);
            self.draw_cell.draw_abs(cx, Rect { pos: dvec2(ra, y), size: dvec2(rb - ra, ROLL) });
        }
        for b in 1..bars {
            let bx = self.roll.pos.x + b as f64 / bars as f64 * self.roll.size.x;
            self.draw_cell.color = color(p.line, 1.0);
            self.draw_cell.draw_abs(cx, Rect { pos: dvec2(bx, y), size: dvec2(1.0, ROLL) });
        }
        let original = vars.cards.first().map_or(track.notes.clone(), |c| c.notes.clone());
        let roll = self.roll;
        self.draw_notes(cx, roll, &original, track.beats, p.accent);
        self.hots.push((self.roll, Hot::Roll));
        y += ROLL + 6.0;
        // The variant buttons.
        let mut x = x0;
        for (i, (kind, label)) in makepad_score_live::vary::KINDS.iter().enumerate() {
            if i == 4 {
                x = x0;
                y += CHIP;
            }
            x += self.button(cx, x, y, label, false, Hot::Make(kind.to_string(), false)) + 4.0;
        }
        y += CHIP;
        x = x0;
        for (i, ask) in asks.iter().enumerate() {
            if i > 0 && i % 3 == 0 {
                x = x0;
                y += CHIP;
            }
            x += self.button(cx, x, y, &format!("AI: {ask}"), false, Hot::Make(ask.clone(), true)) + 4.0;
        }
        y += CHIP;
        let note = match (&vars.busy, region) {
            (Some(busy), _) => busy.clone(),
            (None, Some((a, b))) => format!("variants change bars {}-{} (tap the roll for the whole part)", (a / track.beats_per_bar) as usize + 1, (b / track.beats_per_bar) as usize),
            (None, None) => "variants change the whole part (drag on the roll for some bars)".into(),
        };
        self.draw_small.color = color(p.muted, 1.0);
        self.draw_small.draw_abs(cx, dvec2(x0, y + 2.0), &note);
        // The original's play (A/B), and clearing.
        let w = self.button(cx, x1 - 150.0, y, if vars.playing == Some(0) { "■ original" } else { "▶ original" }, vars.playing == Some(0), Hot::Play(0));
        self.button(cx, x1 - 150.0 + w + 4.0, y, "clear", false, Hot::Clear);
        y += 18.0;
        // The cards.
        let cards: Vec<(usize, Card)> = vars.cards.iter().cloned().enumerate().skip(1).collect();
        for (i, card) in &cards {
            let r = Rect { pos: dvec2(x0, y), size: dvec2(x1 - x0, CARD) };
            self.draw_cell.color = color(if vars.playing == Some(*i) { p.accent_wash } else { p.panel }, 1.0);
            self.draw_cell.draw_abs(cx, r);
            let roll = Rect { pos: dvec2(x0 + 6.0, y + 18.0), size: dvec2(x1 - x0 - 150.0, CARD - 30.0) };
            let changed = self.draw_diff(cx, roll, &card.notes, &original, track.beats);
            self.draw_bold.color = color(p.text_hi, 1.0);
            self.draw_bold.draw_abs(cx, dvec2(x0 + 6.0, y + 4.0), &format!("{}{}  ·  {changed} of {} notes changed", if card.kept { "★ " } else { "" }, card.label, card.notes.len()));
            // Bar picks for the merge.
            for b in 0..bars {
                let cell = Rect { pos: dvec2(roll.pos.x + b as f64 / bars as f64 * roll.size.x + 1.0, y + CARD - 10.0), size: dvec2(roll.size.x / bars as f64 - 2.0, 7.0) };
                let picked = self.picks.get(b).copied() == Some(*i);
                self.draw_cell.color = color(if picked { p.accent } else { p.control }, 1.0);
                self.draw_cell.draw_abs(cx, cell);
                self.hots.push((cell, Hot::Pick(b, *i)));
            }
            let bx = x1 - 138.0;
            let w1 = self.button(cx, bx, y + 18.0, if vars.playing == Some(*i) { "■" } else { "▶" }, vars.playing == Some(*i), Hot::Play(*i));
            let w2 = self.button(cx, bx + w1 + 4.0, y + 18.0, if card.kept { "kept" } else { "keep" }, card.kept, Hot::Keep(*i));
            self.button(cx, bx + w1 + w2 + 8.0, y + 18.0, "apply", false, Hot::Apply(*i));
            y += CARD + 4.0;
        }
        if self.picks.iter().any(|p| *p != 0) {
            let summary: Vec<String> = (0..bars).map(|b| match self.picks.get(b).copied().unwrap_or(0) {
                0 => "orig".to_string(),
                c => vars.cards.get(c).map_or("?".into(), |card| card.label.chars().take(12).collect()),
            }).collect();
            self.button(cx, x0, y + 4.0, &format!("apply merge: {}", summary.join(" | ")), true, Hot::Merge);
        }
        if vars.busy.is_some() || vars.playing.is_some() {
            self.next_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }
}
