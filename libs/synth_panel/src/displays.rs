//! The synth's pictures for an editor panel, each bound to an instrument
//! node by name (`node: "mono"`) and drawn on a screen:
//!
//! - `SynthFilter`: the filter's response; drag sideways for the cutoff,
//!   up and down for the resonance.
//! - `SynthWave`: the oscillator's cycle; a click steps through the waves.
//! - `SynthScope`: what the node sounds like now (its loops' output while
//!   the score plays) with its level.
//!
//! (An envelope is the panel widgets' `PanelEnvelope` over the node's
//! attack, decay, sustain and release paths.)
//!
//! Each field is `node.field` (`mono.cutoff`) unless the widget names
//! another path (`cutoff: "acid.cutoff"`). Every
//! drag is held live (the sound follows) and written once at the release,
//! one undo step; a click on any of them tells the score's agent what you
//! are looking at.

use crate::dsp::{self, Ballistics, ROW};
use crate::host as synth;
use crate::shaders::DrawScreen;
use crate::spec::{self, ParamSpec};
use makepad_value_pickers::panel::host::{self, Bound, BoundKind, PanelValue};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SynthDisplayBase = #(SynthDisplay::register_widget(vm))
    /** An instrument's filter response (cutoff, resonance), dragged. */
    mod.widgets.SynthFilter = set_type_default() do mod.widgets.SynthDisplayBase{
        width: Fill height: 120 kind: 3
        draw_text +: {text_style: theme.font_bold{font_size: 7.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
    }
    /** An instrument's oscillator cycle; a click steps the wave. */
    mod.widgets.SynthWave = mod.widgets.SynthFilter{kind: 2 height: 90}
    /** What an instrument (or loop, or effect) sounds like now, and its level. */
    mod.widgets.SynthScope = mod.widgets.SynthFilter{kind: 1 height: 90}
}

fn color(value: u32, alpha: f32) -> Vec4f {
    let mut c = Vec4f::from_u32(value);
    c.w *= alpha;
    c
}

/// The fields each picture reads, in order.
const FILTER: [&str; 2] = ["cutoff", "res"];
const WAVE: [&str; 1] = ["osc"];

/// A picture's fields (up to four; unused ones stay at their default).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Fields<T: Copy + Default>([T; 4]);

impl<T: Copy + Default> std::ops::Index<usize> for Fields<T> {
    type Output = T;
    fn index(&self, i: usize) -> &T {
        &self.0[i]
    }
}

impl<T: Copy + Default> std::ops::IndexMut<usize> for Fields<T> {
    fn index_mut(&mut self, i: usize) -> &mut T {
        &mut self.0[i]
    }
}

/// A drag in progress: which handle, where it started, the values then.
#[derive(Clone, Copy, Debug)]
struct Drag {
    handle: usize,
    start: DVec2,
    from: Fields<f64>,
}

#[derive(Script, ScriptHook, Widget)]
pub struct SynthDisplay {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_screen: DrawScreen,
    #[live]
    draw_mark: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_small: DrawText,
    /// 1 scope, 2 wave, 3 filter.
    #[live]
    kind: f64,
    /// The instrument node (`mono`), a loop or an effect for a scope.
    #[live]
    node: String,
    #[live]
    label: String,
    /// Paths for the fields, when not `node.field`.
    #[live]
    cutoff: String,
    #[live]
    res: String,
    #[live]
    osc: String,
    #[rust]
    values: Fields<f64>,
    /// Whether each field is written (else its default shows, dimmed).
    #[rust]
    written: Fields<bool>,
    #[rust]
    wave: String,
    #[rust]
    seen: u64,
    #[rust]
    drag: Option<Drag>,
    #[rust]
    hover: usize,
    #[rust]
    plot: Rect,
    #[rust]
    texture: Option<Texture>,
    #[rust]
    row: Vec<f32>,
    #[rust]
    scope: Vec<f32>,
    #[rust]
    scope_gain: f32,
    #[rust]
    level: Ballistics,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_time: f64,
}

impl SynthDisplay {
    fn fields(&self) -> &'static [&'static str] {
        match self.kind.round() as i32 {
            3 => &FILTER,
            2 => &WAVE,
            _ => &[],
        }
    }

    fn path(&self, field: &str) -> String {
        let given = match field {
            "cutoff" => &self.cutoff,
            "res" => &self.res,
            "osc" => &self.osc,
            _ => "",
        };
        if given.is_empty() { format!("{}.{field}", self.node) } else { given.to_string() }
    }

    fn spec(&self, field: &str) -> ParamSpec {
        let ty = synth::type_of(&self.node).unwrap_or_else(|| "Synth".into());
        // A shader instrument's fields read as a synth's of the same name.
        spec::spec(&ty, field).or_else(|| spec::spec("Synth", field)).unwrap_or_else(|| ParamSpec::generic(field, None))
    }

    /// The fields as written (held values while a gesture runs).
    fn read(&mut self) {
        for (i, field) in self.fields().iter().enumerate() {
            let path = self.path(field);
            match host::read(&path) {
                Some(PanelValue::Number(n)) => {
                    self.values[i] = n;
                    self.written[i] = true;
                }
                Some(PanelValue::Code(code)) if *field == "osc" => {
                    self.wave = code.trim().trim_start_matches('@').to_string();
                    self.written[i] = true;
                }
                Some(PanelValue::Code(code)) => match code.trim().parse::<f64>() {
                    Ok(n) => {
                        self.values[i] = n;
                        self.written[i] = true;
                    }
                    Err(_) => self.written[i] = false,
                },
                _ => {
                    self.values[i] = self.spec(field).default;
                    self.written[i] = false;
                    if *field == "osc" {
                        self.wave = "saw".into();
                    }
                }
            }
        }
    }

    fn set(&mut self, i: usize, value: f64) {
        let field = self.fields()[i];
        let spec = self.spec(field);
        let value = value.clamp(spec.min, spec.max);
        self.values[i] = value;
        host::set(&self.path(field), PanelValue::Code(spec.source_text(value)));
    }

    /// Where the handles are on the plot: (x, y) 0..1, y up.
    fn handles(&self) -> Vec<(f64, f64)> {
        match self.kind.round() as i32 {
            3 => {
                let spec = self.spec("res");
                vec![(dsp::freq_x(self.values[0] as f32) as f64, spec.norm(self.values[1]))]
            }
            _ => Vec::new(),
        }
    }

    fn handle_at(&self, abs: DVec2) -> usize {
        let p = self.plot;
        let mut best = (0usize, 16.0f64);
        for (i, (u, v)) in self.handles().iter().enumerate() {
            let at = dvec2(p.pos.x + u * p.size.x, p.pos.y + (1.0 - v) * p.size.y);
            let d = (at - abs).length();
            if d < best.1 {
                best = (i + 1, d);
            }
        }
        best.0
    }

    fn drag_to(&mut self, drag: Drag, abs: DVec2) {
        let p = self.plot;
        let dx = (abs.x - drag.start.x) / p.size.x.max(1.0);
        let dy = (drag.start.y - abs.y) / p.size.y.max(1.0);
        match (self.kind.round() as i32, drag.handle) {
            (3, _) => {
                let (cut, res) = (self.spec("cutoff"), self.spec("res"));
                self.set(0, cut.denorm(cut.norm(drag.from[0]) + dx));
                self.set(1, res.denorm(res.norm(drag.from[1]) + dy));
            }
            _ => {}
        }
    }

    /// The trace for the screen, into `row`.
    fn trace(&mut self, cx: &mut Cx2d) {
        self.row.resize(ROW, 0.0);
        let v = self.values;
        match self.kind.round() as i32 {
            3 => {
                let mode = match host::read(&format!("{}.filter", self.node)).map(|v| v.code()) {
                    Some(code) => match code.trim().trim_start_matches('@') {
                        "bp" | "bandpass" => 2,
                        "hp" | "highpass" => 3,
                        "lp" | "lowpass" => 1,
                        "notch" => 4,
                        _ => 0,
                    },
                    None => 0,
                };
                dsp::filter_row(&mut self.row, mode, v[0] as f32, v[1] as f32);
            }
            2 => {
                let fm = host::read(&format!("{}.fm", self.node)).and_then(|v| v.number()).map(|(r, _)| {
                    let index = host::read(&format!("{}.index", self.node)).and_then(|v| v.number()).map_or(2.0, |(i, _)| i);
                    (r as f32, index as f32)
                });
                let width = host::read(&format!("{}.width", self.node)).and_then(|v| v.number()).map_or(0.5, |(w, _)| w) as f32;
                dsp::wave_row(&mut self.row, &self.wave, width, fm);
            }
            _ => {
                // The node's newest output, ranged as a bench scope's auto-set.
                const SCOPE_N: usize = 1024;
                self.scope.resize(SCOPE_N, 0.0);
                let live = synth::live();
                let now = cx.seconds_since_app_start();
                let dt = (now - self.last_time).clamp(0.0, 0.2) as f32;
                self.last_time = now;
                let heard = live.as_ref().is_some_and(|l| l.scope(&self.node, &mut self.scope));
                if heard {
                    let peak = self.scope.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                    let want = 0.85 / peak.max(0.04);
                    self.scope_gain += (want - self.scope_gain) * if want < self.scope_gain { 0.5 } else { 0.05 };
                    dsp::scope_row(&mut self.row, &self.scope, SCOPE_N / ROW, self.scope_gain);
                } else {
                    self.row.fill(0.5);
                }
                let (peak, rms) = live.as_ref().and_then(|l| l.level(&self.node)).unwrap_or((0.0, 0.0));
                self.level.feed(peak, rms, dt);
            }
        }
    }

    fn upload(&mut self, cx: &mut Cx2d) {
        let data = self.row.clone();
        match &self.texture {
            Some(t) => {
                let mut old = t.take_vec_f32(cx);
                old.clear();
                old.extend_from_slice(&data);
                t.put_back_vec_f32(cx, old, None);
            }
            None => {
                self.texture = Some(Texture::new_with_format(cx, TextureFormat::VecRf32 { width: ROW, height: 1, data: Some(data), updated: TextureUpdated::Full }));
            }
        }
        if let (Some(t), Some(shader)) = (&self.texture, self.draw_screen.draw_vars.draw_shader_id) {
            if let Some(slot) = cx.draw_shaders[shader.index].mapping.textures.iter().position(|t| t.id == live_id!(data)) {
                self.draw_screen.draw_vars.set_texture(slot, t);
            }
        }
    }
}

impl Widget for SynthDisplay {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            self.redraw(cx);
        }
        let kind = self.kind.round() as i32;
        match event.hits(cx, self.draw_screen.area()) {
            Hit::FingerHoverOver(fe) | Hit::FingerHoverIn(fe) => {
                let hover = self.handle_at(fe.abs);
                cx.set_cursor(if hover > 0 || kind == 3 { MouseCursor::Move } else { MouseCursor::Hand });
                if hover != self.hover {
                    self.hover = hover;
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hover = 0;
                self.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                synth::select(&self.node);
                let handle = if kind == 3 { 1 } else { self.handle_at(fe.abs) };
                if handle > 0 {
                    self.drag = Some(Drag { handle, start: fe.abs, from: self.values });
                }
            }
            Hit::FingerMove(fe) => {
                if let Some(drag) = self.drag {
                    self.drag_to(drag, fe.abs);
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(fe) => {
                if self.drag.take().is_none() && kind == 2 && fe.is_over && fe.was_tap() {
                    let next = spec::WAVES.iter().position(|w| *w == self.wave).map_or(0, |i| (i + 1) % spec::WAVES.len());
                    self.wave = spec::WAVES[next].to_string();
                    host::set(&self.path("osc"), PanelValue::Code(format!("@{}", self.wave)));
                }
                self.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rev = host::revision();
        if self.seen != rev && self.drag.is_none() {
            self.seen = rev;
            self.read();
        }
        // What a module's presets, morph and dice move.
        for field in self.fields() {
            let spec = self.spec(field);
            let kind = if spec.is_choice() {
                BoundKind::Choice(spec.options.iter().map(|o| format!("@{o}")).collect())
            } else {
                BoundKind::Numbers { lo: spec.min, hi: spec.max, step: if spec.taper == spec::Taper::Int { 1.0 } else { 0.0 } }
            };
            host::register(Bound { path: self.path(field), kind });
        }
        let p = makepad_look::palette::get(cx);
        let rect = cx.walk_turtle(walk);
        let kind = self.kind.round() as i32;
        self.trace(cx);
        self.upload(cx);
        // The plot: the screen's inner area below its caption band.
        let s = 1.0f64;
        self.plot = Rect { pos: dvec2(rect.pos.x + 3.0 * s, rect.pos.y + 3.0 * s + 15.0 * s), size: dvec2(rect.size.x - 6.0 * s, rect.size.y - 6.0 * s - 19.0 * s) };
        let d = &mut self.draw_screen;
        d.face = color(p.panel, 1.0);
        d.screen = color(p.well, 1.0);
        d.ink = color(p.accent, 1.0);
        d.kind = kind as f32;
        d.row = 0.5;
        d.count = 48.0;
        d.scale = s as f32;
        d.glow = 0.6;
        d.bezel = 1.0;
        d.under = -1.0;
        d.hover = self.hover as f32;
        d.draw_abs(cx, rect);
        // The filter's handle: the cutoff line and the resonance point.
        if kind == 3 {
            if let Some((u, v)) = self.handles().first().copied() {
                let x = self.plot.pos.x + u * self.plot.size.x;
                self.draw_mark.color = color(p.accent, if self.hover > 0 || self.drag.is_some() { 0.9 } else { 0.45 });
                self.draw_mark.draw_abs(cx, Rect { pos: dvec2(x, self.plot.pos.y), size: dvec2(1.0, self.plot.size.y) });
                let y = self.plot.pos.y + (1.0 - v) * self.plot.size.y;
                self.draw_mark.draw_abs(cx, Rect { pos: dvec2(x - 4.0, y - 4.0), size: dvec2(8.0, 8.0) });
            }
        }
        // The scope's level, a bar down its right edge.
        if kind == 1 {
            let h = dsp::Ballistics::height(self.level.peak) as f64 * self.plot.size.y;
            self.draw_mark.color = color(if self.level.peak > 0.95 { p.danger } else { p.live }, 0.8);
            self.draw_mark.draw_abs(cx, Rect { pos: dvec2(self.plot.pos.x + self.plot.size.x - 4.0, self.plot.pos.y + self.plot.size.y - h), size: dvec2(3.0, h) });
        }
        // The caption: what it is and its values.
        let caption = if self.label.is_empty() {
            match kind {
                3 => "FILTER",
                2 => "WAVE",
                _ => "SOUND",
            }
            .to_string()
        } else {
            self.label.to_uppercase()
        };
        self.draw_text.color = color(p.secondary, 1.0);
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x + 8.0, rect.pos.y + 5.0), &format!("{caption}  {}", self.node));
        let values = match kind {
            3 => format!("{}  Q {:.2}", self.spec("cutoff").display(self.values[0]), self.values[1]),
            2 => self.wave.to_uppercase(),
            _ => format!("{:.0} dB", 20.0 * self.level.peak.max(1e-5).log10()),
        };
        self.draw_small.color = color(p.muted, 1.0);
        let w = makepad_widgets::badge::advance(&self.draw_small, cx, &values);
        self.draw_small.draw_abs(cx, dvec2(rect.pos.x + rect.size.x - 8.0 - w, rect.pos.y + 5.0), &values);
        if kind == 1 || self.drag.is_some() {
            self.next_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }
}
