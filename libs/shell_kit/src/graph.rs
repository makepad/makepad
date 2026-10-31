//! Live line graphs drawn with plain quads: a trace is one quad per pair of neighbouring samples
//! whose shader draws the anti-aliased line and the soft fill under it, so an hour of samples is a
//! few hundred instances in one draw call. Shared by the devhub task view and the WM's
//! temperature panel.

use crate::ui::rect;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // One stretch of a trace: the line from line_a to line_b (quad pixels) and the fill below
    // it, fading toward the plot's bottom (anchored to the plot, so neighbours meet seamlessly).
    mod.widgets.DrawShellTrace = set_type_default() do #(DrawShellTrace::script_shader(vm)) {
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
pub struct DrawShellTrace {
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

/// The value and time ranges a trace is drawn against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    /// The times at the plot's left and right edges.
    pub t0: f64,
    pub t1: f64,
    /// The values at the plot's bottom and top.
    pub lo: f32,
    pub hi: f32,
}

impl Span {
    pub fn x(&self, plot: Rect, t: f64) -> f64 {
        plot.pos.x + (t - self.t0) / (self.t1 - self.t0).max(1e-9) * plot.size.x
    }

    pub fn y(&self, plot: Rect, v: f32) -> f64 {
        let f = ((v - self.lo) / (self.hi - self.lo).max(1e-6)).clamp(0.0, 1.0) as f64;
        plot.pos.y + plot.size.y - f * (plot.size.y - 2.0)
    }
}

/// A trace of `(at, value)` samples in time order against `span`; samples further apart than
/// `gap` s, and NaN samples, break the line.
#[allow(clippy::too_many_arguments)]
pub fn trace(g: &mut DrawShellTrace, cx: &mut Cx2d, plot: Rect, points: &mut dyn Iterator<Item = (f64, f32)>, span: Span, gap: f64, color: Vec4f, fill: f32, width: f32) {
    if plot.size.x < 4.0 || plot.size.y < 4.0 {
        return;
    }
    let bottom = plot.pos.y + plot.size.y;
    g.color = color;
    g.fill_alpha = fill;
    g.line_width = width;
    let mut prev: Option<(f64, Vec2d)> = None;
    for (t, v) in points {
        if v.is_nan() {
            prev = None;
            continue;
        }
        let here = dvec2(span.x(plot, t), span.y(plot, v));
        if let Some((pt, a)) = prev {
            let (left, right) = (plot.pos.x, plot.pos.x + plot.size.x);
            if t - pt <= gap && here.x > left && a.x < right && here.x > a.x {
                // clipped to the plot: the stretch is cut where it crosses an edge
                let at = |x: f64| a.y + (here.y - a.y) * (x - a.x) / (here.x - a.x);
                let a = if a.x < left { dvec2(left, at(left)) } else { a };
                let here = if here.x > right { dvec2(right, at(right)) } else { here };
                if here.x > a.x {
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
