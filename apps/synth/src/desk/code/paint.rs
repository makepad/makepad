//! How a block looks: its rail in the gutter, its card under its first
//! line, the chips of folded lenses and of problems. Every card is one
//! frame: a title column (the node's name and what it is), a body, and its
//! tools (under the pointer). The bodies are in `modules.rs` (instruments:
//! modules, pads) and here (clock, loop, effect). One grid for every kind,
//! so the cards line up down the code.

use super::lens::{Knob, Lens, LensKind};
use super::{Hot, ScoreEditor};
use makepad_look::palette::Palette;

use makepad_widgets::badge::advance;
use makepad_widgets::*;

/// The title column, a knob's slot, the tools' column, the card's padding,
/// the gap between modules.
pub(super) const TITLE_W: f64 = 118.0;
pub(super) const KNOB_W: f64 = 40.0;
pub(super) const TOOLS_W: f64 = 22.0;
pub(super) const PAD: f64 = 10.0;
pub(super) const GAP: f64 = 12.0;
/// A module's title row.
pub(super) const HEAD: f64 = 11.0;

pub(super) fn rgba(c: u32) -> Vec4f {
    Vec4f::from_u32(c)
}

pub(super) fn alpha(c: u32, a: f32) -> Vec4f {
    let mut v = Vec4f::from_u32(c);
    v.w = a;
    v
}

/// `a` with `t` of `b` mixed in (opaque).
pub(super) fn mix(a: u32, b: u32, t: f32) -> Vec4f {
    let (a, b) = (Vec4f::from_u32(a), Vec4f::from_u32(b));
    Vec4f { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, z: a.z + (b.z - a.z) * t, w: 1.0 }
}

/// A dial's size for a body `h` high (a label and a value under it).
pub(super) fn dial_for(h: f64) -> f64 {
    (h - 19.0).clamp(14.0, 24.0)
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

    pub(super) fn dot(&mut self, cx: &mut Cx2d, center: DVec2, r: f64, color: Vec4f) {
        self.draw_shape.fill = color;
        self.draw_shape.shape = 2.0;
        self.draw_shape.draw_abs(cx, Rect { pos: center - dvec2(r, r), size: dvec2(r * 2.0, r * 2.0) });
    }

    /// A line from `a` to `b`, `w` wide (as short solid steps).
    pub(super) fn line(&mut self, cx: &mut Cx2d, a: DVec2, b: DVec2, w: f64, color: Vec4f) {
        let d = b - a;
        let n = (d.x.abs().max(d.y.abs())).ceil().max(1.0) as usize;
        for i in 0..n {
            let p = a + d * (i as f64 / n as f64);
            self.fill(cx, Rect { pos: dvec2(p.x - w * 0.5, p.y - w * 0.5), size: dvec2(w, w) }, color);
        }
    }

    /// A tooltip for `rect` (shown when the pointer rests there).
    pub(super) fn tip(&mut self, rect: Rect, text: impl Into<String>) {
        self.tips.push((rect, text.into()));
    }

    /// A small labelled button, lit in `color` while on; its rect.
    pub(super) fn chip(&mut self, cx: &mut Cx2d, p: &Palette, at: DVec2, text: &str, on: bool, color: u32) -> Rect {
        let w = advance(&self.draw_label, cx, text) + 10.0;
        let rect = Rect { pos: at, size: dvec2(w.max(17.0), 16.0) };
        let under = self.mouse.is_some_and(|m| rect.contains(m));
        let (fill, edge, ink) = if on {
            (rgba(color), rgba(color), rgba(p.on_accent))
        } else {
            (rgba(if under { p.control_hover } else { p.control }), rgba(p.line), rgba(if under { p.text_hi } else { p.label }))
        };
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
        let top = card.pos.y + 5.0;
        let h = card.size.y - 10.0;
        // The title column: the node's name, what it is (an instrument's
        // is its picker), and a loop's mute and solo.
        let x0 = card.pos.x + PAD + 2.0;
        self.draw_name.color = rgba(p.text_bright);
        let name: String = lens.node.chars().take(16).collect();
        self.draw_name.draw_abs(cx, dvec2(x0, top), &name);
        let name_rect = Rect { pos: dvec2(x0, top), size: dvec2(advance(&self.draw_name, cx, &name), 13.0) };
        self.tip(name_rect, makepad_score_live::assist::type_doc(&lens.ty).map_or_else(|| lens.ty.clone(), |doc| format!("{}: {doc}", lens.ty)));
        match &lens.kind {
            LensKind::Inst { label, engine, .. } => {
                let pick = self.pick_chip(cx, p, dvec2(x0, top + h - 17.0), engine, label, TITLE_W - 8.0, self.browser.is_open() && self.browser.node == lens.node);
                self.hots.push((pick, Hot::Pick { node: lens.node.clone(), at: pick.pos + dvec2(0.0, pick.size.y + 4.0) }));
                self.tip(pick, "the instrument: a click opens the browser (every engine, its presets and kits)");
            }
            LensKind::Loop { plays, mute, solo, .. } => {
                if let Some(inst) = plays {
                    self.draw_label.color = rgba(p.muted);
                    let text: String = format!("plays {inst}").chars().take(20).collect();
                    self.draw_label.draw_abs(cx, dvec2(x0, top + 15.0), &text);
                }
                let y = top + h - 16.0;
                let m = self.chip(cx, p, dvec2(x0, y), "M", *mute, p.amber);
                self.hots.push((m, Hot::Toggle { node: lens.node.clone(), field: "mute", on: *mute }));
                self.tip(m, if *mute { "muted: a click lets it play" } else { "mute this loop" });
                let s = self.chip(cx, p, dvec2(x0 + m.size.x + 4.0, y), "S", *solo, p.live);
                self.hots.push((s, Hot::Toggle { node: lens.node.clone(), field: "solo", on: *solo }));
                self.tip(s, if *solo { "soloed: a click lets the others play too" } else { "solo: only this loop (and other soloed ones) plays" });
            }
            kind => {
                let what = match kind {
                    LensKind::Clock { .. } => "tempo and time".to_string(),
                    LensKind::Fx { kind, .. } => kind.to_lowercase(),
                    LensKind::Stream { .. } => lens.ty.to_lowercase(),
                    _ => String::new(),
                };
                self.draw_label.color = rgba(p.muted);
                self.draw_label.draw_abs(cx, dvec2(x0, top + h - 14.0), &what.chars().take(20).collect::<String>());
            }
        }
        // Its tools, under the pointer or the caret: icons in a column.
        let right = card.pos.x + card.size.x - PAD - TOOLS_W;
        if hovered || selected {
            self.paint_tools(cx, p, lens, dvec2(right + 4.0, top), h);
        }
        let body = Rect { pos: dvec2(x0 + TITLE_W, top), size: dvec2((right - 6.0 - x0 - TITLE_W).max(20.0), h) };
        match &lens.kind {
            LensKind::Clock { .. } => self.paint_clock(cx, p, lens, body),
            LensKind::Loop { .. } => self.paint_loop(cx, p, lens, body),
            LensKind::Inst { modules, pads, .. } => {
                if !pads.is_empty() {
                    self.paint_pads(cx, p, lens, pads, body)
                } else {
                    self.paint_modules(cx, p, lens, modules, body)
                }
            }
            LensKind::Fx { key, kind } => self.paint_fx(cx, p, lens, body, *key, kind),
            LensKind::Stream { card } => self.paint_stream(cx, p, lens, card, body),
        }
    }

    /// The tools: duplicate, fold the code, fold the lens; drawn icons
    /// with their names as tooltips.
    fn paint_tools(&mut self, cx: &mut Cx2d, p: &Palette, lens: &Lens, at: DVec2, h: f64) {
        let step = (h / 3.0).min(18.0);
        let y0 = at.y + (h - step * 3.0) * 0.5;
        let tools: [(u8, Hot, bool, &str); 3] = [
            (0, Hot::Duplicate { node: lens.node.clone() }, false, "duplicate this block (a copy with a new name)"),
            (1, Hot::Fold { node: lens.node.clone(), code: true }, lens.code_folded, if lens.code_folded { "show its code" } else { "fold its code (the lens stays)" }),
            (2, Hot::Fold { node: lens.node.clone(), code: false }, false, "hide the lens (a chip on the line brings it back)"),
        ];
        for (i, (icon, hot, on, tip)) in tools.into_iter().enumerate() {
            let r = Rect { pos: dvec2(at.x, y0 + i as f64 * step), size: dvec2(TOOLS_W - 4.0, step - 2.0) };
            let under = self.mouse.is_some_and(|m| r.contains(m));
            if under || on {
                self.box_(cx, r, if on { alpha(lens.hue, 0.35) } else { rgba(p.control_hover) }, alpha(0, 0.0), 3.0);
            }
            let ink = rgba(if under { p.text_hi } else if on { p.text } else { p.muted });
            let c = r.pos + r.size * 0.5;
            match icon {
                0 => {
                    // Two sheets, one over the other.
                    self.box_(cx, Rect { pos: c + dvec2(-5.0, -5.0), size: dvec2(7.0, 7.0) }, alpha(0, 0.0), ink, 1.5);
                    self.box_(cx, Rect { pos: c + dvec2(-2.0, -2.0), size: dvec2(7.0, 7.0) }, rgba(if under { p.control_hover } else { p.panel }), ink, 1.5);
                }
                1 => {
                    // Lines of code, folded to one when on.
                    let n = if on { 1 } else { 3 };
                    for j in 0..n {
                        let w = [9.0, 6.0, 8.0][j];
                        let y = if on { c.y } else { c.y - 4.0 + j as f64 * 4.0 };
                        self.fill(cx, Rect { pos: dvec2(c.x - 5.0 + if j == 1 { 2.0 } else { 0.0 }, y - 0.75), size: dvec2(w, 1.5) }, ink);
                    }
                }
                _ => {
                    // A chevron up: the card closes.
                    self.line(cx, c + dvec2(-4.5, 2.0), c + dvec2(0.0, -2.5), 1.5, ink);
                    self.line(cx, c + dvec2(0.0, -2.5), c + dvec2(4.5, 2.0), 1.5, ink);
                }
            }
            self.tip(r, tip);
            self.hots.push((r, hot));
        }
    }

    /// A knob in a slot `KNOB_W` wide at `at`: the dial in `hue`, its label
    /// and value under it. The dial turns by drag (a double-click types);
    /// the value types on a click. `group` are the knobs Tab walks through
    /// while typing.
    pub(super) fn paint_knob(&mut self, cx: &mut Cx2d, p: &Palette, node: &str, hue: u32, knob: &Knob, at: DVec2, h: f64, group: &[Knob]) {
        let dragging = self.dragged(node, knob);
        let knob = dragging.as_ref().unwrap_or(knob);
        let d = dial_for(h);
        let dial = Rect { pos: dvec2(at.x + (KNOB_W - d) * 0.5, at.y), size: dvec2(d, d) };
        let under = self.mouse.is_some_and(|m| dial.contains(m));
        self.draw_shape.fill = rgba(if under || dragging.is_some() { p.control_top } else { p.raised });
        self.draw_shape.edge = alpha(hue, 1.0);
        self.draw_shape.stroke_color = rgba(p.text_bright);
        self.draw_shape.shape = 1.0;
        self.draw_shape.value = knob.norm() as f32;
        self.draw_shape.radius = if knob.spec.bipolar { 1.0 } else { 0.0 };
        self.draw_shape.draw_abs(cx, dial);
        let label = knob.label();
        self.draw_caps.color = rgba(p.muted);
        let w = advance(&self.draw_caps, cx, &label).min(KNOB_W);
        self.draw_caps.draw_abs(cx, dvec2(at.x + (KNOB_W - w) * 0.5, at.y + d + 1.0), &label);
        let value_rect = Rect { pos: dvec2(at.x, at.y + d + 8.0), size: dvec2(KNOB_W, 11.0) };
        if !self.paint_typing(cx, p, node, knob, value_rect, false) {
            let value = knob.readout();
            let over = self.mouse.is_some_and(|m| value_rect.contains(m));
            self.draw_label.color = rgba(if dragging.is_some() { p.accent } else if over { p.text_hi } else { p.text_soft });
            let w = advance(&self.draw_label, cx, &value);
            self.draw_label.draw_abs(cx, dvec2(at.x + (KNOB_W - w) * 0.5, value_rect.pos.y - 1.0), &value);
            if over {
                self.fill(cx, Rect { pos: dvec2(at.x + (KNOB_W - w) * 0.5, value_rect.pos.y + 10.0), size: dvec2(w, 1.0) }, alpha(p.text_soft, 0.5));
            }
        }
        let slot = Rect { pos: dvec2(at.x, at.y), size: dvec2(KNOB_W, d + 8.0) };
        self.hots.push((slot, Hot::Knob { node: node.to_string(), knob: knob.clone(), group: group.to_vec() }));
        self.hots.push((value_rect, Hot::Value { node: node.to_string(), knob: knob.clone(), group: group.to_vec() }));
        let tip = format!("{}, now {}.\nDrag to turn (shift: finer). Double-click, or click the value, to type it ({}). Alt-click resets.", knob.tip, knob.spec.display(knob.value), unit_hint(knob));
        self.tip(slot, tip.clone());
        self.tip(value_rect, tip);
    }

    /// The entry box over `rect` when `knob` is being typed; true then.
    pub(super) fn paint_typing(&mut self, cx: &mut Cx2d, p: &Palette, node: &str, knob: &Knob, rect: Rect, on_point: bool) -> bool {
        let Some(t) = self.typing.as_ref().filter(|t| t.node == node && t.knob.field == knob.field && t.knob.entry == knob.entry && t.on_point == on_point) else { return false };
        let text = t.text.clone();
        let bad = t.error;
        let w = (advance(&self.draw_label, cx, &text) + 10.0).max(rect.size.x);
        let r = Rect { pos: dvec2(rect.pos.x + (rect.size.x - w) * 0.5, rect.pos.y - 2.0), size: dvec2(w, 14.0) };
        self.box_(cx, r, rgba(p.well), rgba(if bad { p.danger } else { p.accent }), 3.0);
        self.draw_label.color = rgba(p.text_bright);
        let tw = advance(&self.draw_label, cx, &text);
        self.draw_label.draw_abs(cx, dvec2(r.pos.x + 5.0, r.pos.y + 1.5), &text);
        // The caret.
        self.fill(cx, Rect { pos: dvec2(r.pos.x + 5.0 + tw + 1.0, r.pos.y + 2.0), size: dvec2(1.0, 10.0) }, rgba(p.accent));
        if let Some(t) = self.typing.as_mut() {
            t.rect = r;
        }
        true
    }

    /// A module's title over `x..x+w` at `y` (caps, quiet), and a hairline
    /// between it and the module before.
    pub(super) fn module_head(&mut self, cx: &mut Cx2d, p: &Palette, title: &str, x: f64, y: f64, w: f64, h: f64, first: bool, tip: &str) {
        if !first {
            self.fill(cx, Rect { pos: dvec2(x - GAP * 0.5, y + 1.0), size: dvec2(1.0, h - 2.0) }, rgba(p.line_faint));
        }
        self.draw_caps.color = rgba(p.secondary);
        self.draw_caps.draw_abs(cx, dvec2(x, y), title);
        if !tip.is_empty() {
            let tw = advance(&self.draw_caps, cx, title);
            self.tip(Rect { pos: dvec2(x, y), size: dvec2(tw.min(w), HEAD) }, tip.to_string());
        }
    }

    /// An instrument's name as its picker: the engine, then what it plays,
    /// with a caret.
    fn pick_chip(&mut self, cx: &mut Cx2d, p: &Palette, at: DVec2, engine: &str, text: &str, max_w: f64, open: bool) -> Rect {
        let ew = advance(&self.draw_caps, cx, &engine.to_uppercase()) + 6.0;
        let mut label: String = text.to_string();
        while label.chars().count() > 3 && advance(&self.draw_label, cx, &label) > max_w - 22.0 - ew {
            label.pop();
        }
        if label.len() < text.len() {
            label.push('…');
        }
        let w = advance(&self.draw_label, cx, &label) + 22.0 + ew;
        let rect = Rect { pos: at, size: dvec2(w, 16.0) };
        let under = self.mouse.is_some_and(|m| rect.contains(m));
        self.box_(cx, rect, rgba(if under || open { p.control_hover } else { p.control }), rgba(if open { p.accent } else { p.line }), 3.0);
        self.draw_caps.color = rgba(p.muted);
        self.draw_caps.draw_abs(cx, dvec2(rect.pos.x + 6.0, rect.pos.y + 4.0), &engine.to_uppercase());
        self.draw_label.color = rgba(p.text);
        self.draw_label.draw_abs(cx, dvec2(rect.pos.x + 6.0 + ew, rect.pos.y + 2.5), &label);
        self.draw_caps.color = rgba(p.muted);
        self.draw_caps.draw_abs(cx, dvec2(rect.pos.x + w - 12.0, rect.pos.y + 4.0), "▾");
        rect
    }

    pub(super) fn meter(&mut self, cx: &mut Cx2d, p: &Palette, rect: Rect, level: f32) {
        self.box_(cx, rect, rgba(p.groove), rgba(p.groove), 1.5);
        let db = if level > 1e-6 { 20.0 * level.log10() } else { -90.0 };
        let t = ((db + 48.0) / 48.0).clamp(0.0, 1.0) as f64;
        let h = rect.size.y * t;
        let color = if db > -1.0 { p.hot } else if db > -9.0 { p.amber } else { p.live };
        if h > 0.5 {
            self.box_(cx, Rect { pos: dvec2(rect.pos.x, rect.pos.y + rect.size.y - h), size: dvec2(rect.size.x, h) }, alpha(color, 0.9), alpha(color, 0.0), 1.5);
        }
    }

    pub(super) fn well(&mut self, cx: &mut Cx2d, p: &Palette, rect: Rect) {
        self.box_(cx, rect, rgba(p.well), rgba(p.line_faint), 3.0);
    }

    /// The clock: the tempo large (drag it, or click to type), the
    /// signature, tap tempo, the beats of the bar lit with where the score
    /// is, and the swing.
    fn paint_clock(&mut self, cx: &mut Cx2d, p: &Palette, lens: &Lens, body: Rect) -> bool {
        let LensKind::Clock { tempo, sig, beats_per_bar, tempo_follows } = &lens.kind else { return false };
        let live = self.live.clone();
        let beat = live.beat.max(0.0);
        let bar = (beat / beats_per_bar).floor();
        let in_bar = beat - bar * beats_per_bar;
        let node = lens.node.clone();
        let mut x = body.pos.x;
        let mid = body.pos.y + body.size.y * 0.5;
        // The tempo.
        self.module_head(cx, p, "TEMPO", x, body.pos.y, 80.0, body.size.y, true, "");
        let dragging = self.dragged(&node, tempo);
        let tempo = dragging.as_ref().unwrap_or(tempo).clone();
        let big = Rect { pos: dvec2(x - 2.0, body.pos.y + HEAD), size: dvec2(84.0, body.size.y - HEAD) };
        let under = self.mouse.is_some_and(|m| big.contains(m));
        if under || dragging.is_some() {
            self.box_(cx, big, rgba(p.control_hover), alpha(0, 0.0), 4.0);
        }
        if !self.paint_typing(cx, p, &node, &tempo, Rect { pos: dvec2(x, mid - 2.0), size: dvec2(78.0, 14.0) }, false) {
            let text = makepad_edit_values::number::fmt_num(tempo.value, 1);
            self.draw_big.color = rgba(if dragging.is_some() { p.accent } else { p.text_bright });
            self.draw_big.draw_abs(cx, dvec2(x + 2.0, body.pos.y + HEAD + 2.0), &text);
            let tw = advance(&self.draw_big, cx, &text);
            self.draw_caps.color = rgba(p.muted);
            self.draw_caps.draw_abs(cx, dvec2(x + 6.0 + tw, body.pos.y + HEAD + 12.0), "BPM");
        }
        if *tempo_follows {
            self.draw_caps.color = rgba(p.amber);
            self.draw_caps.draw_abs(cx, dvec2(x + 2.0, body.pos.y + body.size.y - 8.0), "FOLLOWS A PARAM");
        }
        self.hots.push((big, Hot::Tempo { node: node.clone(), knob: tempo.clone() }));
        self.tip(big, format!("the tempo, {} beats a minute: drag up or down (shift for fine), click to type it", makepad_edit_values::number::fmt_num(tempo.value, 1)));
        x += 84.0 + GAP;
        // The signature and tap tempo.
        self.module_head(cx, p, "TIME", x, body.pos.y, 70.0, body.size.y, false, "");
        let sig_text = format!("{}/{}", sig.0, sig.1);
        let s = self.chip(cx, p, dvec2(x, body.pos.y + HEAD + 2.0), &sig_text, false, p.amber);
        let next = match sig {
            (4, 4) => (3, 4),
            (3, 4) => (6, 8),
            (6, 8) => (5, 4),
            (5, 4) => (7, 8),
            _ => (4, 4),
        };
        self.hots.push((s, Hot::Sig { node: node.clone(), next }));
        self.tip(s, format!("the time signature, {} beats a bar: a click steps 4/4, 3/4, 6/8, 5/4, 7/8", sig.0));
        let tap_on = self.taps.last().is_some_and(|t| Cx::time_now() - t < 0.12);
        let t = self.chip(cx, p, dvec2(x + s.size.x + 4.0, body.pos.y + HEAD + 2.0), "TAP", tap_on, p.amber);
        self.hots.push((t, Hot::Tap { node: node.clone() }));
        self.tip(t, "tap tempo: tap on the beat; from the second tap the tempo follows");
        // The bar and beat now.
        let readout = format!("bar {}  beat {}", bar as i64 + 1, in_bar.floor() as i64 + 1);
        self.draw_label.color = rgba(if live.playing { p.text } else { p.muted });
        self.draw_label.draw_abs(cx, dvec2(x, body.pos.y + body.size.y - 13.0), &readout);
        x += (s.size.x + t.size.x + 4.0).max(advance(&self.draw_label, cx, &readout)) + GAP;
        // The beats of the bar.
        let n = (*beats_per_bar as usize).clamp(1, 16);
        let room = body.pos.x + body.size.x - x - KNOB_W - GAP;
        let step = (room / n as f64).clamp(10.0, 22.0);
        self.module_head(cx, p, "BEATS", x, body.pos.y, step * n as f64, body.size.y, false, "");
        let ly = body.pos.y + HEAD + 8.0;
        for i in 0..n {
            let on = live.playing && in_bar.floor() as usize == i;
            let color = if on { if i == 0 { rgba(p.led_down) } else { rgba(p.amber) } } else { rgba(p.edge_lo) };
            let cxy = dvec2(x + i as f64 * step + 6.0, ly);
            if on {
                self.dot(cx, cxy, 8.0, alpha(if i == 0 { p.led_down } else { p.amber }, 0.25));
            }
            self.dot(cx, cxy, if i == 0 { 5.5 } else { 4.5 }, color);
            let num = (i + 1).to_string();
            self.draw_caps.color = rgba(if on { p.text } else { p.dim });
            let tw = advance(&self.draw_caps, cx, &num);
            self.draw_caps.draw_abs(cx, dvec2(cxy.x - tw * 0.5, ly + 9.0), &num);
        }
        if let Some(section) = &live.section {
            self.draw_label.color = rgba(p.secondary);
            self.draw_label.draw_abs(cx, dvec2(x, body.pos.y + body.size.y - 13.0), &format!("section {section}"));
        }
        x += step * n as f64 + GAP;
        // The swing.
        if let Some(swing) = lens.knobs.first() {
            let kx = (body.pos.x + body.size.x - KNOB_W).max(x);
            self.module_head(cx, p, "GROOVE", kx, body.pos.y, KNOB_W, body.size.y, false, "");
            self.paint_knob(cx, p, &node, lens.hue, swing, dvec2(kx, body.pos.y + HEAD), body.size.y - HEAD, &lens.knobs);
        }
        live.playing
    }

    /// A loop: its steps (or its roll) across the card, its length, its
    /// level now, its level and pan knobs.
    fn paint_loop(&mut self, cx: &mut Cx2d, p: &Palette, lens: &Lens, body: Rect) -> bool {
        let LensKind::Loop { key, mute, beats, pattern, .. } = &lens.kind else { return false };
        let (key, mute) = (*key, *mute);
        let knobs_w = lens.knobs.len() as f64 * KNOB_W;
        let right = body.pos.x + body.size.x;
        let kx = right - knobs_w;
        // Level and pan.
        self.module_head(cx, p, "MIX", kx, body.pos.y, knobs_w, body.size.y, false, "");
        for (i, knob) in lens.knobs.iter().enumerate() {
            self.paint_knob(cx, p, &lens.node, lens.hue, knob, dvec2(kx + i as f64 * KNOB_W, body.pos.y + HEAD), body.size.y - HEAD, &lens.knobs);
        }
        // Its level now.
        let meters = self.meters.clone();
        let (level, _) = meters.as_ref().and_then(|t| t.level(key)).unwrap_or((0.0, 0.0));
        let mx = kx - GAP * 0.5 - 5.0;
        let meter = Rect { pos: dvec2(mx, body.pos.y + 1.0), size: dvec2(4.0, body.size.y - 2.0) };
        self.meter(cx, p, meter, if mute { 0.0 } else { level });
        self.tip(meter, "its level now");
        // Its length: halve or double.
        let mut sx = mx - GAP;
        if let Some(beats) = beats {
            let text = format!("{} b", makepad_edit_values::number::fmt_num(*beats, 2));
            let tw = advance(&self.draw_label, cx, &text).max(36.0);
            let lx = sx - tw;
            self.module_head(cx, p, "LENGTH", lx, body.pos.y, tw, body.size.y, false, "");
            self.draw_label.color = rgba(p.text_soft);
            self.draw_label.draw_abs(cx, dvec2(lx + 1.0, body.pos.y + HEAD + 1.0), &text);
            let by = body.pos.y + body.size.y - 16.0;
            let minus = self.chip(cx, p, dvec2(lx, by), "÷2", false, p.accent);
            self.hots.push((minus, Hot::Beats { node: lens.node.clone(), beats: *beats, up: false }));
            self.tip(minus, "half as long");
            let plus = self.chip(cx, p, dvec2(lx + minus.size.x + 2.0, by), "×2", false, p.accent);
            self.hots.push((plus, Hot::Beats { node: lens.node.clone(), beats: *beats, up: true }));
            self.tip(plus, "twice as long");
            sx = lx - GAP;
        }
        // The cycle across the rest: the pattern's steps, or its notes.
        let strip = Rect { pos: dvec2(body.pos.x, body.pos.y + 1.0), size: dvec2((sx - body.pos.x).max(40.0), body.size.y - 2.0) };
        self.well(cx, p, strip);
        let live = self.live.loops.iter().find(|l| l.key == key).cloned();
        let playing = self.live.playing;
        let phase = live.as_ref().filter(|l| l.len > 0.0 && playing).map(|l| ((self.live.beat - l.start) / l.len).rem_euclid(1.0));
        if let Some(pattern) = pattern {
            let n = pattern.text.chars().filter(|c| !c.is_whitespace() && *c != '|').count().max(1);
            let strip = self.paint_slots(cx, p, &lens.node, n, strip);
            self.paint_steps(cx, p, lens, pattern, strip, phase, mute);
            self.tip(strip, "the pattern: a click on a step adds or removes a hit");
        } else if let Some(live) = &live {
            self.paint_roll(cx, p, lens, live, strip, phase);
        } else {
            self.draw_label.color = rgba(p.dim);
            self.draw_label.draw_abs(cx, dvec2(strip.pos.x + 8.0, strip.pos.y + strip.size.y * 0.5 - 6.0), "plays when the score plays");
        }
        phase.is_some() || level > 1e-4
    }

    /// A `hits:` pattern's steps as pills that toggle the source's
    /// characters, the playing step lit.
    fn paint_steps(&mut self, cx: &mut Cx2d, p: &Palette, lens: &Lens, pattern: &super::lens::Pattern, strip: Rect, phase: Option<f64>, mute: bool) {
        let n = pattern.text.chars().filter(|c| !c.is_whitespace() && *c != '|').count().max(1);
        // Steps in groups of four (a beat of sixteenths), a gap between.
        let groups = n.div_ceil(4).max(1) as f64;
        let gap = 4.0;
        let w = ((strip.size.x - 8.0 - gap * (groups - 1.0)) / n as f64).max(3.0);
        let h = (strip.size.y - 12.0).clamp(8.0, 26.0);
        let y = strip.pos.y + (strip.size.y - h) * 0.5;
        let lit_step = phase.map(|ph| (ph * n as f64).floor() as usize);
        let mut i = 0;
        for (offset, c) in pattern.text.char_indices() {
            if c.is_whitespace() || c == '|' {
                continue;
            }
            let on = !matches!(c, '.' | '-' | '_');
            let x = strip.pos.x + 4.0 + i as f64 * w + (i / 4) as f64 * gap;
            let cell = Rect { pos: dvec2(x + 1.0, y), size: dvec2((w - 2.0).max(2.0), h) };
            let lit = lit_step == Some(i) && !mute;
            let fill = match (on, lit) {
                (true, true) => rgba(p.warm),
                (true, false) => alpha(lens.hue, if c == 'o' { 0.45 } else { 0.85 }),
                (false, true) => alpha(p.warm, 0.3),
                (false, false) => if i % 4 == 0 { rgba(p.track) } else { rgba(p.raised) },
            };
            self.box_(cx, cell, fill, alpha(0, 0.0), 3.0);
            // An accent (`X`) a brighter top edge.
            if c == 'X' {
                self.fill(cx, Rect { pos: dvec2(cell.pos.x + 2.0, cell.pos.y + 2.0), size: dvec2(cell.size.x - 4.0, 2.0) }, rgba(p.text_bright));
            }
            self.hots.push((cell, Hot::Step { at: pattern.at + offset, on }));
            i += 1;
        }
    }

    /// What a loop played this cycle as a small piano roll, the playhead
    /// over it.
    fn paint_roll(&mut self, cx: &mut Cx2d, p: &Palette, lens: &Lens, live: &super::LoopLive, strip: Rect, phase: Option<f64>) {
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
        let mid = strip.pos.y + strip.size.y * 0.5;
        if live.resting {
            self.draw_label.color = rgba(p.muted);
            self.draw_label.draw_abs(cx, dvec2(strip.pos.x + 8.0, mid - 6.0), "resting this section");
        }
        if let Some(error) = &live.error {
            self.draw_label.color = rgba(p.danger);
            self.draw_label.draw_abs(cx, dvec2(strip.pos.x + 8.0, mid - 6.0), error);
        }
        if let Some(ph) = phase {
            let px = strip.pos.x + ph * strip.size.x;
            self.fill(cx, Rect { pos: dvec2(px - 0.5, strip.pos.y + 1.0), size: dvec2(1.5, strip.size.y - 2.0) }, rgba(p.accent));
        }
    }

    /// A two-pole filter's response on a log frequency axis.
    pub(super) fn paint_response(&mut self, cx: &mut Cx2d, p: &Palette, rect: Rect, cutoff: f32, q: f32, filter: u8, ink: u32) {
        self.well(cx, p, rect);
        let n = (rect.size.x - 4.0).max(1.0) as usize;
        for i in 0..n {
            let hz = 20.0 * 1000f32.powf(i as f32 / n as f32);
            let gain = response(hz, cutoff, q, filter);
            let db = 20.0 * gain.max(1e-4).log10();
            let v = ((db + 36.0) / 48.0).clamp(0.0, 1.0) as f64;
            let h = v * (rect.size.y - 6.0);
            self.fill(cx, Rect { pos: dvec2(rect.pos.x + 2.0 + i as f64, rect.pos.y + rect.size.y - 3.0 - h), size: dvec2(1.0, h.max(1.0)) }, alpha(ink, 0.2));
            self.fill(cx, Rect { pos: dvec2(rect.pos.x + 2.0 + i as f64, rect.pos.y + rect.size.y - 3.0 - h), size: dvec2(1.0, 1.0) }, alpha(ink, 0.95));
        }
    }

    /// An effect: a picture of what it does from its settings, its level,
    /// its knobs.
    fn paint_fx(&mut self, cx: &mut Cx2d, p: &Palette, lens: &Lens, body: Rect, key: u64, kind: &str) -> bool {
        let value = |name: &str, default: f64| lens.knobs.iter().find(|k| k.field == name).map_or(default, |k| k.value);
        let knobs_w = lens.knobs.len() as f64 * KNOB_W;
        let pic_w = (body.size.x - knobs_w - GAP * 2.0 - 60.0).clamp(60.0, 140.0);
        self.module_head(cx, p, &kind.to_uppercase(), body.pos.x, body.pos.y, pic_w, body.size.y, true, "");
        let pic = Rect { pos: dvec2(body.pos.x, body.pos.y + HEAD), size: dvec2(pic_w, body.size.y - HEAD) };
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
        let x = pic.pos.x + pic.size.x + GAP;
        let kx = body.pos.x + body.size.x - knobs_w;
        let room = kx - GAP - x;
        let meters = self.meters.clone();
        let (peak, rms) = meters.as_ref().and_then(|t| t.level(key)).unwrap_or((0.0, 0.0));
        if room > 24.0 {
            let mid = pic.pos.y + pic.size.y * 0.5;
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
            self.tip(bar, "its level now (the bar) and its peak (the tick)");
        }
        if !lens.knobs.is_empty() {
            self.module_head(cx, p, "SETTINGS", kx, body.pos.y, knobs_w, body.size.y, false, "");
            for (i, knob) in lens.knobs.iter().enumerate() {
                self.paint_knob(cx, p, &lens.node, lens.hue, knob, dvec2(kx + i as f64 * KNOB_W, body.pos.y + HEAD), body.size.y - HEAD, &lens.knobs);
            }
        }
        self.live.playing
    }

    /// A block whose code is folded: its folded lines covered by a quiet
    /// chip saying how many there are (a click unfolds them).
    pub(super) fn paint_folded_code(&mut self, cx: &mut Cx2d, p: &Palette, at: DVec2, lines: usize, node: String, hue: u32) {
        let text = format!("{lines} line{} folded", if lines == 1 { "" } else { "s" });
        let w = (advance(&self.draw_caps, cx, &text) + 14.0).max(96.0);
        let r = Rect { pos: at, size: dvec2(w, 13.0) };
        let under = self.mouse.is_some_and(|m| r.contains(m));
        self.box_(cx, r, rgba(if under { p.control_hover } else { p.well }), alpha(hue, 0.35), 6.5);
        self.draw_caps.color = rgba(if under { p.text } else { p.muted });
        self.draw_caps.draw_abs(cx, dvec2(r.pos.x + 7.0, r.pos.y + 2.0), &text);
        self.tip(r, "a click shows the code again");
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
        self.tip(r, "show the lens again");
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

/// A two-pole filter's gain at `hz` (`filter` 0 off, 1 low, 2 high, 3 band).
pub(super) fn response(hz: f32, cutoff: f32, q: f32, filter: u8) -> f32 {
    let w = hz / cutoff.max(10.0);
    let den = ((1.0 - w * w).powi(2) + (w / q).powi(2)).sqrt();
    match filter {
        0 => 1.0,
        2 => w * w / den,
        3 => (w / q) / den,
        _ => 1.0 / den,
    }
}

/// What units a knob's entry takes, for its tooltip.
fn unit_hint(knob: &Knob) -> &'static str {
    use makepad_synth_panel::spec::Unit;
    if knob.spec.is_choice() {
        return "a name";
    }
    match knob.spec.unit {
        Unit::Hz => "like 800, 2k or 1.5kHz",
        Unit::Secs | Unit::Ms => "like 700ms or 1.2s",
        Unit::Percent => "like 80%",
        Unit::Db => "like -3dB",
        Unit::Cents => "like +7c or 1st",
        Unit::Semis => "like +2st",
        _ => "a number; +, * or / changes it from what it is",
    }
}
