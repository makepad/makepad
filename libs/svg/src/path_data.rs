/// SVG path "d" attribute parser.
/// Parses M/L/H/V/C/S/Q/T/A/Z commands into VectorPath calls.
/// All relative commands converted to absolute. Arcs converted to cubic beziers.
use crate::path::VectorPath;
use std::f32::consts::PI;

pub fn parse_path_data(d: &str, path: &mut VectorPath) {
    let mut parser = PathDataParser::new(d);
    parser.parse(path);
}

struct PathDataParser<'a> {
    data: &'a [u8],
    pos: usize,
    // Current point
    cx: f32,
    cy: f32,
    // Start of current subpath (for Z)
    sx: f32,
    sy: f32,
    // Last control point (for S and T)
    last_ctrl_x: f32,
    last_ctrl_y: f32,
    last_cmd: u8,
}

impl<'a> PathDataParser<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            data: s.as_bytes(),
            pos: 0,
            cx: 0.0,
            cy: 0.0,
            sx: 0.0,
            sy: 0.0,
            last_ctrl_x: 0.0,
            last_ctrl_y: 0.0,
            last_cmd: 0,
        }
    }

    fn skip_whitespace_and_commas(&mut self) {
        while self.pos < self.data.len() {
            let c = self.data[self.pos];
            if c.is_ascii_whitespace() || c == b',' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_number(&mut self) -> Option<f32> {
        self.skip_whitespace_and_commas();
        if self.pos >= self.data.len() {
            return None;
        }

        let start = self.pos;
        let mut has_dot = false;
        let mut has_exp = false;
        let mut has_digit = false;

        // Optional sign
        if self.pos < self.data.len()
            && (self.data[self.pos] == b'+' || self.data[self.pos] == b'-')
        {
            self.pos += 1;
        }

        // Integer part
        while self.pos < self.data.len() && self.data[self.pos].is_ascii_digit() {
            has_digit = true;
            self.pos += 1;
        }

        // Decimal part
        if self.pos < self.data.len() && self.data[self.pos] == b'.' {
            has_dot = true;
            self.pos += 1;
            while self.pos < self.data.len() && self.data[self.pos].is_ascii_digit() {
                has_digit = true;
                self.pos += 1;
            }
        }

        if !has_digit {
            self.pos = start;
            return None;
        }

        // Exponent
        if self.pos < self.data.len()
            && (self.data[self.pos] == b'e' || self.data[self.pos] == b'E')
        {
            has_exp = true;
            self.pos += 1;
            if self.pos < self.data.len()
                && (self.data[self.pos] == b'+' || self.data[self.pos] == b'-')
            {
                self.pos += 1;
            }
            while self.pos < self.data.len() && self.data[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }

        let _ = has_dot;
        let _ = has_exp;
        let s = std::str::from_utf8(&self.data[start..self.pos]).ok()?;
        s.parse::<f32>().ok()
    }

    fn parse_flag(&mut self) -> Option<bool> {
        self.skip_whitespace_and_commas();
        if self.pos >= self.data.len() {
            return None;
        }
        match self.data[self.pos] {
            b'0' => {
                self.pos += 1;
                Some(false)
            }
            b'1' => {
                self.pos += 1;
                Some(true)
            }
            _ => None,
        }
    }

    fn at_number_start(&self) -> bool {
        if self.pos >= self.data.len() {
            return false;
        }
        let mut p = self.pos;
        // skip ws and commas
        while p < self.data.len() && (self.data[p].is_ascii_whitespace() || self.data[p] == b',') {
            p += 1;
        }
        if p >= self.data.len() {
            return false;
        }
        let c = self.data[p];
        c.is_ascii_digit() || c == b'.' || c == b'-' || c == b'+'
    }

    fn parse(&mut self, path: &mut VectorPath) {
        while self.pos < self.data.len() {
            self.skip_whitespace_and_commas();
            if self.pos >= self.data.len() {
                break;
            }

            let c = self.data[self.pos];
            if c.is_ascii_alphabetic() {
                self.pos += 1;
                self.dispatch_command(c, path);
            } else if self.at_number_start() && self.last_cmd != 0 {
                // Implicit repeat of last command
                self.dispatch_command(self.last_cmd, path);
            } else {
                self.pos += 1; // skip unknown
            }
        }
    }

    fn dispatch_command(&mut self, cmd: u8, path: &mut VectorPath) {
        let is_rel = cmd.is_ascii_lowercase();
        match cmd.to_ascii_uppercase() {
            b'M' => self.cmd_move_to(is_rel, path),
            b'L' => self.cmd_line_to(is_rel, path),
            b'H' => self.cmd_horizontal_line(is_rel, path),
            b'V' => self.cmd_vertical_line(is_rel, path),
            b'C' => self.cmd_cubic(is_rel, path),
            b'S' => self.cmd_smooth_cubic(is_rel, path),
            b'Q' => self.cmd_quadratic(is_rel, path),
            b'T' => self.cmd_smooth_quadratic(is_rel, path),
            b'A' => self.cmd_arc(is_rel, path),
            b'Z' => self.cmd_close(path),
            _ => {}
        }
    }

    fn cmd_move_to(&mut self, is_rel: bool, path: &mut VectorPath) {
        if let (Some(mut x), Some(mut y)) = (self.parse_number(), self.parse_number()) {
            if is_rel {
                x += self.cx;
                y += self.cy;
            }
            path.move_to(x, y);
            self.cx = x;
            self.cy = y;
            self.sx = x;
            self.sy = y;
            self.last_ctrl_x = x;
            self.last_ctrl_y = y;
            self.last_cmd = if is_rel { b'l' } else { b'L' }; // subsequent coords are lineTo

            // Implicit lineto for extra coordinate pairs
            while self.at_number_start() {
                if let (Some(mut x), Some(mut y)) = (self.parse_number(), self.parse_number()) {
                    if is_rel {
                        x += self.cx;
                        y += self.cy;
                    }
                    path.line_to(x, y);
                    self.cx = x;
                    self.cy = y;
                    self.last_ctrl_x = x;
                    self.last_ctrl_y = y;
                } else {
                    break;
                }
            }
        }
    }

    fn cmd_line_to(&mut self, is_rel: bool, path: &mut VectorPath) {
        while self.at_number_start() {
            if let (Some(mut x), Some(mut y)) = (self.parse_number(), self.parse_number()) {
                if is_rel {
                    x += self.cx;
                    y += self.cy;
                }
                path.line_to(x, y);
                self.cx = x;
                self.cy = y;
                self.last_ctrl_x = x;
                self.last_ctrl_y = y;
            } else {
                break;
            }
        }
        self.last_cmd = if is_rel { b'l' } else { b'L' };
    }

    fn cmd_horizontal_line(&mut self, is_rel: bool, path: &mut VectorPath) {
        while self.at_number_start() {
            if let Some(mut x) = self.parse_number() {
                if is_rel {
                    x += self.cx;
                }
                path.line_to(x, self.cy);
                self.cx = x;
                self.last_ctrl_x = x;
                self.last_ctrl_y = self.cy;
            } else {
                break;
            }
        }
        self.last_cmd = if is_rel { b'h' } else { b'H' };
    }

    fn cmd_vertical_line(&mut self, is_rel: bool, path: &mut VectorPath) {
        while self.at_number_start() {
            if let Some(mut y) = self.parse_number() {
                if is_rel {
                    y += self.cy;
                }
                path.line_to(self.cx, y);
                self.cy = y;
                self.last_ctrl_x = self.cx;
                self.last_ctrl_y = y;
            } else {
                break;
            }
        }
        self.last_cmd = if is_rel { b'v' } else { b'V' };
    }

    fn cmd_cubic(&mut self, is_rel: bool, path: &mut VectorPath) {
        while self.at_number_start() {
            let nums: Vec<Option<f32>> = (0..6).map(|_| self.parse_number()).collect();
            if nums.iter().all(|n| n.is_some()) {
                let mut x1 = nums[0].unwrap();
                let mut y1 = nums[1].unwrap();
                let mut x2 = nums[2].unwrap();
                let mut y2 = nums[3].unwrap();
                let mut x = nums[4].unwrap();
                let mut y = nums[5].unwrap();
                if is_rel {
                    x1 += self.cx;
                    y1 += self.cy;
                    x2 += self.cx;
                    y2 += self.cy;
                    x += self.cx;
                    y += self.cy;
                }
                path.bezier_to(x1, y1, x2, y2, x, y);
                self.last_ctrl_x = x2;
                self.last_ctrl_y = y2;
                self.cx = x;
                self.cy = y;
            } else {
                break;
            }
        }
        self.last_cmd = if is_rel { b'c' } else { b'C' };
    }

    fn cmd_smooth_cubic(&mut self, is_rel: bool, path: &mut VectorPath) {
        while self.at_number_start() {
            let nums: Vec<Option<f32>> = (0..4).map(|_| self.parse_number()).collect();
            if nums.iter().all(|n| n.is_some()) {
                let mut x2 = nums[0].unwrap();
                let mut y2 = nums[1].unwrap();
                let mut x = nums[2].unwrap();
                let mut y = nums[3].unwrap();
                if is_rel {
                    x2 += self.cx;
                    y2 += self.cy;
                    x += self.cx;
                    y += self.cy;
                }
                // Reflect last control point
                let x1 = 2.0 * self.cx - self.last_ctrl_x;
                let y1 = 2.0 * self.cy - self.last_ctrl_y;
                path.bezier_to(x1, y1, x2, y2, x, y);
                self.last_ctrl_x = x2;
                self.last_ctrl_y = y2;
                self.cx = x;
                self.cy = y;
            } else {
                break;
            }
        }
        self.last_cmd = if is_rel { b's' } else { b'S' };
    }

    fn cmd_quadratic(&mut self, is_rel: bool, path: &mut VectorPath) {
        while self.at_number_start() {
            let nums: Vec<Option<f32>> = (0..4).map(|_| self.parse_number()).collect();
            if nums.iter().all(|n| n.is_some()) {
                let mut qx = nums[0].unwrap();
                let mut qy = nums[1].unwrap();
                let mut x = nums[2].unwrap();
                let mut y = nums[3].unwrap();
                if is_rel {
                    qx += self.cx;
                    qy += self.cy;
                    x += self.cx;
                    y += self.cy;
                }
                path.quad_to(qx, qy, x, y);
                self.last_ctrl_x = qx;
                self.last_ctrl_y = qy;
                self.cx = x;
                self.cy = y;
            } else {
                break;
            }
        }
        self.last_cmd = if is_rel { b'q' } else { b'Q' };
    }

    fn cmd_smooth_quadratic(&mut self, is_rel: bool, path: &mut VectorPath) {
        while self.at_number_start() {
            if let (Some(mut x), Some(mut y)) = (self.parse_number(), self.parse_number()) {
                if is_rel {
                    x += self.cx;
                    y += self.cy;
                }
                // Reflect last quadratic control point
                let qx = 2.0 * self.cx - self.last_ctrl_x;
                let qy = 2.0 * self.cy - self.last_ctrl_y;
                path.quad_to(qx, qy, x, y);
                self.last_ctrl_x = qx;
                self.last_ctrl_y = qy;
                self.cx = x;
                self.cy = y;
            } else {
                break;
            }
        }
        self.last_cmd = if is_rel { b't' } else { b'T' };
    }

    fn cmd_arc(&mut self, is_rel: bool, path: &mut VectorPath) {
        while self.at_number_start() {
            let rx = match self.parse_number() {
                Some(v) => v,
                None => break,
            };
            let ry = match self.parse_number() {
                Some(v) => v,
                None => break,
            };
            let x_rotation = match self.parse_number() {
                Some(v) => v,
                None => break,
            };
            let large_arc = match self.parse_flag() {
                Some(v) => v,
                None => break,
            };
            let sweep = match self.parse_flag() {
                Some(v) => v,
                None => break,
            };
            let mut x = match self.parse_number() {
                Some(v) => v,
                None => break,
            };
            let mut y = match self.parse_number() {
                Some(v) => v,
                None => break,
            };

            if is_rel {
                x += self.cx;
                y += self.cy;
            }

            arc_to_beziers(
                path, self.cx, self.cy, rx, ry, x_rotation, large_arc, sweep, x, y,
            );

            self.cx = x;
            self.cy = y;
            self.last_ctrl_x = x;
            self.last_ctrl_y = y;
        }
        self.last_cmd = if is_rel { b'a' } else { b'A' };
    }

    fn cmd_close(&mut self, path: &mut VectorPath) {
        path.close();
        self.cx = self.sx;
        self.cy = self.sy;
        self.last_ctrl_x = self.sx;
        self.last_ctrl_y = self.sy;
        self.last_cmd = b'Z';
    }
}

/// Convert an SVG arc to cubic bezier curves.
/// Implements the endpoint-to-center parameterization from SVG spec Appendix F.6.
fn arc_to_beziers(
    path: &mut VectorPath,
    x1: f32,
    y1: f32,
    mut rx: f32,
    mut ry: f32,
    x_rotation_deg: f32,
    large_arc: bool,
    sweep: bool,
    x2: f32,
    y2: f32,
) {
    // Degenerate: current == endpoint
    if (x1 - x2).abs() < 1e-6 && (y1 - y2).abs() < 1e-6 {
        return;
    }

    rx = rx.abs();
    ry = ry.abs();

    // Degenerate: zero radius
    if rx < 1e-6 || ry < 1e-6 {
        path.line_to(x2, y2);
        return;
    }

    let phi = x_rotation_deg * PI / 180.0;
    let (sin_phi, cos_phi) = phi.sin_cos();

    // Step 1: Compute (x1', y1')
    let dx2 = (x1 - x2) / 2.0;
    let dy2 = (y1 - y2) / 2.0;
    let x1p = cos_phi * dx2 + sin_phi * dy2;
    let y1p = -sin_phi * dx2 + cos_phi * dy2;

    // Step 2: Compute (cx', cy')
    let x1p2 = x1p * x1p;
    let y1p2 = y1p * y1p;
    let rx2 = rx * rx;
    let ry2 = ry * ry;

    // Ensure radii are large enough
    let lambda = x1p2 / rx2 + y1p2 / ry2;
    if lambda > 1.0 {
        let lambda_sqrt = lambda.sqrt();
        rx *= lambda_sqrt;
        ry *= lambda_sqrt;
        let rx2_new = rx * rx;
        let ry2_new = ry * ry;
        // Recompute with corrected radii
        arc_to_beziers_inner(
            path, x1, y1, rx, ry, sin_phi, cos_phi, large_arc, sweep, x2, y2, x1p, y1p, rx2_new,
            ry2_new, x1p2, y1p2,
        );
    } else {
        arc_to_beziers_inner(
            path, x1, y1, rx, ry, sin_phi, cos_phi, large_arc, sweep, x2, y2, x1p, y1p, rx2, ry2,
            x1p2, y1p2,
        );
    }
}

fn arc_to_beziers_inner(
    path: &mut VectorPath,
    _x1: f32,
    _y1: f32,
    rx: f32,
    ry: f32,
    sin_phi: f32,
    cos_phi: f32,
    large_arc: bool,
    sweep: bool,
    x2: f32,
    y2: f32,
    x1p: f32,
    y1p: f32,
    rx2: f32,
    ry2: f32,
    x1p2: f32,
    y1p2: f32,
) {
    let num = (rx2 * ry2 - rx2 * y1p2 - ry2 * x1p2).max(0.0);
    let den = rx2 * y1p2 + ry2 * x1p2;
    let sq = if den > 1e-10 { (num / den).sqrt() } else { 0.0 };

    let sign = if large_arc == sweep { -1.0 } else { 1.0 };
    let cxp = sign * sq * (rx * y1p / ry);
    let cyp = sign * sq * -(ry * x1p / rx);

    // Step 3: Compute (cx, cy) from (cx', cy')
    let mx = (_x1 + x2) / 2.0;
    let my = (_y1 + y2) / 2.0;
    let cx = cos_phi * cxp - sin_phi * cyp + mx;
    let cy = sin_phi * cxp + cos_phi * cyp + my;

    // Step 4: Compute theta1 and dtheta
    let theta1 = angle_between(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut dtheta = angle_between(
        (x1p - cxp) / rx,
        (y1p - cyp) / ry,
        (-x1p - cxp) / rx,
        (-y1p - cyp) / ry,
    );

    if !sweep && dtheta > 0.0 {
        dtheta -= 2.0 * PI;
    } else if sweep && dtheta < 0.0 {
        dtheta += 2.0 * PI;
    }

    // Split into segments of at most PI/2
    let n_segs = ((dtheta.abs() / (PI * 0.5)).ceil() as usize).max(1);
    let d_per_seg = dtheta / n_segs as f32;
    let k = (4.0 / 3.0) * (d_per_seg / 4.0).tan();

    for i in 0..n_segs {
        let a0 = theta1 + d_per_seg * i as f32;
        let a1 = a0 + d_per_seg;

        let (s0, c0) = a0.sin_cos();
        let (s1, c1) = a1.sin_cos();

        // Control point tangents in ellipse-local space
        let ep1x = -rx * s0;
        let ep1y = ry * c0;
        let ep2x = -rx * s1;
        let ep2y = ry * c1;

        // Endpoints in ellipse-local space
        let e1x = rx * c0;
        let e1y = ry * s0;
        let e2x = rx * c1;
        let e2y = ry * s1;

        // Transform to world space
        let q1x = cos_phi * (e1x + k * ep1x) - sin_phi * (e1y + k * ep1y) + cx;
        let q1y = sin_phi * (e1x + k * ep1x) + cos_phi * (e1y + k * ep1y) + cy;
        let q2x = cos_phi * (e2x - k * ep2x) - sin_phi * (e2y - k * ep2y) + cx;
        let q2y = sin_phi * (e2x - k * ep2x) + cos_phi * (e2y - k * ep2y) + cy;
        let ex = cos_phi * e2x - sin_phi * e2y + cx;
        let ey = sin_phi * e2x + cos_phi * e2y + cy;

        path.bezier_to(q1x, q1y, q2x, q2y, ex, ey);
    }
}

fn angle_between(ux: f32, uy: f32, vx: f32, vy: f32) -> f32 {
    let n = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
    if n < 1e-10 {
        return 0.0;
    }
    let cos_a = ((ux * vx + uy * vy) / n).clamp(-1.0, 1.0);
    let sign = if ux * vy - uy * vx < 0.0 { -1.0 } else { 1.0 };
    sign * cos_a.acos()
}
