//! The palette editor: a scope's colours (the scene's, a file's, or a
//! tag's) as swatches with how many places use each; one edited in the
//! HDR picker, or all of them moved at once by a generator.
//!
//! Generators (`palette_gen`): the harmony rules from the chosen swatch
//! (or the most used), the moves with an amount dragged live (warmth, hue,
//! contrast, chroma, lightness ramp, glow), random within what is kept
//! (lightness, the accent, the paper), shuffled roles, the frame's colours,
//! saved palettes, and a mix between two stored palettes (A and B).
//!
//! A click on a generator writes it at once, as one undo step (another
//! click is another step: cmd-Z walks back through them); a drag on an
//! amount or in the picker is held live and written at its release. Each
//! write goes to every place each colour is written, in that place's form
//! ([`super::host::recolour`]).

use super::control::{DrawPanelShape, R_CONTROL};
use super::host::{self, Swatch};
use super::palette_gen::{self as gen, Harmony, Keep, Move};
use crate::color_picker::{format_code_color, split_hdr, ColorPickAction, HdrColorPicker};
use makepad_widgets::badge::advance;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PanelPaletteBase = #(PanelPalette::register_widget(vm))
    /** A scope's palette: swatches, one edited, all moved by generators. */
    mod.widgets.PanelPalette = set_type_default() do mod.widgets.PanelPaletteBase{
        width: Fill height: Fit flow: Down spacing: 6
        draw_bg +: {color: #x00000000}
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
        picker := mod.widgets.HdrColorPicker{visible: false}
    }
}

fn color(value: u32, alpha: f32) -> Vec4f {
    let mut c = Vec4f::from_u32(value);
    c.w *= alpha;
    c
}

fn shown(c: [f64; 4]) -> Vec4f {
    let (b, _) = split_hdr(c);
    vec4(b[0] as f32, b[1] as f32, b[2] as f32, 1.0)
}

/// What a click on the editor did.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot {
    Swatch(usize),
    Harmony(usize),
    Move(usize),
    Amount,
    Random,
    Keep(usize),
    Shuffle,
    Frame,
    Saved(usize),
    StoreA,
    StoreB,
    Mix,
    /// The generators shown or folded away.
    Tools,
}

const CELL: f64 = 24.0;
const CHIP_H: f64 = 20.0;

#[derive(Script, ScriptHook, Widget)]
pub struct PanelPalette {
    /// When in the film it acts: the host reads it from the panel's code.
    #[live]
    pub when: String,
    #[deref]
    view: View,
    /// Which colours: a file's name, a tag, or empty for the scene's.
    #[live]
    pub scope: String,
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_cell: DrawPanelShape,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_small: DrawText,
    #[rust]
    swatches: Vec<Swatch>,
    #[rust]
    seen: u64,
    /// The swatch open in the picker.
    #[rust]
    chosen: Option<usize>,
    /// The move whose amount is shown, and the amount.
    #[rust]
    move_at: Option<usize>,
    #[rust]
    amount: f64,
    #[rust]
    keep: Keep,
    #[rust]
    seed: u64,
    #[rust]
    a: Option<Vec<[f64; 4]>>,
    #[rust]
    b: Option<Vec<[f64; 4]>>,
    #[rust]
    t: f64,
    /// A drag on an amount or a mix: the palette as it began.
    #[rust]
    drag: Option<(Spot, Vec<[f64; 4]>)>,
    /// The picker's gesture: the swatch's colour as it began.
    #[rust]
    picking: Option<[f64; 4]>,
    #[rust]
    spots: Vec<(Rect, Spot)>,
    #[rust]
    hot: Option<Spot>,
    /// The generators (harmony, moves, random, saved, A/B) are shown.
    #[rust]
    tools_open: bool,
}

impl PanelPalette {
    fn colours(&self) -> Vec<[f64; 4]> {
        self.swatches.iter().map(|s| s.rgba).collect()
    }

    /// The scope's colours moved to `to`, one for one, at every place.
    fn apply(&mut self, from: &[[f64; 4]], to: Vec<[f64; 4]>) {
        let map: Vec<([f64; 4], [f64; 4])> = from.iter().copied().zip(to).filter(|(a, b)| a != b).collect();
        if !map.is_empty() {
            host::recolour(&self.scope, map);
        }
    }

    fn read(&mut self, cx: &mut Cx) {
        let rev = host::revision();
        if self.seen == rev || self.drag.is_some() || self.picking.is_some() {
            return;
        }
        self.seen = rev;
        self.swatches = host::palette(&self.scope);
        if self.chosen.is_some_and(|i| i >= self.swatches.len()) {
            self.chosen = None;
        }
        if let (Some(i), Some(mut picker)) = (self.chosen, self.view.widget(cx, ids!(picker)).borrow_mut::<HdrColorPicker>()) {
            picker.set_value(cx, self.swatches[i].rgba);
        }
    }

    fn base(&self) -> [f64; 4] {
        self.chosen.and_then(|i| self.swatches.get(i)).or(self.swatches.first()).map(|s| s.rgba).unwrap_or([1.0, 0.3, 0.1, 1.0])
    }

    fn click(&mut self, cx: &mut Cx, spot: Spot) {
        let now = self.colours();
        match spot {
            Spot::Swatch(i) => {
                self.chosen = if self.chosen == Some(i) { None } else { Some(i) };
                let picker = self.view.widget(cx, ids!(picker));
                picker.set_visible(cx, self.chosen.is_some());
                if let Some(i) = self.chosen {
                    let rgba = self.swatches[i].rgba;
                    if let Some(mut p) = picker.borrow_mut::<HdrColorPicker>() {
                        p.set_value(cx, rgba);
                        p.set_hdr_allowed(cx, split_hdr(rgba).1 > 1.0);
                    }
                }
            }
            Spot::Harmony(k) => {
                let next = gen::harmony(&now, self.base(), Harmony::ALL[k]);
                self.apply(&now, next);
            }
            Spot::Move(k) => {
                self.move_at = if self.move_at == Some(k) { None } else { Some(k) };
                self.amount = 0.0;
            }
            Spot::Random => {
                self.seed += 1;
                let next = gen::random(&now, self.seed, self.keep);
                self.apply(&now, next);
            }
            Spot::Keep(k) => match k {
                0 => self.keep.lightness = !self.keep.lightness,
                1 => self.keep.accent = !self.keep.accent,
                _ => self.keep.paper = !self.keep.paper,
            },
            Spot::Shuffle => {
                self.seed += 1;
                let next = gen::shuffled(&now, self.seed);
                self.apply(&now, next);
            }
            Spot::Frame => {
                let from = host::frame_colours(now.len().max(3));
                let next = gen::dealt(&now, &from);
                self.apply(&now, next);
            }
            Spot::Saved(k) => {
                if let Some((_, colours)) = host::saved_palettes().get(k) {
                    let next = gen::dealt(&now, colours);
                    self.apply(&now, next);
                }
            }
            Spot::Tools => self.tools_open = !self.tools_open,
            Spot::StoreA => self.a = Some(now),
            Spot::StoreB => self.b = Some(now),
            Spot::Amount | Spot::Mix => {}
        }
        self.draw_bg.redraw(cx);
    }

    /// An amount (or the mix) dragged to `x` in `rect`: the palette as it
    /// began, moved, live.
    fn drag_to(&mut self, cx: &mut Cx, spot: Spot, rect: Rect, x: f64) {
        let Some((_, began)) = self.drag.clone() else { return };
        let t = ((x - rect.pos.x) / rect.size.x).clamp(0.0, 1.0);
        let next = match spot {
            Spot::Amount => {
                self.amount = t * 2.0 - 1.0;
                let Some(k) = self.move_at else { return };
                gen::moved(&began, Move::ALL[k], self.amount)
            }
            Spot::Mix => {
                self.t = t;
                match (&self.a, &self.b) {
                    (Some(a), Some(b)) => {
                        let mixed = gen::mix(a, b, t);
                        began.iter().enumerate().map(|(i, c)| mixed.get(i).copied().unwrap_or(*c)).collect()
                    }
                    _ => return,
                }
            }
            _ => return,
        };
        self.apply(&began, next.clone());
        // Shown as it moves (the host's read follows at the release).
        for (s, c) in self.swatches.iter_mut().zip(next) {
            s.rgba = c;
        }
        self.draw_bg.redraw(cx);
    }
}

impl Widget for PanelPalette {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let at = |s: &Self, abs: DVec2| s.spots.iter().rev().find(|(r, _)| r.contains(abs)).copied();
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = at(self, fe.abs).map(|(_, s)| s);
                cx.set_cursor(if hot.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
                if hot != self.hot {
                    self.hot = hot;
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hot = None;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if let Some((rect, spot @ (Spot::Amount | Spot::Mix))) = at(self, fe.abs) {
                    self.drag = Some((spot, self.colours()));
                    self.drag_to(cx, spot, rect, fe.abs.x);
                }
            }
            Hit::FingerMove(fe) => {
                if let Some((spot, _)) = self.drag.clone() {
                    if let Some((rect, _)) = self.spots.iter().find(|(_, s)| *s == spot).copied() {
                        self.drag_to(cx, spot, rect, fe.abs.x);
                    }
                }
            }
            Hit::FingerUp(fe) => {
                if self.drag.take().is_some() {
                    // The release: the host writes; read it back.
                    self.seen = 0;
                    self.draw_bg.redraw(cx);
                } else if fe.is_over {
                    if let Some((_, spot)) = at(self, fe.abs) {
                        self.click(cx, spot);
                    }
                }
            }
            _ => {}
        }
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            let uid = self.view.widget(cx, ids!(picker)).widget_uid();
            for action in actions.filter_widget_actions_cast::<ColorPickAction>(uid) {
                let Some(i) = self.chosen else { continue };
                let (rgba, done) = match action {
                    ColorPickAction::Preview(c) => (c, false),
                    ColorPickAction::Commit(c) => (c, true),
                    _ => continue,
                };
                let began = *self.picking.get_or_insert(self.swatches[i].rgba);
                host::recolour(&self.scope, vec![(began, rgba)]);
                self.swatches[i].rgba = rgba;
                if done {
                    self.picking = None;
                    self.seen = 0;
                }
                self.draw_bg.redraw(cx);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.read(cx);
        let p = makepad_look::palette::get(cx);
        let width = cx.turtle().inner_width().max(120.0);
        // Laid out first (the rows' heights), then drawn.
        let mut items: Vec<(Rect, Spot, String)> = Vec::new();
        let mut y = 16.0;
        // The swatches: one compact row (wrapped when they are many), the
        // name and uses of the one under the pointer (or chosen) under it.
        let per_row = ((width + 6.0) / (CELL + 6.0)).floor().max(1.0) as usize;
        for i in 0..self.swatches.len() {
            let (c, r) = (i % per_row, i / per_row);
            items.push((Rect { pos: dvec2(c as f64 * (CELL + 6.0) + 2.0, y + 2.0 + r as f64 * (CELL + 6.0)), size: dvec2(CELL, CELL) }, Spot::Swatch(i), String::new()));
        }
        y += self.swatches.len().div_ceil(per_row).max(1) as f64 * (CELL + 6.0) + 2.0;
        let caption_y = y;
        y += 18.0;
        // Chips, wrapped.
        let chips = |items: &mut Vec<(Rect, Spot, String)>, y: &mut f64, title: &str, list: Vec<(Spot, String)>, cx: &mut Cx2d, small: &DrawText| {
            items.push((Rect { pos: dvec2(0.0, *y), size: dvec2(width, 14.0) }, Spot::Amount, format!("§{title}")));
            *y += 15.0;
            let mut x = 0.0;
            for (spot, label) in list {
                let w = advance(small, cx, &label) + 14.0;
                if x + w > width && x > 0.0 {
                    x = 0.0;
                    *y += CHIP_H + 4.0;
                }
                items.push((Rect { pos: dvec2(x, *y), size: dvec2(w, CHIP_H) }, spot, label));
                x += w + 4.0;
            }
            *y += CHIP_H + 6.0;
        };
        // The generators fold behind one chip, so the palette is a row of
        // swatches until they are asked for.
        let tools = if self.tools_open { "Hide generators" } else { "Generators…" };
        items.push((Rect { pos: dvec2(0.0, y), size: dvec2(advance(&self.draw_small, cx, tools) + 16.0, CHIP_H) }, Spot::Tools, tools.to_string()));
        y += CHIP_H + 8.0;
        if self.tools_open {
            chips(&mut items, &mut y, "HARMONY · from the chosen colour", Harmony::ALL.iter().enumerate().map(|(k, h)| (Spot::Harmony(k), h.name().to_string())).collect(), cx, &self.draw_small);
            chips(&mut items, &mut y, "MOVE · drag the amount", Move::ALL.iter().enumerate().map(|(k, m)| (Spot::Move(k), m.name().to_string())).collect(), cx, &self.draw_small);
            if self.move_at.is_some() {
                items.push((Rect { pos: dvec2(0.0, y - 4.0), size: dvec2(width, 16.0) }, Spot::Amount, String::new()));
                y += 18.0;
            }
            let keep = |on: bool, s: &str| format!("{} {s}", if on { "■" } else { "□" });
            chips(&mut items, &mut y, "RANDOM · keeps what is ticked", vec![
                (Spot::Random, "Random".into()),
                (Spot::Keep(0), keep(self.keep.lightness, "lightness")),
                (Spot::Keep(1), keep(self.keep.accent, "accent")),
                (Spot::Keep(2), keep(self.keep.paper, "paper")),
                (Spot::Shuffle, "Shuffle roles".into()),
                (Spot::Frame, "From the frame".into()),
            ], cx, &self.draw_small);
            let saved = host::saved_palettes();
            if !saved.is_empty() {
                chips(&mut items, &mut y, "SAVED", saved.iter().enumerate().map(|(k, (n, _))| (Spot::Saved(k), n.clone())).collect(), cx, &self.draw_small);
            }
            chips(&mut items, &mut y, "A / B · store two, drag between", vec![
                (Spot::StoreA, if self.a.is_some() { "A ✓".into() } else { "Store A".into() }),
                (Spot::StoreB, if self.b.is_some() { "B ✓".into() } else { "Store B".into() }),
            ], cx, &self.draw_small);
            if self.a.is_some() && self.b.is_some() {
                items.push((Rect { pos: dvec2(0.0, y - 4.0), size: dvec2(width, 16.0) }, Spot::Mix, String::new()));
                y += 18.0;
            }
        }
        cx.begin_turtle(walk, Layout::flow_down());
        let rect = cx.walk_turtle(Walk::new(Size::fill(), Size::Fixed(y)));
        self.draw_bg.draw_abs(cx, rect);
        self.spots.clear();
        self.draw_small.color = color(p.secondary, 1.0);
        let title = if self.scope.is_empty() { "PALETTE".to_string() } else { format!("PALETTE · {}", self.scope) };
        self.draw_small.draw_abs(cx, rect.pos, &title);
        for (r, spot, label) in items {
            let r = Rect { pos: rect.pos + r.pos, size: r.size };
            let hot = self.hot == Some(spot);
            match spot {
                Spot::Swatch(i) => {
                    let s = self.swatches[i].clone();
                    let on = self.chosen == Some(i);
                    let ring = if on { color(p.accent, 1.0) } else if hot { color(p.text, 0.8) } else { color(p.border, 1.0) };
                    if on || hot {
                        self.draw_cell.edged_round(cx, Rect { pos: r.pos - dvec2(3.0, 3.0), size: r.size + dvec2(6.0, 6.0) }, color(0, 0.0), ring, 1.5, R_CONTROL + 3.0);
                    }
                    self.draw_cell.edged_round(cx, r, shown(s.rgba), color(p.border, 1.0), 1.0, R_CONTROL);
                    if split_hdr(s.rgba).1 > 1.0 {
                        // Light over 1: a bright bar inside its foot.
                        self.draw_cell.fill_round(cx, Rect { pos: dvec2(r.pos.x + 5.0, r.pos.y + r.size.y - 5.0), size: dvec2(r.size.x - 10.0, 2.0) }, color(0xffffffff, 0.9), 1.0);
                    }
                    self.spots.push((r, spot));
                }
                Spot::Amount if label.starts_with('§') => {
                    self.draw_small.color = color(p.secondary, 1.0);
                    self.draw_small.draw_abs(cx, r.pos, &label[label.char_indices().nth(1).map_or(0, |(i, _)| i)..]);
                }
                Spot::Amount | Spot::Mix => {
                    let t = if spot == Spot::Mix { self.t } else { (self.amount + 1.0) * 0.5 };
                    let track = Rect { pos: dvec2(r.pos.x, r.pos.y + 6.0), size: dvec2(r.size.x - 60.0, 4.0) };
                    self.draw_cell.fill_round(cx, track, color(p.track, 1.0), 2.0);
                    self.draw_cell.edged_round(cx, Rect { pos: dvec2(track.pos.x + track.size.x * t - 6.0, r.pos.y + 2.0), size: dvec2(12.0, 12.0) }, color(p.accent, 1.0), color(p.row, 1.0), 1.0, 6.0);
                    self.draw_small.color = color(p.text_bright, 1.0);
                    let said = if spot == Spot::Mix { format!("A {:.0}% B", t * 100.0) } else { format!("{:+.2}", self.amount) };
                    self.draw_small.draw_abs(cx, dvec2(track.pos.x + track.size.x + 8.0, r.pos.y + 2.0), &said);
                    self.spots.push((track, spot));
                }
                _ => {
                    let on = matches!(spot, Spot::Move(k) if self.move_at == Some(k)) || (spot == Spot::Tools && self.tools_open);
                    self.draw_cell.fill_round(cx, r, color(if on { p.accent } else if hot { p.control_hover } else { p.raised }, 1.0), R_CONTROL);
                    self.draw_small.color = color(if on { p.on_accent } else { p.text }, 1.0);
                    self.draw_small.draw_abs(cx, dvec2(r.pos.x + 7.0, r.pos.y + 5.0), &label);
                    self.spots.push((r, spot));
                }
            }
        }
        // The swatch under the pointer (else the chosen one): its name and
        // how many places use it.
        let about = match self.hot {
            Some(Spot::Swatch(i)) => Some(i),
            _ => self.chosen,
        };
        let caption = match about.and_then(|i| self.swatches.get(i)) {
            Some(s) => {
                let name = if s.name.is_empty() { format_code_color(s.rgba) } else { format!("{} · {}", s.name, format_code_color(s.rgba)) };
                format!("{name} · used {}×", s.uses)
            }
            None => format!("{} colours · click one to edit it", self.swatches.len()),
        };
        self.draw_small.color = color(if about.is_some() { p.text } else { p.muted }, 1.0);
        self.draw_small.draw_abs(cx, dvec2(rect.pos.x + 2.0, rect.pos.y + caption_y + 2.0), &caption);
        if self.chosen.is_some() {
            let _ = self.view.draw_walk(cx, scope, Walk::new(Size::fill(), Size::fit()));
        }
        cx.end_turtle();
        DrawStep::done()
    }
}
