//! How a block looks: its rail in the gutter, its card under its first
//! line (a title column, what it does, its knobs, its tools under the
//! pointer), the chips of folded lenses and of problems. One layout for
//! every kind, so the cards line up down the code.

use super::lens::{Lens, LensKind};
use super::{Hot, ScoreEditor};
use makepad_look::palette::Palette;
use makepad_score_live::source;
use makepad_widgets::badge::advance;
use makepad_widgets::*;

/// The title column, a knob's slot and its dial, the tools' column.
const TITLE_W: f64 = 124.0;
const KNOB_W: f64 = 46.0;
const DIAL: f64 = 22.0;
const TOOLS_W: f64 = 24.0;
const PAD: f64 = 10.0;

fn rgba(c: u32) -> Vec4f {
    Vec4f::from_u32(c)
}

fn alpha(c: u32, a: f32) -> Vec4f {
    let mut v = Vec4f::from_u32(c);
    v.w = a;
    v
}

/// `a` with `t` of `b` mixed in (opaque).
fn mix(a: u32, b: u32, t: f32) -> Vec4f {
    let (a, b) = (Vec4f::from_u32(a), Vec4f::from_u32(b));
    Vec4f { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, z: a.z + (b.z - a.z) * t, w: 1.0 }
}

/// What the body of a kind needs at least, before knobs get room.
fn body_min(kind: &LensKind) -> f64 {
    match kind {
        LensKind::Clock { .. } => 190.0,
        LensKind::Loop { pattern, .. } => if pattern.is_some() { 300.0 } else { 280.0 },
        LensKind::Synth { .. } => 196.0,
        LensKind::Fx { .. } => 190.0,
        LensKind::Inst { .. } => 40.0,
    }
}

impl ScoreEditor {
    pub(super) fn box_(&mut self, cx: &mut Cx2d, rect: Rect, fill: Vec4f, edge: Vec4f, radius: f32) {
        self.draw_shape.fill = fill;
        self.draw_shape.edge = edge;
        self.draw_shape.stroke = 1.0;
        self.draw_shape.radius = radius;
        self.draw_shape.shape = 0.0;
        self.draw_shape.draw_abs(cx, rect);
    }

    pub(super) fn fill(&mut self, cx: &mut Cx2d, rect: Rect, color: Vec4f) {
        self.draw_shape.fill = color;
        self.draw_shape.shape = 3.0;
        self.draw_shape.draw_abs(cx, rect);
    }

    fn dot(&mut self, cx: &mut Cx2d, center: DVec2, r: f64, color: Vec4f) {
        self.draw_shape.fill = color;
        self.draw_shape.shape = 2.0;
        self.draw_shape.draw_abs(cx, Rect { pos: center - dvec2(r, r), size: dvec2(r * 2.0, r * 2.0) });
    }

    /// A small labelled button, lit in `color` while on; its rect.
    pub(super) fn chip(&mut self, cx: &mut Cx2d, p: &Palette, at: DVec2, text: &str, on: bool, color: u32) -> Rect {
        let w = advance(&self.draw_label, cx, text) + 10.0;
        let rect = Rect { pos: at, size: dvec2(w.max(17.0), 16.0) };
        let (fill, edge, ink) = if on { (rgba(color), rgba(color), rgba(p.on_accent)) } else { (rgba(p.control), rgba(p.line), rgba(p.label)) };
        self.box_(cx, rect, fill, edge, 3.0);
        self.draw_label.color = ink;
        let tw = w - 10.0;
        self.draw_label.draw_abs(cx, dvec2(rect.pos.x + (rect.size.x - tw) * 0.5, rect.pos.y + 2.5), text);
        rect
    }

    /// The block's rail: its lines, in its colour, in the gutter's edge.
    pub(super) fn paint_rail(&mut self, cx: &mut Cx2d, _p: &Palette, rail: Rect, hue: u32, selected: bool) {
        self.fill(cx, rail, alpha(hue, if selected { 0.9 } else { 0.45 }));
    }

    /// One card; true while it animates.
    pub(super) fn paint_lens(&mut self, cx: &mut Cx2d, p: &Palette, lens: &Lens, card: Rect, selected: bool) -> bool {
        let hovered = self.hover_card.as_deref() == Some(lens.node.as_str());
        let edge = alpha(lens.hue, if selected { 0.55 } else if hovered { 0.32 } else { 0.16 });
        self.box_(cx, card, mix(p.panel, lens.hue, 0.045), edge, 5.0);
        self.fill(cx, Rect { pos: dvec2(card.pos.x + 1.0, card.pos.y + 5.0), size: dvec2(2.0, card.size.y - 10.0) }, alpha(lens.hue, if selected { 0.95 } else { 0.6 }));
        self.cards.push((card, lens.node.clone()));
        let top = card.pos.y + 4.0;
        let h = card.size.y - 8.0;
        let mid = top + h * 0.5;
        // The title column: the node's name, and what it is (an
        // instrument's is its picker).
        let x0 = card.pos.x + PAD + 2.0;
        self.draw_name.color = rgba(p.text_bright);
        let name: String = lens.node.chars().take(16).collect();
        self.draw_name.draw_abs(cx, dvec2(x0, top + 2.0), &name);
        let sub_y = top + h - 16.0;
        match &lens.kind {
            LensKind::Synth { label, .. } | LensKind::Inst { label } => {
                let pick = self.pick_chip(cx, p, dvec2(x0, sub_y), label, TITLE_W - 8.0);
                self.hots.push((pick, Hot::Pick { node: lens.node.clone() }));
            }
            kind => {
                let what = match kind {
                    LensKind::Clock { .. } => "tempo".to_string(),
                    LensKind::Loop { plays: Some(inst), .. } => format!("plays {inst}"),
                    LensKind::Loop { .. } => "loop".to_string(),
                    LensKind::Fx { kind, .. } => kind.to_lowercase(),
                    _ => String::new(),
                };
                self.draw_label.color = rgba(p.muted);
                self.draw_label.draw_abs(cx, dvec2(x0, sub_y + 2.0), &what.chars().take(20).collect::<String>());
            }
        }
        // The knobs, right-aligned before the tools' column, as many as
        // leave the body its room.
        let right = card.pos.x + card.size.x - PAD - TOOLS_W;
        let body_x = x0 + TITLE_W;
        let fit = (((right - body_x - body_min(&lens.kind)) / KNOB_W).floor().max(0.0) as usize).min(lens.knobs.len());
        let knobs_x = right - fit as f64 * KNOB_W;
        for (i, knob) in lens.knobs.iter().take(fit).enumerate() {
            self.paint_knob(cx, p, lens, knob, dvec2(knobs_x + i as f64 * KNOB_W, top), h);
        }
        // Its tools, under the pointer or the caret: glyphs in a column,
        // a face only under the pointer.
        if hovered || selected {
            let tx = card.pos.x + card.size.x - PAD - TOOLS_W + 4.0;
            let tools: [(&str, Hot, bool); 3] = [
                ("+", Hot::Duplicate { node: lens.node.clone() }, false),
                ("{ }", Hot::Fold { node: lens.node.clone(), code: true }, lens.code_folded),
                ("–", Hot::Fold { node: lens.node.clone(), code: false }, false),
            ];
            let step = h / 3.0;
            for (i, (text, hot, on)) in tools.into_iter().enumerate() {
                let r = Rect { pos: dvec2(tx, top + i as f64 * step), size: dvec2(TOOLS_W - 2.0, step - 1.0) };
                let under = self.mouse.is_some_and(|m| r.contains(m));
                if under || on {
                    self.box_(cx, r, if on { alpha(lens.hue, 0.35) } else { rgba(p.control_hover) }, alpha(0, 0.0), 3.0);
                }
                self.draw_label.color = rgba(if under { p.text_hi } else if on { p.text } else { p.muted });
                let w = advance(&self.draw_label, cx, text);
                self.draw_label.draw_abs(cx, dvec2(r.pos.x + (r.size.x - w) * 0.5, r.pos.y + (r.size.y - 11.0) * 0.5), text);
                self.hots.push((r, hot));
            }
        }
        let body = Rect { pos: dvec2(body_x, top), size: dvec2((knobs_x - 12.0 - body_x).max(20.0), h) };
        match &lens.kind {
            LensKind::Clock { bpm, beats_per_bar } => self.paint_clock(cx, p, body, mid, *bpm, *beats_per_bar),
            LensKind::Loop { .. } => self.paint_loop(cx, p, lens, body, mid),
            LensKind::Synth { adsr, cutoff, res, filter, .. } => {
                let w = body.size.x.min(110.0);
                self.paint_envelope(cx, p, Rect { pos: body.pos, size: dvec2(w, h) }, *adsr, lens.hue);
                if body.size.x > w + 70.0 {
                    let fr = Rect { pos: dvec2(body.pos.x + w + 8.0, body.pos.y), size: dvec2(w.min(body.size.x - w - 8.0), h) };
                    self.paint_response(cx, p, fr, *cutoff, (*res).max(0.3), *filter, lens.hue);
                }
                false
            }
            LensKind::Fx { key, kind } => self.paint_fx(cx, p, lens, body, mid, *key, kind),
            LensKind::Inst { .. } => false,
        }
    }

    /// A knob: the dial in the block's colour, its label and value under it.
    fn paint_knob(&mut self, cx: &mut Cx2d, p: &Palette, lens: &Lens, knob: &super::Knob, at: DVec2, h: f64) {
        let dragging = self.drag.as_ref().filter(|d| d.node == lens.node && d.knob.field == knob.field).map(|d| d.knob.clone());
        let knob = dragging.as_ref().unwrap_or(knob);
        let dial = Rect { pos: dvec2(at.x + (KNOB_W - DIAL) * 0.5, at.y + 1.0), size: dvec2(DIAL, DIAL) };
        let slot = Rect { pos: at, size: dvec2(KNOB_W, h) };
        let under = self.mouse.is_some_and(|m| slot.contains(m));
        self.draw_shape.fill = rgba(if under || dragging.is_some() { p.control_top } else { p.raised });
        self.draw_shape.edge = alpha(lens.hue, 1.0);
        self.draw_shape.stroke_color = rgba(p.text_bright);
        self.draw_shape.shape = 1.0;
        self.draw_shape.value = knob.norm() as f32;
        self.draw_shape.draw_abs(cx, dial);
        let label = knob.label();
        self.draw_caps.color = rgba(p.muted);
        let w = advance(&self.draw_caps, cx, &label);
        self.draw_caps.draw_abs(cx, dvec2(at.x + (KNOB_W - w) * 0.5, at.y + DIAL + 3.0), &label);
        let value = knob.readout();
        self.draw_label.color = rgba(if dragging.is_some() { p.accent } else { p.text_soft });
        let w = advance(&self.draw_label, cx, &value);
        self.draw_label.draw_abs(cx, dvec2(at.x + (KNOB_W - w) * 0.5, at.y + DIAL + 11.0), &value);
        self.hots.push((slot, Hot::Knob { node: lens.node.clone(), knob: knob.clone() }));
    }

    /// An instrument's name as its picker: a quiet button with a caret.
    fn pick_chip(&mut self, cx: &mut Cx2d, p: &Palette, at: DVec2, text: &str, max_w: f64) -> Rect {
        let mut label: String = text.to_string();
        while label.chars().count() > 3 && advance(&self.draw_label, cx, &label) > max_w - 22.0 {
            label.pop();
        }
        if label.len() < text.len() {
            label.push('…');
        }
        let w = advance(&self.draw_label, cx, &label) + 22.0;
        let rect = Rect { pos: at, size: dvec2(w, 16.0) };
        let under = self.mouse.is_some_and(|m| rect.contains(m));
        self.box_(cx, rect, rgba(if under { p.control_hover } else { p.control }), rgba(p.line), 3.0);
        self.draw_label.color = rgba(p.text);
        self.draw_label.draw_abs(cx, dvec2(rect.pos.x + 7.0, rect.pos.y + 2.5), &label);
        self.draw_caps.color = rgba(p.muted);
        self.draw_caps.draw_abs(cx, dvec2(rect.pos.x + w - 12.0, rect.pos.y + 4.0), "▾");
        rect
    }

    fn meter(&mut self, cx: &mut Cx2d, p: &Palette, rect: Rect, level: f32) {
        self.box_(cx, rect, rgba(p.groove), rgba(p.groove), 1.5);
        let db = if level > 1e-6 { 20.0 * level.log10() } else { -90.0 };
        let t = ((db + 48.0) / 48.0).clamp(0.0, 1.0) as f64;
        let h = rect.size.y * t;
        let color = if db > -1.0 { p.hot } else if db > -9.0 { p.amber } else { p.live };
        if h > 0.5 {
            self.box_(cx, Rect { pos: dvec2(rect.pos.x, rect.pos.y + rect.size.y - h), size: dvec2(rect.size.x, h) }, alpha(color, 0.9), alpha(color, 0.0), 1.5);
        }
    }

    fn well(&mut self, cx: &mut Cx2d, p: &Palette, rect: Rect) {
        self.box_(cx, rect, rgba(p.well), rgba(p.line_faint), 3.0);
    }

    fn paint_clock(&mut self, cx: &mut Cx2d, p: &Palette, body: Rect, mid: f64, bpm: f64, beats_per_bar: f64) -> bool {
        let live = self.live.clone();
        let beat = live.beat.max(0.0);
        let bar = (beat / beats_per_bar).floor();
        let in_bar = beat - bar * beats_per_bar;
        let readout = format!("{}.{}", bar as i64 + 1, in_bar.floor() as i64 + 1);
        self.draw_name.color = rgba(if live.playing { p.text_bright } else { p.muted });
        let mut x = body.pos.x;
        self.draw_name.draw_abs(cx, dvec2(x, mid - 13.0), &readout);
        let tempo = format!("{} bpm", source::fmt_num(bpm, 1));
        self.draw_label.color = rgba(p.muted);
        self.draw_label.draw_abs(cx, dvec2(x, mid + 2.0), &tempo);
        x += advance(&self.draw_label, cx, &tempo).max(advance(&self.draw_name, cx, &readout)) + 16.0;
        let n = (beats_per_bar as usize).clamp(1, 16);
        for i in 0..n {
            let on = live.playing && in_bar.floor() as usize == i;
            let color = if on { if i == 0 { rgba(p.led_down) } else { rgba(p.amber) } } else { rgba(p.edge_lo) };
            self.dot(cx, dvec2(x + i as f64 * 14.0 + 5.0, mid), if i == 0 { 5.0 } else { 4.0 }, color);
        }
        x += n as f64 * 14.0 + 12.0;
        if let Some(section) = &live.section {
            self.draw_label.color = rgba(p.secondary);
            self.draw_label.draw_abs(cx, dvec2(x, mid - 6.0), section);
        }
        live.playing
    }

    fn paint_loop(&mut self, cx: &mut Cx2d, p: &Palette, lens: &Lens, body: Rect, mid: f64) -> bool {
        let LensKind::Loop { key, mute, solo, beats, pattern, .. } = &lens.kind else { return false };
        let (key, mute, solo) = (*key, *mute, *solo);
        let mut x = body.pos.x;
        let m = self.chip(cx, p, dvec2(x, mid - 18.0), "M", mute, p.amber);
        self.hots.push((m, Hot::Toggle { node: lens.node.clone(), field: "mute", on: mute }));
        let s = self.chip(cx, p, dvec2(x, mid + 2.0), "S", solo, p.live);
        self.hots.push((s, Hot::Toggle { node: lens.node.clone(), field: "solo", on: solo }));
        x += m.size.x.max(s.size.x) + 8.0;
        if let Some(beats) = beats {
            let minus = self.chip(cx, p, dvec2(x, mid + 2.0), "÷2", false, p.accent);
            self.hots.push((minus, Hot::Beats { node: lens.node.clone(), beats: *beats, up: false }));
            let plus = self.chip(cx, p, dvec2(x + minus.size.x + 2.0, mid + 2.0), "×2", false, p.accent);
            self.hots.push((plus, Hot::Beats { node: lens.node.clone(), beats: *beats, up: true }));
            let text = format!("{} beats", source::fmt_num(*beats, 2));
            self.draw_label.color = rgba(p.text_soft);
            self.draw_label.draw_abs(cx, dvec2(x + 1.0, mid - 16.0), &text);
            x += (minus.size.x + plus.size.x + 2.0).max(advance(&self.draw_label, cx, &text)) + 10.0;
        }
        let meters = self.meters.clone();
        let (level, _) = meters.as_ref().and_then(|t| t.level(key)).unwrap_or((0.0, 0.0));
        self.meter(cx, p, Rect { pos: dvec2(x, body.pos.y + 2.0), size: dvec2(4.0, body.size.y - 4.0) }, if mute { 0.0 } else { level });
        x += 10.0;
        // Its own scope.
        let scope_w = 60.0;
        let scope = Rect { pos: dvec2(x, body.pos.y + 1.0), size: dvec2(scope_w, body.size.y - 2.0) };
        self.well(cx, p, scope);
        if let Some(t) = &meters {
            self.scope_buf.resize(240, 0.0);
            let mut samples = std::mem::take(&mut self.scope_buf);
            if t.node_scope(key, &mut samples) {
                let cols = (scope_w as usize - 4).max(1);
                let per = (samples.len() / cols).max(1);
                let smid = scope.pos.y + scope.size.y * 0.5;
                for i in 0..cols {
                    let column = &samples[(i * per).min(samples.len() - 1)..((i + 1) * per).min(samples.len())];
                    let lo = column.iter().copied().fold(0.0f32, f32::min) as f64;
                    let hi = column.iter().copied().fold(0.0f32, f32::max) as f64;
                    let top = smid - hi.clamp(-1.0, 1.0) * scope.size.y * 0.42;
                    let bottom = smid - lo.clamp(-1.0, 1.0) * scope.size.y * 0.42;
                    self.fill(cx, Rect { pos: dvec2(scope.pos.x + 2.0 + i as f64, top), size: dvec2(1.0, (bottom - top).max(1.0)) }, alpha(lens.hue, 0.85));
                }
            }
            self.scope_buf = samples;
        }
        x += scope_w + 8.0;
        // The cycle: its notes as a small roll, or the pattern's pills.
        let strip = Rect { pos: dvec2(x, body.pos.y + 1.0), size: dvec2((body.pos.x + body.size.x - x).max(40.0), body.size.y - 2.0) };
        self.well(cx, p, strip);
        let live = self.live.loops.iter().find(|l| l.key == key).cloned();
        let playing = self.live.playing;
        let beat = self.live.beat;
        let phase = live.as_ref().filter(|l| l.len > 0.0 && playing).map(|l| ((beat - l.start) / l.len).rem_euclid(1.0));
        if let Some(pattern) = pattern {
            let n = pattern.text.chars().filter(|c| !c.is_whitespace() && *c != '|').count().max(1);
            let strip = self.paint_slots(cx, p, &lens.node, n, strip);
            let w = (strip.size.x - 4.0) / n as f64;
            let lit_step = phase.map(|ph| (ph * n as f64).floor() as usize);
            let mut i = 0;
            for (offset, c) in pattern.text.char_indices() {
                if c.is_whitespace() || c == '|' {
                    continue;
                }
                let on = !matches!(c, '.' | '-' | '_');
                let cell = Rect { pos: dvec2(strip.pos.x + 2.0 + i as f64 * w + 1.0, strip.pos.y + 4.0), size: dvec2((w - 2.0).max(2.0), strip.size.y - 8.0) };
                let lit = lit_step == Some(i) && !mute;
                let fill = match (on, lit) {
                    (true, true) => rgba(p.warm),
                    (true, false) => alpha(lens.hue, if c == 'o' { 0.45 } else { 0.85 }),
                    (false, true) => alpha(p.warm, 0.3),
                    (false, false) => if i % 4 == 0 { rgba(p.track) } else { rgba(p.raised) },
                };
                self.box_(cx, cell, fill, alpha(0, 0.0), 2.0);
                self.hots.push((cell, Hot::Step { at: pattern.at + offset, on }));
                i += 1;
            }
        } else if let Some(live) = &live {
            let pitched: Vec<&(f64, f32, f32, bool)> = live.notes.iter().filter(|n| !n.3).collect();
            let (lo, hi) = pitched.iter().fold((f32::MAX, f32::MIN), |(lo, hi), n| (lo.min(n.1), hi.max(n.1)));
            let span = (hi - lo).max(11.0);
            let len = live.len.max(1e-6);
            for b in 0..live.len.ceil() as usize {
                let bx = strip.pos.x + b as f64 / len * strip.size.x;
                self.fill(cx, Rect { pos: dvec2(bx, strip.pos.y + 1.0), size: dvec2(1.0, strip.size.y - 2.0) }, alpha(p.line_soft, if b % 4 == 0 { 1.0 } else { 0.45 }));
            }
            for (i, (offset, note, vel, hit)) in live.notes.iter().enumerate() {
                let nx = strip.pos.x + offset / len * strip.size.x;
                let dur = live.durs.get(i).copied().filter(|d| *d > 0.0).map_or(0.25, |d| d as f64);
                let (y, w, hh) = if *hit {
                    let rows = 4.0;
                    (strip.pos.y + 3.0 + (*note as f64 % rows) * (strip.size.y - 6.0) / rows, 3.0, (strip.size.y - 6.0) / rows - 1.0)
                } else {
                    (strip.pos.y + 3.0 + (1.0 - ((note - lo) / span) as f64) * (strip.size.y - 10.0), (dur / len * strip.size.x - 1.0).max(3.0), 4.0)
                };
                let recent = phase.is_some_and(|ph| ph * live.len >= *offset && ph * live.len < offset + dur.max(0.25));
                let c = if recent { rgba(p.warm) } else { alpha(lens.hue, 0.35 + 0.6 * vel.clamp(0.0, 1.0)) };
                self.box_(cx, Rect { pos: dvec2(nx, y), size: dvec2(w, hh) }, c, alpha(0, 0.0), 1.5);
            }
            // Sung lines: each syllable under its note, where it fits.
            let mut free = strip.pos.x;
            for (offset, text) in &live.lyrics {
                let lx = strip.pos.x + offset / len * strip.size.x;
                if lx >= free {
                    self.draw_caps.color = rgba(p.text);
                    self.draw_caps.draw_abs(cx, dvec2(lx, strip.pos.y + strip.size.y - 10.0), text);
                    free = lx + advance(&self.draw_caps, cx, text) + 4.0;
                }
            }
            if live.resting {
                self.draw_label.color = rgba(p.muted);
                self.draw_label.draw_abs(cx, dvec2(strip.pos.x + 8.0, mid - 6.0), "resting this section");
            }
            if let Some(error) = &live.error {
                self.draw_label.color = rgba(p.danger);
                self.draw_label.draw_abs(cx, dvec2(strip.pos.x + 8.0, mid - 6.0), error);
            }
        } else {
            self.draw_label.color = rgba(p.dim);
            self.draw_label.draw_abs(cx, dvec2(strip.pos.x + 8.0, mid - 6.0), "plays when the score plays");
        }
        if let Some(ph) = phase.filter(|_| pattern.is_none()) {
            let px = strip.pos.x + ph * strip.size.x;
            self.fill(cx, Rect { pos: dvec2(px - 0.5, strip.pos.y + 1.0), size: dvec2(1.5, strip.size.y - 2.0) }, rgba(p.accent));
        }
        phase.is_some() || level > 1e-4
    }

    /// The envelope: attack, decay, a held sustain, release.
    fn paint_envelope(&mut self, cx: &mut Cx2d, p: &Palette, rect: Rect, adsr: [f32; 4], hue: u32) {
        let rect = Rect { pos: rect.pos, size: dvec2(rect.size.x.min(110.0), rect.size.y) };
        self.well(cx, p, rect);
        let [a, d, s, r] = adsr;
        let hold = 0.3f32;
        let total = (a + d + hold + r).max(1e-3);
        let n = (rect.size.x - 4.0) as usize;
        for i in 0..n {
            let t = i as f32 / n as f32 * total;
            let v = if t < a {
                t / a.max(1e-4)
            } else if t < a + d {
                1.0 - (1.0 - s) * ((t - a) / d.max(1e-4))
            } else if t < a + d + hold {
                s
            } else {
                s * (1.0 - ((t - a - d - hold) / r.max(1e-4)).min(1.0))
            };
            let h = (v as f64) * (rect.size.y - 6.0);
            self.fill(cx, Rect { pos: dvec2(rect.pos.x + 2.0 + i as f64, rect.pos.y + rect.size.y - 3.0 - h), size: dvec2(1.0, h.max(1.0)) }, alpha(hue, 0.28));
            self.fill(cx, Rect { pos: dvec2(rect.pos.x + 2.0 + i as f64, rect.pos.y + rect.size.y - 3.0 - h), size: dvec2(1.0, 1.0) }, alpha(hue, 1.0));
        }
    }

    /// A two-pole filter's response on a log frequency axis.
    fn paint_response(&mut self, cx: &mut Cx2d, p: &Palette, rect: Rect, cutoff: f32, q: f32, filter: u8, ink: u32) {
        self.well(cx, p, rect);
        let n = (rect.size.x - 4.0) as usize;
        for i in 0..n {
            let hz = 20.0 * 1000f32.powf(i as f32 / n as f32);
            let w = hz / cutoff.max(10.0);
            let den = ((1.0 - w * w).powi(2) + (w / q).powi(2)).sqrt();
            let gain = match filter {
                0 => 1.0,
                2 => w * w / den,
                3 => (w / q) / den,
                _ => 1.0 / den,
            };
            let db = 20.0 * gain.max(1e-4).log10();
            let v = ((db + 36.0) / 48.0).clamp(0.0, 1.0) as f64;
            let h = v * (rect.size.y - 6.0);
            self.fill(cx, Rect { pos: dvec2(rect.pos.x + 2.0 + i as f64, rect.pos.y + rect.size.y - 3.0 - h), size: dvec2(1.0, h.max(1.0)) }, alpha(ink, 0.2));
            self.fill(cx, Rect { pos: dvec2(rect.pos.x + 2.0 + i as f64, rect.pos.y + rect.size.y - 3.0 - h), size: dvec2(1.0, 1.0) }, alpha(ink, 0.95));
        }
    }

    /// An effect: a picture of what it does from its settings, its level.
    fn paint_fx(&mut self, cx: &mut Cx2d, p: &Palette, lens: &Lens, body: Rect, mid: f64, key: u64, kind: &str) -> bool {
        let value = |name: &str, default: f64| lens.knobs.iter().find(|k| k.field == name).map_or(default, |k| k.value);
        let pic = Rect { pos: body.pos, size: dvec2(body.size.x.min(120.0), body.size.y) };
        let ink = p.text_cool;
        match kind {
            "Lowpass" | "Highpass" | "Bandpass" | "Filter" => {
                let filter = match kind {
                    "Highpass" => 2,
                    "Bandpass" => 3,
                    _ => 1,
                };
                self.paint_response(cx, p, pic, value("cutoff", 1000.0) as f32, value("res", 0.7).max(0.3) as f32, filter, ink);
            }
            _ => {
                self.well(cx, p, pic);
                let n = (pic.size.x - 4.0) as usize;
                let col = |this: &mut Self, cx: &mut Cx2d, i: usize, v: f64, a: f32| {
                    let h = v.clamp(0.0, 1.0) * (pic.size.y - 6.0);
                    this.fill(cx, Rect { pos: dvec2(pic.pos.x + 2.0 + i as f64, pic.pos.y + pic.size.y - 3.0 - h), size: dvec2(1.0, h.max(1.0)) }, alpha(ink, a));
                };
                match kind {
                    "Delay" => {
                        // The taps: a delay time apart, each feedback lower.
                        let (time, feedback) = (value("time", 0.75).max(0.05), value("feedback", 0.4).clamp(0.0, 0.98));
                        let span = 8.0;
                        let (mut level, mut t) = (1.0, 0.0);
                        while t <= span && level > 0.02 {
                            let i = (t / span * (n - 1) as f64) as usize;
                            for dx in 0..3 {
                                col(self, cx, (i + dx).min(n - 1), level, 0.75);
                            }
                            t += time;
                            level *= feedback;
                        }
                    }
                    "Reverb" => {
                        // The tail: its length from the room.
                        let tail = 0.3 + value("room", 0.5).clamp(0.0, 1.0) * 5.0;
                        for i in 0..n {
                            let t = i as f64 / n as f64 * 6.0;
                            col(self, cx, i, (-(t / tail) * 3.0).exp(), 0.45);
                        }
                    }
                    "Comp" | "Duck" | "Limiter" => {
                        // The transfer curve, in to out (dB).
                        let (threshold, ratio) = (value("threshold", -18.0), value("ratio", 4.0).max(1.0));
                        for i in 0..n {
                            let input = -48.0 + 48.0 * i as f64 / n as f64;
                            let out = if kind == "Duck" { input - value("depth", 6.0) * 0.5 } else if input > threshold { threshold + (input - threshold) / ratio } else { input };
                            col(self, cx, i, (out + 48.0) / 48.0, 0.5);
                        }
                    }
                    _ => {
                        // A shaper's curve from its first setting.
                        let amount = lens.knobs.first().map_or(0.5, |k| k.norm());
                        for i in 0..n {
                            let t = i as f64 / n as f64 * 2.0 - 1.0;
                            col(self, cx, i, 0.5 + 0.5 * (t * (1.0 + amount * 4.0)).tanh() / (1.0 + amount * 4.0).tanh(), 0.5);
                        }
                    }
                }
            }
        }
        // Its level now: RMS, the peak as a tick.
        let x = pic.pos.x + pic.size.x + 10.0;
        let room = body.pos.x + body.size.x - x;
        let meters = self.meters.clone();
        let (peak, rms) = meters.as_ref().and_then(|t| t.level(key)).unwrap_or((0.0, 0.0));
        if room > 24.0 {
            let bar = Rect { pos: dvec2(x, mid - 3.0), size: dvec2(room.min(160.0), 6.0) };
            self.box_(cx, bar, rgba(p.groove), rgba(p.groove), 3.0);
            let t = ((20.0 * rms.max(1e-6).log10() + 48.0) / 48.0).clamp(0.0, 1.0) as f64;
            if t > 0.01 {
                self.box_(cx, Rect { pos: bar.pos, size: dvec2(bar.size.x * t, bar.size.y) }, alpha(lens.hue, 0.9), alpha(0, 0.0), 3.0);
            }
            let pk = ((20.0 * peak.max(1e-6).log10() + 48.0) / 48.0).clamp(0.0, 1.0) as f64;
            if pk > 0.01 {
                self.fill(cx, Rect { pos: dvec2(bar.pos.x + bar.size.x * pk - 1.0, bar.pos.y - 2.0), size: dvec2(2.0, bar.size.y + 4.0) }, rgba(p.text_hi));
            }
            if let Some(gr) = meters.as_ref().and_then(|t| t.gr(key)).filter(|g| *g < -0.1) {
                self.draw_caps.color = rgba(p.amber);
                self.draw_caps.draw_abs(cx, dvec2(bar.pos.x, bar.pos.y + 9.0), &format!("GR {gr:.1} dB"));
            }
        }
        self.live.playing
    }

    /// A block whose code is folded: its folded lines covered by a quiet
    /// chip saying how many there are (a click unfolds them).
    pub(super) fn paint_folded_code(&mut self, cx: &mut Cx2d, p: &Palette, at: DVec2, lines: usize, node: String, hue: u32) {
        let text = format!("{{ {lines} line{} }}", if lines == 1 { "" } else { "s" });
        let w = (advance(&self.draw_caps, cx, &text) + 14.0).max(96.0);
        let r = Rect { pos: at, size: dvec2(w, 13.0) };
        let under = self.mouse.is_some_and(|m| r.contains(m));
        self.box_(cx, r, rgba(if under { p.control_hover } else { p.well }), alpha(hue, 0.35), 6.5);
        self.draw_caps.color = rgba(if under { p.text } else { p.muted });
        self.draw_caps.draw_abs(cx, dvec2(r.pos.x + 7.0, r.pos.y + 2.0), &text);
        self.hots.push((r, Hot::Fold { node, code: true }));
    }

    /// A folded lens: a small chip after its block's first line.
    pub(super) fn paint_folded_chip(&mut self, cx: &mut Cx2d, p: &Palette, line: Rect, node: String, hue: u32) {
        let text = "lens";
        let w = advance(&self.draw_caps, cx, text) + 20.0;
        let r = Rect { pos: dvec2(line.pos.x + 14.0, line.pos.y + (line.size.y - 14.0) * 0.5), size: dvec2(w, 14.0) };
        let under = self.mouse.is_some_and(|m| r.contains(m));
        self.box_(cx, r, rgba(if under { p.control_hover } else { p.raised }), alpha(hue, 0.4), 7.0);
        self.dot(cx, dvec2(r.pos.x + 7.0, r.pos.y + 7.0), 2.5, alpha(hue, 1.0));
        self.draw_caps.color = rgba(p.secondary);
        self.draw_caps.draw_abs(cx, dvec2(r.pos.x + 13.0, r.pos.y + 2.5), text);
        self.hots.push((r, Hot::Fold { node, code: false }));
    }

    /// A problem at the end of its line.
    pub(super) fn paint_problem(&mut self, cx: &mut Cx2d, p: &Palette, line: Rect, message: &str, clip: Rect) {
        let w = advance(&self.draw_chip_text, cx, message) + 14.0;
        let chip = Rect { pos: dvec2(line.pos.x + 18.0, line.pos.y + 1.0), size: dvec2(w, (line.size.y - 2.0).max(14.0)) };
        if chip.pos.y < clip.pos.y - 2.0 || chip.pos.y + chip.size.y > clip.pos.y + clip.size.y {
            return;
        }
        self.box_(cx, chip, rgba(p.danger_wash), alpha(p.danger, 0.7), 3.0);
        self.draw_chip_text.draw_abs(cx, dvec2(chip.pos.x + 7.0, chip.pos.y + (chip.size.y - 11.0) * 0.5), message);
    }
}
