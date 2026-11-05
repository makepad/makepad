//! Floating boxes (the tooltips, the inbox list) on a popup overlay over every panel.
//!
//! A panel after a Fill sibling (the right column, the landing strip) draws before that fill
//! resolves and is moved after it, and its parent view clips it: a box drawn inline would be
//! clipped to the column or painted under the panels drawn after it. On an overlay of its own,
//! placed from the host area's final rect (never from where the host drew), it floats over all
//! of them wherever the host ends up.

use crate::timeline::DrawBar;
use makepad_widgets::*;

/// the room kept round a box for its shadow (the overlay clips at its origin)
const MARGIN: f64 = 12.0;

/// One floating box of lines, each in its own colour.
pub struct Float<'a> {
    /// the box's top left, in the host's drawn coordinates
    pub at: DVec2,
    pub w: f64,
    pub lines: &'a [(String, Vec4f)],
    pub line_h: f64,
    pub pad: f64,
    pub bg: Vec4f,
    /// the shadow's alpha
    pub shadow: f32,
}

impl Float<'_> {
    pub fn height(lines: usize, line_h: f64, pad: f64) -> f64 {
        lines as f64 * line_h + pad * 2.0 - 2.0
    }
}

/// Draws `float` on `list`, placed from `host`'s final rect (`drawn` is where the host drew).
/// Called on every draw of the host, with None too, so a box that went away is cleared.
pub fn draw_float(cx: &mut Cx2d, list: &mut Option<DrawList2d>, host: Area, drawn: Rect, float: Option<Float>, bar: &mut DrawBar, text: &mut DrawText) {
    let mut l = list.take().unwrap_or_else(|| DrawList2d::new(cx));
    l.begin_popup_overlay(cx);
    let pass = cx.current_pass_size();
    cx.begin_root_turtle(pass, Layout::flow_down());
    let mut shift = DVec2::default();
    if let Some(f) = float {
        // drawn from the overlay's origin, then moved as a whole to its place
        shift = f.at - drawn.pos - dvec2(MARGIN, MARGIN);
        let (x, y) = (MARGIN, MARGIN);
        let h = Float::height(f.lines.len(), f.line_h, f.pad);
        bar.glowing(cx, Rect { pos: dvec2(x, y), size: dvec2(f.w, h) }, f.bg, vec4(0.0, 0.0, 0.0, 0.0), vec4(0.0, 0.0, 0.0, f.shadow), 6.0, 4.0);
        let max_w = f.w - f.pad * 2.0;
        for (i, (s, c)) in f.lines.iter().enumerate() {
            let at = dvec2(x + f.pad, y + f.pad - 1.0 + i as f64 * f.line_h);
            text.color = *c;
            let w = text.layout(cx, 0.0, 0.0, None, false, Align::default(), s).size_in_lpxs.width as f64;
            if w > max_w {
                cx.push_clip_rect(Rect { pos: dvec2(at.x, at.y - 4.0), size: dvec2(max_w, f.line_h + 6.0) });
                text.draw_abs(cx, at, s);
                cx.pop_clip_rect();
            } else {
                text.draw_abs(cx, at, s);
            }
        }
    }
    cx.end_pass_sized_turtle_with_shift(host, shift);
    l.end(cx);
    *list = Some(l);
}
