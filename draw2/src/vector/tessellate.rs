use crate::vector::path::*;
use std::f32::consts::PI;

// Output vertex: position + texcoord for AA + distance along stroke
#[derive(Clone, Copy, Default, Debug)]
pub struct VVertex {
    pub x: f32,
    pub y: f32,
    pub u: f32,
    pub v: f32,
    pub stroke_dist: f32,
}

impl VVertex {
    fn new(x: f32, y: f32, u: f32, v: f32) -> Self {
        Self {
            x,
            y,
            u,
            v,
            stroke_dist: 0.0,
        }
    }
    fn with_dist(x: f32, y: f32, u: f32, v: f32, stroke_dist: f32) -> Self {
        Self {
            x,
            y,
            u,
            v,
            stroke_dist,
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
}

#[derive(Debug)]
struct SubPath {
    first: usize,
    count: usize,
    closed: bool,
    winding: Winding,
    convex: bool,
    nbevel: usize,
}

impl Tessellator {
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

    /// Generate stroke geometry. Returns (vertices, indices).
    pub fn stroke(
        &mut self,
        w: f32,
        line_cap: LineCap,
        line_join: LineJoin,
        miter_limit: f32,
        aa: f32,
    ) -> (Vec<VVertex>, Vec<u32>) {
        let hw = w * 0.5 + aa * 0.5;
        self.calculate_joins(hw, line_join, miter_limit);
        let mut verts = Vec::new();
        let mut indices = Vec::new();
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
            let mut cum_dists = vec![0.0f32; count];
            {
                let mut d = 0.0f32;
                for j in 1..count {
                    d += self.points[first + j - 1].len;
                    cum_dists[j] = d;
                }
            }
            // caps for open paths
            if !is_loop {
                let p0 = self.points[first];
                let p1 = self.points[first + 1];
                let dx = p1.x - p0.x;
                let dy = p1.y - p0.y;
                let len = (dx * dx + dy * dy).sqrt();
                let (ndx, ndy) = if len > 1e-6 {
                    (dx / len, dy / len)
                } else {
                    (0.0, 0.0)
                };
                self.emit_cap_start(&mut verts, p0.x, p0.y, ndx, ndy, hw, aa, u0, u1, line_cap);
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
                let dist = cum_dists[j];
                let flags = p1.flags;
                if (flags & (PT_BEVEL | PT_INNERBEVEL)) != 0 {
                    let vi_before = verts.len();
                    self.emit_bevel_join(&mut verts, &mut indices, p0, p1, hw, hw, u0, u1);
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
                // end cap
                let p0 = self.points[first + count - 2];
                let p1 = self.points[first + count - 1];
                let dx = p1.x - p0.x;
                let dy = p1.y - p0.y;
                let len = (dx * dx + dy * dy).sqrt();
                let (ndx, ndy) = if len > 1e-6 {
                    (dx / len, dy / len)
                } else {
                    (0.0, 0.0)
                };
                let vi_before = verts.len();
                self.emit_cap_end(
                    &mut verts,
                    &mut indices,
                    p1.x,
                    p1.y,
                    ndx,
                    ndy,
                    hw,
                    aa,
                    u0,
                    u1,
                    line_cap,
                );
                let total_dist = cum_dists[count - 1];
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
        (verts, indices)
    }

    fn emit_cap_start(
        &self,
        verts: &mut Vec<VVertex>,
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
                let ncap = ((w * PI).ceil() as usize).max(2).min(32);
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
                    verts.push(VVertex::new(px, py, 0.5, 1.0));
                }
                verts.push(VVertex::new(px + dlx * w, py + dly * w, u0, 1.0));
                verts.push(VVertex::new(px - dlx * w, py - dly * w, u1, 1.0));
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
                verts.push(VVertex::new(px + dlx * w, py + dly * w, u0, 1.0));
                verts.push(VVertex::new(px - dlx * w, py - dly * w, u1, 1.0));
                for i in 0..ncap {
                    let a = i as f32 / (ncap - 1) as f32 * PI;
                    let ax = a.cos() * w;
                    let ay = a.sin() * w;
                    verts.push(VVertex::new(px, py, 0.5, 1.0));
                    verts.push(VVertex::new(
                        px - dlx * ax + dx * ay,
                        py - dly * ax + dy * ay,
                        u0,
                        1.0,
                    ));
                }
            }
        }
        // connect cap end to previous pair
        if vi >= 2 {
            indices.push(vi - 2);
            indices.push(vi - 1);
            indices.push(vi);
            indices.push(vi - 1);
            indices.push(vi + 1);
            indices.push(vi);
        }
        // stitch cap triangles
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

    /// Generate fill geometry (convex fan + AA fringe). Returns (vertices, indices).
    pub fn fill(
        &mut self,
        aa: f32,
        line_join: LineJoin,
        miter_limit: f32,
    ) -> (Vec<VVertex>, Vec<u32>) {
        let woff = aa * 0.5;
        self.calculate_joins(woff, line_join, miter_limit);
        let mut verts = Vec::new();
        let mut indices = Vec::new();
        for pi in 0..self.paths.len() {
            let sp = &self.paths[pi];
            let first = sp.first;
            let count = sp.count;
            if count < 3 {
                continue;
            }
            // convex fan at original path positions (fully opaque)
            let base = verts.len() as u32;
            for j in 0..count {
                let pt = self.points[first + j];
                verts.push(VVertex::new(pt.x, pt.y, 0.5, 1.0));
            }
            for j in 2..count as u32 {
                indices.push(base);
                indices.push(base + j - 1);
                indices.push(base + j);
            }
            // AA fringe: from path edge (opaque) outward (transparent)
            // dm points inward for CCW, so -dm is outward
            if woff > 0.0 {
                let fringe_base = verts.len() as u32;
                for j in 0..count {
                    let p1 = self.points[first + j];
                    // inner: at path edge (opaque)
                    verts.push(VVertex::new(p1.x, p1.y, 0.5, 1.0));
                    // outer: pushed outward by -dm*aa (transparent)
                    verts.push(VVertex::new(
                        p1.x - p1.dmx * woff * 2.0,
                        p1.y - p1.dmy * woff * 2.0,
                        0.0,
                        1.0,
                    ));
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
        (verts, indices)
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
    ) -> (Vec<VVertex>, Vec<u32>) {
        let expand = blur * 3.0;
        self.calculate_joins(expand, line_join, miter_limit);
        let mut verts = Vec::new();
        let mut indices = Vec::new();
        for pi in 0..self.paths.len() {
            let sp = &self.paths[pi];
            let first = sp.first;
            let count = sp.count;
            if count < 3 {
                continue;
            }
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
                let mut ox = -p1.dmx * expand;
                let mut oy = -p1.dmy * expand;
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
        (verts, indices)
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
