//! An instrument's body: its modules left to right in signal-flow order,
//! each a titled group with its picture and its own knobs together, and a
//! drum kit's pads. The pictures are controls: an envelope's points and a
//! filter's handle drag the knobs they draw (one undo step a gesture,
//! heard while dragging); a double-click on a point types its value.
//!
//! When the card is narrow the pictures shrink first, then the modules that
//! matter least give way (a "+N" says how many), so the oscillator, filter,
//! envelope and output stay together.

use super::lens::{Graph, Knob, Lens, Module, Pad};
use super::paint::{alpha, dial_for, response, rgba, GAP, HEAD, KNOB_W};
use super::{Axis, Hot, ScoreEditor};
use makepad_look::palette::Palette;
use makepad_widgets::badge::advance;
use makepad_widgets::*;

/// A picture's width: as wide as there is room for, within these.
const GRAPH_MIN: f64 = 56.0;
const GRAPH_MAX: f64 = 150.0;
/// A pad's size, at most.
const PAD_W: f64 = 58.0;

/// A module's width with its picture `gw` wide and its first `n` knobs.
fn module_w(m: &Module, gw: f64, n: usize, title_w: f64) -> f64 {
    let g = if matches!(m.graph, Graph::None) { 0.0 } else { gw + if n > 0 { 6.0 } else { 0.0 } };
    (g + n as f64 * KNOB_W).max(title_w)
}

/// Where each shown module goes: (index, x, picture width, knobs shown),
/// and how many gave way.
fn layout(modules: &[Module], titles: &[f64], x0: f64, w: f64) -> (Vec<(usize, f64, f64, usize)>, usize) {
    let mut shown: Vec<usize> = (0..modules.len()).collect();
    let total = |shown: &[usize], gw: f64| -> f64 {
        shown.iter().map(|i| module_w(&modules[*i], gw, modules[*i].knobs.len(), titles[*i])).sum::<f64>() + GAP * shown.len().saturating_sub(1) as f64
    };
    // Drop the modules that matter least until the rest fit at the
    // smallest pictures (keep one).
    while shown.len() > 1 && total(&shown, GRAPH_MIN) > w {
        let least = shown.iter().enumerate().min_by_key(|(_, i)| (modules[**i].rank, std::cmp::Reverse(**i))).map(|(at, _)| at).unwrap_or(0);
        shown.remove(least);
    }
    // The pictures take the room left, within their bounds.
    let graphs = shown.iter().filter(|i| !matches!(modules[**i].graph, Graph::None)).count().max(1) as f64;
    let spare = w - total(&shown, GRAPH_MIN);
    let gw = (GRAPH_MIN + (spare / graphs).max(0.0)).min(GRAPH_MAX);
    let mut out = Vec::new();
    let mut x = x0;
    for i in &shown {
        let m = &modules[*i];
        // A last module that still does not fit shows fewer knobs.
        let mut n = m.knobs.len();
        while n > 0 && x + module_w(m, gw, n, titles[*i]) > x0 + w {
            n -= 1;
        }
        out.push((*i, x, gw, n));
        x += module_w(m, gw, n, titles[*i]) + GAP;
    }
    (out, modules.len() - shown.len())
}

impl ScoreEditor {
    /// The knob as it is under a drag now, if it is.
    pub(super) fn dragged(&self, node: &str, knob: &Knob) -> Option<Knob> {
        let drag = self.drag.as_ref().filter(|d| d.node == node)?;
        drag.axes.iter().find(|a| a.knob.field == knob.field && a.knob.entry == knob.entry).map(|a| a.knob.clone())
    }

    /// The knobs of a module as they are now (a drag's values in place).
    fn live_knobs(&self, node: &str, knobs: &[Knob]) -> Vec<Knob> {
        knobs.iter().map(|k| self.dragged(node, k).unwrap_or_else(|| k.clone())).collect()
    }

    pub(super) fn paint_modules(&mut self, cx: &mut Cx2d, p: &Palette, lens: &Lens, modules: &[Module], body: Rect) -> bool {
        if modules.is_empty() {
            self.draw_label.color = rgba(p.muted);
            self.draw_label.draw_abs(cx, dvec2(body.pos.x, body.pos.y + body.size.y * 0.5 - 6.0), "no settings to show: the browser picks what it plays");
            return false;
        }
        let titles: Vec<f64> = modules.iter().map(|m| advance(&self.draw_caps, cx, &m.title) + 4.0).collect();
        let more_w = 26.0;
        let (placed, dropped) = layout(modules, &titles, body.pos.x, body.size.x - more_w);
        let gy = body.pos.y + HEAD;
        let gh = body.size.y - HEAD;
        for (n, (i, x, gw, shown)) in placed.iter().enumerate() {
            let m = &modules[*i];
            let knobs = self.live_knobs(&lens.node, &m.knobs);
            let w = module_w(m, *gw, *shown, titles[*i]);
            self.module_head(cx, p, &m.title, *x, body.pos.y, w, body.size.y, n == 0, &m.tip);
            let mut kx = *x;
            if !matches!(m.graph, Graph::None) {
                let g = Rect { pos: dvec2(*x, gy + 1.0), size: dvec2(*gw, gh - 2.0) };
                self.paint_graph(cx, p, lens, m, &knobs, g);
                kx += gw + 6.0;
            }
            for (k, knob) in knobs.iter().take(*shown).enumerate() {
                self.paint_knob(cx, p, &lens.node, lens.hue, knob, dvec2(kx + k as f64 * KNOB_W, gy), gh, &m.knobs);
            }
        }
        if dropped > 0 {
            let text = format!("+{dropped}");
            let x = body.pos.x + body.size.x - more_w + 6.0;
            self.draw_label.color = rgba(p.muted);
            self.draw_label.draw_abs(cx, dvec2(x, gy + gh * 0.5 - 6.0), &text);
            let names: Vec<&str> = modules.iter().enumerate().filter(|(i, _)| !placed.iter().any(|(j, ..)| j == i)).map(|(_, m)| m.title.as_str()).collect();
            self.tip(Rect { pos: dvec2(x - 2.0, gy), size: dvec2(more_w - 4.0, gh) }, format!("{} more (a wider view shows them): {}", dropped, names.join(", ").to_lowercase()));
        }
        false
    }

    /// A module's picture and its handles.
    fn paint_graph(&mut self, cx: &mut Cx2d, p: &Palette, lens: &Lens, m: &Module, knobs: &[Knob], g: Rect) {
        let node = lens.node.clone();
        let hue = lens.hue;
        self.well(cx, p, g);
        let inner = Rect { pos: g.pos + dvec2(4.0, 4.0), size: g.size - dvec2(8.0, 8.0) };
        let bottom = inner.pos.y + inner.size.y;
        match &m.graph {
            Graph::None => {}
            Graph::Env { attack, decay, sustain, release } => {
                let (Some(a), Some(d), Some(s), Some(r)) = (knobs.get(*attack), knobs.get(*decay), knobs.get(*sustain), knobs.get(*release)) else { return };
                // Each stage a share of the width by its knob's travel, so a
                // point follows the pointer: attack, decay, a held sustain,
                // release.
                let seg = inner.size.x * 0.28;
                let hold = inner.size.x - seg * 3.0;
                let xa = inner.pos.x + a.norm() * seg;
                let xd = xa + d.norm() * seg;
                let xs = xd + hold;
                let xr = xs + r.norm() * seg;
                let ys = bottom - s.spec.norm(s.value) * inner.size.y;
                let top = inner.pos.y;
                let ink = alpha(hue, 1.0);
                let wash = alpha(hue, 0.18);
                // The fill under the curve, then the curve (decay and
                // release bow as an exponential does).
                let curve = |t: f64, from: f64, to: f64| from + (to - from) * (1.0 - (-4.0 * t).exp()) / (1.0 - (-4.0f64).exp());
                let mut prev: Option<DVec2> = None;
                let n = inner.size.x.max(1.0) as usize;
                for i in 0..=n {
                    let x = inner.pos.x + i as f64;
                    let y = if x <= xa {
                        bottom - (x - inner.pos.x) / (xa - inner.pos.x).max(1e-3) * inner.size.y
                    } else if x <= xd {
                        curve((x - xa) / (xd - xa).max(1e-3), top, ys)
                    } else if x <= xs {
                        ys
                    } else if x <= xr {
                        curve((x - xs) / (xr - xs).max(1e-3), ys, bottom)
                    } else {
                        bottom
                    };
                    self.fill(cx, Rect { pos: dvec2(x, y), size: dvec2(1.0, (bottom - y).max(0.0)) }, wash);
                    if let Some(q) = prev {
                        self.line(cx, q, dvec2(x, y), 1.5, ink);
                    }
                    prev = Some(dvec2(x, y));
                }
                let pts = [
                    (dvec2(xa, top), vec![Axis { knob: a.clone(), per: dvec2(1.0 / seg, 0.0) }], "attack: drag across for its time"),
                    (dvec2(xd, ys), vec![Axis { knob: d.clone(), per: dvec2(1.0 / seg, 0.0) }, Axis { knob: s.clone(), per: dvec2(0.0, -1.0 / inner.size.y) }], "decay and sustain: across for the decay time, up and down for the sustain level"),
                    (dvec2(xr, bottom), vec![Axis { knob: r.clone(), per: dvec2(1.0 / seg, 0.0) }], "release: drag across for its time"),
                ];
                for (at, axes, tip) in pts {
                    self.handle(cx, p, &node, hue, at, axes, None, &m.knobs, tip);
                }
            }
            Graph::Filter { cutoff, res, drive, mode, q_unit } => {
                let Some(c) = knobs.get(*cutoff) else { return };
                let r = res.and_then(|i| knobs.get(i));
                let dr = drive.and_then(|i| knobs.get(i));
                let q = match r {
                    Some(r) if *q_unit => r.value.max(0.3) as f32,
                    Some(r) => (0.5 + r.value * 12.0) as f32,
                    None => 0.707,
                };
                let n = inner.size.x.max(1.0) as usize;
                let mut prev: Option<DVec2> = None;
                for i in 0..=n {
                    let hz = c.spec.denorm(i as f64 / n as f64) as f32;
                    let gain = response(hz, c.value as f32, q, *mode);
                    let db = 20.0 * gain.max(1e-4).log10();
                    let v = ((db + 30.0) / 42.0).clamp(0.0, 1.0) as f64;
                    let at = dvec2(inner.pos.x + i as f64, bottom - v * inner.size.y);
                    self.fill(cx, Rect { pos: at, size: dvec2(1.0, (bottom - at.y).max(0.0)) }, alpha(hue, 0.16));
                    if let Some(q) = prev {
                        self.line(cx, q, at, 1.5, alpha(hue, 1.0));
                    }
                    prev = Some(at);
                }
                // The drive warms the curve's top edge.
                if let Some(dr) = dr.filter(|d| d.norm() > 0.01) {
                    self.fill(cx, Rect { pos: dvec2(inner.pos.x, inner.pos.y - 2.0), size: dvec2(inner.size.x * dr.norm(), 2.0) }, alpha(p.warm, 0.8));
                }
                let x = inner.pos.x + c.norm() * inner.size.x;
                let y = bottom - r.map_or(0.5, |r| r.norm()) * inner.size.y;
                let mut axes = vec![Axis { knob: c.clone(), per: dvec2(1.0 / inner.size.x, 0.0) }];
                if let Some(r) = r {
                    axes.push(Axis { knob: r.clone(), per: dvec2(0.0, -1.0 / inner.size.y) });
                }
                self.handle(cx, p, &node, hue, dvec2(x, y), axes, dr.cloned(), &m.knobs, "the filter: across for the cutoff, up for the resonance, the wheel for the drive");
            }
            Graph::Wave { shape, pos } => {
                let t = pos.and_then(|i| knobs.get(i)).map_or(0.0, |k| k.norm());
                let n = inner.size.x.max(1.0) as usize;
                let mid = inner.pos.y + inner.size.y * 0.5;
                let amp = inner.size.y * 0.42;
                let mut prev: Option<DVec2> = None;
                let mut seed = 0x9e37u32;
                for i in 0..=n {
                    let ph = i as f64 / n as f64 * 2.0;
                    let f = ph.fract();
                    let saw = 1.0 - 2.0 * f;
                    let sine = (ph * std::f64::consts::TAU).sin();
                    let v = match shape {
                        0 => sine,
                        1 => 1.0 - 4.0 * (f - 0.5).abs(),
                        2 => saw,
                        3 => if f < 0.5 { 0.8 } else { -0.8 },
                        4 => if f < 0.3 { 0.8 } else { -0.8 },
                        5 => {
                            seed ^= seed << 13;
                            seed ^= seed >> 17;
                            seed ^= seed << 5;
                            (seed % 2000) as f64 / 1000.0 - 1.0
                        }
                        6 => (ph * std::f64::consts::TAU + 1.6 * (ph * std::f64::consts::TAU * 2.0).sin()).sin(),
                        7 => sine * (-ph * 1.6).exp() * 1.4,
                        // A wavetable: its frames from sine to a bright
                        // stack as its position moves.
                        _ => {
                            let mut v = 0.0;
                            for h in 1..=8 {
                                let w = if h == 1 { 1.0 } else { t / h as f64 };
                                v += w * (ph * std::f64::consts::TAU * h as f64).sin();
                            }
                            v / (1.0 + t * 1.2)
                        }
                    };
                    let at = dvec2(inner.pos.x + i as f64, mid - v.clamp(-1.0, 1.0) * amp);
                    if let Some(q) = prev {
                        self.line(cx, q, at, 1.3, alpha(hue, 0.95));
                    }
                    prev = Some(at);
                }
                match pos.and_then(|i| knobs.get(i)) {
                    Some(k) => {
                        let axes = vec![Axis { knob: k.clone(), per: dvec2(1.0 / inner.size.x, 0.0) }];
                        self.handle(cx, p, &node, hue, dvec2(inner.pos.x + t * inner.size.x, inner.pos.y + 2.0), axes, None, &m.knobs, "where in the wavetable: drag across");
                    }
                    None if *shape <= 4 => {
                        let next = ["@triangle", "@saw", "@square", "@pulse", "@sine"][(*shape as usize).min(4)];
                        self.hots.push((g, Hot::Wave { node: node.clone(), next: next.to_string() }));
                        self.tip(g, format!("the wave: a click picks the next ({})", next.trim_start_matches('@')));
                    }
                    None => {}
                }
            }
            Graph::Lfo { shape, rate } => {
                let n = inner.size.x.max(1.0) as usize;
                let mid = inner.pos.y + inner.size.y * 0.5;
                let cycles = rate.and_then(|i| knobs.get(i)).map_or(2.0, |k| 1.0 + k.norm() * 4.0);
                let mut prev: Option<DVec2> = None;
                for i in 0..=n {
                    let ph = i as f64 / n as f64 * cycles;
                    let f = ph.fract();
                    let v = match shape.as_str() {
                        "triangle" => 1.0 - 4.0 * (f - 0.5).abs(),
                        "saw_up" => 2.0 * f - 1.0,
                        "saw_down" => 1.0 - 2.0 * f,
                        "square" => if f < 0.5 { 0.8 } else { -0.8 },
                        "step" | "smooth" | "random" => (((ph.floor() * 12.9898).sin() * 43758.5453).fract() * 2.0 - 1.0) * 0.8,
                        _ => (ph * std::f64::consts::TAU).sin(),
                    };
                    let at = dvec2(inner.pos.x + i as f64, mid - v * inner.size.y * 0.42);
                    if let Some(q) = prev {
                        self.line(cx, q, at, 1.3, alpha(hue, 0.9));
                    }
                    prev = Some(at);
                }
            }
        }
    }

    /// A handle on a picture: a dot that drags its axes (and the wheel its
    /// `wheel` knob); a double-click types the first axis.
    fn handle(&mut self, cx: &mut Cx2d, p: &Palette, node: &str, hue: u32, at: DVec2, axes: Vec<Axis>, wheel: Option<Knob>, group: &[Knob], tip: &str) {
        let r = Rect { pos: at - dvec2(9.0, 9.0), size: dvec2(18.0, 18.0) };
        let active = self.drag.as_ref().is_some_and(|d| d.node == node && d.axes.iter().zip(axes.iter()).all(|(a, b)| a.knob.field == b.knob.field));
        let under = self.mouse.is_some_and(|m| r.contains(m));
        if under || active {
            self.dot(cx, at, 6.0, alpha(hue, 0.3));
        }
        self.dot(cx, at, if under || active { 3.5 } else { 3.0 }, rgba(if active { p.accent } else { p.text_bright }));
        let values: Vec<String> = axes.iter().map(|a| format!("{} {}", a.knob.label().to_lowercase(), a.knob.spec.display(a.knob.value))).collect();
        if let Some(first) = axes.first() {
            // While it is typed, its entry shows beside the point.
            let box_at = Rect { pos: dvec2(at.x - KNOB_W * 0.5, (at.y - 18.0).max(r.pos.y - 18.0)), size: dvec2(KNOB_W, 11.0) };
            self.paint_typing(cx, p, node, &first.knob.clone(), box_at, true);
        }
        self.tip(r, format!("{tip} ({}); a double-click types it", values.join(", ")));
        self.hots.push((r, Hot::Handle { node: node.to_string(), axes, wheel, group: group.to_vec() }));
    }

    /// A kit: a pad per voice (a click plays it and selects it), the
    /// selected voice's level and tuning.
    pub(super) fn paint_pads(&mut self, cx: &mut Cx2d, p: &Palette, lens: &Lens, pads: &[Pad], body: Rect) -> bool {
        let node = lens.node.clone();
        let sel = self.pad_sel.get(&node).copied().unwrap_or(0).min(pads.len().saturating_sub(1));
        let knobs_w = KNOB_W * 2.0;
        let grid_w = (body.size.x - knobs_w - GAP).max(80.0);
        let cols = 8usize;
        let rows = pads.len().div_ceil(cols).max(1);
        let gy = body.pos.y + HEAD;
        let gh = body.size.y - HEAD;
        let pw = ((grid_w + 3.0) / cols as f64 - 3.0).min(PAD_W);
        let ph = (gh + 3.0) / rows as f64 - 3.0;
        self.module_head(cx, p, "PADS", body.pos.x, body.pos.y, grid_w, body.size.y, true, "");
        // Which pads sound now: the hits of the loops that play this kit.
        let now = self.live.beat;
        let mut lit = [false; 16];
        if self.live.playing {
            let players: Vec<u64> = self.lenses.lenses.iter().filter_map(|l| match &l.kind {
                super::lens::LensKind::Loop { key, plays: Some(inst), mute: false, .. } if *inst == node => Some(*key),
                _ => None,
            }).collect();
            for l in self.live.loops.iter().filter(|l| players.contains(&l.key) && l.len > 0.0) {
                let at = (now - l.start).rem_euclid(l.len);
                for (offset, slot, _, hit) in &l.notes {
                    if *hit && at >= *offset && at < offset + 0.2 {
                        if let Some(x) = lit.get_mut(*slot as usize) {
                            *x = true;
                        }
                    }
                }
            }
        }
        for (i, pad) in pads.iter().enumerate() {
            let (c, r) = (i % cols, i / cols);
            let rect = Rect { pos: dvec2(body.pos.x + c as f64 * (pw + 3.0), gy + r as f64 * (ph + 3.0)), size: dvec2(pw, ph) };
            let under = self.mouse.is_some_and(|m| rect.contains(m));
            let on = lit.get(pad.slot).copied().unwrap_or(false) || self.pad_flash.as_ref().is_some_and(|(n, s, t)| *n == node && *s == pad.slot && Cx::time_now() - t < 0.18);
            let face = if on {
                alpha(lens.hue, 0.85)
            } else if !pad.filled {
                rgba(p.well)
            } else if under {
                rgba(p.control_hover)
            } else {
                rgba(p.raised)
            };
            let edge = if i == sel { alpha(lens.hue, 1.0) } else { rgba(p.line_faint) };
            self.box_(cx, rect, face, edge, 3.0);
            // Its level as a bar along the bottom.
            let level = (pad.level.value / 2.0).clamp(0.0, 1.0);
            self.fill(cx, Rect { pos: dvec2(rect.pos.x + 3.0, rect.pos.y + rect.size.y - 3.0), size: dvec2((rect.size.x - 6.0) * level, 1.5) }, alpha(lens.hue, if pad.filled { 0.9 } else { 0.3 }));
            let label: String = pad.name.replace('_', " ").to_uppercase().chars().take(((pw - 4.0) / 5.0).max(2.0) as usize).collect();
            self.draw_caps.color = rgba(if on { p.on_accent } else if pad.filled { p.text } else { p.dim });
            let tw = advance(&self.draw_caps, cx, &label);
            self.draw_caps.draw_abs(cx, dvec2(rect.pos.x + (rect.size.x - tw) * 0.5, rect.pos.y + (rect.size.y - 9.0) * 0.5), &label);
            if pad.tune.value.abs() > 0.05 && ph > 18.0 {
                let t = format!("{:+.0}", pad.tune.value);
                self.draw_caps.color = rgba(p.muted);
                self.draw_caps.draw_abs(cx, dvec2(rect.pos.x + 3.0, rect.pos.y + 2.0), &t);
            }
            self.hots.push((rect, Hot::Pad { node: node.clone(), slot: pad.slot, drum: pad.name.to_string() }));
            self.tip(rect, if pad.filled { format!("{}: a click plays it and shows its level and tuning", pad.name.replace('_', " ")) } else { format!("{}: this kit has no sound here", pad.name.replace('_', " ")) });
        }
        // The selected voice's level and tuning.
        if let Some(pad) = pads.get(sel) {
            let kx = body.pos.x + body.size.x - knobs_w;
            self.module_head(cx, p, &pad.name.replace('_', " ").to_uppercase(), kx, body.pos.y, knobs_w, body.size.y, false, "");
            let group = vec![pad.level.clone(), pad.tune.clone()];
            let knobs = self.live_knobs(&node, &group);
            let d = dial_for(gh);
            let _ = d;
            self.paint_knob(cx, p, &node, lens.hue, &knobs[0], dvec2(kx, gy), gh, &group);
            self.paint_knob(cx, p, &node, lens.hue, &knobs[1], dvec2(kx + KNOB_W, gy), gh, &group);
        }
        self.live.playing || self.pad_flash.is_some()
    }
}
