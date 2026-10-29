//! Morph's pictures for an editor panel, bound to a `Morph` node by name:
//!
//! - `SynthTable{node: "lead" osc: "a"}`: the oscillator's frame as it
//!   plays (the table at its position through both warps, or the spectral
//!   resynthesis); drag sideways for the position, a click steps the table.
//! - `SynthLfo{node: "lead" lfo: 2}`: an LFO's shape over one cycle (drawn
//!   points, chaos, paths); a click steps the shape, drag up and down for
//!   its var.
//! - `SynthMods{node: "lead"}`: the mod rows, source to destination with
//!   their amounts; drag a row sideways for its amount.
//!
//! The pictures run the synth kit's own code (the same oscillator and LFO
//! the engine plays), on the values the panel reads; every drag is held
//! live and written once at the release, like every panel control.

use crate::dsp::ROW;
use crate::host as synth;
use crate::shaders::DrawScreen;
use crate::spec;
use makepad_audio_synth::kit::lfo::{Lfo, LfoBlock};
use makepad_audio_synth::kit::osc::{cycle_picture, OscBlock, OscSource};
use makepad_audio_synth::kit::table::{self, Table};
use makepad_audio_synth::morph::{self, layout};
use makepad_value_pickers::panel::host::{self, numbers_in, Bound, BoundKind, PanelValue};
use makepad_widgets::*;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SynthMorphBase = #(SynthMorph::register_widget(vm))
    /** A Morph oscillator's frame as it plays (warps included): drag sideways for the position, a click steps the table. */
    mod.widgets.SynthTable = set_type_default() do mod.widgets.SynthMorphBase{
        width: Fill height: 110 kind: 1 osc: "a" lfo: 1
        draw_text +: {text_style: theme.font_bold{font_size: 7.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
    }
    /** A Morph LFO's shape over a cycle: a click steps the shape, drag up and down for its var. */
    mod.widgets.SynthLfo = mod.widgets.SynthTable{kind: 2 height: 90}
    /** A Morph's mod rows (source, destination, amount): drag a row sideways for its amount. */
    mod.widgets.SynthMods = mod.widgets.SynthTable{kind: 3 height: 150}
}

fn color(value: u32, alpha: f32) -> Vec4f {
    let mut c = Vec4f::from_u32(value);
    c.w *= alpha;
    c
}

/// One mod row as written: its words (`@lfo1 @a_pos 0.4 @m1`).
type Row = Vec<String>;

/// The rows of a `mods` value (`[[@lfo1 @a_pos 0.4] [@env2 @f1_cutoff 0.5]]`).
pub fn parse_rows(code: &str) -> Vec<Row> {
    let spaced = code.replace('[', " [ ").replace(']', " ] ").replace(',', " ");
    let mut rows = Vec::new();
    let mut depth = 0;
    let mut row: Row = Vec::new();
    for w in spaced.split_whitespace() {
        match w {
            "[" => {
                depth += 1;
                if depth == 2 {
                    row.clear();
                }
            }
            "]" => {
                if depth == 2 && !row.is_empty() {
                    rows.push(std::mem::take(&mut row));
                }
                depth -= 1;
            }
            _ if depth == 2 => row.push(w.to_string()),
            _ => {}
        }
    }
    rows
}

/// Rows written back (commas, so a negative amount stays a number).
pub fn rows_code(rows: &[Row]) -> String {
    let rows: Vec<String> = rows.iter().map(|r| format!("[{}]", r.join(", "))).collect();
    format!("[{}]", rows.join(" "))
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    start: DVec2,
    from: f64,
    row: usize,
    moved: bool,
}

#[derive(Script, ScriptHook, Widget)]
pub struct SynthMorph {
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
    /// 1 table, 2 LFO, 3 mod rows.
    #[live]
    kind: f64,
    #[live]
    node: String,
    /// The oscillator a table shows (`a`, `b`, `c`).
    #[live]
    osc: String,
    /// The LFO a shape shows (1..8).
    #[live]
    lfo: f64,
    #[live]
    label: String,
    #[rust]
    vals: Vec<f32>,
    #[rust]
    frames: Vec<Vec<f32>>,
    #[rust]
    points: Vec<[f32; 3]>,
    #[rust]
    rows: Vec<Row>,
    #[rust]
    user: Option<(Vec<Vec<f32>>, Arc<Table>)>,
    #[rust]
    seen: u64,
    #[rust]
    drag: Option<Drag>,
    #[rust]
    plot: Rect,
    #[rust]
    texture: Option<Texture>,
    #[rust]
    row: Vec<f32>,
}

impl SynthMorph {
    fn osc_index(&self) -> usize {
        ["a", "b", "c"].iter().position(|o| *o == self.osc).unwrap_or(0)
    }

    fn lfo_index(&self) -> usize {
        (self.lfo.round() as usize).clamp(1, 8) - 1
    }

    fn path(&self, key: &str) -> String {
        format!("{}.{key}", self.node)
    }

    /// The keys this picture reads.
    fn keys(&self) -> Vec<&'static str> {
        let schema = &layout().schema;
        let prefix = match self.kind.round() as i32 {
            1 => format!("{}_", self.osc),
            2 => format!("lfo{}_", self.lfo_index() + 1),
            _ => return Vec::new(),
        };
        schema.params.iter().map(|p| p.key.as_str()).filter(|k| k.starts_with(&prefix)).collect()
    }

    /// Read the node's fields (held values while a gesture runs).
    fn read(&mut self) {
        let schema = &layout().schema;
        self.vals = schema.defaults();
        for key in self.keys() {
            let Some(i) = schema.index(key) else { continue };
            let p = &schema.params[i];
            match host::read(&self.path(key)) {
                Some(PanelValue::Number(n)) => self.vals[i] = p.clamp(n as f32),
                Some(PanelValue::Bool(b)) => self.vals[i] = b as u8 as f32,
                Some(PanelValue::Code(code)) => {
                    let code = code.trim();
                    if let Some(o) = p.option(code) {
                        self.vals[i] = o as f32;
                    } else if code == "true" || code == "false" {
                        self.vals[i] = (code == "true") as u8 as f32;
                    } else if let Ok(n) = code.parse::<f32>() {
                        self.vals[i] = p.clamp(n);
                    }
                }
                None => {}
            }
        }
        match self.kind.round() as i32 {
            1 => {
                let code = host::read(&self.path(&format!("{}_frames", self.osc))).map(|v| v.code()).unwrap_or_default();
                // `[[1 0.5] [1 0 0.3]]` or one frame `[1 0.5]`.
                let mut frames: Vec<Vec<f32>> = code.split(']').map(|f| numbers_in(f).into_iter().map(|x| x as f32).collect::<Vec<f32>>()).filter(|f| !f.is_empty()).collect();
                frames.truncate(table::MAX_FRAMES);
                self.frames = frames;
            }
            2 => {
                let code = host::read(&self.path(&format!("lfo{}_points", self.lfo_index() + 1))).map(|v| v.code()).unwrap_or_default();
                let mut pts: Vec<[f32; 3]> = code.split(']').map(numbers_in).filter(|n| n.len() >= 2).map(|n| [n[0] as f32, n[1] as f32, n.get(2).copied().unwrap_or(0.0) as f32]).collect();
                pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
                self.points = pts;
            }
            _ => {
                let code = host::read(&self.path("mods")).map(|v| v.code()).unwrap_or_default();
                self.rows = parse_rows(&code);
            }
        }
    }

    fn table(&mut self) -> Arc<Table> {
        let ix = &layout().ix.osc[self.osc_index()];
        let kind = self.vals[ix.table] as usize;
        match morph::TABLES.get(kind).copied() {
            Some("user") if !self.frames.is_empty() => {
                if self.user.as_ref().is_none_or(|(f, _)| *f != self.frames) {
                    self.user = Some((self.frames.clone(), Arc::new(Table::from_partials(&self.frames))));
                }
                self.user.as_ref().map(|(_, t)| t.clone()).unwrap_or_else(table::sine)
            }
            Some("user") | Some("import") => table::sine(),
            _ => table::builtin(kind),
        }
    }

    fn trace(&mut self) {
        self.row.resize(ROW, 0.5);
        match self.kind.round() as i32 {
            1 => {
                let ix = &layout().ix.osc[self.osc_index()];
                let blk = OscBlock::read(&self.vals[..], ix, 261.6, 0.0);
                let src = OscSource { table: self.table(), sample: None };
                let mut cycle = vec![0.0f32; ROW];
                cycle_picture(&blk, &src, &mut cycle);
                for (o, v) in self.row.iter_mut().zip(cycle.iter()) {
                    *o = (v.clamp(-1.1, 1.1) * 0.42 + 0.5).clamp(0.0, 1.0);
                }
            }
            2 => {
                let ix = &layout().ix.lfo[self.lfo_index()];
                let mut b = LfoBlock::read(&self.vals[..], ix);
                // One cycle as a note would play it, from its start.
                b.mode = 1;
                b.delay = 0.0;
                b.rise = 0.0;
                b.sync_beats = 0.0;
                b.rate = 1.0;
                b.smooth = 0.0;
                let mut lfo = Lfo::new(7);
                lfo.trigger(&b);
                let dt = 1.0 / ROW as f32;
                for o in self.row.iter_mut() {
                    lfo.advance(&b, &self.points, dt, 2.0, 0.0);
                    let x = lfo.out()[0];
                    let x = if b.uni { x * 2.0 - 1.0 } else { x };
                    *o = (x * 0.42 + 0.5).clamp(0.0, 1.0);
                }
            }
            _ => self.row.fill(0.5),
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

    /// The parameter a drag moves: the table's position, the LFO's var.
    fn drag_key(&self) -> Option<String> {
        match self.kind.round() as i32 {
            1 => Some(format!("{}_pos", self.osc)),
            2 => Some(format!("lfo{}_var", self.lfo_index() + 1)),
            _ => None,
        }
    }

    /// The choice a click steps: the table, the LFO's shape.
    fn step_key(&self) -> Option<String> {
        match self.kind.round() as i32 {
            1 => Some(format!("{}_table", self.osc)),
            2 => Some(format!("lfo{}_shape", self.lfo_index() + 1)),
            _ => None,
        }
    }

    fn value(&self, key: &str) -> f64 {
        layout().schema.index(key).map_or(0.0, |i| self.vals[i] as f64)
    }

    fn set(&mut self, key: &str, value: f64) {
        let spec = spec::spec_or_generic("Morph", key, None);
        let value = value.clamp(spec.min, spec.max);
        if let Some(i) = layout().schema.index(key) {
            self.vals[i] = value as f32;
        }
        host::set(&self.path(key), PanelValue::Code(spec.source_text(value)));
    }

    fn row_at(&self, abs: DVec2) -> Option<usize> {
        let y = abs.y - self.plot.pos.y;
        if y < 0.0 {
            return None;
        }
        let i = (y / 14.0) as usize;
        (i < self.rows.len()).then_some(i)
    }

    fn row_amount(&self, i: usize) -> f64 {
        self.rows.get(i).and_then(|r| r.get(2)).and_then(|a| a.parse::<f64>().ok()).unwrap_or(0.0)
    }
}

impl Widget for SynthMorph {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let kind = self.kind.round() as i32;
        match event.hits(cx, self.draw_screen.area()) {
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => cx.set_cursor(if kind == 3 { MouseCursor::EwResize } else { MouseCursor::Move }),
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                synth::select(&self.node);
                if kind == 3 {
                    if let Some(row) = self.row_at(fe.abs) {
                        self.drag = Some(Drag { start: fe.abs, from: self.row_amount(row), row, moved: false });
                    }
                } else if let Some(key) = self.drag_key() {
                    self.drag = Some(Drag { start: fe.abs, from: self.value(&key), row: 0, moved: false });
                }
            }
            Hit::FingerMove(fe) => {
                if let Some(mut drag) = self.drag {
                    let dx = (fe.abs.x - drag.start.x) / self.plot.size.x.max(1.0);
                    let dy = (drag.start.y - fe.abs.y) / self.plot.size.y.max(1.0);
                    drag.moved |= dx.abs() > 0.01 || dy.abs() > 0.01;
                    self.drag = Some(drag);
                    match kind {
                        3 => {
                            let amount = (drag.from + dx * 2.0).clamp(-1.0, 1.0);
                            if let Some(row) = self.rows.get_mut(drag.row) {
                                while row.len() < 3 {
                                    row.push("0".into());
                                }
                                row[2] = format!("{:.2}", amount).trim_end_matches('0').trim_end_matches('.').to_string();
                                let code = rows_code(&self.rows);
                                host::set(&self.path("mods"), PanelValue::Code(code));
                            }
                        }
                        1 => {
                            if let Some(key) = self.drag_key() {
                                self.set(&key, drag.from + dx);
                            }
                        }
                        _ => {
                            if let Some(key) = self.drag_key() {
                                self.set(&key, drag.from + dy);
                            }
                        }
                    }
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(fe) => {
                let moved = self.drag.take().is_some_and(|d| d.moved);
                if !moved && fe.is_over && fe.was_tap() {
                    if let Some(key) = self.step_key() {
                        let spec = spec::spec_or_generic("Morph", &key, None);
                        let n = spec.options.len().max(1);
                        let next = (self.value(&key) as usize + 1) % n;
                        self.set(&key, next as f64);
                    }
                }
                self.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rev = host::revision();
        if self.seen != rev && self.drag.is_none() || self.vals.is_empty() {
            self.seen = rev;
            self.read();
        }
        // What a module's presets, morph and dice move.
        for key in self.keys() {
            let spec = spec::spec_or_generic("Morph", key, None);
            let kind = if spec.is_choice() {
                BoundKind::Choice(spec.options.iter().map(|o| format!("@{o}")).collect())
            } else {
                BoundKind::Numbers { lo: spec.min, hi: spec.max, step: if spec.taper == spec::Taper::Int { 1.0 } else { 0.0 } }
            };
            host::register(Bound { path: self.path(key), kind });
        }
        let p = makepad_look::palette::get(cx);
        let rect = cx.walk_turtle(walk);
        let kind = self.kind.round() as i32;
        self.trace();
        self.upload(cx);
        self.plot = Rect { pos: dvec2(rect.pos.x + 3.0, rect.pos.y + 18.0), size: dvec2(rect.size.x - 6.0, rect.size.y - 22.0) };
        let d = &mut self.draw_screen;
        d.face = color(p.panel, 1.0);
        d.screen = color(p.well, 1.0);
        d.ink = color(p.accent, 1.0);
        d.kind = if kind == 3 { 1.0 } else { 2.0 };
        d.row = 0.5;
        d.count = 48.0;
        d.scale = 1.0;
        d.glow = 0.6;
        d.bezel = 1.0;
        d.under = -1.0;
        d.hover = 0.0;
        d.draw_abs(cx, rect);
        let schema = &layout().schema;
        let (caption, values) = match kind {
            1 => {
                let ix = &layout().ix.osc[self.osc_index()];
                let table = schema.params[ix.table].options.get(self.vals[ix.table] as usize).copied().unwrap_or("");
                let mode = schema.params[ix.mode].options.get(self.vals[ix.mode] as usize).copied().unwrap_or("");
                // The position, a bar along the bottom.
                let pos = self.vals[ix.pos] as f64;
                self.draw_mark.color = color(p.accent, 0.7);
                self.draw_mark.draw_abs(cx, Rect { pos: dvec2(self.plot.pos.x, self.plot.pos.y + self.plot.size.y - 3.0), size: dvec2(self.plot.size.x * pos, 2.0) });
                (format!("OSC {}", self.osc.to_uppercase()), format!("{} {} {:.0}%", mode.to_uppercase(), table.to_uppercase(), pos * 100.0))
            }
            2 => {
                let ix = &layout().ix.lfo[self.lfo_index()];
                let shape = schema.params[ix.shape].options.get(self.vals[ix.shape] as usize).copied().unwrap_or("");
                // The drawn points, as dots.
                if shape == "drawn" {
                    self.draw_mark.color = color(p.accent, 0.9);
                    for pt in &self.points {
                        let x = self.plot.pos.x + pt[0] as f64 * self.plot.size.x;
                        let y = self.plot.pos.y + (1.0 - (0.08 + 0.84 * pt[1] as f64)) * self.plot.size.y;
                        self.draw_mark.draw_abs(cx, Rect { pos: dvec2(x - 2.5, y - 2.5), size: dvec2(5.0, 5.0) });
                    }
                }
                (format!("LFO {}", self.lfo_index() + 1), shape.to_uppercase())
            }
            _ => {
                // The rows, each with its amount as a bar from the middle.
                let mid = self.plot.pos.x + self.plot.size.x * 0.75;
                for (i, row) in self.rows.iter().enumerate() {
                    let y = self.plot.pos.y + i as f64 * 14.0;
                    if y + 12.0 > self.plot.pos.y + self.plot.size.y {
                        break;
                    }
                    let word = |k: usize| row.get(k).map(|w| w.trim_start_matches('@').to_uppercase()).unwrap_or_default();
                    let aux = row.iter().skip(3).find(|w| w.starts_with('@') && !matches!(w.as_str(), "@uni" | "@bi" | "@inv")).map(|w| format!(" x{}", w.trim_start_matches('@').to_uppercase())).unwrap_or_default();
                    self.draw_small.color = color(p.secondary, 1.0);
                    self.draw_small.draw_abs(cx, dvec2(self.plot.pos.x + 6.0, y + 1.0), &format!("{} > {}{aux}", word(0), word(1)));
                    let a = self.row_amount(i);
                    let w = a * self.plot.size.x * 0.2;
                    self.draw_mark.color = color(if a < 0.0 { p.danger } else { p.accent }, 0.75);
                    let (x, w) = if w < 0.0 { (mid + w, -w) } else { (mid, w) };
                    self.draw_mark.draw_abs(cx, Rect { pos: dvec2(x, y + 4.0), size: dvec2(w.max(1.0), 6.0) });
                }
                ("MODS".to_string(), format!("{} rows", self.rows.len()))
            }
        };
        let caption = if self.label.is_empty() { caption } else { self.label.to_uppercase() };
        self.draw_text.color = color(p.secondary, 1.0);
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x + 8.0, rect.pos.y + 5.0), &format!("{caption}  {}", self.node));
        self.draw_small.color = color(p.muted, 1.0);
        let w = makepad_widgets::badge::advance(&self.draw_small, cx, &values);
        self.draw_small.draw_abs(cx, dvec2(rect.pos.x + rect.size.x - 8.0 - w, rect.pos.y + 5.0), &values);
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mod_rows_round_trip() {
        let rows = parse_rows("[[@lfo1 @a_pos 0.4] [@env2, @f1_cutoff, -0.5, @m1, @inv]]");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], vec!["@env2", "@f1_cutoff", "-0.5", "@m1", "@inv"]);
        assert_eq!(parse_rows(&rows_code(&rows)), rows);
    }
}
