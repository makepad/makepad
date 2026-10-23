//! The panel's instruments for values with shape: a list played as steps,
//! points on a map, an ease's curve, a gradient of colours, and a macro
//! knob that drives several paths at once. Each is one record bound by a
//! path (the macro by its targets), drawn by hand in one pass, every step
//! of a drag set live and the release written once.

use super::control::{fit_text, LABEL};
use super::host::{self, fmt_number, numbers_in, with_numbers, Bound, BoundKind, PanelValue};
use crate::color_picker::{format_code_color, parse_code_color, split_hdr, ColorPickAction, HdrColorPicker};
use makepad_widgets::makepad_script::ScriptFnRef;
use makepad_widgets::*;
use std::collections::VecDeque;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PanelStepsBase = #(PanelSteps::register_widget(vm))
    /** A list of numbers as steps (one per beat, per word): drag across to draw. */
    mod.widgets.PanelSteps = set_type_default() do mod.widgets.PanelStepsBase{
        width: Fill height: 96
        draw_bg +: {color: #x00000000}
        draw_cell +: {color: #x00000000
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, min(4.0, min(self.rect_size.x, self.rect_size.y) * 0.5))
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
    }

    mod.widgets.PanelPointsBase = #(PanelPoints::register_widget(vm))
    /** Points on a map (a path, a polygon, camera stops): drag one. */
    mod.widgets.PanelPoints = set_type_default() do mod.widgets.PanelPointsBase{
        width: Fill height: 170
        draw_bg +: {color: #x00000000}
        draw_cell +: {color: #x00000000
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, min(4.0, min(self.rect_size.x, self.rect_size.y) * 0.5))
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
    }

    mod.widgets.PanelCurveBase = #(PanelCurve::register_widget(vm))
    /** A cubic ease (`cubic(x1, y1, x2, y2)`): its two handles, and presets. */
    mod.widgets.PanelCurve = set_type_default() do mod.widgets.PanelCurveBase{
        width: Fill height: 150
        draw_bg +: {color: #x00000000}
        draw_cell +: {color: #x00000000
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, min(4.0, min(self.rect_size.x, self.rect_size.y) * 0.5))
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
    }

    mod.widgets.PanelGradientBase = #(PanelGradient::register_widget(vm))
    /** A list of colours as a gradient: a stop opens the picker; add, remove. */
    mod.widgets.PanelGradient = set_type_default() do mod.widgets.PanelGradientBase{
        width: Fill height: Fit flow: Down spacing: 6
        draw_bg +: {color: #x00000000}
        draw_cell +: {color: #x00000000
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, min(4.0, min(self.rect_size.x, self.rect_size.y) * 0.5))
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
        picker := mod.widgets.HdrColorPicker{visible: false}
    }

    mod.widgets.PanelMeterBase = #(PanelMeter::register_widget(vm))
    /** A path's live value: a bar and its last seconds as a trace. */
    mod.widgets.PanelMeter = set_type_default() do mod.widgets.PanelMeterBase{
        width: Fill height: 48
        draw_bg +: {color: #x00000000}
        draw_cell +: {color: #x00000000
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, min(4.0, min(self.rect_size.x, self.rect_size.y) * 0.5))
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
    }

    mod.widgets.PanelEnvelopeBase = #(PanelEnvelope::register_widget(vm))
    /** Attack, decay, sustain, release as one shape over the song: drag its corners. */
    mod.widgets.PanelEnvelope = set_type_default() do mod.widgets.PanelEnvelopeBase{
        width: Fill height: 110
        draw_bg +: {color: #x00000000}
        draw_cell +: {color: #x00000000
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, min(4.0, min(self.rect_size.x, self.rect_size.y) * 0.5))
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
    }

    mod.widgets.PanelAnchorBase = #(PanelAnchor::register_widget(vm))
    /** A time put on the song: click a beat or a sung word to set it there. */
    mod.widgets.PanelAnchor = set_type_default() do mod.widgets.PanelAnchorBase{
        width: Fill height: 70
        draw_bg +: {color: #x00000000}
        draw_cell +: {color: #x00000000
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, min(4.0, min(self.rect_size.x, self.rect_size.y) * 0.5))
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.0}}
    }

    mod.widgets.PanelKeysBase = #(PanelKeys::register_widget(vm))
    /** Keys over time (`keys([t, v, ease], …)`, `[[t, v], …]`): drag a key in time and value. */
    mod.widgets.PanelKeys = set_type_default() do mod.widgets.PanelKeysBase{
        width: Fill height: 110
        draw_bg +: {color: #x00000000}
        draw_cell +: {color: #x00000000
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, min(4.0, min(self.rect_size.x, self.rect_size.y) * 0.5))
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
    }

    mod.widgets.PanelModBase = #(PanelMod::register_widget(vm))
    /** A modulation patch on a path: a source (LFO, beat, word, noise, time), its rate and depth. */
    mod.widgets.PanelMod = set_type_default() do mod.widgets.PanelModBase{
        width: Fill height: 50
        draw_bg +: {color: #x00000000}
        draw_cell +: {color: #x00000000
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, min(4.0, min(self.rect_size.x, self.rect_size.y) * 0.5))
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
    }

    mod.widgets.PanelMacroBase = #(PanelMacro::register_widget(vm))
    /** One knob driving several paths: `targets: "A.x 0 1, B.y 2 0"`. */
    mod.widgets.PanelMacro = set_type_default() do mod.widgets.PanelMacroBase{
        width: Fill height: Fit
        draw_bg +: {color: #x00000000}
        draw_cell +: {color: #x00000000
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, min(4.0, min(self.rect_size.x, self.rect_size.y) * 0.5))
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
    }
}

fn color(value: u32, alpha: f32) -> Vec4f {
    let mut c = Vec4f::from_u32(value);
    c.w *= alpha;
    c
}


/// A bound list's value now, from the host (or the record's own).
fn read_code(path: &str, value: &str) -> Option<String> {
    host::read(path).map(|v| v.code()).or_else(|| (!value.is_empty()).then(|| value.to_string()))
}

fn write_code(cx: &mut Cx, uid: WidgetUid, source: &ScriptObjectRef, on_change: &ScriptFnRef, path: &str, code: &str) {
    if host::hosted() && !path.is_empty() {
        host::set(path, PanelValue::Code(code.to_string()));
    }
    if host::bound(on_change) {
        let arg = cx.with_vm(|vm| vm.bx.heap_mut().new_string_from_str(code));
        cx.widget_to_script_call(uid, NIL, source.clone(), on_change.clone(), &[arg]);
    }
}

fn label_or(label: &str, path: &str) -> String {
    if label.is_empty() { host::label_of(path) } else { label.to_string() }
}

// ---------------------------------------------------------------- steps

#[derive(Script, ScriptHook, Widget)]
pub struct PanelSteps {
    /// When in the film it acts ("beats 9-16"): the host reads it from the
    /// panel's code.
    #[live]
    pub when: String,
    #[rust]
    place: Area,
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
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
    draw_small: DrawText,
    #[live]
    pub path: String,
    #[live]
    pub label: String,
    #[live]
    pub value: String,
    /// The range a step moves over (both 0: 0 to the largest, or 0..1).
    #[live]
    pub min: f64,
    #[live]
    pub max: f64,
    #[live]
    on_change: ScriptFnRef,
    #[rust]
    code: String,
    #[rust]
    values: Vec<f64>,
    #[rust]
    seen: u64,
    #[rust]
    rect: Rect,
    #[rust]
    dragging: bool,
    #[rust]
    range: (f64, f64),
}

impl PanelSteps {
    fn paint(&mut self, cx: &mut Cx, abs: DVec2) {
        let n = self.values.len();
        if n == 0 || self.rect.size.x <= 0.0 {
            return;
        }
        let i = (((abs.x - self.rect.pos.x) / self.rect.size.x) * n as f64).floor().clamp(0.0, n as f64 - 1.0) as usize;
        let t = 1.0 - ((abs.y - self.rect.pos.y) / self.rect.size.y).clamp(0.0, 1.0);
        let (lo, hi) = self.range;
        let whole = self.values.iter().all(|v| v.fract() == 0.0) && hi - lo >= 4.0;
        let v = lo + (hi - lo) * t;
        self.values[i] = if whole { v.round() } else { (v * 100.0).round() / 100.0 };
        self.code = with_numbers(&self.code, &self.values, 2);
        write_code(cx, self.uid, &self.source, &self.on_change, &self.path, &self.code);
        self.draw_bg.redraw(cx);
    }
}

impl Widget for PanelSteps {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Crosshair),
            Hit::FingerDown(fe) if fe.is_primary_hit() && self.rect.contains(fe.abs) => {
                self.dragging = true;
                self.paint(cx, fe.abs);
            }
            Hit::FingerMove(fe) if self.dragging => self.paint(cx, fe.abs),
            Hit::FingerUp(_) => self.dragging = false,
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rev = host::revision();
        if self.seen != rev && !self.dragging {
            self.seen = rev;
            if let Some(code) = read_code(&self.path, &self.value) {
                self.values = numbers_in(&code);
                self.code = code;
            }
        }
        let maxv = self.values.iter().cloned().fold(0.0f64, f64::max);
        self.range = if self.max > self.min { (self.min, self.max) } else if maxv <= 1.0 { (0.0, 1.0) } else { (0.0, maxv * 1.5) };
        host::register(Bound { path: self.path.clone(), kind: BoundKind::Numbers { lo: self.range.0, hi: self.range.1, step: 0.01 } });
        let p = makepad_look::palette::get(cx);
        let rect = cx.walk_turtle(walk);
        let (uid, paths) = (self.widget_uid().0, vec![self.path.clone()]);
        host::placed(cx, &mut self.place, uid, rect, paths);
        self.draw_bg.color = color(0, 0.0);
        self.draw_bg.draw_abs(cx, rect);
        self.draw_text.color = color(p.label, 1.0);
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, rect.pos.y + 4.0), &label_or(&self.label, &self.path));
        let area = Rect { pos: dvec2(rect.pos.x + LABEL, rect.pos.y), size: dvec2(rect.size.x - LABEL, rect.size.y - 14.0) };
        self.rect = area;
        self.draw_cell.color = color(p.well, 1.0);
        self.draw_cell.draw_abs(cx, area);
        let n = self.values.len().max(1);
        let w = area.size.x / n as f64;
        let (lo, hi) = self.range;
        for (i, v) in self.values.iter().enumerate() {
            let t = if hi > lo { ((v - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 };
            let h = (area.size.y - 4.0) * t;
            // Every fourth step a little brighter: the bar's downbeat.
            self.draw_cell.color = color(p.accent, if i % 4 == 0 { 1.0 } else { 0.7 });
            self.draw_cell.draw_abs(cx, Rect { pos: dvec2(area.pos.x + i as f64 * w + 1.0, area.pos.y + area.size.y - 2.0 - h), size: dvec2((w - 2.0).max(1.0), h.max(1.0)) });
        }
        self.draw_small.color = color(p.muted, 1.0);
        self.draw_small.draw_abs(cx, dvec2(area.pos.x, area.pos.y + area.size.y + 2.0), &format!("{} steps · {}–{}", self.values.len(), fmt_number(lo, 2), fmt_number(hi, 2)));
        DrawStep::done()
    }
}

// ---------------------------------------------------------------- points

#[derive(Script, ScriptHook, Widget)]
pub struct PanelPoints {
    /// When in the film it acts ("beats 9-16"): the host reads it from the
    /// panel's code.
    #[live]
    pub when: String,
    #[rust]
    place: Area,
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
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
    draw_small: DrawText,
    #[live]
    pub path: String,
    #[live]
    pub label: String,
    #[live]
    pub value: String,
    /// Draw the points joined in order (a path), or apart.
    #[live(true)]
    pub joined: bool,
    #[live]
    on_change: ScriptFnRef,
    #[rust]
    code: String,
    #[rust]
    values: Vec<f64>,
    #[rust]
    seen: u64,
    #[rust]
    rect: Rect,
    /// The map's bounds (x0, y0, x1, y1), fixed for a drag.
    #[rust]
    bounds: [f64; 4],
    #[rust]
    dragging: Option<usize>,
    #[rust]
    hot: Option<usize>,
}

impl PanelPoints {
    fn to_screen(&self, x: f64, y: f64) -> DVec2 {
        let [x0, y0, x1, y1] = self.bounds;
        dvec2(self.rect.pos.x + (x - x0) / (x1 - x0).max(1e-9) * self.rect.size.x, self.rect.pos.y + (y - y0) / (y1 - y0).max(1e-9) * self.rect.size.y)
    }

    fn nearest(&self, abs: DVec2) -> Option<usize> {
        (0..self.values.len() / 2).map(|i| (i, self.to_screen(self.values[2 * i], self.values[2 * i + 1]))).filter(|(_, p)| (p.x - abs.x).hypot(p.y - abs.y) < 12.0).min_by(|a, b| (a.1.x - abs.x).hypot(a.1.y - abs.y).total_cmp(&(b.1.x - abs.x).hypot(b.1.y - abs.y))).map(|(i, _)| i)
    }

    fn move_point(&mut self, cx: &mut Cx, i: usize, abs: DVec2) {
        let [x0, y0, x1, y1] = self.bounds;
        let x = x0 + ((abs.x - self.rect.pos.x) / self.rect.size.x).clamp(-0.1, 1.1) * (x1 - x0);
        let y = y0 + ((abs.y - self.rect.pos.y) / self.rect.size.y).clamp(-0.1, 1.1) * (y1 - y0);
        let decimals = if (x1 - x0).abs() > 50.0 { 0 } else { 3 };
        self.values[2 * i] = x;
        self.values[2 * i + 1] = y;
        self.code = with_numbers(&self.code, &self.values, decimals);
        write_code(cx, self.uid, &self.source, &self.on_change, &self.path, &self.code);
        self.draw_bg.redraw(cx);
    }
}

impl Widget for PanelPoints {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = self.nearest(fe.abs);
                cx.set_cursor(if hot.is_some() { MouseCursor::Move } else { MouseCursor::Default });
                if hot != self.hot {
                    self.hot = hot;
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => self.dragging = self.nearest(fe.abs),
            Hit::FingerMove(fe) => {
                if let Some(i) = self.dragging {
                    self.move_point(cx, i, fe.abs);
                }
            }
            Hit::FingerUp(_) => self.dragging = None,
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rev = host::revision();
        if self.seen != rev && self.dragging.is_none() {
            self.seen = rev;
            if let Some(code) = read_code(&self.path, &self.value) {
                self.values = numbers_in(&code);
                self.code = code;
            }
            // The map: the points' extent and a margin.
            let xs: Vec<f64> = self.values.iter().step_by(2).copied().collect();
            let ys: Vec<f64> = self.values.iter().skip(1).step_by(2).copied().collect();
            let (mut x0, mut x1) = (xs.iter().cloned().fold(f64::MAX, f64::min), xs.iter().cloned().fold(f64::MIN, f64::max));
            let (mut y0, mut y1) = (ys.iter().cloned().fold(f64::MAX, f64::min), ys.iter().cloned().fold(f64::MIN, f64::max));
            if x0 > x1 {
                (x0, x1, y0, y1) = (0.0, 1.0, 0.0, 1.0);
            }
            let (mx, my) = (((x1 - x0) * 0.15).max(1e-3), ((y1 - y0) * 0.15).max(1e-3));
            self.bounds = [x0 - mx, y0 - my, x1 + mx, y1 + my];
        }
        host::register(Bound { path: self.path.clone(), kind: BoundKind::Other });
        let p = makepad_look::palette::get(cx);
        let rect = cx.walk_turtle(walk);
        let (uid, paths) = (self.widget_uid().0, vec![self.path.clone()]);
        host::placed(cx, &mut self.place, uid, rect, paths);
        self.draw_bg.color = color(0, 0.0);
        self.draw_bg.draw_abs(cx, rect);
        self.draw_text.color = color(p.label, 1.0);
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, rect.pos.y + 4.0), &label_or(&self.label, &self.path));
        let area = Rect { pos: dvec2(rect.pos.x + LABEL, rect.pos.y), size: dvec2(rect.size.x - LABEL, rect.size.y) };
        self.rect = area;
        self.draw_cell.color = color(p.well, 1.0);
        self.draw_cell.draw_abs(cx, area);
        let pts: Vec<DVec2> = (0..self.values.len() / 2).map(|i| self.to_screen(self.values[2 * i], self.values[2 * i + 1])).collect();
        if self.joined {
            self.draw_cell.color = color(p.accent, 0.5);
            for w in pts.windows(2) {
                // The segment as dots: cheap, and enough to read the path.
                let steps = ((w[1] - w[0]).length() / 3.0).ceil().max(1.0) as usize;
                for k in 0..steps {
                    let q = w[0] + (w[1] - w[0]) * (k as f64 / steps as f64);
                    self.draw_cell.draw_abs(cx, Rect { pos: q - dvec2(0.75, 0.75), size: dvec2(1.5, 1.5) });
                }
            }
        }
        for (i, q) in pts.iter().enumerate() {
            let on = self.hot == Some(i) || self.dragging == Some(i);
            let s = if on { 9.0 } else { 7.0 };
            self.draw_cell.color = color(if on { p.text_hi } else { p.accent }, 1.0);
            self.draw_cell.draw_abs(cx, Rect { pos: *q - dvec2(s * 0.5, s * 0.5), size: dvec2(s, s) });
        }
        self.draw_small.color = color(p.muted, 1.0);
        let said = match self.dragging.or(self.hot) {
            Some(i) => format!("{} · {}, {}", i + 1, fmt_number(self.values[2 * i], 2), fmt_number(self.values[2 * i + 1], 2)),
            None => format!("{} points", pts.len()),
        };
        self.draw_small.draw_abs(cx, dvec2(area.pos.x + 4.0, area.pos.y + area.size.y - 14.0), &said);
        DrawStep::done()
    }
}

// ---------------------------------------------------------------- curve

/// The cubic eases a curve editor offers.
const CURVES: [(&str, [f64; 4]); 7] = [
    ("linear", [0.0, 0.0, 1.0, 1.0]),
    ("in", [0.42, 0.0, 1.0, 1.0]),
    ("out", [0.0, 0.0, 0.58, 1.0]),
    ("in out", [0.42, 0.0, 0.58, 1.0]),
    ("snap", [0.2, 0.0, 0.0, 1.0]),
    ("back", [0.34, 1.56, 0.64, 1.0]),
    ("anticipate", [0.36, -0.4, 0.64, 1.0]),
];

fn cubic_at(c: [f64; 4], t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    let x = 3.0 * u * u * t * c[0] + 3.0 * u * t * t * c[2] + t * t * t;
    let y = 3.0 * u * u * t * c[1] + 3.0 * u * t * t * c[3] + t * t * t;
    (x, y)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CurveSpot {
    Handle(usize),
    Preset(usize),
}

#[derive(Script, ScriptHook, Widget)]
pub struct PanelCurve {
    /// When in the film it acts ("beats 9-16"): the host reads it from the
    /// panel's code.
    #[live]
    pub when: String,
    #[rust]
    place: Area,
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
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
    draw_small: DrawText,
    #[live]
    pub path: String,
    #[live]
    pub label: String,
    #[live]
    pub value: String,
    #[live]
    on_change: ScriptFnRef,
    #[rust]
    code: String,
    #[rust]
    c: [f64; 4],
    #[rust]
    seen: u64,
    #[rust]
    plot: Rect,
    #[rust]
    spots: Vec<(Rect, CurveSpot)>,
    #[rust]
    dragging: Option<usize>,
}

impl PanelCurve {
    fn write(&mut self, cx: &mut Cx) {
        let n = |v: f64| fmt_number(v, 3);
        self.code = if numbers_in(&self.code).len() >= 4 {
            with_numbers(&self.code, &self.c, 3)
        } else {
            format!("cubic({}, {}, {}, {})", n(self.c[0]), n(self.c[1]), n(self.c[2]), n(self.c[3]))
        };
        write_code(cx, self.uid, &self.source, &self.on_change, &self.path, &self.code);
        self.draw_bg.redraw(cx);
    }

    fn pt(&self, x: f64, y: f64) -> DVec2 {
        // y over 1 or under 0 (overshoot) has room: the plot's middle 70%.
        dvec2(self.plot.pos.x + x * self.plot.size.x, self.plot.pos.y + self.plot.size.y * (0.85 - y * 0.7))
    }
}

impl Widget for PanelCurve {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let at = |s: &Self, abs: DVec2| s.spots.iter().find(|(r, _)| r.contains(abs)).map(|(_, x)| *x);
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::FingerDown(fe) if fe.is_primary_hit() => match at(self, fe.abs) {
                Some(CurveSpot::Handle(h)) => self.dragging = Some(h),
                Some(CurveSpot::Preset(k)) => {
                    self.c = CURVES[k].1;
                    self.write(cx);
                }
                None => {}
            },
            Hit::FingerMove(fe) => {
                if let Some(h) = self.dragging {
                    let x = ((fe.abs.x - self.plot.pos.x) / self.plot.size.x).clamp(0.0, 1.0);
                    let y = (0.85 - (fe.abs.y - self.plot.pos.y) / self.plot.size.y) / 0.7;
                    self.c[h * 2] = (x * 1000.0).round() / 1000.0;
                    self.c[h * 2 + 1] = (y.clamp(-1.0, 2.0) * 1000.0).round() / 1000.0;
                    self.write(cx);
                }
            }
            Hit::FingerUp(_) => self.dragging = None,
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rev = host::revision();
        if self.seen != rev && self.dragging.is_none() {
            self.seen = rev;
            if let Some(code) = read_code(&self.path, &self.value) {
                let n = numbers_in(&code);
                if n.len() >= 4 {
                    self.c = [n[0], n[1], n[2], n[3]];
                }
                self.code = code;
            }
        }
        host::register(Bound { path: self.path.clone(), kind: BoundKind::Numbers { lo: -1.0, hi: 2.0, step: 0.001 } });
        let p = makepad_look::palette::get(cx);
        let rect = cx.walk_turtle(walk);
        let (uid, paths) = (self.widget_uid().0, vec![self.path.clone()]);
        host::placed(cx, &mut self.place, uid, rect, paths);
        self.draw_bg.color = color(0, 0.0);
        self.draw_bg.draw_abs(cx, rect);
        self.spots.clear();
        self.draw_text.color = color(p.label, 1.0);
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, rect.pos.y + 4.0), &label_or(&self.label, &self.path));
        // Presets down the label column.
        for (k, (name, c)) in CURVES.iter().enumerate() {
            let r = Rect { pos: dvec2(rect.pos.x, rect.pos.y + 22.0 + k as f64 * 17.0), size: dvec2(LABEL - 10.0, 15.0) };
            let on = *c == self.c;
            self.draw_cell.color = color(if on { p.accent } else { p.raised }, 1.0);
            self.draw_cell.draw_abs(cx, r);
            self.draw_small.color = color(if on { p.on_accent } else { p.text }, 1.0);
            self.draw_small.draw_abs(cx, dvec2(r.pos.x + 5.0, r.pos.y + 2.0), name);
            self.spots.push((r, CurveSpot::Preset(k)));
        }
        let side = (rect.size.x - LABEL).min(rect.size.y);
        self.plot = Rect { pos: dvec2(rect.pos.x + LABEL, rect.pos.y), size: dvec2(side, side) };
        self.draw_cell.color = color(p.well, 1.0);
        self.draw_cell.draw_abs(cx, self.plot);
        // 0 and 1.
        self.draw_cell.color = color(p.line_faint, 1.0);
        for y in [0.0, 1.0] {
            let q = self.pt(0.0, y);
            self.draw_cell.draw_abs(cx, Rect { pos: q, size: dvec2(self.plot.size.x, 1.0) });
        }
        // The curve, as dots.
        self.draw_cell.color = color(p.text_hi, 1.0);
        for k in 0..=80 {
            let (x, y) = cubic_at(self.c, k as f64 / 80.0);
            let q = self.pt(x, y);
            self.draw_cell.draw_abs(cx, Rect { pos: q - dvec2(1.0, 1.0), size: dvec2(2.0, 2.0) });
        }
        // The handles, and their arms.
        for h in 0..2 {
            let (ax, ay) = if h == 0 { (0.0, 0.0) } else { (1.0, 1.0) };
            let a = self.pt(ax, ay);
            let b = self.pt(self.c[h * 2], self.c[h * 2 + 1]);
            self.draw_cell.color = color(p.accent, 0.6);
            let steps = ((b - a).length() / 3.0).ceil().max(1.0) as usize;
            for k in 0..steps {
                let q = a + (b - a) * (k as f64 / steps as f64);
                self.draw_cell.draw_abs(cx, Rect { pos: q - dvec2(0.5, 0.5), size: dvec2(1.0, 1.0) });
            }
            let r = Rect { pos: b - dvec2(5.0, 5.0), size: dvec2(10.0, 10.0) };
            self.draw_cell.color = color(if self.dragging == Some(h) { p.text_hi } else { p.accent }, 1.0);
            self.draw_cell.draw_abs(cx, r);
            self.spots.push((Rect { pos: b - dvec2(9.0, 9.0), size: dvec2(18.0, 18.0) }, CurveSpot::Handle(h)));
        }
        self.draw_small.color = color(p.muted, 1.0);
        self.draw_small.draw_abs(cx, dvec2(self.plot.pos.x + self.plot.size.x + 8.0, rect.pos.y + 4.0), &self.code);
        DrawStep::done()
    }
}

// ---------------------------------------------------------------- gradient

#[derive(Clone, Copy, Debug, PartialEq)]
enum GradSpot {
    Stop(usize),
    Add,
    Remove,
}

#[derive(Script, ScriptHook, Widget)]
pub struct PanelGradient {
    /// When in the film it acts ("beats 9-16"): the host reads it from the
    /// panel's code.
    #[live]
    pub when: String,
    #[rust]
    place: Area,
    #[deref]
    view: View,
    #[live]
    pub path: String,
    #[live]
    pub label: String,
    #[live]
    pub value: String,
    #[live]
    on_change: ScriptFnRef,
    #[source]
    source: ScriptObjectRef,
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_cell: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_small: DrawText,
    #[rust]
    stops: Vec<[f64; 4]>,
    #[rust]
    seen: u64,
    #[rust]
    chosen: Option<usize>,
    #[rust]
    spots: Vec<(Rect, GradSpot)>,
    #[rust]
    moving: bool,
}

/// The colours a list writes, in order.
fn colours_in(code: &str) -> Vec<[f64; 4]> {
    let t = code.trim().trim_start_matches('[').trim_end_matches(']');
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, ch) in t.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                if let Some(c) = parse_code_color(t[start..i].trim()) {
                    out.push(c);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    if let Some(c) = parse_code_color(t[start..].trim()) {
        out.push(c);
    }
    out
}

impl PanelGradient {
    fn write(&mut self, cx: &mut Cx) {
        let code = format!("[{}]", self.stops.iter().map(|c| format_code_color(*c)).collect::<Vec<_>>().join(", "));
        let uid = self.widget_uid();
        write_code(cx, uid, &self.source, &self.on_change, &self.path, &code);
        self.draw_bg.redraw(cx);
    }

    fn choose(&mut self, cx: &mut Cx, i: Option<usize>) {
        self.chosen = i;
        let picker = self.view.widget(cx, ids!(picker));
        picker.set_visible(cx, i.is_some());
        if let Some(i) = i {
            let c = self.stops[i];
            if let Some(mut p) = picker.borrow_mut::<HdrColorPicker>() {
                p.set_value(cx, c);
            }
        }
        self.view.redraw(cx);
    }
}

impl Widget for PanelGradient {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let at = |s: &Self, abs: DVec2| s.spots.iter().find(|(r, _)| r.contains(abs)).map(|(_, x)| *x);
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::FingerUp(fe) if fe.is_over && fe.is_primary_hit() => match at(self, fe.abs) {
                Some(GradSpot::Stop(i)) => self.choose(cx, if self.chosen == Some(i) { None } else { Some(i) }),
                Some(GradSpot::Add) => {
                    let last = self.stops.last().copied().unwrap_or([1.0, 1.0, 1.0, 1.0]);
                    self.stops.push(last);
                    self.write(cx);
                    self.choose(cx, Some(self.stops.len() - 1));
                }
                Some(GradSpot::Remove) => {
                    if let Some(i) = self.chosen.filter(|_| self.stops.len() > 2) {
                        self.stops.remove(i);
                        self.write(cx);
                        self.choose(cx, None);
                    }
                }
                None => {}
            },
            _ => {}
        }
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            let uid = self.view.widget(cx, ids!(picker)).widget_uid();
            for action in actions.filter_widget_actions_cast::<ColorPickAction>(uid) {
                let Some(i) = self.chosen else { continue };
                match action {
                    ColorPickAction::Preview(c) => {
                        self.moving = true;
                        self.stops[i] = c;
                        self.write(cx);
                    }
                    ColorPickAction::Commit(c) => {
                        self.moving = false;
                        self.stops[i] = c;
                        self.write(cx);
                    }
                    _ => {}
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let rev = host::revision();
        if self.seen != rev && !self.moving {
            self.seen = rev;
            if let Some(code) = read_code(&self.path, &self.value) {
                self.stops = colours_in(&code);
            }
        }
        host::register(Bound { path: self.path.clone(), kind: BoundKind::Other });
        let p = makepad_look::palette::get(cx);
        cx.begin_turtle(walk, Layout::flow_down());
        let rect = cx.walk_turtle(Walk::new(Size::fill(), Size::Fixed(40.0)));
        self.draw_bg.color = color(0, 0.0);
        self.draw_bg.draw_abs(cx, rect);
        self.spots.clear();
        self.draw_text.color = color(p.label, 1.0);
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, rect.pos.y + 4.0), &label_or(&self.label, &self.path));
        let bar = Rect { pos: dvec2(rect.pos.x + LABEL, rect.pos.y + 2.0), size: dvec2(rect.size.x - LABEL - 52.0, 18.0) };
        // The gradient, in thin slices.
        let n = self.stops.len();
        if n >= 1 {
            let slices = (bar.size.x / 2.0).ceil() as usize;
            for k in 0..slices {
                let t = k as f64 / slices.max(1) as f64 * (n.max(2) - 1) as f64;
                let (i, f) = ((t.floor() as usize).min(n - 1), t.fract());
                let a = split_hdr(self.stops[i]).0;
                let b = split_hdr(self.stops[(i + 1).min(n - 1)]).0;
                let c = [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f];
                self.draw_cell.color = vec4(c[0] as f32, c[1] as f32, c[2] as f32, 1.0);
                self.draw_cell.draw_abs(cx, Rect { pos: dvec2(bar.pos.x + k as f64 * 2.0, bar.pos.y), size: dvec2(2.0, bar.size.y) });
            }
        }
        for (i, c) in self.stops.iter().enumerate() {
            let x = bar.pos.x + if n > 1 { bar.size.x * i as f64 / (n - 1) as f64 } else { 0.0 };
            let r = Rect { pos: dvec2(x - 6.0, bar.pos.y + bar.size.y + 2.0), size: dvec2(12.0, 14.0) };
            self.draw_cell.color = color(if self.chosen == Some(i) { p.accent } else { p.text }, 1.0);
            self.draw_cell.draw_abs(cx, r);
            let b = split_hdr(*c).0;
            self.draw_cell.color = vec4(b[0] as f32, b[1] as f32, b[2] as f32, 1.0);
            self.draw_cell.draw_abs(cx, Rect { pos: r.pos + dvec2(2.0, 2.0), size: r.size - dvec2(4.0, 4.0) });
            self.spots.push((r, GradSpot::Stop(i)));
        }
        for (k, (label, spot)) in [("+", GradSpot::Add), ("−", GradSpot::Remove)].into_iter().enumerate() {
            let r = Rect { pos: dvec2(bar.pos.x + bar.size.x + 8.0 + k as f64 * 22.0, bar.pos.y), size: dvec2(18.0, 18.0) };
            self.draw_cell.color = color(p.raised, 1.0);
            self.draw_cell.draw_abs(cx, r);
            self.draw_text.color = color(p.text, 1.0);
            self.draw_text.draw_abs(cx, dvec2(r.pos.x + 5.0, r.pos.y + 2.0), label);
            self.spots.push((r, spot));
        }
        if self.chosen.is_some() {
            let _ = self.view.draw_walk(cx, scope, Walk::new(Size::fill(), Size::fit()));
        }
        let placed_rect = cx.end_turtle();
        let (uid, paths) = (self.widget_uid().0, vec![self.path.clone()]);
        host::placed(cx, &mut self.place, uid, placed_rect, paths);
        DrawStep::done()
    }
}

// ---------------------------------------------------------------- macro

#[derive(Script, ScriptHook, Widget)]
pub struct PanelMacro {
    /// When in the film it acts ("beats 9-16"): the host reads it from the
    /// panel's code.
    #[live]
    pub when: String,
    #[rust]
    place: Area,
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
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
    draw_small: DrawText,
    #[live]
    pub label: String,
    /// What it drives: `"PATH from to, PATH from to"` (from at 0, to at 1).
    #[live]
    pub targets: String,
    #[live]
    on_change: ScriptFnRef,
    #[rust]
    t: f64,
    #[rust]
    rect: Rect,
    /// A drag: where it began and the amount then.
    #[rust]
    dragging: Option<(DVec2, f64)>,
}

impl PanelMacro {
    pub(crate) fn parsed(&self) -> Vec<(String, f64, f64)> {
        self.targets.split(',').filter_map(|t| {
            let parts: Vec<&str> = t.split_whitespace().collect();
            let path = parts.first()?.to_string();
            let from = parts.get(1).and_then(|v| v.parse().ok()).unwrap_or(0.0);
            let to = parts.get(2).and_then(|v| v.parse().ok()).unwrap_or(1.0);
            Some((path, from, to))
        }).collect()
    }

    fn set_t(&mut self, cx: &mut Cx, t: f64) {
        self.t = t.clamp(0.0, 1.0);
        for (path, from, to) in self.parsed() {
            let v = from + (to - from) * self.t;
            let Some(now) = host::read(&path).map(|v| v.code()) else { continue };
            let code = if numbers_in(&now).is_empty() { fmt_number(v, 3) } else { with_numbers(&now, &[v], 3) };
            host::set(&path, PanelValue::Code(code));
        }
        if host::bound(&self.on_change) {
            cx.widget_to_script_call(self.uid, NIL, self.source.clone(), self.on_change.clone(), &[ScriptValue::from_f64(self.t)]);
        }
        host::changed();
        self.draw_bg.redraw(cx);
    }
}

impl Widget for PanelMacro {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::NsResize),
            Hit::FingerDown(fe) if fe.is_primary_hit() && self.rect.contains(fe.abs) => self.dragging = Some((fe.abs, self.t)),
            Hit::FingerMove(fe) => {
                if let Some((origin, t0)) = self.dragging {
                    let t = t0 + ((origin.y - fe.abs.y) + (fe.abs.x - origin.x) * 0.25) / 160.0;
                    self.set_t(cx, t);
                }
            }
            Hit::FingerUp(_) => self.dragging = None,
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let targets = self.parsed();
        let p = makepad_look::palette::get(cx);
        // One compact row: the label in its column, the amount as a slim
        // level (dragged up and down), its percentage, and what it drives
        // in one line under that.
        let rect = cx.walk_turtle(Walk { height: Size::Fixed(40.0), ..walk });
        let (uid, paths) = (self.widget_uid().0, targets.iter().map(|t| t.0.clone()).collect());
        host::placed(cx, &mut self.place, uid, rect, paths);
        self.draw_bg.color = color(0, 0.0);
        self.draw_bg.draw_abs(cx, rect);
        self.draw_text.color = color(p.label, 1.0);
        let label = if self.label.is_empty() { "Macro".to_string() } else { self.label.clone() };
        let (label, _) = fit_text(&self.draw_text, cx, &label, LABEL - 4.0);
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, rect.pos.y + 6.0), &label);
        let k = Rect { pos: dvec2(rect.pos.x + LABEL + 8.0, rect.pos.y + 3.0), size: dvec2(12.0, 34.0) };
        self.rect = Rect { pos: k.pos - dvec2(4.0, 2.0), size: dvec2(64.0, 38.0) };
        self.draw_cell.color = color(p.track, 1.0);
        self.draw_cell.draw_abs(cx, k);
        let fill = (k.size.y - 4.0) * self.t;
        self.draw_cell.color = color(p.accent, 1.0);
        self.draw_cell.draw_abs(cx, Rect { pos: dvec2(k.pos.x + 2.0, k.pos.y + k.size.y - 2.0 - fill), size: dvec2(k.size.x - 4.0, fill.max(0.0)) });
        self.draw_text.color = color(p.text_bright, 1.0);
        self.draw_text.draw_abs(cx, dvec2(k.pos.x + 22.0, rect.pos.y + 6.0), &format!("{:.0}%", self.t * 100.0));
        let names: Vec<String> = targets.iter().map(|(path, _, _)| host::label_of(path).to_lowercase()).collect();
        let said = format!("{} values · {}", names.len(), names.join(", "));
        let (said, _) = fit_text(&self.draw_small, cx, &said, (rect.pos.x + rect.size.x - k.pos.x - 22.0).max(20.0));
        self.draw_small.color = color(p.muted, 1.0);
        self.draw_small.draw_abs(cx, dvec2(k.pos.x + 22.0, rect.pos.y + 23.0), &said);
        DrawStep::done()
    }
}

// ---------------------------------------------------------------- meter

#[derive(Script, ScriptHook, Widget)]
pub struct PanelMeter {
    /// When in the film it acts ("beats 9-16"): the host reads it from the
    /// panel's code.
    #[live]
    pub when: String,
    #[rust]
    place: Area,
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
    draw_small: DrawText,
    #[live]
    pub path: String,
    #[live]
    pub label: String,
    /// The scale (both 0: grows to what it has seen).
    #[live]
    pub min: f64,
    #[live]
    pub max: f64,
    /// The trace: (playhead time, value).
    #[rust]
    trace: VecDeque<(f64, f64)>,
    #[rust]
    seen: (f64, f64),
}

impl Widget for PanelMeter {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let p = makepad_look::palette::get(cx);
        let rect = cx.walk_turtle(walk);
        let (uid, paths) = (self.widget_uid().0, vec![self.path.clone()]);
        host::placed(cx, &mut self.place, uid, rect, paths);
        self.draw_bg.color = color(0, 0.0);
        self.draw_bg.draw_abs(cx, rect);
        self.draw_text.color = color(p.label, 1.0);
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, rect.pos.y + 4.0), &label_or(&self.label, &self.path));
        let live = host::live(&self.path);
        let head = host::playhead();
        if let (Some(v), Some((t, _, playing))) = (live, head) {
            // A jump back (a scrub, a loop) starts the trace again.
            if self.trace.back().is_some_and(|(tl, _)| t < *tl) {
                self.trace.clear();
            }
            if self.trace.back().is_none_or(|(tl, _)| t > *tl) {
                self.trace.push_back((t, v));
            }
            while self.trace.front().is_some_and(|(t0, _)| t - t0 > 4.0) {
                self.trace.pop_front();
            }
            self.seen = (self.seen.0.min(v), self.seen.1.max(v));
            if playing {
                cx.redraw_area_in_draw(self.draw_bg.area());
            }
        }
        let (lo, hi) = if self.max > self.min { (self.min, self.max) } else if self.seen.1 > self.seen.0 { self.seen } else { (0.0, 1.0) };
        let t_of = |v: f64| if hi > lo { ((v - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 };
        let area = Rect { pos: dvec2(rect.pos.x + LABEL, rect.pos.y + 2.0), size: dvec2(rect.size.x - LABEL - 56.0, rect.size.y - 4.0) };
        self.draw_cell.color = color(p.well, 1.0);
        self.draw_cell.draw_abs(cx, area);
        // The trace, as dots over its last four seconds.
        if let Some(&(t_end, _)) = self.trace.back() {
            self.draw_cell.color = color(p.accent, 0.8);
            for (t, v) in &self.trace {
                let x = area.pos.x + area.size.x * (1.0 - (t_end - t) / 4.0);
                let y = area.pos.y + area.size.y * (1.0 - t_of(*v));
                self.draw_cell.draw_abs(cx, Rect { pos: dvec2(x - 1.0, y - 1.0), size: dvec2(2.0, 2.0) });
            }
        }
        // The bar at the right edge: the value now.
        let now = live.unwrap_or(0.0);
        let bar = Rect { pos: dvec2(area.pos.x + area.size.x + 6.0, area.pos.y), size: dvec2(6.0, area.size.y) };
        self.draw_cell.color = color(p.well, 1.0);
        self.draw_cell.draw_abs(cx, bar);
        let h = bar.size.y * t_of(now);
        self.draw_cell.color = color(p.accent, 1.0);
        self.draw_cell.draw_abs(cx, Rect { pos: dvec2(bar.pos.x, bar.pos.y + bar.size.y - h), size: dvec2(bar.size.x, h) });
        self.draw_small.color = color(p.text_bright, 1.0);
        let said = match live {
            Some(v) => fmt_number(v, 3),
            None => "—".to_string(),
        };
        self.draw_small.draw_abs(cx, dvec2(bar.pos.x + 10.0, rect.pos.y + 4.0), &said);
        DrawStep::done()
    }
}

// ---------------------------------------------------------------- envelope

/// The envelope's corners a drag moves.
#[derive(Clone, Copy, Debug, PartialEq)]
enum EnvHandle {
    /// Attack's end (x: attack).
    Peak,
    /// Decay's end (x: decay, y: sustain).
    Sustain,
    /// Release's end (x: release).
    End,
}

#[derive(Script, ScriptHook, Widget)]
pub struct PanelEnvelope {
    /// When in the film it acts ("beats 9-16"): the host reads it from the
    /// panel's code.
    #[live]
    pub when: String,
    #[rust]
    place: Area,
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
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
    draw_small: DrawText,
    #[live]
    pub label: String,
    /// The four paths: times (attack, decay, release) and a level (sustain).
    #[live]
    pub attack: String,
    #[live]
    pub decay: String,
    #[live]
    pub sustain: String,
    #[live]
    pub release: String,
    /// Where the shape sits on the song (seconds; the song drawn under it
    /// when the host has it). `at: 0` and no length: the span the envelope
    /// takes.
    #[live]
    pub at: f64,
    #[rust]
    adsr: [f64; 4],
    #[rust]
    codes: [String; 4],
    #[rust]
    seen: u64,
    #[rust]
    area: Rect,
    #[rust]
    dragging: Option<EnvHandle>,
}

impl PanelEnvelope {
    pub(crate) fn paths(&self) -> [&str; 4] {
        [&self.attack, &self.decay, &self.sustain, &self.release]
    }

    /// The span drawn: the envelope's own length with room after it.
    fn span(&self) -> f64 {
        let [a, d, _, r] = self.adsr;
        ((a + d + r) * 1.6).max(0.2)
    }

    fn corners(&self) -> [DVec2; 5] {
        let [a, d, s, r] = self.adsr;
        let span = self.span();
        let x = |t: f64| self.area.pos.x + self.area.size.x * (t / span);
        let y = |l: f64| self.area.pos.y + self.area.size.y * (1.0 - l.clamp(0.0, 1.2) / 1.2);
        // The sustain holds for a fifth of the span, so release has room.
        let hold = span * 0.2;
        [dvec2(x(0.0), y(0.0)), dvec2(x(a), y(1.0)), dvec2(x(a + d), y(s)), dvec2(x(a + d + hold), y(s)), dvec2(x(a + d + hold + r), y(0.0))]
    }

    fn drag_to(&mut self, cx: &mut Cx, h: EnvHandle, abs: DVec2) {
        let span = self.span();
        let t = ((abs.x - self.area.pos.x) / self.area.size.x).clamp(0.0, 1.0) * span;
        let l = (1.0 - (abs.y - self.area.pos.y) / self.area.size.y) * 1.2;
        let [a, d, _, _] = self.adsr;
        match h {
            EnvHandle::Peak => self.adsr[0] = t.max(0.0),
            EnvHandle::Sustain => {
                self.adsr[1] = (t - a).max(0.0);
                self.adsr[2] = l.clamp(0.0, 1.0);
            }
            EnvHandle::End => self.adsr[3] = (t - a - d - span * 0.2).max(0.0),
        }
        for k in 0..4 {
            let path = self.paths()[k].to_string();
            if path.is_empty() {
                continue;
            }
            let code = if numbers_in(&self.codes[k]).is_empty() { fmt_number(self.adsr[k], 3) } else { with_numbers(&self.codes[k], &[self.adsr[k]], 3) };
            if code != self.codes[k] {
                self.codes[k] = code.clone();
                host::set(&path, PanelValue::Code(code));
            }
        }
        self.draw_bg.redraw(cx);
    }
}

impl Widget for PanelEnvelope {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let near = |s: &Self, abs: DVec2| {
            let c = s.corners();
            [(c[1], EnvHandle::Peak), (c[2], EnvHandle::Sustain), (c[4], EnvHandle::End)].into_iter().filter(|(p, _)| (p.x - abs.x).hypot(p.y - abs.y) < 12.0).map(|(_, h)| h).next()
        };
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => cx.set_cursor(if near(self, fe.abs).is_some() { MouseCursor::Move } else { MouseCursor::Default }),
            Hit::FingerDown(fe) if fe.is_primary_hit() => self.dragging = near(self, fe.abs),
            Hit::FingerMove(fe) => {
                if let Some(h) = self.dragging {
                    self.drag_to(cx, h, fe.abs);
                }
            }
            Hit::FingerUp(_) => self.dragging = None,
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rev = host::revision();
        if self.seen != rev && self.dragging.is_none() {
            self.seen = rev;
            for k in 0..4 {
                if let Some(code) = host::read(self.paths()[k]).map(|v| v.code()) {
                    self.adsr[k] = host::number_with_unit(&code).map(|(v, _)| v).unwrap_or(0.0);
                    self.codes[k] = code;
                }
            }
        }
        for k in 0..4 {
            let path = self.paths()[k].to_string();
            let kind = if k == 2 { BoundKind::Numbers { lo: 0.0, hi: 1.0, step: 0.01 } } else { BoundKind::Numbers { lo: 0.0, hi: 2.0, step: 0.01 } };
            host::register(Bound { path, kind });
        }
        let p = makepad_look::palette::get(cx);
        let rect = cx.walk_turtle(walk);
        let (uid, paths) = (self.widget_uid().0, self.paths().iter().map(|s| s.to_string()).collect());
        host::placed(cx, &mut self.place, uid, rect, paths);
        self.draw_bg.color = color(0, 0.0);
        self.draw_bg.draw_abs(cx, rect);
        self.draw_text.color = color(p.label, 1.0);
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, rect.pos.y + 4.0), if self.label.is_empty() { "Envelope" } else { &self.label });
        self.area = Rect { pos: dvec2(rect.pos.x + LABEL, rect.pos.y + 2.0), size: dvec2(rect.size.x - LABEL, rect.size.y - 18.0) };
        let area = self.area;
        self.draw_cell.color = color(p.well, 1.0);
        self.draw_cell.draw_abs(cx, area);
        // The song under it, where the host has it.
        let peaks = host::song(self.at, self.at + self.span(), (area.size.x / 2.0) as usize);
        if !peaks.is_empty() {
            self.draw_cell.color = color(p.line, 1.0);
            let w = area.size.x / peaks.len() as f64;
            for (i, v) in peaks.iter().enumerate() {
                let h = area.size.y * (*v as f64).clamp(0.0, 1.0) * 0.8;
                self.draw_cell.draw_abs(cx, Rect { pos: dvec2(area.pos.x + i as f64 * w, area.pos.y + (area.size.y - h) * 0.5), size: dvec2(w.max(1.0), h) });
            }
        }
        // The shape as dots along its four segments, the corners as handles.
        let c = self.corners();
        self.draw_cell.color = color(p.accent, 1.0);
        for seg in 0..4 {
            let (a, b) = (c[seg], c[seg + 1]);
            let steps = ((b - a).length() / 2.5).ceil().max(1.0) as usize;
            for k in 0..steps {
                let q = a + (b - a) * (k as f64 / steps as f64);
                self.draw_cell.draw_abs(cx, Rect { pos: q - dvec2(1.0, 1.0), size: dvec2(2.0, 2.0) });
            }
        }
        for (i, q) in c.iter().enumerate().filter(|(i, _)| matches!(i, 1 | 2 | 4)) {
            let on = matches!((i, self.dragging), (1, Some(EnvHandle::Peak)) | (2, Some(EnvHandle::Sustain)) | (4, Some(EnvHandle::End)));
            self.draw_cell.color = color(if on { p.text_hi } else { p.accent }, 1.0);
            self.draw_cell.draw_abs(cx, Rect { pos: *q - dvec2(4.5, 4.5), size: dvec2(9.0, 9.0) });
        }
        let [a, d, s, r] = self.adsr;
        self.draw_small.color = color(p.muted, 1.0);
        self.draw_small.draw_abs(cx, dvec2(area.pos.x, area.pos.y + area.size.y + 3.0), &format!("A {}  D {}  S {}  R {}", fmt_number(a, 3), fmt_number(d, 3), fmt_number(s, 2), fmt_number(r, 3)));
        DrawStep::done()
    }
}

// ---------------------------------------------------------------- anchor

#[derive(Script, ScriptHook, Widget)]
pub struct PanelAnchor {
    /// When in the film it acts ("beats 9-16"): the host reads it from the
    /// panel's code.
    #[live]
    pub when: String,
    #[rust]
    place: Area,
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
    draw_small: DrawText,
    #[live]
    pub path: String,
    #[live]
    pub label: String,
    /// The span shown (seconds; both 0: the whole film).
    #[live]
    pub from: f64,
    #[live]
    pub to: f64,
    #[rust]
    code: String,
    #[rust]
    seen: u64,
    #[rust]
    area: Rect,
    #[rust]
    span: (f64, f64),
    #[rust]
    beats: Vec<f64>,
    #[rust]
    words: Vec<(f64, f64, String)>,
    #[rust]
    hot: Option<f64>,
}

impl PanelAnchor {
    fn t_at(&self, x: f64) -> f64 {
        self.span.0 + (self.span.1 - self.span.0) * ((x - self.area.pos.x) / self.area.size.x).clamp(0.0, 1.0)
    }

    fn x_at(&self, t: f64) -> f64 {
        self.area.pos.x + self.area.size.x * ((t - self.span.0) / (self.span.1 - self.span.0).max(1e-9))
    }

    /// What a click at `x` snaps to: the nearest beat or word start, as
    /// code in the value's form, and its time.
    fn snap(&self, x: f64) -> Option<(String, f64)> {
        let t = self.t_at(x);
        let near_beat = self.beats.iter().enumerate().min_by(|a, b| (a.1 - t).abs().total_cmp(&(b.1 - t).abs()));
        let near_word = self.words.iter().min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs()));
        let px = |tt: f64| (self.x_at(tt) - x).abs();
        let code = self.code.trim();
        match (near_beat, near_word) {
            (_, Some(w)) if code.starts_with("W.") || near_beat.is_none_or(|b| px(w.0) < px(*b.1)) => {
                let word: String = w.2.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect();
                if code.starts_with("W.") && !word.is_empty() {
                    Some((format!("W.{word}.start"), w.0))
                } else {
                    Some((self.seconds(w.0), w.0))
                }
            }
            (Some((i, bt)), _) => {
                if code.starts_with("beat(") {
                    Some((format!("beat({i})"), *bt))
                } else {
                    Some((self.seconds(*bt), *bt))
                }
            }
            _ => Some((self.seconds(t), t)),
        }
    }

    fn seconds(&self, t: f64) -> String {
        let unit = host::number_with_unit(&self.code).map(|(_, u)| u).unwrap_or_default();
        if unit == "ms" { format!("{}ms", fmt_number(t * 1000.0, 0)) } else { format!("{}{}", fmt_number(t, 3), if unit == "s" { "s" } else { "" }) }
    }
}

impl Widget for PanelAnchor {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                if self.area.contains(fe.abs) {
                    cx.set_cursor(MouseCursor::Hand);
                    self.hot = self.snap(fe.abs.x).map(|(_, t)| t);
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hot = None;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerUp(fe) if fe.is_over && fe.is_primary_hit() && self.area.contains(fe.abs) => {
                if let Some((code, _)) = self.snap(fe.abs.x) {
                    self.code = code.clone();
                    host::set(&self.path, PanelValue::Code(code));
                    self.draw_bg.redraw(cx);
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rev = host::revision();
        if self.seen != rev {
            self.seen = rev;
            if let Some(code) = host::read(&self.path).map(|v| v.code()) {
                self.code = code;
            }
        }
        host::register(Bound { path: self.path.clone(), kind: BoundKind::Other });
        let duration = host::playhead().map(|p| p.1).unwrap_or(10.0);
        self.span = if self.to > self.from { (self.from, self.to) } else { (0.0, duration.max(0.1)) };
        self.beats = host::beats(self.span.0, self.span.1);
        self.words = host::words(self.span.0, self.span.1);
        let live = host::live(&self.path);
        let p = makepad_look::palette::get(cx);
        let rect = cx.walk_turtle(walk);
        let (uid, paths) = (self.widget_uid().0, vec![self.path.clone()]);
        host::placed(cx, &mut self.place, uid, rect, paths);
        self.draw_bg.color = color(0, 0.0);
        self.draw_bg.draw_abs(cx, rect);
        self.draw_text.color = color(p.label, 1.0);
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, rect.pos.y + 4.0), &label_or(&self.label, &self.path));
        self.area = Rect { pos: dvec2(rect.pos.x + LABEL, rect.pos.y + 2.0), size: dvec2(rect.size.x - LABEL, rect.size.y - 18.0) };
        let area = self.area;
        self.draw_cell.color = color(p.well, 1.0);
        self.draw_cell.draw_abs(cx, area);
        let peaks = host::song(self.span.0, self.span.1, (area.size.x / 2.0) as usize);
        if !peaks.is_empty() {
            self.draw_cell.color = color(p.line, 1.0);
            let w = area.size.x / peaks.len() as f64;
            for (i, v) in peaks.iter().enumerate() {
                let h = area.size.y * (*v as f64).clamp(0.0, 1.0) * 0.7;
                self.draw_cell.draw_abs(cx, Rect { pos: dvec2(area.pos.x + i as f64 * w, area.pos.y + (area.size.y - h) * 0.5), size: dvec2(w.max(1.0), h) });
            }
        }
        // Beats: a tick each, every fourth taller.
        for (i, b) in self.beats.clone().iter().enumerate() {
            let x = self.x_at(*b);
            let h = if i % 4 == 0 { area.size.y } else { area.size.y * 0.35 };
            self.draw_cell.color = color(p.muted, if i % 4 == 0 { 0.7 } else { 0.5 });
            self.draw_cell.draw_abs(cx, Rect { pos: dvec2(x, area.pos.y + area.size.y - h), size: dvec2(1.0, h) });
        }
        // Words: their spans along the top, named where they fit.
        for (t0, t1, w) in self.words.clone() {
            let (x0, x1) = (self.x_at(t0), self.x_at(t1));
            self.draw_cell.color = color(p.accent, 0.25);
            self.draw_cell.draw_abs(cx, Rect { pos: dvec2(x0, area.pos.y + 1.0), size: dvec2((x1 - x0).max(1.0), 12.0) });
            if x1 - x0 > 18.0 {
                self.draw_small.color = color(p.text, 1.0);
                self.draw_small.draw_abs(cx, dvec2(x0 + 2.0, area.pos.y + 1.0), &w);
            }
        }
        // The value's time, and where a click would put it.
        let at = live.or_else(|| host::number_with_unit(&self.code).map(|(v, u)| if u == "ms" { v / 1000.0 } else { v }));
        if let Some(t) = at {
            let x = self.x_at(t);
            self.draw_cell.color = color(p.accent, 1.0);
            self.draw_cell.draw_abs(cx, Rect { pos: dvec2(x - 1.0, area.pos.y), size: dvec2(2.0, area.size.y) });
        }
        if let Some(t) = self.hot {
            let x = self.x_at(t);
            self.draw_cell.color = color(p.text_hi, 0.6);
            self.draw_cell.draw_abs(cx, Rect { pos: dvec2(x, area.pos.y), size: dvec2(1.0, area.size.y) });
        }
        self.draw_small.color = color(p.muted, 1.0);
        self.draw_small.draw_abs(cx, dvec2(area.pos.x, area.pos.y + area.size.y + 3.0), &self.code);
        DrawStep::done()
    }
}

// ---------------------------------------------------------------- keys

#[derive(Script, ScriptHook, Widget)]
pub struct PanelKeys {
    /// When in the film it acts ("beats 9-16"): the host reads it from the
    /// panel's code.
    #[live]
    pub when: String,
    #[rust]
    place: Area,
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
    draw_small: DrawText,
    #[live]
    pub path: String,
    #[live]
    pub label: String,
    #[rust]
    code: String,
    /// Each key's time and value (the first two numbers of each).
    #[rust]
    keys: Vec<(f64, f64)>,
    /// Every number of the value, and which of them each key's are.
    #[rust]
    numbers: Vec<f64>,
    #[rust]
    at: Vec<usize>,
    #[rust]
    seen: u64,
    #[rust]
    area: Rect,
    #[rust]
    bounds: [f64; 4],
    #[rust]
    dragging: Option<usize>,
}

impl PanelKeys {
    /// The keys of a value: each bracketed group's first two numbers.
    fn parse(&mut self) {
        self.numbers = numbers_in(&self.code);
        self.keys.clear();
        self.at.clear();
        // Count numbers group by group: a `[` opens a key (its first two
        // numbers), nested brackets beyond the first level are ignored.
        let mut depth = 0i32;
        let mut index = 0usize;
        let mut in_key: Option<(usize, usize)> = None;
        let b = self.code.as_bytes();
        let mut i = 0;
        let start_depth = if self.code.trim_start().starts_with("keys(") { 1 } else { 2 };
        while i < b.len() {
            let c = b[i];
            let is_num = c.is_ascii_digit() || (c == b'-' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit() || *d == b'.')) || (c == b'.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit()));
            let in_word = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'#');
            match c {
                b'[' | b'(' => {
                    depth += 1;
                    if depth == start_depth {
                        in_key = Some((index, 0));
                    }
                }
                b']' | b')' => {
                    if depth == start_depth {
                        if let Some((first, n)) = in_key.take() {
                            if n >= 2 {
                                self.at.push(first);
                                self.keys.push((self.numbers[first], self.numbers[first + 1]));
                            }
                        }
                    }
                    depth -= 1;
                }
                _ if is_num && !in_word => {
                    while i + 1 < b.len() && (b[i + 1].is_ascii_digit() || b[i + 1] == b'.' || b[i + 1] == b'e') {
                        i += 1;
                    }
                    if let Some((_, n)) = &mut in_key {
                        *n += 1;
                    }
                    index += 1;
                }
                _ => {}
            }
            i += 1;
        }
        let ts: Vec<f64> = self.keys.iter().map(|k| k.0).collect();
        let vs: Vec<f64> = self.keys.iter().map(|k| k.1).collect();
        let (t1, v0, v1) = (ts.iter().cloned().fold(0.0, f64::max), vs.iter().cloned().fold(f64::MAX, f64::min), vs.iter().cloned().fold(f64::MIN, f64::max));
        let (v0, v1) = if v0 > v1 { (0.0, 1.0) } else if v1 - v0 < 1e-9 { (v0 - 1.0, v1 + 1.0) } else { let m = (v1 - v0) * 0.2; (v0 - m, v1 + m) };
        self.bounds = [0.0, (t1 * 1.15).max(0.1), v0, v1];
    }

    fn screen(&self, k: (f64, f64)) -> DVec2 {
        let [t0, t1, v0, v1] = self.bounds;
        dvec2(self.area.pos.x + self.area.size.x * (k.0 - t0) / (t1 - t0), self.area.pos.y + self.area.size.y * (1.0 - (k.1 - v0) / (v1 - v0)))
    }
}

impl Widget for PanelKeys {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let near = |s: &Self, abs: DVec2| (0..s.keys.len()).find(|i| { let q = s.screen(s.keys[*i]); (q.x - abs.x).hypot(q.y - abs.y) < 10.0 });
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => cx.set_cursor(if near(self, fe.abs).is_some() { MouseCursor::Move } else { MouseCursor::Default }),
            Hit::FingerDown(fe) if fe.is_primary_hit() => self.dragging = near(self, fe.abs),
            Hit::FingerMove(fe) => {
                if let Some(i) = self.dragging {
                    let [t0, t1, v0, v1] = self.bounds;
                    let t = (t0 + (t1 - t0) * ((fe.abs.x - self.area.pos.x) / self.area.size.x)).max(0.0);
                    let v = v0 + (v1 - v0) * (1.0 - (fe.abs.y - self.area.pos.y) / self.area.size.y);
                    // A key stays between its neighbours in time.
                    let lo = if i > 0 { self.keys[i - 1].0 } else { 0.0 };
                    let hi = self.keys.get(i + 1).map_or(f64::MAX, |k| k.0);
                    let t = (t.clamp(lo, hi) * 1000.0).round() / 1000.0;
                    let v = (v * 1000.0).round() / 1000.0;
                    self.keys[i] = (t, v);
                    let first = self.at[i];
                    self.numbers[first] = t;
                    self.numbers[first + 1] = v;
                    self.code = with_numbers(&self.code, &self.numbers, 3);
                    host::set(&self.path, PanelValue::Code(self.code.clone()));
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerUp(_) => self.dragging = None,
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rev = host::revision();
        if self.seen != rev && self.dragging.is_none() {
            self.seen = rev;
            if let Some(code) = host::read(&self.path).map(|v| v.code()) {
                self.code = code;
                self.parse();
            }
        }
        host::register(Bound { path: self.path.clone(), kind: BoundKind::Other });
        let p = makepad_look::palette::get(cx);
        let rect = cx.walk_turtle(walk);
        let (uid, paths) = (self.widget_uid().0, vec![self.path.clone()]);
        host::placed(cx, &mut self.place, uid, rect, paths);
        self.draw_bg.color = color(0, 0.0);
        self.draw_bg.draw_abs(cx, rect);
        self.draw_text.color = color(p.label, 1.0);
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, rect.pos.y + 4.0), &label_or(&self.label, &self.path));
        self.area = Rect { pos: dvec2(rect.pos.x + LABEL, rect.pos.y + 2.0), size: dvec2(rect.size.x - LABEL, rect.size.y - 16.0) };
        self.draw_cell.color = color(p.well, 1.0);
        self.draw_cell.draw_abs(cx, self.area);
        // The playhead, where the host says.
        if let Some((t, _, playing)) = host::playhead() {
            let x = self.screen((t, 0.0)).x;
            if x >= self.area.pos.x && x <= self.area.pos.x + self.area.size.x {
                self.draw_cell.color = color(p.text_hi, 0.4);
                self.draw_cell.draw_abs(cx, Rect { pos: dvec2(x, self.area.pos.y), size: dvec2(1.0, self.area.size.y) });
            }
            if playing {
                cx.redraw_area_in_draw(self.draw_bg.area());
            }
        }
        let pts: Vec<DVec2> = self.keys.iter().map(|k| self.screen(*k)).collect();
        self.draw_cell.color = color(p.accent, 0.6);
        for w in pts.windows(2) {
            let steps = ((w[1] - w[0]).length() / 3.0).ceil().max(1.0) as usize;
            for k in 0..steps {
                let q = w[0] + (w[1] - w[0]) * (k as f64 / steps as f64);
                self.draw_cell.draw_abs(cx, Rect { pos: q - dvec2(0.75, 0.75), size: dvec2(1.5, 1.5) });
            }
        }
        for (i, q) in pts.iter().enumerate() {
            // A key drawn as a diamond-ish square.
            self.draw_cell.color = color(if self.dragging == Some(i) { p.text_hi } else { p.accent }, 1.0);
            self.draw_cell.draw_abs(cx, Rect { pos: *q - dvec2(4.0, 4.0), size: dvec2(8.0, 8.0) });
        }
        self.draw_small.color = color(p.muted, 1.0);
        let said = match self.dragging {
            Some(i) => format!("key {} · t {} · {}", i + 1, fmt_number(self.keys[i].0, 3), fmt_number(self.keys[i].1, 3)),
            None => format!("{} keys", self.keys.len()),
        };
        self.draw_small.draw_abs(cx, dvec2(self.area.pos.x, self.area.pos.y + self.area.size.y + 2.0), &said);
        DrawStep::done()
    }
}

// ---------------------------------------------------------------- modulation

/// The modulation sources, as `modulate` names them ("off": none).
const SOURCES: [&str; 6] = ["off", "lfo", "beat", "word", "noise", "time"];

#[derive(Clone, Copy, Debug, PartialEq)]
enum ModSpot {
    Source(usize),
    Rate,
    Depth,
}

#[derive(Script, ScriptHook, Widget)]
pub struct PanelMod {
    /// When in the film it acts ("beats 9-16"): the host reads it from the
    /// panel's code.
    #[live]
    pub when: String,
    #[rust]
    place: Area,
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
    draw_small: DrawText,
    #[live]
    pub path: String,
    #[live]
    pub label: String,
    #[rust]
    code: String,
    /// The value without modulation, the source, its rate and depth.
    #[rust]
    base: String,
    #[rust]
    source: usize,
    #[rust]
    rate: f64,
    #[rust]
    depth: f64,
    /// Kept as read (`modulate`'s phase, 0 when not written).
    #[rust]
    phase: f64,
    #[rust]
    seen: u64,
    #[rust]
    spots: Vec<(Rect, ModSpot)>,
    #[rust]
    dragging: Option<(ModSpot, DVec2, f64)>,
}

impl PanelMod {
    /// Read a value: `modulate(base, @src, {rate: r depth: d phase: p})`, else a
    /// plain value (no modulation).
    fn parse(&mut self) {
        let t = self.code.trim();
        if let Some(inner) = t.strip_prefix("modulate(").and_then(|r| r.strip_suffix(')')) {
            let mut depth = 0i32;
            let mut split = None;
            for (i, c) in inner.char_indices() {
                match c {
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' => depth -= 1,
                    ',' if depth == 0 => {
                        split = Some(i);
                        break;
                    }
                    _ => {}
                }
            }
            if let Some(i) = split {
                self.base = inner[..i].trim().to_string();
                let rest = &inner[i + 1..];
                let src = rest.split(|c: char| c == ',' || c.is_whitespace()).find(|w| w.starts_with('@')).unwrap_or("@lfo");
                self.source = SOURCES.iter().position(|s| *s == &src[1..]).unwrap_or(1);
                let field = |name: &str| rest.find(name).and_then(|k| rest[k + name.len()..].trim_start().strip_prefix(':')).and_then(|r| host::number_with_unit(r.split(|c: char| c.is_whitespace() || c == '}').find(|w| !w.is_empty()).unwrap_or(""))).map(|(v, _)| v);
                self.rate = field("rate").unwrap_or(1.0);
                self.depth = field("depth").unwrap_or(0.0);
                self.phase = field("phase").unwrap_or(0.0);
                return;
            }
        }
        self.base = t.to_string();
        self.source = 0;
        self.phase = 0.0;
        if self.rate == 0.0 {
            self.rate = 1.0;
        }
    }

    /// `modulate` takes a plain number as its base (no unit, no expression).
    fn modulable(&self) -> bool {
        self.base.trim().parse::<f64>().is_ok()
    }

    fn write(&mut self) {
        self.code = if self.source == 0 {
            self.base.clone()
        } else {
            let phase = if self.phase != 0.0 { format!(" phase: {}", fmt_number(self.phase, 3)) } else { String::new() };
            format!("modulate({}, @{}, {{rate: {} depth: {}{phase}}})", self.base, SOURCES[self.source], fmt_number(self.rate, 3), fmt_number(self.depth, 3))
        };
        host::set(&self.path, PanelValue::Code(self.code.clone()));
    }
}

impl Widget for PanelMod {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let at = |s: &Self, abs: DVec2| s.spots.iter().find(|(r, _)| r.contains(abs)).map(|(_, x)| *x);
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => cx.set_cursor(match at(self, fe.abs) {
                Some(ModSpot::Source(_)) => MouseCursor::Hand,
                Some(_) => MouseCursor::EwResize,
                None => MouseCursor::Default,
            }),
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if let Some(spot @ (ModSpot::Rate | ModSpot::Depth)) = at(self, fe.abs) {
                    self.dragging = Some((spot, fe.abs, if spot == ModSpot::Rate { self.rate } else { self.depth }));
                }
            }
            Hit::FingerMove(fe) => {
                if let Some((spot, origin, start)) = self.dragging {
                    let d = fe.abs.x - origin.x;
                    match spot {
                        // Rate on a log scale (a twentieth of an octave a point).
                        ModSpot::Rate => self.rate = (start * 2f64.powf(d / 20.0)).clamp(0.01, 64.0),
                        // Depth in the base's units: a hundredth of the base (or of 1) a point.
                        _ => {
                            let base = host::numbers_in(&self.base).first().copied().unwrap_or(1.0).abs().max(0.01);
                            self.depth = start + d * base * 0.01;
                        }
                    }
                    if self.source != 0 {
                        self.write();
                    }
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerUp(fe) => {
                if self.dragging.take().is_none() && fe.is_over {
                    if let Some(ModSpot::Source(k)) = at(self, fe.abs).filter(|_| self.modulable()) {
                        self.source = k;
                        if k != 0 && self.depth == 0.0 {
                            // A depth to hear: a fifth of the base.
                            self.depth = host::numbers_in(&self.base).first().copied().unwrap_or(1.0).abs().max(0.01) * 0.2;
                        }
                        self.write();
                        self.draw_bg.redraw(cx);
                    }
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rev = host::revision();
        if self.seen != rev && self.dragging.is_none() {
            self.seen = rev;
            if let Some(code) = host::read(&self.path).map(|v| v.code()) {
                self.code = code;
                self.parse();
            }
        }
        let p = makepad_look::palette::get(cx);
        let rect = cx.walk_turtle(walk);
        let (uid, paths) = (self.widget_uid().0, vec![self.path.clone()]);
        host::placed(cx, &mut self.place, uid, rect, paths);
        self.draw_bg.color = color(0, 0.0);
        self.draw_bg.draw_abs(cx, rect);
        self.spots.clear();
        self.draw_text.color = color(p.label, 1.0);
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, rect.pos.y + 4.0), &format!("{} ~", label_or(&self.label, &self.path)));
        let mut x = rect.pos.x + LABEL;
        for (k, name) in SOURCES.iter().enumerate() {
            let w = makepad_widgets::badge::advance(&self.draw_small, cx, name) + 10.0;
            let r = Rect { pos: dvec2(x, rect.pos.y + 2.0), size: dvec2(w, 18.0) };
            let on = self.source == k;
            self.draw_cell.color = color(if on { p.accent } else { p.raised }, 1.0);
            self.draw_cell.draw_abs(cx, r);
            self.draw_small.color = color(if on { p.on_accent } else if k == 0 || self.modulable() { p.text } else { p.muted }, 1.0);
            self.draw_small.draw_abs(cx, dvec2(r.pos.x + 5.0, r.pos.y + 4.0), name);
            self.spots.push((r, ModSpot::Source(k)));
            x += w + 3.0;
        }
        if self.source != 0 {
            let y = rect.pos.y + 26.0;
            for (k, (name, v, spot)) in [("rate", format!("{} Hz", fmt_number(self.rate, 2)), ModSpot::Rate), ("depth", format!("± {}", fmt_number(self.depth, 3)), ModSpot::Depth)].into_iter().enumerate() {
                let r = Rect { pos: dvec2(rect.pos.x + LABEL + k as f64 * 110.0, y), size: dvec2(104.0, 20.0) };
                self.draw_cell.color = color(p.well, 1.0);
                self.draw_cell.draw_abs(cx, r);
                self.draw_small.color = color(p.muted, 1.0);
                self.draw_small.draw_abs(cx, dvec2(r.pos.x + 5.0, r.pos.y + 5.0), name);
                self.draw_small.color = color(p.text_bright, 1.0);
                self.draw_small.draw_abs(cx, dvec2(r.pos.x + 38.0, r.pos.y + 5.0), &v);
                self.spots.push((r, spot));
            }
        }
        DrawStep::done()
    }
}
