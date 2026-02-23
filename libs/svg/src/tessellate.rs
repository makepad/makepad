use crate::path::*;
use std::f32::consts::PI;

// Output vertex: position + texcoord for AA + distance along stroke
#[derive(Clone, Copy, Default, Debug)]
pub struct VVertex {
    pub x: f32,
    pub y: f32,
    pub u: f32,
    pub v: f32,
    pub stroke_dist: f32,
    /// Maximum distance from this vertex to any other vertex it shares a triangle with.
    /// Used for early clip rejection in the vertex shader.
    pub clip_radius: f32,
}

impl VVertex {
    fn new(x: f32, y: f32, u: f32, v: f32) -> Self {
        Self {
            x,
            y,
            u,
            v,
            stroke_dist: 0.0,
            clip_radius: 0.0,
        }
    }
    fn with_dist(x: f32, y: f32, u: f32, v: f32, stroke_dist: f32) -> Self {
        Self {
            x,
            y,
            u,
            v,
            stroke_dist,
            clip_radius: 0.0,
        }
    }
}

// Internal point with computed direction/miter info
#[derive(Clone, Copy, Default, Debug)]
struct VPoint {
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,  // direction to next
    len: f32, // segment length
    dmx: f32,
    dmy: f32, // miter direction
    flags: u8,
}

const PT_CORNER: u8 = 1;
const PT_LEFT: u8 = 2;
const PT_BEVEL: u8 = 4;
const PT_INNERBEVEL: u8 = 8;

#[derive(Default, Debug)]
pub struct Tessellator {
    points: Vec<VPoint>,
    paths: Vec<SubPath>,
    cum_dists: Vec<f32>,
}

#[derive(Debug)]
struct SubPath {
    first: usize,
    count: usize,
    closed: bool,
    winding: Winding,
    has_explicit_winding: bool,
    convex: bool,
    nbevel: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FillRule {
    EvenOdd,
    NonZero,
}

impl Tessellator {
    /// Bounding box of flattened points: (min_x, min_y, max_x, max_y).
    /// Call after `flatten()`.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        let (mut min_x, mut min_y) = (f32::MAX, f32::MAX);
        let (mut max_x, mut max_y) = (f32::MIN, f32::MIN);
        for pt in &self.points {
            min_x = min_x.min(pt.x);
            min_y = min_y.min(pt.y);
            max_x = max_x.max(pt.x);
            max_y = max_y.max(pt.y);
        }
        (min_x, min_y, max_x, max_y)
    }

    pub fn flatten(&mut self, path: &VectorPath, tess_tol: f32) {
        self.points.clear();
        self.paths.clear();
        let dist_tol = 0.01;
        for cmd in &path.cmds {
            match *cmd {
                PathCmd::MoveTo(x, y) => {
                    self.paths.push(SubPath {
                        first: self.points.len(),
                        count: 0,
                        closed: false,
                        winding: Winding::CCW,
                        has_explicit_winding: false,
                        convex: false,
                        nbevel: 0,
                    });
                    self.add_point(x, y, PT_CORNER, dist_tol);
                }
                PathCmd::LineTo(x, y) => {
                    self.add_point(x, y, PT_CORNER, dist_tol);
                }
                PathCmd::BezierTo(cx1, cy1, cx2, cy2, x, y) => {
                    if let Some(last) = self.points.last().copied() {
                        self.tesselate_bezier(
                            last.x, last.y, cx1, cy1, cx2, cy2, x, y, 0, PT_CORNER, tess_tol,
                        );
                    }
                }
                PathCmd::Close => {
                    if let Some(p) = self.paths.last_mut() {
                        p.closed = true;
                    }
                }
                PathCmd::Winding(w) => {
                    if let Some(p) = self.paths.last_mut() {
                        p.winding = w;
                        p.has_explicit_winding = true;
                    }
                }
            }
        }
        self.prepare_points(dist_tol);
    }

    fn add_point(&mut self, x: f32, y: f32, flags: u8, dist_tol: f32) {
        if let Some(p) = self.paths.last_mut() {
            if p.count > 0 {
                if let Some(last) = self.points.last() {
                    let dx = x - last.x;
                    let dy = y - last.y;
                    if dx * dx + dy * dy < dist_tol * dist_tol {
                        self.points.last_mut().unwrap().flags |= flags;
                        return;
                    }
                }
            }
            self.points.push(VPoint {
                x,
                y,
                flags,
                ..Default::default()
            });
            p.count += 1;
        }
    }

    fn tesselate_bezier(
        &mut self,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        x3: f32,
        y3: f32,
        x4: f32,
        y4: f32,
        level: usize,
        flags: u8,
        tess_tol: f32,
    ) {
        if level > 10 {
            return;
        }
        let x12 = (x1 + x2) * 0.5;
        let y12 = (y1 + y2) * 0.5;
        let x23 = (x2 + x3) * 0.5;
        let y23 = (y2 + y3) * 0.5;
        let x34 = (x3 + x4) * 0.5;
        let y34 = (y3 + y4) * 0.5;
        let x123 = (x12 + x23) * 0.5;
        let y123 = (y12 + y23) * 0.5;
        let dx = x4 - x1;
        let dy = y4 - y1;
        let d2 = ((x2 - x4) * dy - (y2 - y4) * dx).abs();
        let d3 = ((x3 - x4) * dy - (y3 - y4) * dx).abs();
        if (d2 + d3) * (d2 + d3) < tess_tol * (dx * dx + dy * dy) {
            self.add_point(x4, y4, flags, 0.01);
            return;
        }
        let x234 = (x23 + x34) * 0.5;
        let y234 = (y23 + y34) * 0.5;
        let x1234 = (x123 + x234) * 0.5;
        let y1234 = (y123 + y234) * 0.5;
        self.tesselate_bezier(
            x1,
            y1,
            x12,
            y12,
            x123,
            y123,
            x1234,
            y1234,
            level + 1,
            0,
            tess_tol,
        );
        self.tesselate_bezier(
            x1234,
            y1234,
            x234,
            y234,
            x34,
            y34,
            x4,
            y4,
            level + 1,
            flags,
            tess_tol,
        );
    }

    fn prepare_points(&mut self, dist_tol: f32) {
        for i in 0..self.paths.len() {
            let p = &mut self.paths[i];
            if p.count < 2 {
                continue;
            }
            let first = p.first;
            let count = p.count;
            // close duplicate check
            {
                let last = &self.points[first + count - 1];
                let fst = &self.points[first];
                let dx = fst.x - last.x;
                let dy = fst.y - last.y;
                if dx * dx + dy * dy < dist_tol * dist_tol {
                    p.count -= 1;
                    p.closed = true;
                }
            }
            let count = p.count;
            if count < 2 {
                continue;
            }
            // ensure correct winding
            if count > 2 {
                let area = poly_area(&self.points[first..first + count]);
                if p.has_explicit_winding {
                    match p.winding {
                        Winding::CCW => {
                            if area < 0.0 {
                                self.points[first..first + count].reverse();
                            }
                        }
                        Winding::CW => {
                            if area > 0.0 {
                                self.points[first..first + count].reverse();
                            }
                        }
                    }
                }
            }
            // compute segment directions + lengths
            for j in 0..count {
                let j1 = if j + 1 < count { j + 1 } else { 0 };
                let p0 = self.points[first + j];
                let p1 = self.points[first + j1];
                let mut dx = p1.x - p0.x;
                let mut dy = p1.y - p0.y;
                let len = (dx * dx + dy * dy).sqrt();
                if len > 1e-6 {
                    let il = 1.0 / len;
                    dx *= il;
                    dy *= il;
                }
                self.points[first + j].dx = dx;
                self.points[first + j].dy = dy;
                self.points[first + j].len = len;
            }
        }
    }

    fn calculate_joins(&mut self, w: f32, line_join: LineJoin, miter_limit: f32) {
        let iw = if w > 0.0 { 1.0 / w } else { 0.0 };
        for i in 0..self.paths.len() {
            let sp = &self.paths[i];
            let first = sp.first;
            let count = sp.count;
            if count < 2 {
                continue;
            }
            let mut nleft = 0usize;
            let mut nbevel = 0usize;
            for j in 0..count {
                let j0 = if j == 0 { count - 1 } else { j - 1 };
                let p0_dx = self.points[first + j0].dx;
                let p0_dy = self.points[first + j0].dy;
                let p1_dx = self.points[first + j].dx;
                let p1_dy = self.points[first + j].dy;
                let dlx0 = p0_dy;
                let dly0 = -p0_dx;
                let dlx1 = p1_dy;
                let dly1 = -p1_dx;
                let mut dmx = (dlx0 + dlx1) * 0.5;
                let mut dmy = (dly0 + dly1) * 0.5;
                let dmr2 = dmx * dmx + dmy * dmy;
                if dmr2 > 1e-6 {
                    let s = (1.0 / dmr2).min(600.0);
                    dmx *= s;
                    dmy *= s;
                }
                self.points[first + j].dmx = dmx;
                self.points[first + j].dmy = dmy;
                let mut flags = self.points[first + j].flags & PT_CORNER;
                let cross = p1_dx * p0_dy - p0_dx * p1_dy;
                if cross > 0.0 {
                    nleft += 1;
                    flags |= PT_LEFT;
                }
                let p0_len = self.points[first + j0].len;
                let p1_len = self.points[first + j].len;
                let limit = (p0_len.min(p1_len) * iw).max(1.01);
                if dmr2 * limit * limit < 1.0 {
                    flags |= PT_INNERBEVEL;
                }
                if (flags & PT_CORNER) != 0 {
                    if dmr2 * miter_limit * miter_limit < 1.0
                        || matches!(line_join, LineJoin::Bevel | LineJoin::Round)
                    {
                        flags |= PT_BEVEL;
                    }
                }
                if (flags & (PT_BEVEL | PT_INNERBEVEL)) != 0 {
                    nbevel += 1;
                }
                self.points[first + j].flags = flags;
            }
            self.paths[i].convex = nleft == count;
            self.paths[i].nbevel = nbevel;
        }
    }

    /// Generate stroke geometry into the provided vecs (clears them first).
    pub fn stroke(
        &mut self,
        w: f32,
        line_cap: LineCap,
        line_join: LineJoin,
        miter_limit: f32,
        aa: f32,
        verts: &mut Vec<VVertex>,
        indices: &mut Vec<u32>,
    ) {
        let hw = w * 0.5 + aa * 0.5;
        self.calculate_joins(hw, line_join, miter_limit);
        verts.clear();
        indices.clear();
        let (u0, u1) = if aa > 0.0 { (0.0, 1.0) } else { (0.5, 0.5) };
        for pi in 0..self.paths.len() {
            let sp = &self.paths[pi];
            let first = sp.first;
            let count = sp.count;
            if count < 2 {
                continue;
            }
            let is_loop = sp.closed;
            let base = verts.len() as u32;
            // compute cumulative distances for stroke_dist
            self.cum_dists.clear();
            self.cum_dists.resize(count, 0.0);
            {
                let mut d = 0.0f32;
                for j in 1..count {
                    d += self.points[first + j - 1].len;
                    self.cum_dists[j] = d;
                }
            }
            // caps for open paths
            if !is_loop {
                // Find a valid (non-degenerate) direction at the start of the path
                // by walking forward until we find two points far enough apart.
                let (ndx, ndy) = {
                    let mut dir = (0.0f32, 0.0f32);
                    for j in 1..count {
                        let dx = self.points[first + j].x - self.points[first].x;
                        let dy = self.points[first + j].y - self.points[first].y;
                        let len = (dx * dx + dy * dy).sqrt();
                        if len > 1e-6 {
                            dir = (dx / len, dy / len);
                            break;
                        }
                    }
                    dir
                };
                let p0 = self.points[first];
                self.emit_cap_start(
                    verts, indices, p0.x, p0.y, ndx, ndy, hw, aa, u0, u1, line_cap,
                );
                // stamp stroke_dist=0 on cap verts
                let cap_end = verts.len();
                for v in &mut verts[base as usize..cap_end] {
                    v.stroke_dist = 0.0;
                }
            }
            // body
            let (s, e) = if is_loop { (0, count) } else { (1, count - 1) };
            for j in s..e {
                let j0 = if j == 0 { count - 1 } else { j - 1 };
                let p0 = self.points[first + j0];
                let p1 = self.points[first + j];
                let dist = self.cum_dists[j];
                let flags = p1.flags;
                if (flags & (PT_BEVEL | PT_INNERBEVEL)) != 0 {
                    let vi_before = verts.len();
                    self.emit_bevel_join(verts, indices, p0, p1, hw, hw, u0, u1);
                    for v in &mut verts[vi_before..] {
                        v.stroke_dist = dist;
                    }
                } else {
                    let vi = verts.len() as u32;
                    verts.push(VVertex::with_dist(
                        p1.x + p1.dmx * hw,
                        p1.y + p1.dmy * hw,
                        u0,
                        1.0,
                        dist,
                    ));
                    verts.push(VVertex::with_dist(
                        p1.x - p1.dmx * hw,
                        p1.y - p1.dmy * hw,
                        u1,
                        1.0,
                        dist,
                    ));
                    if vi >= base + 2 {
                        indices.push(vi - 2);
                        indices.push(vi - 1);
                        indices.push(vi);
                        indices.push(vi - 1);
                        indices.push(vi + 1);
                        indices.push(vi);
                    }
                }
            }
            if !is_loop {
                // end cap: find a valid direction by walking backward from the end
                let p1 = self.points[first + count - 1];
                let (ndx, ndy) = {
                    let mut dir = (0.0f32, 0.0f32);
                    for j in (0..count - 1).rev() {
                        let dx = p1.x - self.points[first + j].x;
                        let dy = p1.y - self.points[first + j].y;
                        let len = (dx * dx + dy * dy).sqrt();
                        if len > 1e-6 {
                            dir = (dx / len, dy / len);
                            break;
                        }
                    }
                    dir
                };
                let vi_before = verts.len();
                self.emit_cap_end(
                    verts, indices, p1.x, p1.y, ndx, ndy, hw, aa, u0, u1, line_cap,
                );
                let total_dist = self.cum_dists[count - 1];
                for v in &mut verts[vi_before..] {
                    v.stroke_dist = total_dist;
                }
            } else {
                // close loop: connect last pair to first pair
                let vi = verts.len() as u32;
                if vi >= base + 4 {
                    indices.push(vi - 2);
                    indices.push(vi - 1);
                    indices.push(base);
                    indices.push(vi - 1);
                    indices.push(base + 1);
                    indices.push(base);
                }
            }
        }
    }

    fn emit_cap_start(
        &self,
        verts: &mut Vec<VVertex>,
        indices: &mut Vec<u32>,
        px: f32,
        py: f32,
        dx: f32,
        dy: f32,
        w: f32,
        aa: f32,
        u0: f32,
        u1: f32,
        cap: LineCap,
    ) {
        let dlx = dy;
        let dly = -dx;
        match cap {
            LineCap::Butt => {
                verts.push(VVertex::new(
                    px + dlx * w - dx * aa,
                    py + dly * w - dy * aa,
                    u0,
                    0.0,
                ));
                verts.push(VVertex::new(
                    px - dlx * w - dx * aa,
                    py - dly * w - dy * aa,
                    u1,
                    0.0,
                ));
                verts.push(VVertex::new(px + dlx * w, py + dly * w, u0, 1.0));
                verts.push(VVertex::new(px - dlx * w, py - dly * w, u1, 1.0));
            }
            LineCap::Square => {
                verts.push(VVertex::new(
                    px + dlx * w - dx * (w - aa),
                    py + dly * w - dy * (w - aa),
                    u0,
                    0.0,
                ));
                verts.push(VVertex::new(
                    px - dlx * w - dx * (w - aa),
                    py - dly * w - dy * (w - aa),
                    u1,
                    0.0,
                ));
                verts.push(VVertex::new(px + dlx * w, py + dly * w, u0, 1.0));
                verts.push(VVertex::new(px - dlx * w, py - dly * w, u1, 1.0));
            }
            LineCap::Round => {
                // Emit a triangle fan from center to arc points, then
                // end with the (left, right) pair for the stroke body.
                let ncap = ((w * PI).ceil() as usize).max(2).min(32);
                let center_vi = verts.len() as u32;
                verts.push(VVertex::new(px, py, 0.5, 1.0));
                // Arc vertices from +left through back to -left
                for i in 0..ncap {
                    let a = i as f32 / (ncap - 1) as f32 * PI;
                    let ax = a.cos() * w;
                    let ay = a.sin() * w;
                    verts.push(VVertex::new(
                        px - dlx * ax - dx * ay,
                        py - dly * ax - dy * ay,
                        u0,
                        1.0,
                    ));
                }
                // Fan triangles: center + consecutive arc points
                let arc_start = center_vi + 1;
                for i in 0..(ncap as u32 - 1) {
                    indices.push(center_vi);
                    indices.push(arc_start + i);
                    indices.push(arc_start + i + 1);
                }
                // Final pair for body stitching: (left_edge, right_edge)
                verts.push(VVertex::new(px + dlx * w, py + dly * w, u0, 1.0));
                verts.push(VVertex::new(px - dlx * w, py - dly * w, u1, 1.0));
                // Connect last arc point to the left edge, and first arc point to the right edge
                let left_vi = verts.len() as u32 - 2;
                let right_vi = verts.len() as u32 - 1;
                // Left edge triangle: center, first arc point (at +left side), left_edge
                indices.push(center_vi);
                indices.push(arc_start);
                indices.push(left_vi);
                // Right edge triangle: center, last arc point (at -left side), right_edge
                indices.push(center_vi);
                indices.push(arc_start + ncap as u32 - 1);
                indices.push(right_vi);
            }
        }
    }

    fn emit_cap_end(
        &self,
        verts: &mut Vec<VVertex>,
        indices: &mut Vec<u32>,
        px: f32,
        py: f32,
        dx: f32,
        dy: f32,
        w: f32,
        aa: f32,
        u0: f32,
        u1: f32,
        cap: LineCap,
    ) {
        let dlx = dy;
        let dly = -dx;
        let vi = verts.len() as u32;
        match cap {
            LineCap::Butt => {
                verts.push(VVertex::new(px + dlx * w, py + dly * w, u0, 1.0));
                verts.push(VVertex::new(px - dlx * w, py - dly * w, u1, 1.0));
                verts.push(VVertex::new(
                    px + dlx * w + dx * aa,
                    py + dly * w + dy * aa,
                    u0,
                    0.0,
                ));
                verts.push(VVertex::new(
                    px - dlx * w + dx * aa,
                    py - dly * w + dy * aa,
                    u1,
                    0.0,
                ));
            }
            LineCap::Square => {
                verts.push(VVertex::new(px + dlx * w, py + dly * w, u0, 1.0));
                verts.push(VVertex::new(px - dlx * w, py - dly * w, u1, 1.0));
                verts.push(VVertex::new(
                    px + dlx * w + dx * (w - aa),
                    py + dly * w + dy * (w - aa),
                    u0,
                    0.0,
                ));
                verts.push(VVertex::new(
                    px - dlx * w + dx * (w - aa),
                    py - dly * w + dy * (w - aa),
                    u1,
                    0.0,
                ));
            }
            LineCap::Round => {
                let ncap = ((w * PI).ceil() as usize).max(2).min(32);
                // Connect body's last pair to the (left, right) pair
                verts.push(VVertex::new(px + dlx * w, py + dly * w, u0, 1.0));
                verts.push(VVertex::new(px - dlx * w, py - dly * w, u1, 1.0));
                if vi >= 2 {
                    indices.push(vi - 2);
                    indices.push(vi - 1);
                    indices.push(vi);
                    indices.push(vi - 1);
                    indices.push(vi + 1);
                    indices.push(vi);
                }
                // Center vertex for fan
                let center_vi = verts.len() as u32;
                verts.push(VVertex::new(px, py, 0.5, 1.0));
                // Arc vertices from +left through front to -left
                let arc_start = center_vi + 1;
                for i in 0..ncap {
                    let a = i as f32 / (ncap - 1) as f32 * PI;
                    let ax = a.cos() * w;
                    let ay = a.sin() * w;
                    verts.push(VVertex::new(
                        px - dlx * ax + dx * ay,
                        py - dly * ax + dy * ay,
                        u0,
                        1.0,
                    ));
                }
                // Fan triangles: center + consecutive arc points
                for i in 0..(ncap as u32 - 1) {
                    indices.push(center_vi);
                    indices.push(arc_start + i);
                    indices.push(arc_start + i + 1);
                }
                // Connect left_edge to first arc, right_edge to last arc
                indices.push(center_vi);
                indices.push(vi);
                indices.push(arc_start);
                indices.push(center_vi);
                indices.push(arc_start + ncap as u32 - 1);
                indices.push(vi + 1);
                return;
            }
        }
        // connect cap end to previous pair (Butt/Square only)
        if vi >= 2 {
            indices.push(vi - 2);
            indices.push(vi - 1);
            indices.push(vi);
            indices.push(vi - 1);
            indices.push(vi + 1);
            indices.push(vi);
        }
        // stitch cap triangles (Butt/Square: simple quad strip)
        let n = (verts.len() as u32 - vi) / 2;
        for i in 1..n {
            let a = vi + (i - 1) * 2;
            let b = vi + i * 2;
            indices.push(a);
            indices.push(a + 1);
            indices.push(b);
            indices.push(a + 1);
            indices.push(b + 1);
            indices.push(b);
        }
    }

    fn emit_bevel_join(
        &self,
        verts: &mut Vec<VVertex>,
        indices: &mut Vec<u32>,
        p0: VPoint,
        p1: VPoint,
        lw: f32,
        rw: f32,
        u0: f32,
        u1: f32,
    ) {
        let vi = verts.len() as u32;
        let dlx0 = p0.dy;
        let dly0 = -p0.dx;
        let dlx1 = p1.dy;
        let dly1 = -p1.dx;
        if (p1.flags & PT_LEFT) != 0 {
            let lx0 = p1.x + dlx0 * lw;
            let ly0 = p1.y + dly0 * lw;
            let lx1 = p1.x + dlx1 * lw;
            let ly1 = p1.y + dly1 * lw;
            verts.push(VVertex::new(lx0, ly0, u0, 1.0));
            verts.push(VVertex::new(p1.x - dlx0 * rw, p1.y - dly0 * rw, u1, 1.0));
            verts.push(VVertex::new(lx1, ly1, u0, 1.0));
            verts.push(VVertex::new(p1.x - dlx1 * rw, p1.y - dly1 * rw, u1, 1.0));
        } else {
            let rx0 = p1.x - dlx0 * rw;
            let ry0 = p1.y - dly0 * rw;
            let rx1 = p1.x - dlx1 * rw;
            let ry1 = p1.y - dly1 * rw;
            verts.push(VVertex::new(p1.x + dlx0 * lw, p1.y + dly0 * lw, u0, 1.0));
            verts.push(VVertex::new(rx0, ry0, u1, 1.0));
            verts.push(VVertex::new(p1.x + dlx1 * lw, p1.y + dly1 * lw, u0, 1.0));
            verts.push(VVertex::new(rx1, ry1, u1, 1.0));
        }
        // connect to previous pair and within bevel
        if vi >= 2 {
            indices.push(vi - 2);
            indices.push(vi - 1);
            indices.push(vi);
            indices.push(vi - 1);
            indices.push(vi + 1);
            indices.push(vi);
        }
        indices.push(vi);
        indices.push(vi + 1);
        indices.push(vi + 2);
        indices.push(vi + 1);
        indices.push(vi + 3);
        indices.push(vi + 2);
    }

    /// Generate fill geometry into the provided vecs (clears them first).
    pub fn fill(
        &mut self,
        aa: f32,
        line_join: LineJoin,
        miter_limit: f32,
        gpu_expand_fill: bool,
        verts: &mut Vec<VVertex>,
        indices: &mut Vec<u32>,
    ) {
        let woff = aa * 0.5;
        self.calculate_joins(woff, line_join, miter_limit);
        verts.clear();
        indices.clear();

        // Collect valid subpaths (>= 3 points) with per-contour AA side sign.
        // sign > 0: fill side is along +dmx/+dmy, sign < 0: fill side is along -dmx/-dmy.
        // We derive this from a non-zero fill test around each contour edge.
        let mut valid_paths: Vec<(usize, usize, f32)> = Vec::new();
        for pi in 0..self.paths.len() {
            let sp = &self.paths[pi];
            if sp.count >= 3 {
                valid_paths.push((sp.first, sp.count, 1.0));
            }
        }

        if valid_paths.is_empty() {
            return;
        }

        let fill_rule = if self
            .paths
            .iter()
            .any(|sp| sp.count >= 3 && sp.has_explicit_winding)
        {
            FillRule::NonZero
        } else {
            FillRule::EvenOdd
        };
        let body_inset_woff = if matches!(fill_rule, FillRule::EvenOdd) {
            // Implicit font-like outlines are fragile under inward body shrink.
            // Keep body on-edge and let the fringe provide AA falloff.
            0.0
        } else {
            woff
        };

        // Determine fill-side sign per contour by sampling both sides of contour edges.
        // This avoids corner-miter ambiguity on sharp/reflex vertices.
        for i in 0..valid_paths.len() {
            let (first, count, _) = valid_paths[i];
            let mut sign = 1.0f32;
            let mut found = false;

            for j in 0..count {
                let j1 = (j + 1) % count;
                let p0 = self.points[first + j];
                let p1 = self.points[first + j1];
                let mut ex = p1.x - p0.x;
                let mut ey = p1.y - p0.y;
                let e2 = ex * ex + ey * ey;
                if e2 <= 1e-12 {
                    continue;
                }

                let inv_e = 1.0 / e2.sqrt();
                ex *= inv_e;
                ey *= inv_e;
                let nx = ey;
                let ny = -ex;
                let mx = (p0.x + p1.x) * 0.5;
                let my = (p0.y + p1.y) * 0.5;
                let local_len = p0.len.max(p1.len);
                let base_eps = (local_len * 1e-3).max(1e-4);
                let eps_scales = [1.0f32, 4.0, 16.0];

                for s in eps_scales {
                    let eps = base_eps * s;
                    let plus_filled = point_in_fill_rule(
                        mx + nx * eps,
                        my + ny * eps,
                        &self.points,
                        &valid_paths,
                        fill_rule,
                    );
                    let minus_filled = point_in_fill_rule(
                        mx - nx * eps,
                        my - ny * eps,
                        &self.points,
                        &valid_paths,
                        fill_rule,
                    );

                    if plus_filled != minus_filled {
                        sign = if plus_filled { 1.0 } else { -1.0 };
                        found = true;
                        break;
                    }
                }
                if found {
                    break;
                }
            }

            if !found {
                // Fallback to contour orientation if local fill-side probe was inconclusive.
                let area = poly_area(&self.points[first..first + count]);
                sign = if area >= 0.0 { 1.0 } else { -1.0 };
            }
            valid_paths[i].2 = sign;
        }

        // Emit fill body vertices.
        // In regular mode we inset by woff like NanoVG. In GPU-expand mode
        // we keep vertices on the edge and let the vertex shader place them.
        let fill_base = verts.len() as u32;
        for &(first, count, sign) in &valid_paths {
            for j in 0..count {
                let pt = self.points[first + j];
                if gpu_expand_fill {
                    let nx = -pt.dmx * sign;
                    let ny = -pt.dmy * sign;
                    verts.push(VVertex {
                        x: pt.x,
                        y: pt.y,
                        u: 0.5,
                        v: nx,
                        stroke_dist: ny,
                        clip_radius: 0.0,
                    });
                } else {
                    verts.push(VVertex::new(
                        pt.x + pt.dmx * body_inset_woff * sign,
                        pt.y + pt.dmy * body_inset_woff * sign,
                        0.5,
                        1.0,
                    ));
                }
            }
        }

        // Feed edges from ALL subpaths into a single sweep-line tessellator.
        // Regions are classified with the non-zero winding rule.
        {
            let all_fill_verts = &verts[fill_base as usize..];
            let mut tess = SweepTessellator::new(fill_rule);
            let mut offset = 0usize;
            for &(_first, count, _) in &valid_paths {
                for i in 0..count {
                    let j = (i + 1) % count;
                    let vi = &all_fill_verts[offset + i];
                    let vj = &all_fill_verts[offset + j];
                    let i_index = fill_base + (offset + i) as u32;
                    let j_index = fill_base + (offset + j) as u32;
                    // Preserve original contour order for non-zero winding.
                    tess.push_edge(
                        FPoint::new(vi.x, vi.y),
                        i_index,
                        FPoint::new(vj.x, vj.y),
                        j_index,
                    );
                }
                offset += count;
            }
            let tri = tess.tessellate_vverts();
            indices.extend_from_slice(&tri);
        }

        // AA fringe: inner vertex at body edge (opaque, u=0.5),
        // outer vertex also at body edge but tagged with the outward
        // normal in (v, stroke_dist) so the vertex shader can expand
        // it to the correct screen-space width.
        if woff > 0.0 {
            for &(first, count, sign) in &valid_paths {
                let fringe_base = verts.len() as u32;
                for j in 0..count {
                    let p1 = self.points[first + j];
                    if gpu_expand_fill {
                        // Anchor both fringe vertices at the edge and encode
                        // outward normal for GPU-side fringe placement.
                        let bx = p1.x;
                        let by = p1.y;
                        let nx = -p1.dmx * sign;
                        let ny = -p1.dmy * sign;
                        verts.push(VVertex {
                            x: bx,
                            y: by,
                            u: 0.5,
                            v: nx,
                            stroke_dist: ny,
                            clip_radius: 0.0,
                        });
                        verts.push(VVertex {
                            x: bx,
                            y: by,
                            u: 0.0,
                            v: nx,
                            stroke_dist: ny,
                            clip_radius: 0.0,
                        });
                    } else {
                        // Classic NanoVG-style physical fringe geometry.
                        verts.push(VVertex::new(
                            p1.x + p1.dmx * body_inset_woff * sign,
                            p1.y + p1.dmy * body_inset_woff * sign,
                            0.5,
                            1.0,
                        ));
                        verts.push(VVertex::new(
                            p1.x - p1.dmx * woff * sign,
                            p1.y - p1.dmy * woff * sign,
                            0.0,
                            1.0,
                        ));
                    }
                }
                for j in 0..count as u32 {
                    let j1 = if j + 1 < count as u32 { j + 1 } else { 0 };
                    let a = fringe_base + j * 2;
                    let b = fringe_base + j1 * 2;
                    indices.push(a);
                    indices.push(a + 1);
                    indices.push(b);
                    indices.push(a + 1);
                    indices.push(b + 1);
                    indices.push(b);
                }
            }
        }
    }

    /// Generate shadow geometry for arbitrary filled shapes.
    /// Like fill() but with a wide fringe (3*blur) for gaussian falloff.
    /// The v coordinate encodes normalized distance from edge: 1.0 = inside, 0.0 = outer limit.
    /// stroke_dist is repurposed to carry the blur radius for the shader.
    pub fn fill_shadow(
        &mut self,
        blur: f32,
        line_join: LineJoin,
        miter_limit: f32,
        verts: &mut Vec<VVertex>,
        indices: &mut Vec<u32>,
    ) {
        let expand = blur * 3.0;
        // Ensure CCW winding so that dmx/dmy normals point outward.
        // prepare_points no longer auto-corrects winding for paths
        // without explicit winding, so we must enforce it here.
        for pi in 0..self.paths.len() {
            let sp = &self.paths[pi];
            let first = sp.first;
            let count = sp.count;
            if count >= 3 {
                let area = poly_area(&self.points[first..first + count]);
                if area < 0.0 {
                    self.points[first..first + count].reverse();
                }
            }
        }
        self.calculate_joins(expand, line_join, miter_limit);
        verts.clear();
        indices.clear();
        for pi in 0..self.paths.len() {
            let sp = &self.paths[pi];
            let first = sp.first;
            let count = sp.count;
            if count < 3 {
                continue;
            }
            // Determine outward direction sign from contour winding.
            // For CCW (area >= 0), dmx/dmy points outward.
            // For CW (area < 0), dmx/dmy points inward.
            // We want to push fringe outward, so use sign * dmx.
            let area = poly_area(&self.points[first..first + count]);
            let sign: f32 = if area >= 0.0 { 1.0 } else { -1.0 };
            // convex fan at original path positions (fully opaque, v=1.0)
            let base = verts.len() as u32;
            for j in 0..count {
                let pt = self.points[first + j];
                // u=0.5 (opaque in AA formula), v=1.0 (inside)
                let mut v = VVertex::new(pt.x, pt.y, 0.5, 1.0);
                v.stroke_dist = blur; // carry blur radius
                verts.push(v);
            }
            for j in 2..count as u32 {
                indices.push(base);
                indices.push(base + j - 1);
                indices.push(base + j);
            }
            // wide fringe for shadow falloff
            let fringe_base = verts.len() as u32;
            for j in 0..count {
                let p1 = self.points[first + j];
                // inner: at path edge (v=1.0, will be fully opaque)
                let mut vi = VVertex::new(p1.x, p1.y, 0.5, 1.0);
                vi.stroke_dist = blur;
                verts.push(vi);
                // outer: pushed outward, clamped to expand distance
                let mut ox = sign * p1.dmx * expand;
                let mut oy = sign * p1.dmy * expand;
                let ol = (ox * ox + oy * oy).sqrt();
                if ol > expand {
                    let s = expand / ol;
                    ox *= s;
                    oy *= s;
                }
                let mut vo = VVertex::new(p1.x + ox, p1.y + oy, 0.0, 0.0);
                vo.stroke_dist = blur;
                verts.push(vo);
            }
            // stitch fringe strip
            for j in 0..count as u32 {
                let j1 = if j + 1 < count as u32 { j + 1 } else { 0 };
                let a = fringe_base + j * 2;
                let b = fringe_base + j1 * 2;
                indices.push(a);
                indices.push(a + 1);
                indices.push(b);
                indices.push(a + 1);
                indices.push(b + 1);
                indices.push(b);
            }
        }
    }
}

/// Compute per-vertex clip_radius from the triangle index buffer.
/// For each vertex, this is the maximum distance to any other vertex
/// it shares a triangle with. This allows the vertex shader to skip
/// triangles that are entirely outside the clip rect.
pub fn compute_clip_radii(verts: &mut [VVertex], indices: &[u32]) {
    // Process triangles: for each triangle (a, b, c), update each vertex's
    // clip_radius to be the max distance to any other vertex in that triangle.
    let mut i = 0;
    while i + 2 < indices.len() {
        let ia = indices[i] as usize;
        let ib = indices[i + 1] as usize;
        let ic = indices[i + 2] as usize;
        if ia < verts.len() && ib < verts.len() && ic < verts.len() {
            let ax = verts[ia].x;
            let ay = verts[ia].y;
            let bx = verts[ib].x;
            let by = verts[ib].y;
            let cx = verts[ic].x;
            let cy = verts[ic].y;

            let dab = ((ax - bx) * (ax - bx) + (ay - by) * (ay - by)).sqrt();
            let dac = ((ax - cx) * (ax - cx) + (ay - cy) * (ay - cy)).sqrt();
            let dbc = ((bx - cx) * (bx - cx) + (by - cy) * (by - cy)).sqrt();

            let ra = dab.max(dac);
            let rb = dab.max(dbc);
            let rc = dac.max(dbc);

            if ra > verts[ia].clip_radius {
                verts[ia].clip_radius = ra;
            }
            if rb > verts[ib].clip_radius {
                verts[ib].clip_radius = rb;
            }
            if rc > verts[ic].clip_radius {
                verts[ic].clip_radius = rc;
            }
        }
        i += 3;
    }
}

fn poly_area(pts: &[VPoint]) -> f32 {
    let mut area = 0.0;
    let n = pts.len();
    for i in 2..n {
        area += (pts[i].x - pts[0].x) * (pts[i - 1].y - pts[0].y)
            - (pts[i - 1].x - pts[0].x) * (pts[i].y - pts[0].y);
    }
    area * 0.5
}

fn contour_winding_at_point(px: f32, py: f32, pts: &[VPoint], first: usize, count: usize) -> i32 {
    if count < 3 {
        return 0;
    }
    let mut winding = 0i32;
    for i in 0..count {
        let j = (i + 1) % count;
        let x0 = pts[first + i].x;
        let y0 = pts[first + i].y;
        let x1 = pts[first + j].x;
        let y1 = pts[first + j].y;

        if y0 <= py {
            if y1 > py {
                let is_left = (x1 - x0) * (py - y0) - (px - x0) * (y1 - y0);
                if is_left > 0.0 {
                    winding += 1;
                }
            }
        } else if y1 <= py {
            let is_left = (x1 - x0) * (py - y0) - (px - x0) * (y1 - y0);
            if is_left < 0.0 {
                winding -= 1;
            }
        }
    }
    winding
}

fn point_in_fill_rule(
    px: f32,
    py: f32,
    pts: &[VPoint],
    contours: &[(usize, usize, f32)],
    fill_rule: FillRule,
) -> bool {
    let mut winding = 0i32;
    for &(first, count, _) in contours {
        winding += contour_winding_at_point(px, py, pts, first, count);
    }
    match fill_rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => (winding & 1) != 0,
    }
}

// ---- Sweep-line monotone polygon tessellator (ported from bender) ----
// Correctly triangulates concave (and self-intersecting) polygons using
// sweep-line decomposition into monotone sub-polygons, then triangulating each.

// Minimal 2D point for the tessellator
#[derive(Clone, Copy, Debug)]
struct FPoint {
    x: f32,
    y: f32,
}

impl FPoint {
    fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

impl PartialEq for FPoint {
    fn eq(&self, other: &Self) -> bool {
        self.x == other.x && self.y == other.y
    }
}

impl PartialOrd for FPoint {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        // Match bender geometry Point ordering: x first, then y.
        match self.x.partial_cmp(&other.x) {
            Some(std::cmp::Ordering::Equal) => self.y.partial_cmp(&other.y),
            ord => ord,
        }
    }
}

// Line segment for sweep-line
#[derive(Clone, Copy, Debug)]
struct FSegment {
    start: FPoint,
    end: FPoint,
}

impl FSegment {
    fn new(start: FPoint, end: FPoint) -> Self {
        Self { start, end }
    }

    // Returns ordering of point relative to segment: Less = right, Greater = left, Equal = on
    fn compare_to_point(&self, p: FPoint) -> std::cmp::Ordering {
        let c =
            (p.x - self.start.x) * (self.end.y - p.y) - (p.y - self.start.y) * (self.end.x - p.x);
        if c > 0.0 {
            std::cmp::Ordering::Greater
        } else if c < 0.0 {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Equal
        }
    }
}

// Running winding accumulation for non-zero fill rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FWinding(i32);

impl FWinding {
    const ZERO: Self = Self(0);
    const POS: Self = Self(1);
    const NEG: Self = Self(-1);
}

impl std::ops::Add for FWinding {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self(self.0 + other.0)
    }
}

// Events for the sweep line
#[derive(Clone, Copy, Debug)]
struct SweepEvent {
    vertex: FPoint,
    vertex_index: u32,
    pending_edge: Option<SweepPendingEdge>,
}

impl PartialEq for SweepEvent {
    fn eq(&self, other: &Self) -> bool {
        self.vertex == other.vertex
    }
}
impl Eq for SweepEvent {}

impl PartialOrd for SweepEvent {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SweepEvent {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse order for min-heap behavior with BinaryHeap (which is max-heap)
        other
            .vertex
            .partial_cmp(&self.vertex)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

#[derive(Clone, Copy, Debug)]
struct SweepPendingEdge {
    winding: FWinding,
    end: FPoint,
    end_index: u32,
}

impl SweepPendingEdge {
    fn to_segment(self, start: FPoint) -> FSegment {
        FSegment::new(start, self.end)
    }

    fn compare(&self, other: &Self, start: FPoint) -> std::cmp::Ordering {
        if self
            .end
            .partial_cmp(&other.end)
            .map_or(false, |o| o != std::cmp::Ordering::Greater)
        {
            other.to_segment(start).compare_to_point(self.end).reverse()
        } else {
            self.to_segment(start).compare_to_point(other.end)
        }
    }

    fn overlaps(&self, other: &Self, start: FPoint) -> bool {
        self.compare(other, start) == std::cmp::Ordering::Equal
    }

    fn splice(&mut self, mut other: Self) -> Option<SweepEvent> {
        if other
            .end
            .partial_cmp(&self.end)
            .map_or(false, |o| o != std::cmp::Ordering::Greater)
        {
            std::mem::swap(self, &mut other);
        }
        self.winding = self.winding + other.winding;
        if self.end == other.end {
            return None;
        }
        Some(SweepEvent {
            vertex: self.end,
            vertex_index: self.end_index,
            pending_edge: Some(other),
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct SweepActiveEdge {
    is_temporary: bool,
    winding: FWinding,
    start_index: u32,
    end_index: u32,
    edge: FSegment,
    upper_region_winding: FWinding,
    lower_mono: Option<usize>,
    upper_mono: Option<usize>,
}

impl SweepActiveEdge {
    fn split(&mut self, vertex: FPoint) -> Option<SweepPendingEdge> {
        let end = self.edge.end;
        if vertex == end {
            return None;
        }
        self.edge = FSegment::new(self.edge.start, vertex);
        Some(SweepPendingEdge {
            winding: self.winding,
            end,
            end_index: self.end_index,
        })
    }
}

// Monotone polygon tessellator
#[derive(Clone, Copy, Debug, PartialEq)]
enum MonoSide {
    Lower,
    Upper,
}

#[derive(Clone, Debug)]
struct MonoVertex {
    index: u32,
    pos: FPoint,
}

#[derive(Clone, Debug)]
struct MonoPoly {
    side: MonoSide,
    stack: Vec<MonoVertex>,
}

impl MonoPoly {
    fn new() -> Self {
        Self {
            side: MonoSide::Lower,
            stack: Vec::new(),
        }
    }

    fn start(&mut self, index: u32, pos: FPoint) {
        self.stack.clear();
        self.stack.push(MonoVertex { index, pos });
    }

    fn finish(&mut self, index: u32, out: &mut Vec<u32>) {
        let mut v1 = self.stack.pop().unwrap();
        while let Some(v0) = self.stack.pop() {
            out.push(v0.index);
            out.push(v1.index);
            out.push(index);
            v1 = v0;
        }
    }

    fn push_vertex(&mut self, side: MonoSide, index: u32, pos: FPoint, out: &mut Vec<u32>) {
        if side == self.side {
            let mut v1 = self.stack.pop().unwrap();
            loop {
                let v0 = if let Some(v0) = self.stack.last() {
                    v0.clone()
                } else {
                    break;
                };
                let seg = FSegment::new(v0.pos, pos);
                let cmp = seg.compare_to_point(v1.pos);
                match (cmp, side) {
                    (std::cmp::Ordering::Less, MonoSide::Lower) => break,
                    (std::cmp::Ordering::Equal, _) => break,
                    (std::cmp::Ordering::Greater, MonoSide::Upper) => break,
                    _ => (),
                }
                self.stack.pop();
                out.push(v0.index);
                out.push(v1.index);
                out.push(index);
                v1 = v0;
            }
            self.stack.push(v1);
            self.stack.push(MonoVertex { index, pos });
        } else {
            let vertex = self.stack.pop().unwrap();
            let mut v1 = vertex.clone();
            while let Some(v0) = self.stack.pop() {
                out.push(v0.index);
                out.push(v1.index);
                out.push(index);
                v1 = v0;
            }
            self.stack.push(vertex);
            self.stack.push(MonoVertex { index, pos });
            self.side = side;
        }
    }
}

// Simple arena for monotone polygons
struct MonoArena {
    polys: Vec<Option<MonoPoly>>,
    free: Vec<usize>,
    pool: Vec<MonoPoly>,
}

impl MonoArena {
    fn new() -> Self {
        Self {
            polys: Vec::new(),
            free: Vec::new(),
            pool: Vec::new(),
        }
    }

    fn insert(&mut self, poly: MonoPoly) -> usize {
        if let Some(idx) = self.free.pop() {
            self.polys[idx] = Some(poly);
            idx
        } else {
            let idx = self.polys.len();
            self.polys.push(Some(poly));
            idx
        }
    }

    fn remove(&mut self, idx: usize) -> MonoPoly {
        let poly = self.polys[idx].take().unwrap();
        self.free.push(idx);
        poly
    }

    fn get_mut(&mut self, idx: usize) -> &mut MonoPoly {
        self.polys[idx].as_mut().unwrap()
    }

    fn start_mono(&mut self, index: u32, pos: FPoint) -> usize {
        let mut poly = self.pool.pop().unwrap_or_else(MonoPoly::new);
        poly.start(index, pos);
        self.insert(poly)
    }

    fn finish_mono(&mut self, idx: usize, index: u32, out: &mut Vec<u32>) {
        let mut poly = self.remove(idx);
        poly.finish(index, out);
        self.pool.push(poly);
    }
}

// The main sweep-line tessellator
struct SweepTessellator {
    fill_rule: FillRule,
    active_edges: Vec<SweepActiveEdge>,
    event_queue: std::collections::BinaryHeap<SweepEvent>,
    mono_arena: MonoArena,
}

impl SweepTessellator {
    fn new(fill_rule: FillRule) -> Self {
        Self {
            fill_rule,
            active_edges: Vec::new(),
            event_queue: std::collections::BinaryHeap::new(),
            mono_arena: MonoArena::new(),
        }
    }

    fn push_edge(&mut self, start: FPoint, start_index: u32, end: FPoint, end_index: u32) {
        if start == end {
            return;
        }
        let (start, start_index, end, end_index, winding) = match self.fill_rule {
            FillRule::NonZero => {
                if start.partial_cmp(&end) == Some(std::cmp::Ordering::Less) {
                    (start, start_index, end, end_index, FWinding::POS)
                } else {
                    (end, end_index, start, start_index, FWinding::NEG)
                }
            }
            FillRule::EvenOdd => {
                if start.partial_cmp(&end) == Some(std::cmp::Ordering::Less) {
                    (start, start_index, end, end_index, FWinding::POS)
                } else {
                    (end, end_index, start, start_index, FWinding::POS)
                }
            }
        };
        self.event_queue.push(SweepEvent {
            vertex: start,
            vertex_index: start_index,
            pending_edge: Some(SweepPendingEdge {
                winding,
                end,
                end_index,
            }),
        });
        self.event_queue.push(SweepEvent {
            vertex: end,
            vertex_index: end_index,
            pending_edge: None,
        });
    }

    fn tessellate_vverts(mut self) -> Vec<u32> {
        let mut out = Vec::new();
        let mut pending = Vec::new();
        let mut left_edges = Vec::new();

        while let Some((vertex, vi)) = self.pop_events(&mut pending) {
            self.handle_vertex(vertex, vi, &mut pending, &mut left_edges, &mut out);
            pending.clear();
            left_edges.clear();
        }
        out
    }

    fn pop_events(&mut self, pending: &mut Vec<SweepPendingEdge>) -> Option<(FPoint, u32)> {
        let event = self.event_queue.pop()?;
        let mut vertex_index = event.vertex_index;
        if let Some(pe) = event.pending_edge {
            pending.push(pe);
        }
        loop {
            let next = if let Some(next) = self.event_queue.peek() {
                if next.vertex == event.vertex {
                    next.clone()
                } else {
                    break;
                }
            } else {
                break;
            };
            self.event_queue.pop();
            vertex_index = vertex_index.min(next.vertex_index);
            if let Some(pe) = next.pending_edge {
                pending.push(pe);
            }
        }
        Some((event.vertex, vertex_index))
    }

    fn handle_vertex(
        &mut self,
        vertex: FPoint,
        vi: u32,
        pending: &mut Vec<SweepPendingEdge>,
        left_edges: &mut Vec<SweepActiveEdge>,
        out: &mut Vec<u32>,
    ) {
        let mut incident_range = self.find_incident_range(vertex);
        self.fix_temporary_edges(vertex, &mut incident_range);
        let incident_start = incident_range.start;

        // Remove incident edges and collect split pending edges
        for mut ae in self.active_edges.drain(incident_range.clone()) {
            if let Some(pe) = ae.split(vertex) {
                pending.push(pe);
            }
            left_edges.push(ae);
        }

        // Sort and splice pending edges
        pending.sort_by(|a, b| a.compare(b, vertex));
        let mut write = 0;
        for read in 1..pending.len() {
            let pe1 = pending[read];
            if pending[write].overlaps(&pe1, vertex) {
                if let Some(ev) = pending[write].splice(pe1) {
                    self.event_queue.push(ev);
                }
            } else {
                write += 1;
                pending[write] = pe1;
            }
        }
        if !pending.is_empty() {
            pending.truncate(write + 1);
        }

        // Determine lower/upper monotone polygons from left edges
        let (lower_mono, upper_mono) = if left_edges.is_empty() {
            self.connect_left_vertex(incident_start)
        } else {
            self.finish_left_monos(vi, left_edges, out)
        };

        if let Some(lm) = lower_mono {
            self.mono_arena
                .get_mut(lm)
                .push_vertex(MonoSide::Upper, vi, vertex, out);
        }
        if let Some(um) = upper_mono {
            self.mono_arena
                .get_mut(um)
                .push_vertex(MonoSide::Lower, vi, vertex, out);
        }

        if pending.is_empty() {
            self.connect_right_vertex(vi, vertex, incident_start, lower_mono, upper_mono);
        } else {
            self.create_right_edges(vi, vertex, incident_start, pending, lower_mono, upper_mono);
        }
    }

    fn find_incident_range(&self, vertex: FPoint) -> std::ops::Range<usize> {
        let start = self
            .active_edges
            .iter()
            .position(|ae| ae.edge.compare_to_point(vertex) != std::cmp::Ordering::Less)
            .unwrap_or(self.active_edges.len());
        let end = self
            .active_edges
            .iter()
            .rposition(|ae| ae.edge.compare_to_point(vertex) != std::cmp::Ordering::Greater)
            .map_or(0, |i| i + 1);
        start..end
    }

    fn fix_temporary_edges(&mut self, vertex: FPoint, range: &mut std::ops::Range<usize>) {
        while range.start > 0 && self.active_edges[range.start - 1].is_temporary {
            range.start -= 1;
            self.active_edges[range.start].split(vertex);
        }
        while range.end < self.active_edges.len() && self.active_edges[range.end].is_temporary {
            self.active_edges[range.end].split(vertex);
            range.end += 1;
        }
    }

    fn last_lower_winding(&self, incident_start: usize) -> FWinding {
        if incident_start == 0 {
            FWinding::ZERO
        } else {
            self.active_edges[incident_start - 1].upper_region_winding
        }
    }

    fn region_is_interior(&self, winding: FWinding) -> bool {
        match self.fill_rule {
            FillRule::NonZero => winding.0 != 0,
            FillRule::EvenOdd => (winding.0 & 1) != 0,
        }
    }

    fn connect_left_vertex(&mut self, incident_start: usize) -> (Option<usize>, Option<usize>) {
        if !self.region_is_interior(self.last_lower_winding(incident_start)) {
            return (None, None);
        }
        let ae0 = self.active_edges[incident_start - 1];
        let ae1 = self.active_edges[incident_start];
        if ae0.edge.start.partial_cmp(&ae1.edge.start) != Some(std::cmp::Ordering::Greater) {
            let um = self.mono_arena.start_mono(ae1.start_index, ae1.edge.start);
            let old = self.active_edges[incident_start].lower_mono.replace(um);
            (old, Some(um))
        } else {
            let lm = self.mono_arena.start_mono(ae0.start_index, ae0.edge.start);
            let old = self.active_edges[incident_start - 1].upper_mono.replace(lm);
            (Some(lm), old)
        }
    }

    fn finish_left_monos(
        &mut self,
        vi: u32,
        left_edges: &[SweepActiveEdge],
        out: &mut Vec<u32>,
    ) -> (Option<usize>, Option<usize>) {
        for le in &left_edges[..left_edges.len() - 1] {
            if self.region_is_interior(le.upper_region_winding) {
                if let Some(um) = le.upper_mono {
                    self.mono_arena.finish_mono(um, vi, out);
                }
            }
        }
        (
            left_edges.first().unwrap().lower_mono,
            left_edges.last().unwrap().upper_mono,
        )
    }

    fn connect_right_vertex(
        &mut self,
        vi: u32,
        vertex: FPoint,
        incident_start: usize,
        lower_mono: Option<usize>,
        upper_mono: Option<usize>,
    ) {
        let lower_winding = self.last_lower_winding(incident_start);
        if !self.region_is_interior(lower_winding) {
            return;
        }
        let end_point = {
            let ae0 = self.active_edges[incident_start - 1];
            let ae1 = self.active_edges[incident_start];
            if ae0.edge.end.partial_cmp(&ae1.edge.end) != Some(std::cmp::Ordering::Greater) {
                (ae0.edge.end, ae0.end_index)
            } else {
                (ae1.edge.end, ae1.end_index)
            }
        };
        self.active_edges.insert(
            incident_start,
            SweepActiveEdge {
                is_temporary: true,
                winding: FWinding::ZERO,
                start_index: vi,
                end_index: end_point.1,
                edge: FSegment::new(vertex, end_point.0),
                upper_region_winding: lower_winding,
                lower_mono,
                upper_mono,
            },
        );
    }

    fn create_right_edges(
        &mut self,
        vi: u32,
        vertex: FPoint,
        incident_start: usize,
        pending: &[SweepPendingEdge],
        mut lower_mono: Option<usize>,
        upper_mono: Option<usize>,
    ) {
        let mut lower_winding = self.last_lower_winding(incident_start);
        let new_edges: Vec<SweepActiveEdge> = pending
            .iter()
            .enumerate()
            .map(|(i, pe)| {
                let upper_winding = lower_winding + pe.winding;
                let um = if self.region_is_interior(upper_winding) {
                    if i == pending.len() - 1 {
                        upper_mono
                    } else {
                        Some(self.mono_arena.start_mono(vi, vertex))
                    }
                } else {
                    None
                };
                let ae = SweepActiveEdge {
                    is_temporary: false,
                    winding: pe.winding,
                    start_index: vi,
                    end_index: pe.end_index,
                    edge: pe.to_segment(vertex),
                    upper_region_winding: upper_winding,
                    lower_mono,
                    upper_mono: um,
                };
                lower_winding = upper_winding;
                lower_mono = um;
                ae
            })
            .collect();
        self.active_edges
            .splice(incident_start..incident_start, new_edges);
    }
}
