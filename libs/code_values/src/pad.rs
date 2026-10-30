//! A 2D point ([`Control::Point`], `vec2(x, y)`) dragged on a pad: y runs
//! down, as a picture's does. The pad's range is fixed while it is open
//! (scroll zooms); shift snaps to the grid.

use crate::data::*;
use crate::*;

pub(crate) const PAD_W: f64 = 248.0;
pub(crate) const PAD_H: f64 = 284.0;
const HEAD: f64 = 26.0;
const FOOT: f64 = 18.0;

#[derive(Clone, Debug)]
pub(crate) struct PadDrag {
    /// From the pointer to the dot, as pressed (a press on the dot moves
    /// it without a jump).
    grab: DVec2,
    written: [String; 2],
}

#[derive(Clone, Debug)]
pub(crate) struct PadState {
    pub lo: [f64; 2],
    pub hi: [f64; 2],
    pub hover: bool,
    pub drag: Option<PadDrag>,
}

impl PadState {
    /// A pad around `v`: a picture's quadrant for positive coordinates of
    /// some size, else symmetric about zero.
    pub fn around(v: [f64; 2]) -> Self {
        let m = v[0].abs().max(v[1].abs());
        if v[0] >= 0.0 && v[1] >= 0.0 && m >= 4.0 {
            let e = nice_ceil(m * 1.25);
            Self { lo: [0.0, 0.0], hi: [e, e], hover: false, drag: None }
        } else {
            let e = nice_ceil((m * 1.3).max(1e-3)).max(if m == 0.0 { 1.0 } else { 0.0 });
            Self { lo: [-e, -e], hi: [e, e], hover: false, drag: None }
        }
    }
}

/// The pad's square inside the popup.
pub(crate) fn pad_area(rect: Rect) -> Rect {
    let side = (rect.size.x - 24.0).min(rect.size.y - HEAD - FOOT - 8.0);
    Rect { pos: dvec2(rect.pos.x + (rect.size.x - side) * 0.5, rect.pos.y + HEAD + 2.0), size: dvec2(side, side) }
}

fn to_screen(st: &PadState, area: Rect, v: [f64; 2]) -> DVec2 {
    dvec2(
        area.pos.x + (v[0] - st.lo[0]) / (st.hi[0] - st.lo[0]) * area.size.x,
        area.pos.y + (v[1] - st.lo[1]) / (st.hi[1] - st.lo[1]) * area.size.y,
    )
}

fn from_screen(st: &PadState, area: Rect, p: DVec2) -> [f64; 2] {
    let u = ((p.x - area.pos.x) / area.size.x).clamp(0.0, 1.0);
    let v = ((p.y - area.pos.y) / area.size.y).clamp(0.0, 1.0);
    [st.lo[0] + u * (st.hi[0] - st.lo[0]), st.lo[1] + v * (st.hi[1] - st.lo[1])]
}

impl CodeValues {
    pub(crate) fn draw_pad_popup(&mut self, cx: &mut Cx2d, site: usize, rect: Rect, st: &PadState) {
        self.boxed(cx, rect, vec4(0.08, 0.09, 0.11, 1.0), vec4(1.0, 1.0, 1.0, 0.16), 6.0);
        let start = self.sites.get(site).map_or(0, |s| s.range.start);
        let items = vector_items(&self.source, start);
        let area = pad_area(rect);
        self.boxed(cx, area, vec4(0.047, 0.055, 0.067, 1.0), vec4(1.0, 1.0, 1.0, 0.10), 3.0);
        // The grid: ten steps, zero brighter.
        let step = nice_step(st.hi[0] - st.lo[0]);
        for axis in 0..2 {
            let mut v = (st.lo[axis] / step).ceil() * step;
            while v <= st.hi[axis] + 1e-9 {
                let zero = v.abs() < step * 1e-6;
                let c = if zero { vec4(1.0, 1.0, 1.0, 0.22) } else { vec4(1.0, 1.0, 1.0, 0.06) };
                if axis == 0 {
                    let x = to_screen(st, area, [v, st.lo[1]]).x;
                    self.solid(cx, Rect { pos: dvec2(x.round(), area.pos.y + 1.0), size: dvec2(1.0, area.size.y - 2.0) }, c);
                } else {
                    let y = to_screen(st, area, [st.lo[0], v]).y;
                    self.solid(cx, Rect { pos: dvec2(area.pos.x + 1.0, y.round()), size: dvec2(area.size.x - 2.0, 1.0) }, c);
                }
                v += step;
            }
        }
        self.draw_glyph.color = vec4(0.5, 0.53, 0.58, 1.0);
        self.draw_glyph.draw_abs(cx, area.pos + dvec2(4.0, 3.0), &format!("{}, {}", fmt_axis(st.lo[0]), fmt_axis(st.lo[1])));
        let hi = format!("{}, {}", fmt_axis(st.hi[0]), fmt_axis(st.hi[1]));
        let w = self.text_w(cx, &hi, true);
        self.draw_glyph.draw_abs(cx, area.pos + area.size - dvec2(w + 4.0, 13.0), &hi);
        if items.len() == 2 {
            let p = to_screen(st, area, [items[0].1, items[1].1]);
            let inside = area.contains(p);
            let p = dvec2(p.x.clamp(area.pos.x, area.pos.x + area.size.x), p.y.clamp(area.pos.y, area.pos.y + area.size.y));
            // A crosshair through it, the dot.
            let teal = vec4(0.31, 0.79, 0.69, 0.35);
            self.solid(cx, Rect { pos: dvec2(area.pos.x + 1.0, p.y.round()), size: dvec2(area.size.x - 2.0, 1.0) }, teal);
            self.solid(cx, Rect { pos: dvec2(p.x.round(), area.pos.y + 1.0), size: dvec2(1.0, area.size.y - 2.0) }, teal);
            let r = if st.hover || st.drag.is_some() { 6.5 } else { 5.5 };
            self.dot(cx, p, r + 1.5, vec4(0.0, 0.0, 0.0, 0.7));
            self.dot(cx, p, r, if inside { vec4(1.0, 0.48, 0.18, 1.0) } else { vec4(1.0, 0.48, 0.18, 0.5) });
            // The header: x … y …
            let mut x = rect.pos.x + 10.0;
            for (k, name) in ["x", "y"].iter().enumerate() {
                self.draw_glyph.color = Vec4f::from_u32((crate::gizmo::AXIS_INK[k] << 8) | 0xff);
                self.draw_glyph.draw_abs(cx, dvec2(x, rect.pos.y + 8.0), name);
                let t = self.source[items[k].0.clone()].to_string();
                self.draw_label.color = vec4(0.88, 0.9, 0.93, 1.0);
                self.draw_label.draw_abs(cx, dvec2(x + 9.0, rect.pos.y + 7.0), &t);
                x += 9.0 + self.text_w(cx, &t, false) + 14.0;
            }
        }
        self.draw_glyph.color = vec4(0.5, 0.53, 0.58, 1.0);
        self.draw_glyph.draw_abs(cx, dvec2(rect.pos.x + 10.0, rect.pos.y + rect.size.y - FOOT + 3.0), "drag · shift snaps · scroll zooms · y runs down");
    }

    pub(crate) fn pad_event(&mut self, cx: &mut Cx, event: &Event, site: usize, rect: Rect, mut st: PadState) -> CodeValuesResponse {
        let mut r = CodeValuesResponse::default();
        let start = self.sites.get(site).map_or(0, |s| s.range.start);
        let items = vector_items(&self.source, start);
        let area = pad_area(rect);
        let set = |this: &mut Self, st: PadState| this.popup = Some(Popup::Pad { site, rect, pad: st });
        if items.len() != 2 {
            if let Event::MouseDown(_) | Event::KeyDown(_) = event {
                self.close(&mut r);
                r.consumed = true;
                r.redraw = true;
            }
            return r;
        }
        let at = to_screen(&st, area, [items[0].1, items[1].1]);
        match event {
            Event::MouseDown(e) if !rect.contains(e.abs) => {
                self.close(&mut r);
                r.consumed = true;
                r.redraw = true;
            }
            Event::MouseDown(e) => {
                r.consumed = true;
                if area.contains(e.abs) {
                    let grab = if (e.abs - at).length() < 10.0 { at - e.abs } else { dvec2(0.0, 0.0) };
                    let span = items[0].0.start..items[1].0.end;
                    let text = self.source[span.clone()].to_string();
                    let id = self.next_gesture();
                    r.focus = Some(span.start);
                    self.gesture = Some(Gesture { id, span, last: text.clone(), original: text, scrub: None });
                    let d = PadDrag { grab, written: [self.source[items[0].0.clone()].to_string(), self.source[items[1].0.clone()].to_string()] };
                    self.pad_move(&mut r, &st, area, &d, e.abs, e.modifiers.shift, false);
                    st.drag = Some(d);
                    set(self, st);
                    r.redraw = true;
                }
            }
            Event::MouseMove(e) => {
                r.consumed = true;
                if let Some(d) = st.drag.clone() {
                    self.pad_move(&mut r, &st, area, &d, e.abs, e.modifiers.shift, false);
                    r.redraw = true;
                } else {
                    let over = (e.abs - at).length() < 10.0;
                    if over != st.hover {
                        st.hover = over;
                        set(self, st);
                        r.redraw = true;
                    }
                    if area.contains(e.abs) {
                        cx.set_cursor(if over { MouseCursor::Hand } else { MouseCursor::Crosshair });
                    }
                }
            }
            Event::MouseUp(e) => {
                r.consumed = true;
                if let Some(d) = st.drag.take() {
                    self.pad_move(&mut r, &st, area, &d, e.abs, e.modifiers.shift, true);
                    self.gesture = None;
                    set(self, st);
                    r.redraw = true;
                }
            }
            Event::Scroll(e) if rect.contains(e.abs) => {
                // Zoom about the middle: a wheel notch (32+ points) is one step, a touchpad's small deltas a fraction of one.
                let step = if e.scroll.y > 0.0 { 1.25 } else if e.scroll.y < 0.0 { 0.8 } else { 1.0 };
                let k = f64::powf(step, (e.scroll.y.abs() / 32.0).min(1.0));
                for a in 0..2 {
                    let c = (st.lo[a] + st.hi[a]) * 0.5;
                    let h = (st.hi[a] - st.lo[a]) * 0.5 * k;
                    st.lo[a] = c - h;
                    st.hi[a] = c + h;
                }
                set(self, st);
                r.consumed = true;
                r.redraw = true;
            }
            Event::KeyDown(k) if k.key_code == KeyCode::Escape || k.key_code == KeyCode::ReturnKey => {
                if st.drag.is_some() && k.key_code == KeyCode::Escape {
                    // Back to where the drag began, and closed.
                    self.revert(&mut r);
                } else {
                    self.close(&mut r);
                }
                r.consumed = true;
                r.redraw = true;
            }
            Event::Scroll(_) | Event::KeyDown(_) | Event::KeyUp(_) | Event::TextInput(_) => r.consumed = true,
            _ => {}
        }
        if r.redraw {
            self.pop_list.redraw(cx);
        }
        r
    }

    fn pad_move(&mut self, r: &mut CodeValuesResponse, st: &PadState, area: Rect, d: &PadDrag, abs: DVec2, shift: bool, done: bool) {
        let Some(g) = self.gesture.clone() else { return };
        let v = from_screen(st, area, abs + d.grab);
        let span = st.hi[0] - st.lo[0];
        let step = nice_step(span);
        let text = |k: usize| if shift { fmt_snapped(v[k], step) } else { fmt_like(v[k], &d.written[k], span) };
        let now = crate::gizmo::vector_like(&self.source, g.span.start);
        if now.len() != 2 {
            return;
        }
        let sep = self.source[now[0].0.end..now[1].0.start].to_string();
        let out = format!("{}{sep}{}", text(0), text(1));
        if done && g.last == g.original && out == g.original {
            self.gesture = None;
            return;
        }
        self.write(r, out, done);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_frame_their_point() {
        let st = PadState::around([960.0, 540.0]);
        assert_eq!((st.lo, st.hi), ([0.0, 0.0], [2000.0, 2000.0]));
        let st = PadState::around([0.0, -40.0]);
        assert_eq!((st.lo, st.hi), ([-100.0, -100.0], [100.0, 100.0]));
        let st = PadState::around([0.0, 0.0]);
        assert_eq!((st.lo, st.hi), ([-1.0, -1.0], [1.0, 1.0]));
        let area = Rect { pos: dvec2(10.0, 20.0), size: dvec2(200.0, 200.0) };
        let st = PadState::around([0.25, 0.5]);
        let p = to_screen(&st, area, [0.25, 0.5]);
        let back = from_screen(&st, area, p);
        assert!((back[0] - 0.25).abs() < 1e-9 && (back[1] - 0.5).abs() < 1e-9);
        // y runs down.
        assert!(to_screen(&st, area, [0.0, 0.5]).y > to_screen(&st, area, [0.0, 0.0]).y);
    }
}
