//! Live graphs for the task-manager view, drawn with plain quads: a trace is one quad per pair of
//! neighbouring samples whose shader draws the anti-aliased line and the soft fill under it, so a
//! minute of samples is a few hundred instances in one draw call; frame times are one bar per
//! pixel column (the slowest frame in it).

use crate::live::{Frame, WINDOW_S};
use makepad_shell_kit::ui::{rect, HAlign, ShellDraw};
use makepad_shell_kit::{alpha, ShellTokens};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // One stretch of a trace: the line from line_a to line_b (quad pixels) and the fill below
    // it, fading toward the plot's bottom (anchored to the plot, so neighbours meet seamlessly).
    mod.widgets.DrawDevhubTrace = set_type_default() do #(DrawDevhubTrace::script_shader(vm)) {
        ..mod.draw.DrawQuad
        pixel: fn() {
            let p = self.pos * self.rect_size
            let dx = max(self.line_b.x - self.line_a.x, 0.0001)
            let t = clamp((p.x - self.line_a.x) / dx, 0.0, 1.0)
            let ly = mix(self.line_a.y, self.line_b.y, t)
            let slope = (self.line_b.y - self.line_a.y) / dx
            let d = abs(p.y - ly) / sqrt(1.0 + slope * slope)
            let line = clamp(self.line_width * 0.5 - d + 0.5, 0.0, 1.0)
            let below = clamp(p.y - ly + 0.5, 0.0, 1.0)
            let g = clamp((p.y - self.plot_top) / max(self.plot_height, 1.0), 0.0, 1.0)
            let fill = self.fill_alpha * below * (1.0 - 0.75 * g)
            let a = max(line * self.color.a, fill)
            return vec4(self.color.rgb * a, a)
        }
    }
}

/// Instance fields after the draw base, as the instance buffer reads them.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawDevhubTrace {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub line_a: Vec2f,
    #[live]
    pub line_b: Vec2f,
    #[live]
    pub plot_top: f32,
    #[live]
    pub plot_height: f32,
    #[live]
    pub fill_alpha: f32,
    #[live(1.5)]
    pub line_width: f32,
}

/// The view's colours, from the shell's theme tokens: text in three strengths mixed toward the
/// panel's background (readable on any theme, unlike a darkened foreground), the accent and the
/// series' hues.
#[derive(Clone, Copy)]
pub struct Palette {
    pub fg: Vec4f,
    pub secondary: Vec4f,
    pub tertiary: Vec4f,
    pub well: Vec4f,
    pub rule: Vec4f,
    pub accent: Vec4f,
    pub tier: Vec4f,
    pub cpu: Vec4f,
    pub frame: Vec4f,
    pub warn: Vec4f,
    pub err: Vec4f,
    pub ok: Vec4f,
}

fn mix(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    Vec4f { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, z: a.z + (b.z - a.z) * t, w: 1.0 }
}

impl Palette {
    pub fn new(tok: &ShellTokens) -> Palette {
        let fg = alpha(tok.popups.text, 1.0);
        let bg = alpha(tok.popups.background, 1.0);
        Palette {
            fg,
            secondary: mix(bg, fg, 0.78),
            tertiary: mix(bg, fg, 0.56),
            well: alpha(fg, 0.05),
            rule: alpha(fg, 0.10),
            accent: alpha(tok.bar.active, 1.0),
            tier: Vec4f { x: 0.74, y: 0.56, z: 1.0, w: 1.0 },
            cpu: Vec4f { x: 0.40, y: 0.78, z: 0.98, w: 1.0 },
            frame: Vec4f { x: 0.45, y: 0.85, z: 0.55, w: 1.0 },
            warn: Vec4f { x: 1.0, y: 0.76, z: 0.34, w: 1.0 },
            err: Vec4f { x: 1.0, y: 0.45, z: 0.40, w: 1.0 },
            ok: Vec4f { x: 0.45, y: 0.85, z: 0.5, w: 1.0 },
        }
    }
}

/// x of board time `at` in `plot` whose right edge is `now`.
fn x_of(plot: Rect, at: f64, now: f64) -> f64 {
    plot.pos.x + plot.size.x - (now - at) / WINDOW_S * plot.size.x
}

/// A well: its background, `label` at the top left, `value` at the top right, and a foot line
/// under the plot: legend swatches (colour, text) on the left, a note on the right. Returns the
/// plot rect between them.
#[allow(clippy::too_many_arguments)]
pub fn well(d: &mut ShellDraw, cx: &mut Cx2d, tok: &ShellTokens, p: &Palette, r: Rect, label: &str, value: &str, value_color: Vec4f, legend: &[(Vec4f, &str)], note: Option<(&str, Vec4f)>) -> Rect {
    d.bordered(cx, r, p.well, p.rule, p.rule, 0.0, 1.0);
    let head = rect(r.pos.x + 8.0, r.pos.y + 5.0, r.size.x - 16.0, 16.0);
    let vw = d.measure(cx, true, tok.font.body_small, value);
    d.label_elided(cx, rect(head.pos.x, head.pos.y, (head.size.x - vw - 6.0).max(0.0), head.size.y), false, tok.font.caption, p.secondary, HAlign::Left, label);
    d.label(cx, head, true, tok.font.body_small, value_color, HAlign::Right, value);
    let foot = rect(r.pos.x + 8.0, r.pos.y + r.size.y - 16.0, r.size.x - 16.0, 13.0);
    let small = tok.font.caption * 0.9;
    // the note keeps its width; the legend is elided into what is left
    let nw = note.map_or(0.0, |(t, _)| d.measure(cx, false, small, t) + 8.0);
    let end = foot.pos.x + foot.size.x - nw;
    let mut x = foot.pos.x;
    for (c, t) in legend {
        if end - x < 24.0 {
            break;
        }
        d.solid(cx, rect(x, foot.pos.y + 4.0, 6.0, 6.0), *c);
        let tw = d.measure(cx, false, small, t).min(end - x - 9.0);
        d.label_elided(cx, rect(x + 9.0, foot.pos.y, tw, foot.size.y), false, small, p.tertiary, HAlign::Left, t);
        x += 9.0 + tw + 10.0;
    }
    if let Some((t, c)) = note {
        d.label(cx, foot, false, small, c, HAlign::Right, t);
    }
    rect(r.pos.x + 1.0, r.pos.y + 24.0, r.size.x - 2.0, r.size.y - 24.0 - 19.0)
}

/// A trace of `(at, value)` samples on `0..max`; samples further apart than `gap` s are not
/// joined.
#[allow(clippy::too_many_arguments)]
pub fn trace(g: &mut DrawDevhubTrace, cx: &mut Cx2d, plot: Rect, points: &mut dyn Iterator<Item = (f64, f32)>, now: f64, max: f32, gap: f64, color: Vec4f, fill: f32) {
    if plot.size.x < 4.0 || plot.size.y < 4.0 {
        return;
    }
    let bottom = plot.pos.y + plot.size.y;
    let at = |t: f64, v: f32| dvec2(x_of(plot, t, now), bottom - ((v / max.max(1e-6)).clamp(0.0, 1.0) as f64) * (plot.size.y - 2.0));
    g.color = color;
    g.fill_alpha = fill;
    g.line_width = 1.4;
    let mut prev: Option<(f64, Vec2d)> = None;
    for (t, v) in points {
        let here = at(t, v);
        if let Some((pt, a)) = prev {
            if t - pt <= gap && here.x > plot.pos.x - 40.0 {
                let (x0, x1) = (a.x.max(plot.pos.x), here.x.min(plot.pos.x + plot.size.x));
                if x1 > x0 {
                    let top = a.y.min(here.y) - 2.0;
                    let quad = rect(a.x, top, (here.x - a.x).max(0.5), bottom - top);
                    g.line_a = Vec2f { x: 0.0, y: (a.y - top) as f32 };
                    g.line_b = Vec2f { x: (here.x - a.x) as f32, y: (here.y - top) as f32 };
                    g.plot_top = (plot.pos.y - top) as f32;
                    g.plot_height = plot.size.y as f32;
                    g.draw_abs(cx, quad);
                }
            }
        }
        prev = Some((t, here));
    }
}

/// The frame budget for frames that come about every `typical` ms: the display rate they keep
/// (60, 90, 120, 144 Hz), else 60 Hz.
pub fn budget_ms(frames: &[Frame]) -> f32 {
    let mut iv: Vec<f32> = frames.iter().rev().take(120).map(|f| f.interval_ms).filter(|i| *i > 0.5 && *i < 100.0).collect();
    if iv.len() < 10 {
        return 1000.0 / 60.0;
    }
    iv.sort_by(|a, b| a.total_cmp(b));
    let median = iv[iv.len() / 2];
    [1000.0 / 144.0, 1000.0 / 120.0, 1000.0 / 90.0, 1000.0 / 60.0].into_iter().find(|b| (median - b).abs() <= b * 0.12).unwrap_or(1000.0 / 60.0)
}

/// A frame is an idle gap (the app drew nothing for a while), not a slow frame: long, and it
/// worked a small part of it.
pub fn idle_gap(f: &Frame) -> bool {
    f.interval_ms > 250.0 && f.work_ms < f.interval_ms * 0.5
}

/// Frame times: per pixel column the slowest frame's interval (light) with its work (solid),
/// hitches in the warning colours, and the budget's guide line.
#[allow(clippy::too_many_arguments)]
pub fn frame_bars(d: &mut ShellDraw, cx: &mut Cx2d, tok: &ShellTokens, p: &Palette, plot: Rect, frames: &std::collections::VecDeque<Frame>, now: f64, budget: f32) {
    if plot.size.x < 4.0 || plot.size.y < 4.0 {
        return;
    }
    let peak = frames.iter().filter(|f| !idle_gap(f) && now - f.at <= WINDOW_S).map(|f| f.interval_ms).fold(0.0f32, f32::max);
    let top_ms = (budget * 2.0).max(peak.min(budget * 6.0)) * 1.1;
    let bottom = plot.pos.y + plot.size.y;
    let y_of = |ms: f32| bottom - ((ms / top_ms).clamp(0.0, 1.0) as f64) * (plot.size.y - 2.0);
    let cols = plot.size.x.floor().max(1.0) as usize;
    let mut col: Vec<(f32, f32)> = vec![(0.0, 0.0); cols];
    for f in frames.iter() {
        if idle_gap(f) {
            continue;
        }
        let x = x_of(plot, f.at, now) - plot.pos.x;
        if x < 0.0 || x >= plot.size.x {
            continue;
        }
        let c = &mut col[(x as usize).min(cols - 1)];
        if f.interval_ms > c.0 {
            *c = (f.interval_ms, f.work_ms.min(f.interval_ms));
        }
    }
    for (i, (iv, work)) in col.iter().enumerate() {
        if *iv <= 0.0 {
            continue;
        }
        let x = plot.pos.x + i as f64;
        let color = if *iv > budget * 3.0 { p.err } else if *iv > budget * 1.5 { p.warn } else { p.frame };
        let yi = y_of(*iv);
        d.solid(cx, rect(x, yi, 1.0, bottom - yi), alpha(color, 0.30));
        let yw = y_of(*work);
        d.solid(cx, rect(x, yw, 1.0, bottom - yw), alpha(color, 0.85));
    }
    // the budget's guide
    let gy = y_of(budget).round();
    let mut x = plot.pos.x;
    while x < plot.pos.x + plot.size.x {
        d.solid(cx, rect(x, gy, 3.0f64.min(plot.pos.x + plot.size.x - x), 1.0), alpha(p.fg, 0.30));
        x += 6.0;
    }
    d.label(cx, rect(plot.pos.x + 3.0, gy - 11.0, 40.0, 10.0), false, tok.font.caption * 0.85, p.tertiary, HAlign::Left, &format!("{:.1}", budget));
}

/// A quiet note in the middle of a plot (nothing recorded yet).
pub fn note(d: &mut ShellDraw, cx: &mut Cx2d, tok: &ShellTokens, p: &Palette, plot: Rect, s: &str) {
    d.label_elided(cx, plot, false, tok.font.caption, p.tertiary, HAlign::Center, s);
}
