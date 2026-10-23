//! `ScoreTree{}`: the top of every synth panel, the score as a tree. Each
//! voice (a loop) is a row: its name, what it plays as a small roll, a
//! meter, mute and solo, level and pan (dragged: heard at once, written at
//! the release). Under it, its instrument: its name and what it is, with
//! the instrument switch.
//!
//! The tree unfolds in place, right under the voice: a click on a voice or
//! its instrument opens the instrument switch (a compact list: resting on
//! a row plays it, a click switches, one undo step, and the list closes)
//! and the instrument's tweaks (the panel's `SynthTweaks` for it, drawn
//! here); "variations" opens the voice's timeline and melody variants (the
//! panel's `SynthTrack`, drawn here). The same click again folds it.

use crate::choices::{ChoiceList, Clicked};
use crate::host::{self as synth, Inline, Track};
use makepad_value_pickers::panel::control::{fit_text, DrawPanelShape, R_CARD, R_CONTROL, R_GROUP};
use makepad_value_pickers::panel::host::{self, PanelValue};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ScoreTreeBase = #(ScoreTree::register_widget(vm))
    /** The score's voices and their instruments: mute, solo, level, pan, the instrument switch; unfolds a voice's controls or variants under it. */
    mod.widgets.ScoreTree = set_type_default() do mod.widgets.ScoreTreeBase{
        width: Fill height: Fit margin: Inset{left: 6 right: 6}
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

/// What an instrument is, in words, from its label as the score writes it
/// (`Instrument @soundfont @harp`): its program when that is not its name,
/// and where it comes from (`pizzicato · soundfont`, `soundfont`, `synth`).
fn instrument_words(name: &str, label: &str) -> String {
    let mut words = label.split_whitespace();
    let kind = words.next().unwrap_or("").to_lowercase();
    let fields: Vec<String> = words.map(|w| w.trim_start_matches('@').replace('_', " ")).collect();
    let same = |f: &str| f.eq_ignore_ascii_case(name) || f.replace(' ', "_").eq_ignore_ascii_case(name);
    match fields.as_slice() {
        [] => kind,
        [only] if same(only) => "built in".to_string(),
        [only] => only.clone(),
        [source, .., program] if same(program) => source.clone(),
        [source, .., program] => format!("{program} · {source}"),
    }
}

const HEAD: f64 = 26.0;
const VOICE: f64 = 26.0;
const INST: f64 = 22.0;
/// The instrument switch's list: its search and nine rows.
const LIST: f64 = crate::choices::ROW_H * 10.0;
/// The unfolded part's indent (under the instrument's name).
const INDENT: f64 = 20.0;

#[derive(Clone, Debug, PartialEq)]
enum Hot {
    Voice(String),
    Vary(String),
    /// A voice's instrument row: (voice, instrument).
    Instrument(String, String),
    Switch(String, String),
    Mute(String, bool),
    Solo(String, bool),
    /// A drag: the voice, the field, the bar's x range, its range.
    Level(String, &'static str, f64, f64, f64, f64),
}

#[derive(Script, ScriptHook, Widget)]
pub struct ScoreTree {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawPanelShape,
    #[live]
    draw_cell: DrawPanelShape,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_bold: DrawText,
    #[live]
    draw_small: DrawText,
    #[rust]
    hots: Vec<(Rect, Hot)>,
    /// The voice unfolded by a click here (its own or its instrument's).
    #[rust]
    unfold: Option<String>,
    /// The instrument switch open under a voice: (voice, instrument).
    #[rust]
    open: Option<(String, String)>,
    #[rust]
    list: ChoiceList,
    #[rust]
    drag: Option<Hot>,
    /// What was last brought into view (a voice unfolded, and how).
    #[rust]
    shown: Option<(String, bool)>,
    #[rust]
    next_frame: NextFrame,
}

impl ScoreTree {
    fn set_level(&self, hot: &Hot, x: f64) {
        if let Hot::Level(voice, field, x0, x1, lo, hi) = hot {
            let t = ((x - x0) / (x1 - x0).max(1.0)).clamp(0.0, 1.0);
            let v = lo + t * (hi - lo);
            host::set(&format!("{voice}.{field}"), PanelValue::Code(makepad_value_pickers::panel::host::fmt_number(v, 2)));
        }
    }

    /// The voice unfolded now and whether for its variations: the one
    /// clicked here while the selection is still its own, else the voice
    /// selected elsewhere (or the first that plays the instrument selected).
    fn unfolded(&self, tracks: &[Track]) -> Option<(String, bool)> {
        let selected = synth::selected()?;
        let variations = synth::selected_for_variations();
        if let Some(voice) = &self.unfold {
            if let Some(t) = tracks.iter().find(|t| t.name == *voice) {
                if selected == t.name || t.instrument.as_deref() == Some(selected.as_str()) {
                    return Some((voice.clone(), variations));
                }
            }
        }
        tracks.iter().find(|t| t.name == selected).or_else(|| tracks.iter().find(|t| t.instrument.as_deref() == Some(selected.as_str()))).map(|t| (t.name.clone(), variations))
    }

    fn open_list(&mut self, cx: &mut Cx, voice: &str, inst: &str) {
        self.list.load(inst);
        self.open = Some((voice.to_string(), inst.to_string()));
        cx.set_key_focus(self.draw_bg.area());
        synth::set_searching(true);
    }

    fn close_list(&mut self) {
        if self.open.take().is_some() {
            self.list.stop_preview();
        }
        synth::set_searching(false);
    }

    /// A voice (or its instrument, `node`) unfolded for its controls, or
    /// folded when that is what shows.
    fn toggle_controls(&mut self, cx: &mut Cx, voice: &str, node: &str, inst: Option<&str>) {
        let tracks = synth::tracks();
        let showing = self.unfolded(&tracks).is_some_and(|(v, vars)| v == voice && !vars) && synth::selected().as_deref() == Some(node);
        if showing {
            self.fold();
            return;
        }
        synth::select(node);
        self.unfold = Some(voice.to_string());
        match inst {
            Some(inst) => self.open_list(cx, voice, inst),
            None => self.close_list(),
        }
    }

    fn fold(&mut self) {
        self.close_list();
        self.unfold = None;
        synth::unselect();
    }

    fn handle_hit(&mut self, cx: &mut Cx, hit: Hit) {
        let hot_at = |abs: DVec2, hots: &[(Rect, Hot)]| hots.iter().find(|(r, _)| r.contains(abs)).map(|(_, h)| h.clone());
        match hit {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::FingerHoverOver(fe) if self.open.is_some() => {
                let (changed, wake) = self.list.hover_at(fe.abs);
                if wake {
                    self.next_frame = cx.new_next_frame();
                }
                if changed {
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.list.stop_preview();
                self.redraw(cx);
            }
            Hit::KeyFocus(_) => synth::set_searching(self.open.is_some()),
            Hit::KeyFocusLost(_) => synth::set_searching(false),
            Hit::TextInput(te) if self.open.is_some() => {
                self.list.type_text(&te.input);
                self.redraw(cx);
            }
            Hit::KeyDown(ke) if self.open.is_some() && ke.key_code == KeyCode::Backspace => {
                self.list.backspace();
                self.redraw(cx);
            }
            Hit::KeyDown(ke) if self.open.is_some() && ke.key_code == KeyCode::Escape => {
                self.close_list();
                self.redraw(cx);
            }
            Hit::FingerScroll(fe) if self.open.is_some() && self.list.contains(fe.abs) => {
                self.list.scroll_by(fe.scroll.y);
                self.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if let Some(hot @ Hot::Level(..)) = hot_at(fe.abs, &self.hots) {
                    self.set_level(&hot, fe.abs.x);
                    self.drag = Some(hot);
                    self.redraw(cx);
                }
            }
            Hit::FingerMove(fe) => {
                if let Some(hot) = self.drag.clone() {
                    self.set_level(&hot, fe.abs.x);
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(fe) => {
                if self.drag.take().is_some() {
                    return;
                }
                if !(fe.is_over && fe.was_tap()) {
                    return;
                }
                if self.open.is_some() && self.list.contains(fe.abs) {
                    // A choice made: the switch closes, the tweaks stay.
                    if self.list.click(fe.abs) == Clicked::Switched {
                        self.close_list();
                    }
                    self.redraw(cx);
                    return;
                }
                let tracks = synth::tracks();
                let inst_of = |v: &str| tracks.iter().find(|t| t.name == v).and_then(|t| t.instrument.clone());
                match hot_at(fe.abs, &self.hots) {
                    Some(Hot::Voice(v)) => {
                        let inst = inst_of(&v);
                        self.toggle_controls(cx, &v, &v, inst.as_deref());
                    }
                    Some(Hot::Instrument(v, i)) => self.toggle_controls(cx, &v, &i, Some(&i)),
                    Some(Hot::Vary(v)) => {
                        if self.unfolded(&tracks).is_some_and(|(u, vars)| u == v && vars) {
                            self.fold();
                        } else {
                            self.close_list();
                            synth::select_variations(&v);
                            self.unfold = Some(v);
                        }
                    }
                    Some(Hot::Switch(v, i)) => {
                        if self.open.as_ref().is_some_and(|(ov, oi)| *ov == v && *oi == i) {
                            self.close_list();
                        } else {
                            // Under its voice, with the instrument's tweaks.
                            if self.unfolded(&tracks).is_none_or(|(u, vars)| u != v || vars) {
                                synth::select(&i);
                                self.unfold = Some(v.clone());
                            }
                            self.open_list(cx, &v, &i);
                        }
                    }
                    Some(Hot::Mute(v, on)) => host::set(&format!("{v}.mute"), PanelValue::Code((!on).to_string())),
                    Some(Hot::Solo(v, on)) => host::set(&format!("{v}.solo"), PanelValue::Code((!on).to_string())),
                    _ => {}
                }
                self.redraw(cx);
            }
            _ => {}
        }
    }

    /// Draw the panel's widgets registered for the voice in its unfolded
    /// part: its variants, or the tweaks of its instrument (and its own).
    fn draw_inline(&mut self, cx: &mut Cx2d, scope: &mut Scope, voice: &Track, variations: bool) -> usize {
        let mut drawn = 0;
        for (what, uid) in synth::inline_widgets() {
            let wanted = match &what {
                Inline::Track => variations,
                Inline::Tweaks(node) => !variations && (*node == voice.name || voice.instrument.as_deref() == Some(node.as_str())),
            };
            if !wanted {
                continue;
            }
            let widget = cx.widget_tree().widget(uid);
            if widget.is_empty() {
                // Gone with a panel mounted before this one.
                synth::forget_inline(uid);
                continue;
            }
            synth::set_drawing_inline(true);
            widget.draw_all(cx, scope);
            synth::set_drawing_inline(false);
            drawn += 1;
        }
        drawn
    }
}

impl Widget for ScoreTree {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            if self.list.tick() {
                self.next_frame = cx.new_next_frame();
            }
            self.redraw(cx);
        }
        let hit = event.hits(cx, self.draw_bg.area());
        self.handle_hit(cx, hit);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let tracks: Vec<Track> = synth::tracks();
        let p = makepad_look::palette::get(cx);
        let selected = synth::selected();
        let unfolded = self.unfolded(&tracks);
        // The switch belongs to the voice unfolded.
        if self.open.as_ref().is_some_and(|(v, _)| unfolded.as_ref().is_none_or(|(u, vars)| u != v || *vars)) {
            self.close_list();
        }
        // The tree is one rounded card; each voice a compact row, its
        // instrument a quiet line under it.
        self.draw_bg.shape = 0.0;
        self.draw_bg.radius = R_CARD;
        self.draw_bg.stroke = 1.0;
        self.draw_bg.fill = color(p.row, 1.0);
        self.draw_bg.edge = color(p.separator, 1.0);
        self.draw_bg.begin(cx, walk, Layout { padding: Inset { bottom: 6.0, ..Inset::default() }, ..Layout::flow_down() });
        self.hots.clear();
        let row_walk = |h: f64| Walk { width: Size::fill(), height: Size::Fixed(h), ..Walk::default() };
        let head = cx.walk_turtle(row_walk(HEAD));
        self.draw_small.color = color(p.secondary, 1.0);
        self.draw_small.draw_abs(cx, head.pos + dvec2(10.0, 7.0), "VOICES");
        let vw = makepad_widgets::badge::advance(&self.draw_small, cx, "VOICES");
        self.draw_small.color = color(p.dim_hi, 1.0);
        self.draw_small.draw_abs(cx, head.pos + dvec2(18.0 + vw, 7.0), &tracks.len().to_string());
        let live = synth::live();
        let mut bring_into_view = None;
        for t in &tracks {
            // The voice row.
            let row = cx.walk_turtle(row_walk(VOICE));
            let (x0, x1, y) = (row.pos.x, row.pos.x + row.size.x, row.pos.y);
            let cy = y + VOICE * 0.5;
            let open_here = unfolded.as_ref().is_some_and(|(u, _)| *u == t.name);
            let is_selected = selected.as_deref() == Some(t.name.as_str());
            if is_selected || open_here {
                let wash = Rect { pos: dvec2(x0 + 4.0, y + 1.0), size: dvec2(row.size.x - 8.0, VOICE - 2.0) };
                self.draw_cell.fill_round(cx, wash, color(p.accent_wash, if is_selected { 1.0 } else { 0.55 }), R_CONTROL);
            }
            let dim = if t.mute { 0.45 } else { 1.0 };
            self.draw_bold.color = color(p.text_hi, dim);
            let (name, _) = fit_text(&self.draw_bold, cx, &t.name, 82.0);
            self.draw_bold.draw_abs(cx, dvec2(x0 + 10.0, y + 7.0), &name);
            // The right end, from the edge in: pan, level, solo, mute, and
            // "variations" before them.
            let pan_w = 34.0;
            let amp_w = 48.0;
            let pan_x = x1 - 10.0 - pan_w;
            let amp_x = pan_x - 10.0 - amp_w;
            let solo_x = amp_x - 10.0 - 16.0;
            let mute_x = solo_x - 4.0 - 16.0;
            let vary_w = makepad_widgets::badge::advance(&self.draw_small, cx, "variations") + 14.0;
            let vary_x = mute_x - 8.0 - vary_w;
            // What it plays, one cycle, as a small roll.
            let roll = Rect { pos: dvec2(x0 + 96.0, y + 5.0), size: dvec2((vary_x - 12.0 - x0 - 96.0).max(40.0), VOICE - 10.0) };
            self.draw_cell.fill_round(cx, roll, color(p.well, 1.0), 3.0);
            let (lo, hi) = t.notes.iter().filter(|n| !n.4).fold((f32::MAX, f32::MIN), |(a, b), n| (a.min(n.1), b.max(n.1)));
            let span = if hi > lo { (hi - lo) as f64 } else { 12.0 };
            for n in &t.notes {
                let nx = roll.pos.x + 2.0 + n.0 / t.beats.max(1.0) * (roll.size.x - 4.0);
                if nx > roll.pos.x + roll.size.x - 3.0 {
                    continue;
                }
                let nw = (n.2 as f64 / t.beats.max(1.0) * (roll.size.x - 4.0)).max(2.0).min(roll.pos.x + roll.size.x - 2.0 - nx);
                let ny = if n.4 { roll.pos.y + roll.size.y * 0.5 - 1.0 } else { roll.pos.y + roll.size.y - 3.0 - ((n.1 - lo) as f64 / span) * (roll.size.y - 6.0) };
                self.draw_cell.fill_round(cx, Rect { pos: dvec2(nx, ny), size: dvec2(nw, 2.0) }, color(p.accent, 0.4 + 0.6 * n.3.clamp(0.0, 1.0) * dim), 1.0);
            }
            self.hots.push((Rect { pos: row.pos, size: dvec2(roll.pos.x + roll.size.x - x0, VOICE) }, Hot::Voice(t.name.clone())));
            // Its meter, a slim bar beside the roll.
            let level = live.as_ref().and_then(|l| l.level(&t.name)).map_or(0.0, |(peak, _)| peak);
            let mh = crate::dsp::Ballistics::height(level) as f64 * (VOICE - 10.0);
            if mh > 0.5 {
                self.draw_cell.fill_round(cx, Rect { pos: dvec2(roll.pos.x + roll.size.x + 4.0, y + VOICE - 5.0 - mh), size: dvec2(3.0, mh) }, color(p.live, 0.85), 1.5);
            }
            // Its melody variants: a quiet chip, lit while open.
            let varying = unfolded.as_ref().is_some_and(|(u, vars)| *u == t.name && *vars);
            let r = Rect { pos: dvec2(vary_x, cy - 9.0), size: dvec2(vary_w, 18.0) };
            if varying {
                self.draw_cell.edged_round(cx, r, color(p.accent_wash, 1.0), color(p.accent, 1.0), 1.0, R_CONTROL);
            } else {
                self.draw_cell.edged_round(cx, r, color(0, 0.0), color(p.line_soft, 1.0), 1.0, R_CONTROL);
            }
            self.draw_small.color = color(if varying { p.text_hi } else { p.muted }, 1.0);
            self.draw_small.draw_abs(cx, dvec2(r.pos.x + 7.0, cy - 5.0), "variations");
            self.hots.push((r, Hot::Vary(t.name.clone())));
            // Mute and solo: quiet rings until they are on.
            for (label, on, hot, x, lit) in [("M", t.mute, Hot::Mute(t.name.clone(), t.mute), mute_x, p.amber), ("S", t.solo, Hot::Solo(t.name.clone(), t.solo), solo_x, p.accent)] {
                let r = Rect { pos: dvec2(x, cy - 8.0), size: dvec2(16.0, 16.0) };
                if on {
                    self.draw_cell.fill_round(cx, r, color(lit, 1.0), 8.0);
                } else {
                    self.draw_cell.edged_round(cx, r, color(0, 0.0), color(p.line_soft, 1.0), 1.0, 8.0);
                }
                self.draw_small.color = color(if on { p.on_accent } else { p.dim_hi }, 1.0);
                let lw = makepad_widgets::badge::advance(&self.draw_small, cx, label);
                self.draw_small.draw_abs(cx, dvec2(x + (16.0 - lw) * 0.5, cy - 5.0), label);
                self.hots.push((Rect { pos: r.pos - dvec2(2.0, 4.0), size: r.size + dvec2(4.0, 8.0) }, hot));
            }
            // Level and pan: slim tracks with a small round thumb.
            for (field, x, w, lo, hi, v) in [("amp", amp_x, amp_w, 0.0, 1.5, t.amp), ("pan", pan_x, pan_w, -1.0, 1.0, t.pan)] {
                let track = Rect { pos: dvec2(x, cy - 1.5), size: dvec2(w, 3.0) };
                self.draw_cell.fill_round(cx, track, color(p.track, 1.0), 1.5);
                let tpos = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
                if field == "amp" {
                    self.draw_cell.fill_round(cx, Rect { pos: track.pos, size: dvec2(w * tpos, 3.0) }, color(p.accent, 0.85 * dim as f32), 1.5);
                } else {
                    self.draw_cell.fill_round(cx, Rect { pos: dvec2(x + w * 0.5 - 0.5, cy - 4.0), size: dvec2(1.0, 8.0) }, color(p.muted, 0.6), 0.5);
                }
                self.draw_cell.edged_round(cx, Rect { pos: dvec2(x + w * tpos - 4.0, cy - 4.0), size: dvec2(8.0, 8.0) }, color(p.text_bright, 1.0), color(p.row, 1.0), 1.0, 4.0);
                self.hots.push((Rect { pos: dvec2(x, y + 2.0), size: dvec2(w, VOICE - 4.0) }, Hot::Level(t.name.clone(), field, x, x + w, lo, hi)));
            }
            if open_here && self.shown != unfolded {
                bring_into_view = Some(row);
            }
            // Its instrument: a thin elbow from the voice, the instrument's
            // name, what it is (dimmed), and one small switch.
            let inst = t.instrument.clone().unwrap_or_default();
            let row = cx.walk_turtle(row_walk(INST));
            let y = row.pos.y;
            let cy = y + INST * 0.5;
            if !inst.is_empty() && selected.as_deref() == Some(inst.as_str()) && open_here {
                let wash = Rect { pos: dvec2(x0 + 26.0, y + 1.0), size: dvec2(row.size.x - 30.0, INST - 2.0) };
                self.draw_cell.fill_round(cx, wash, color(p.accent_wash, 0.7), R_CONTROL);
            }
            self.draw_cell.fill_round(cx, Rect { pos: dvec2(x0 + 16.0, y - 3.0), size: dvec2(1.0, cy - y + 3.0) }, color(p.line_soft, 1.0), 0.5);
            self.draw_cell.fill_round(cx, Rect { pos: dvec2(x0 + 16.0, cy), size: dvec2(8.0, 1.0) }, color(p.line_soft, 1.0), 0.5);
            let picking = self.open.as_ref().is_some_and(|(v, i)| *v == t.name && *i == inst);
            let switch_w = makepad_widgets::badge::advance(&self.draw_small, cx, if picking { "close" } else { "switch" }) + 28.0;
            let room = (x1 - 10.0 - switch_w - 8.0 - (x0 + 30.0)).max(20.0);
            if inst.is_empty() {
                self.draw_small.color = color(p.muted, dim);
                self.draw_small.draw_abs(cx, dvec2(x0 + 30.0, cy - 5.0), "plays no instrument");
            } else {
                self.draw_text.color = color(p.text, dim);
                let (name, _) = fit_text(&self.draw_text, cx, &inst, room);
                self.draw_text.draw_abs(cx, dvec2(x0 + 30.0, cy - 6.0), &name);
                let nw = makepad_widgets::badge::advance(&self.draw_text, cx, &name);
                let what = instrument_words(&inst, &t.inst_label);
                if !what.is_empty() && room - nw > 30.0 {
                    let (what, _) = fit_text(&self.draw_small, cx, &what, room - nw - 10.0);
                    self.draw_small.color = color(p.dim_hi, dim);
                    self.draw_small.draw_abs(cx, dvec2(x0 + 30.0 + nw + 8.0, cy - 5.0), &what);
                }
            }
            if !inst.is_empty() {
                self.hots.push((Rect { pos: dvec2(x0 + 26.0, y), size: dvec2((x1 - switch_w - 18.0 - x0 - 26.0).max(10.0), INST) }, Hot::Instrument(t.name.clone(), inst.clone())));
                let label = if picking { "close" } else { "switch" };
                let r = Rect { pos: dvec2(x1 - 10.0 - switch_w, cy - 9.0), size: dvec2(switch_w, 18.0) };
                self.draw_cell.fill_round(cx, r, color(if picking { p.accent_wash } else { p.raised }, 1.0), R_CONTROL);
                self.draw_small.color = color(if picking { p.text_hi } else { p.label }, 1.0);
                self.draw_small.draw_abs(cx, dvec2(r.pos.x + 8.0, cy - 5.0), label);
                self.draw_cell.chevron(cx, Rect { pos: dvec2(r.pos.x + r.size.x - 17.0, cy - 6.0), size: dvec2(12.0, 12.0) }, color(if picking { p.text_hi } else { p.muted }, 1.0), picking);
                self.hots.push((r, Hot::Switch(t.name.clone(), inst.clone())));
            }
            if !open_here {
                continue;
            }
            // Unfolded: the switch's list, then the voice's controls or
            // variants, indented under the instrument, a rule beside them.
            cx.begin_turtle(Walk::fill_fit(), Layout { padding: Inset { left: INDENT, right: 6.0, top: 2.0, bottom: 8.0 }, spacing: 6.0, ..Layout::flow_down() });
            if picking {
                let list = cx.walk_turtle(row_walk(LIST));
                self.draw_cell.edged_round(cx, list, color(p.well, 1.0), color(p.line_faint, 1.0), 1.0, R_GROUP);
                self.list.draw(cx, list, &mut self.draw_cell, &mut self.draw_text, &mut self.draw_small);
            }
            let variations = unfolded.as_ref().is_some_and(|(_, vars)| *vars);
            if self.draw_inline(cx, scope, t, variations) == 0 && !picking {
                let line = cx.walk_turtle(row_walk(18.0));
                self.draw_small.color = color(p.muted, 1.0);
                let words = if inst.is_empty() { format!("{} plays no instrument", t.name) } else { format!("no controls written for {inst} yet: switch it here, or Remake the panel") };
                self.draw_small.draw_abs(cx, line.pos + dvec2(16.0, 4.0), &words);
            }
            let part = cx.end_turtle();
            self.draw_cell.fill_round(cx, Rect { pos: dvec2(part.pos.x + 12.0, part.pos.y + 2.0), size: dvec2(2.0, (part.size.y - 10.0).max(0.0)) }, color(p.accent, 0.5), 1.0);
        }
        self.draw_bg.end(cx);
        // Just unfolded: the voice comes into view when it is low in the
        // panel, its unfolded part under it.
        if let Some(row) = bring_into_view {
            makepad_value_pickers::panel::host::scroll_to(row);
        }
        if unfolded.is_none() || bring_into_view.is_some() {
            self.shown = unfolded;
        }
        if live.as_ref().is_some_and(|l| l.playing()) {
            self.next_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }
}
