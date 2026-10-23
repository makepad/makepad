//! `SynthGen{node: "lead" gen: "arp" choices: "root a2 chord m7 pattern updown"}`:
//! a code generator as a control. Its choices (the generator's parameters,
//! `makepad_score_live::generate::GENERATORS`) are drawn as rows: a choice
//! or a note steps with the arrows, a number drags sideways. Every change
//! is heard (the loop rewritten, rendered off the audio thread and played
//! on the preview route: the cue, or the main output when asked); Write
//! makes it the loop's code, one undo step. `choices` gives the starting
//! choices (pairs of name and value); the rest start at their defaults.

use crate::host as synth;
use makepad_score_live::generate::{self, ParamKind};
use makepad_score_live::theory;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SynthGenBase = #(SynthGen::register_widget(vm))
    /** A code generator for a loop: arpeggio, chords, euclid, transpose, scale, density, humanize, velocity, swing. */
    mod.widgets.SynthGen = set_type_default() do mod.widgets.SynthGenBase{
        width: Fill height: Fit gen: "arp"
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

const HEAD: f64 = 24.0;
const ROW: f64 = 22.0;
const FOOT: f64 = 28.0;
const LABEL: f64 = 84.0;

/// Progressions a degree list steps through.
const PROGRESSIONS: [&str; 8] = ["1 5 6 4", "1 6 4 5", "6 4 1 5", "2 5 1 1", "1 4 5 4", "1 6 3 7", "1 7 6 7", "1 3 4 4"];

/// What a press is on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hot {
    Prev(usize),
    Next(usize),
    Drag(usize),
    Write,
    Hear,
}

#[derive(Script, ScriptHook, Widget)]
pub struct SynthGen {
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
    /// The loop it writes.
    #[live]
    node: String,
    /// Which generator.
    #[live]
    gen: String,
    /// Starting choices: `name value name value …`.
    #[live]
    choices: String,
    #[live]
    label: String,
    /// The choices now, by parameter.
    #[rust]
    values: Vec<(String, String)>,
    #[rust]
    loaded: bool,
    #[rust]
    hots: Vec<(Rect, Hot)>,
    #[rust]
    drag: Option<(usize, f64, f64)>,
    /// A change not yet heard, since when.
    #[rust]
    changed: Option<f64>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    note: String,
}

impl SynthGen {
    fn spec(&self) -> Option<&'static generate::Generator> {
        generate::find(&self.gen)
    }

    fn load(&mut self) {
        self.loaded = true;
        let Some(spec) = self.spec() else { return };
        self.values = spec.params.iter().map(|p| (p.name.to_string(), p.default.to_string())).collect();
        let words: Vec<&str> = self.choices.split_whitespace().collect();
        let mut i = 0;
        while i + 1 < words.len() {
            let name = words[i];
            // A text value runs to the next parameter name.
            let mut j = i + 1;
            let mut value = vec![words[j].trim_start_matches('@').trim_matches(|c| c == '"' || c == '\'').to_string()];
            while j + 1 < words.len() && !spec.params.iter().any(|p| p.name == words[j + 1]) {
                j += 1;
                value.push(words[j].trim_matches(|c| c == '"' || c == '\'').to_string());
            }
            if let Some(slot) = self.values.iter_mut().find(|(n, _)| n == name) {
                slot.1 = value.join(" ");
            }
            i = j + 1;
        }
    }

    fn step(&mut self, row: usize, by: i32) {
        let Some(spec) = self.spec() else { return };
        let Some(param) = spec.params.get(row) else { return };
        let now = self.values[row].1.clone();
        let next = match &param.kind {
            ParamKind::Choice(options) => {
                let i = options.iter().position(|o| *o == now).unwrap_or(0) as i32;
                options[(i + by).rem_euclid(options.len() as i32) as usize].to_string()
            }
            ParamKind::Note => {
                let n = theory::note_number(&now).unwrap_or(57.0) + by as f32;
                theory::note_name(n.clamp(12.0, 108.0))
            }
            ParamKind::Number { min, max, step } => {
                let v = now.parse::<f64>().unwrap_or(*min) + by as f64 * step;
                makepad_score_live::source::fmt_num(v.clamp(*min, *max), 3)
            }
            ParamKind::Text => {
                let i = PROGRESSIONS.iter().position(|p| *p == now).map_or(0, |i| i as i32 + by);
                PROGRESSIONS[i.rem_euclid(PROGRESSIONS.len() as i32) as usize].to_string()
            }
        };
        self.values[row].1 = next;
    }

    fn changed(&mut self, cx: &mut Cx) {
        self.changed = Some(Cx::time_now());
        self.next_frame = cx.new_next_frame();
        self.redraw(cx);
    }
}

impl Widget for SynthGen {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            // Heard once the choices rest a moment.
            match self.changed {
                Some(at) if Cx::time_now() - at > 0.2 => {
                    self.changed = None;
                    synth::generate(&self.node, &self.gen, &self.values, false);
                }
                Some(_) => self.next_frame = cx.new_next_frame(),
                None => {}
            }
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                synth::select(&self.node);
                match self.hots.iter().find(|(r, _)| r.contains(fe.abs)).map(|(_, h)| *h) {
                    Some(Hot::Prev(row)) => {
                        self.step(row, -1);
                        self.changed(cx);
                    }
                    Some(Hot::Next(row)) => {
                        self.step(row, 1);
                        self.changed(cx);
                    }
                    Some(Hot::Drag(row)) => {
                        let v = self.values[row].1.parse::<f64>().unwrap_or(0.0);
                        self.drag = Some((row, fe.abs.x, v));
                    }
                    Some(Hot::Write) => {
                        synth::generate(&self.node, &self.gen, &self.values, true);
                        self.note = "written".into();
                        self.redraw(cx);
                    }
                    Some(Hot::Hear) => synth::generate(&self.node, &self.gen, &self.values, false),
                    None => {}
                }
            }
            Hit::FingerMove(fe) => {
                if let (Some((row, x0, v0)), Some(spec)) = (self.drag, self.spec()) {
                    if let Some(ParamKind::Number { min, max, step }) = spec.params.get(row).map(|p| &p.kind) {
                        // The whole range over 200 points.
                        let v = v0 + (fe.abs.x - x0) / 200.0 * (max - min);
                        let v = if *step > 0.0 { (v / step).round() * step } else { v };
                        let text = makepad_score_live::source::fmt_num(v.clamp(*min, *max), 3);
                        if text != self.values[row].1 {
                            self.values[row].1 = text;
                            self.changed(cx);
                        }
                    }
                }
            }
            Hit::FingerUp(_) => self.drag = None,
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.loaded {
            self.load();
        }
        let p = makepad_look::palette::get(cx);
        let Some(spec) = self.spec() else {
            let rect = cx.walk_turtle(Walk { height: Size::Fixed(ROW), ..walk });
            self.draw_small.color = color(p.danger, 1.0);
            self.draw_small.draw_abs(cx, rect.pos + dvec2(6.0, 5.0), &format!("no generator `{}`", self.gen));
            return DrawStep::done();
        };
        let height = HEAD + ROW * spec.params.len() as f64 + FOOT;
        let rect = cx.walk_turtle(Walk { height: Size::Fixed(height), ..walk });
        self.draw_bg.color = color(p.well, 1.0);
        self.draw_bg.draw_abs(cx, rect);
        self.hots.clear();
        let title = if self.label.is_empty() { spec.label.to_uppercase() } else { self.label.to_uppercase() };
        self.draw_bold.color = color(p.text_hi, 1.0);
        self.draw_bold.draw_abs(cx, rect.pos + dvec2(8.0, 6.0), &format!("{title}  "));
        self.draw_small.color = color(p.muted, 1.0);
        let w = makepad_widgets::badge::advance(&self.draw_bold, cx, &format!("{title}  "));
        self.draw_small.draw_abs(cx, rect.pos + dvec2(8.0 + w, 7.5), &self.node);
        for (row, param) in spec.params.iter().enumerate() {
            let y = rect.pos.y + HEAD + row as f64 * ROW;
            let value = self.values.get(row).map_or("", |(_, v)| v.as_str()).to_string();
            self.draw_text.color = color(p.label, 1.0);
            self.draw_text.draw_abs(cx, dvec2(rect.pos.x + 8.0, y + 4.0), param.name);
            let x = rect.pos.x + LABEL;
            let right = rect.pos.x + rect.size.x - 8.0;
            match &param.kind {
                ParamKind::Number { min, max, .. } => {
                    let bar = Rect { pos: dvec2(x, y + 6.0), size: dvec2(right - x - 48.0, ROW - 12.0) };
                    let v = value.parse::<f64>().unwrap_or(*min);
                    let t = if max > min { ((v - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
                    self.draw_cell.color = color(p.panel, 1.0);
                    self.draw_cell.draw_abs(cx, bar);
                    self.draw_cell.color = color(p.accent, if self.drag.is_some_and(|(r, ..)| r == row) { 1.0 } else { 0.75 });
                    self.draw_cell.draw_abs(cx, Rect { pos: bar.pos, size: dvec2(bar.size.x * t, bar.size.y) });
                    self.hots.push((Rect { pos: dvec2(x, y), size: dvec2(bar.size.x, ROW) }, Hot::Drag(row)));
                    self.draw_text.color = color(p.text, 1.0);
                    self.draw_text.draw_abs(cx, dvec2(right - 42.0, y + 4.0), &value);
                }
                _ => {
                    let prev = Rect { pos: dvec2(x, y + 2.0), size: dvec2(18.0, ROW - 4.0) };
                    let next = Rect { pos: dvec2(right - 18.0, y + 2.0), size: dvec2(18.0, ROW - 4.0) };
                    for (r, glyph) in [(prev, "‹"), (next, "›")] {
                        self.draw_cell.color = color(p.control, 1.0);
                        self.draw_cell.draw_abs(cx, r);
                        self.draw_text.color = color(p.text_hi, 1.0);
                        self.draw_text.draw_abs(cx, dvec2(r.pos.x + 6.0, y + 4.0), glyph);
                    }
                    self.hots.push((prev, Hot::Prev(row)));
                    self.hots.push((next, Hot::Next(row)));
                    self.draw_text.color = color(p.text, 1.0);
                    let tw = makepad_widgets::badge::advance(&self.draw_text, cx, &value);
                    self.draw_text.draw_abs(cx, dvec2((x + right - tw) * 0.5, y + 4.0), &value);
                }
            }
        }
        // Hear and Write.
        let y = rect.pos.y + rect.size.y - FOOT + 4.0;
        let write = Rect { pos: dvec2(rect.pos.x + rect.size.x - 80.0, y), size: dvec2(72.0, FOOT - 8.0) };
        let hear = Rect { pos: dvec2(write.pos.x - 76.0, y), size: dvec2(68.0, FOOT - 8.0) };
        for (r, text, hot, fill) in [(hear, "Hear", Hot::Hear, p.control), (write, "Write", Hot::Write, p.accent_wash)] {
            self.draw_cell.color = color(fill, 1.0);
            self.draw_cell.draw_abs(cx, r);
            self.draw_bold.color = color(p.text_hi, 1.0);
            let tw = makepad_widgets::badge::advance(&self.draw_bold, cx, text);
            self.draw_bold.draw_abs(cx, dvec2(r.pos.x + (r.size.x - tw) * 0.5, r.pos.y + 4.0), text);
            self.hots.push((r, hot));
        }
        if !self.note.is_empty() {
            self.draw_small.color = color(p.muted, 1.0);
            self.draw_small.draw_abs(cx, dvec2(rect.pos.x + 8.0, y + 5.0), &self.note);
        }
        DrawStep::done()
    }
}
