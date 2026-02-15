use std::f32::consts::PI;

#[derive(Clone, Copy, Debug)]
pub enum Winding {
    CCW,
    CW,
}

#[derive(Clone, Copy, Debug)]
pub enum FillRule {
    NonZero,
    EvenOdd,
}

#[derive(Clone, Copy, Debug)]
pub enum LineCap {
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug)]
pub enum LineJoin {
    Miter,
    Round,
    Bevel,
}

#[derive(Clone, Debug)]
pub enum PathCmd {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    BezierTo(f32, f32, f32, f32, f32, f32),
    Close,
    Winding(Winding),
}

#[derive(Default, Clone, Debug)]
pub struct VectorPath {
    pub cmds: Vec<PathCmd>,
}

impl VectorPath {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn clear(&mut self) {
        self.cmds.clear();
    }

    pub fn move_to(&mut self, x: f32, y: f32) {
        self.cmds.push(PathCmd::MoveTo(x, y));
    }

    pub fn line_to(&mut self, x: f32, y: f32) {
        self.cmds.push(PathCmd::LineTo(x, y));
    }

    pub fn bezier_to(&mut self, cx1: f32, cy1: f32, cx2: f32, cy2: f32, x: f32, y: f32) {
        self.cmds.push(PathCmd::BezierTo(cx1, cy1, cx2, cy2, x, y));
    }

    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        if let Some(last) = self.last_pos() {
            let (lx, ly) = last;
            // convert quadratic to cubic
            let cx1 = lx + 2.0 / 3.0 * (cx - lx);
            let cy1 = ly + 2.0 / 3.0 * (cy - ly);
            let cx2 = x + 2.0 / 3.0 * (cx - x);
            let cy2 = y + 2.0 / 3.0 * (cy - y);
            self.bezier_to(cx1, cy1, cx2, cy2, x, y);
        }
    }

    pub fn close(&mut self) {
        self.cmds.push(PathCmd::Close);
    }

    pub fn winding(&mut self, w: Winding) {
        self.cmds.push(PathCmd::Winding(w));
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.move_to(x, y);
        self.line_to(x + w, y);
        self.line_to(x + w, y + h);
        self.line_to(x, y + h);
        self.close();
    }

    pub fn rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        if r < 0.1 {
            self.rect(x, y, w, h);
            return;
        }
        let r = r.min(w * 0.5).min(h * 0.5);
        // top edge
        self.move_to(x + r, y);
        self.line_to(x + w - r, y);
        // top-right corner: center at (x+w-r, y+r), arc from -90 to 0
        self.arc_to_bezier(x + w - r, y + r, r, r, -PI * 0.5, PI * 0.5);
        // right edge
        self.line_to(x + w, y + h - r);
        // bottom-right corner: center at (x+w-r, y+h-r), arc from 0 to 90
        self.arc_to_bezier(x + w - r, y + h - r, r, r, 0.0, PI * 0.5);
        // bottom edge
        self.line_to(x + r, y + h);
        // bottom-left corner: center at (x+r, y+h-r), arc from 90 to 180
        self.arc_to_bezier(x + r, y + h - r, r, r, PI * 0.5, PI * 0.5);
        // left edge
        self.line_to(x, y + r);
        // top-left corner: center at (x+r, y+r), arc from 180 to 270
        self.arc_to_bezier(x + r, y + r, r, r, PI, PI * 0.5);
        self.close();
    }

    fn arc_to_bezier(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, start: f32, sweep: f32) {
        let n = ((sweep.abs() / (PI * 0.5)).ceil() as usize).max(1);
        let sweep_per = sweep / n as f32;
        let k = (4.0 / 3.0) * (sweep_per / 4.0).tan();
        for i in 0..n {
            let a0 = start + sweep_per * i as f32;
            let a1 = a0 + sweep_per;
            let (s0, c0) = a0.sin_cos();
            let (s1, c1) = a1.sin_cos();
            let x0 = cx + c0 * rx;
            let y0 = cy + s0 * ry;
            let x1 = cx + c1 * rx;
            let y1 = cy + s1 * ry;
            let dx0 = -s0 * rx;
            let dy0 = c0 * ry;
            let dx1 = -s1 * rx;
            let dy1 = c1 * ry;
            if i == 0 {
                if self.cmds.is_empty() {
                    self.move_to(x0, y0);
                } else {
                    self.line_to(x0, y0);
                }
            }
            self.bezier_to(
                x0 + dx0 * k,
                y0 + dy0 * k,
                x1 - dx1 * k,
                y1 - dy1 * k,
                x1,
                y1,
            );
        }
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32) {
        self.ellipse(cx, cy, r, r);
    }

    pub fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32) {
        self.move_to(cx + rx, cy);
        self.arc_to_bezier(cx, cy, rx, ry, 0.0, PI * 2.0);
        self.close();
    }

    fn last_pos(&self) -> Option<(f32, f32)> {
        for cmd in self.cmds.iter().rev() {
            match cmd {
                PathCmd::MoveTo(x, y)
                | PathCmd::LineTo(x, y)
                | PathCmd::BezierTo(_, _, _, _, x, y) => {
                    return Some((*x, *y));
                }
                _ => {}
            }
        }
        None
    }
}
