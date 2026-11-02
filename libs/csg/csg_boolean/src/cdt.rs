// 2D Constrained Delaunay Triangulation (CDT).
//
// Used to re-triangulate triangle faces split by intersection segments
// during mesh corefinement.
//
// Algorithm:
// 1. Super-triangle containing all points.
// 2. Bowyer-Watson incremental insertion (cavity-based).
// 3. Constraint enforcement: the triangles a constraint crosses are retriangulated on both
//    sides of it (constrained Delaunay pseudo-polygons).
// 4. Remove super-triangle vertices.
//
// Adjacency: adj[3*t+e] stores the HALF-EDGE index (3*t2+e2) of the twin,
// or NONE if boundary. This gives O(1) access to the neighboring triangle
// and the specific shared edge.

use makepad_csg_math::{in_circle, orient2d, thread_pool};
use makepad_csg_math::fast_hash::FastMap as HashMap;

#[derive(Clone, Copy, Debug)]
pub struct Point2 {
    pub x: f64,
    pub y: f64,
}

const NONE: u32 = u32::MAX;

pub struct CDT {
    points: Vec<Point2>,
    tri: Vec<u32>,          // tri[3*t+i] = vertex index for corner i
    adj: Vec<u32>,          // adj[3*t+e] = twin half-edge (3*t2+e2) or NONE
    constrained: Vec<bool>, // constrained[3*t+e] = edge is a constraint
    constraints: Vec<(u32, u32)>,
    num_super: u32,
    /// A live triangle at each point, while constraints go in.
    vert_tri: Vec<u32>,
    /// Per triangle, the insertion that last visited it / found it in the cavity (`gen`):
    /// marks reused across insertions instead of two cleared arrays per point (O(n) each).
    seen: Vec<u32>,
    bad_at: Vec<u32>,
    gen: u32,
}

impl CDT {
    pub fn new(bounds_min: Point2, bounds_max: Point2) -> CDT {
        let dx = bounds_max.x - bounds_min.x;
        let dy = bounds_max.y - bounds_min.y;
        let margin = (dx + dy).max(1e-10) * 20.0;
        let cx = (bounds_min.x + bounds_max.x) * 0.5;
        let cy = (bounds_min.y + bounds_max.y) * 0.5;

        CDT {
            points: vec![
                Point2 {
                    x: cx - 2.0 * margin,
                    y: cy - margin,
                },
                Point2 {
                    x: cx + 2.0 * margin,
                    y: cy - margin,
                },
                Point2 {
                    x: cx,
                    y: cy + 2.0 * margin,
                },
            ],
            tri: vec![0, 1, 2],
            adj: vec![NONE, NONE, NONE],
            constrained: vec![false, false, false],
            constraints: Vec::new(),
            num_super: 3,
            vert_tri: Vec::new(),
            seen: Vec::new(),
            bad_at: Vec::new(),
            gen: 0,
        }
    }

    fn num_tris(&self) -> usize {
        self.tri.len() / 3
    }

    fn tri_verts(&self, t: usize) -> [u32; 3] {
        [self.tri[t * 3], self.tri[t * 3 + 1], self.tri[t * 3 + 2]]
    }

    fn is_deleted(&self, t: usize) -> bool {
        self.tri[t * 3] == NONE
    }

    fn pt(&self, v: u32) -> Point2 {
        self.points[v as usize]
    }

    pub fn insert_point(&mut self, x: f64, y: f64) -> u32 {
        let idx = self.points.len() as u32;
        self.points.push(Point2 { x, y });
        // (cancellation is looked at once per point and once per constraint: each is bounded
        // work, and the thread-local read per walk or flood step was the inner loop's cost)
        if !thread_pool::cancelled() {
            self.bowyer_watson_insert(idx);
        }
        idx
    }

    pub fn add_constraint(&mut self, a: u32, b: u32) {
        self.constraints.push((a, b));
    }

    pub fn get_triangles(&self) -> Vec<[u32; 3]> {
        let ns = self.num_super;
        let mut result = Vec::new();
        for t in 0..self.num_tris() {
            if self.is_deleted(t) {
                continue;
            }
            let [a, b, c] = self.tri_verts(t);
            if a >= ns && b >= ns && c >= ns {
                result.push([a - ns, b - ns, c - ns]);
            }
        }
        result
    }

    pub fn finalize(&mut self) {
        self.vert_tri = vec![NONE; self.points.len()];
        for t in 0..self.num_tris() {
            if !self.is_deleted(t) {
                for v in self.tri_verts(t) {
                    self.vert_tri[v as usize] = t as u32;
                }
            }
        }
        for i in 0..self.constraints.len() {
            if thread_pool::cancelled() {
                return;
            }
            let (a, b) = self.constraints[i];
            self.enforce_constraint(a, b);
        }
    }

    // =========================================================================
    // Bowyer-Watson
    // =========================================================================

    fn bowyer_watson_insert(&mut self, p_idx: u32) {
        let p = self.pt(p_idx);

        // Find containing triangle
        let start = match self.find_containing_triangle(p) {
            Some(t) => t,
            None => return,
        };

        // Check for duplicate
        for &v in &self.tri_verts(start) {
            let q = self.pt(v);
            if (p.x - q.x) * (p.x - q.x) + (p.y - q.y) * (p.y - q.y) < 1e-24 {
                return;
            }
        }

        // Flood-fill to find all "bad" triangles whose circumcircle contains p
        let mut bad = Vec::new();
        self.gen = self.gen.wrapping_add(1);
        if self.gen == 0 {
            // (after 2^32 insertions: start the marks over)
            self.seen.iter_mut().for_each(|m| *m = 0);
            self.bad_at.iter_mut().for_each(|m| *m = 0);
            self.gen = 1;
        }
        let gen = self.gen;
        let n = self.num_tris();
        if self.seen.len() < n {
            self.seen.resize(n, 0);
            self.bad_at.resize(n, 0);
        }
        let mut stack = vec![start];

        while let Some(t) = stack.pop() {
            if self.seen[t] == gen {
                continue;
            }
            self.seen[t] = gen;

            if self.is_deleted(t) {
                continue;
            }
            let [a, b, c] = self.tri_verts(t);

            if self.in_circumcircle_test(a, b, c, p) {
                bad.push(t);
                for e in 0..3 {
                    let tw = self.adj[t * 3 + e];
                    if tw != NONE {
                        let nb = (tw / 3) as usize;
                        if self.seen[nb] != gen {
                            stack.push(nb);
                        }
                    }
                }
            }
        }

        if bad.is_empty() {
            return;
        }

        // Find boundary edges of the cavity
        for &t in &bad {
            self.bad_at[t] = gen;
        }

        // (from, to, outside_half_edge_or_NONE)
        let mut boundary: Vec<(u32, u32, u32)> = Vec::new();
        for &t in &bad {
            let verts = self.tri_verts(t);
            for e in 0..3usize {
                let tw = self.adj[t * 3 + e];
                let nb_is_bad = tw != NONE && self.bad_at[(tw / 3) as usize] == gen;
                if !nb_is_bad {
                    let from = verts[(e + 1) % 3];
                    let to = verts[(e + 2) % 3];
                    boundary.push((from, to, tw));
                }
            }
        }

        // Delete bad triangles
        for &t in &bad {
            self.tri[t * 3] = NONE;
            self.tri[t * 3 + 1] = NONE;
            self.tri[t * 3 + 2] = NONE;
            self.adj[t * 3] = NONE;
            self.adj[t * 3 + 1] = NONE;
            self.adj[t * 3 + 2] = NONE;
            self.constrained[t * 3] = false;
            self.constrained[t * 3 + 1] = false;
            self.constrained[t * 3 + 2] = false;
        }

        // Create new triangles: one per boundary edge
        let n_new = boundary.len();
        let mut free: Vec<usize> = bad.clone();
        let mut new_tris: Vec<usize> = Vec::with_capacity(n_new);

        for i in 0..n_new {
            let (from, to, _) = boundary[i];
            let t = if let Some(slot) = free.pop() {
                slot
            } else {
                let slot = self.num_tris();
                self.tri.extend_from_slice(&[0, 0, 0]);
                self.adj.extend_from_slice(&[NONE, NONE, NONE]);
                self.constrained.extend_from_slice(&[false, false, false]);
                slot
            };

            // Triangle: (from, to, p_idx)
            // edge 0 (opp from): to -> p    [internal: connects to another new tri]
            // edge 1 (opp to):   p -> from  [internal: connects to another new tri]
            // edge 2 (opp p):    from -> to [external: connects to outside]
            self.tri[t * 3] = from;
            self.tri[t * 3 + 1] = to;
            self.tri[t * 3 + 2] = p_idx;
            self.adj[t * 3] = NONE;
            self.adj[t * 3 + 1] = NONE;
            self.adj[t * 3 + 2] = NONE;
            self.constrained[t * 3] = false;
            self.constrained[t * 3 + 1] = false;
            self.constrained[t * 3 + 2] = false;

            new_tris.push(t);
        }

        // Fix adjacency for external edges (edge 2: from -> to)
        for i in 0..n_new {
            let t = new_tris[i];
            let outside_he = boundary[i].2;
            self.adj[t * 3 + 2] = outside_he;
            if outside_he != NONE {
                self.adj[outside_he as usize] = (t * 3 + 2) as u32;
            }
        }

        // Fix adjacency for internal edges between new triangles.
        // Build lookup: from_vertex → boundary index (O(n) instead of O(n^2)).
        // edge 0 of tri i (to_i -> p): twin is edge 1 of tri j where from_j == to_i
        let mut from_to_idx: HashMap<u32, usize> = HashMap::with_capacity_and_hasher(n_new, Default::default());
        for i in 0..n_new {
            let from_i = boundary[i].0;
            from_to_idx.insert(from_i, i);
        }
        for i in 0..n_new {
            let ti = new_tris[i];
            let to_i = boundary[i].1;
            if let Some(&j) = from_to_idx.get(&to_i) {
                let tj = new_tris[j];
                self.adj[ti * 3] = (tj * 3 + 1) as u32;
                self.adj[tj * 3 + 1] = (ti * 3) as u32;
            }
        }
    }

    fn in_circumcircle_test(&self, a: u32, b: u32, c: u32, p: Point2) -> bool {
        let pa = self.pt(a);
        let pb = self.pt(b);
        let pc = self.pt(c);
        let orient = orient2d(pa.x, pa.y, pb.x, pb.y, pc.x, pc.y);
        if orient > 0.0 {
            in_circle(pa.x, pa.y, pb.x, pb.y, pc.x, pc.y, p.x, p.y) > 0.0
        } else if orient < 0.0 {
            in_circle(pa.x, pa.y, pc.x, pc.y, pb.x, pb.y, p.x, p.y) > 0.0
        } else {
            true // degenerate
        }
    }

    fn find_containing_triangle(&self, p: Point2) -> Option<usize> {
        // Walk towards p using adjacency. O(sqrt(n)) expected vs O(n) scan.
        // Find any non-deleted triangle to start from.
        let mut t = 0;
        // Start at the newest triangle: it is alive, and points tend to come in order. The
        // older ones are mostly deleted, and walking past them grows with every insert.
        t = t.max(self.num_tris().saturating_sub(1));
        while t > 0 && self.is_deleted(t) {
            t -= 1;
        }
        if self.is_deleted(t) {
            return None;
        }

        let max_steps = self.num_tris() * 2;
        for _ in 0..max_steps {
            if self.is_deleted(t) {
                // Fallback to linear scan if walk hits deleted triangle
                return self.find_containing_triangle_linear(p);
            }
            let [a, b, c] = self.tri_verts(t);
            let pa = self.pt(a);
            let pb = self.pt(b);
            let pc = self.pt(c);
            let d0 = orient2d(pa.x, pa.y, pb.x, pb.y, p.x, p.y);
            let d1 = orient2d(pb.x, pb.y, pc.x, pc.y, p.x, p.y);
            let d2 = orient2d(pc.x, pc.y, pa.x, pa.y, p.x, p.y);

            // Check if p is inside this triangle (works for both CCW and CW)
            if !(d0 < 0.0 || d1 < 0.0 || d2 < 0.0) && (d0 > 0.0 || d1 > 0.0 || d2 > 0.0) {
                return Some(t);
            }
            if !(d0 > 0.0 || d1 > 0.0 || d2 > 0.0) && (d0 < 0.0 || d1 < 0.0 || d2 < 0.0) {
                return Some(t);
            }

            // Walk across the edge p is furthest outside of. Edge e is the one opposite corner
            // e (adj's layout): d0 is a-b (edge 2), d1 b-c (edge 0), d2 c-a (edge 1). Crossing
            // the wrong one wandered until the walk gave up into the linear scan.
            let min_edge = if d0 <= d1 && d0 <= d2 {
                2
            } else if d1 <= d2 {
                0
            } else {
                1
            };

            let twin = self.adj[t * 3 + min_edge];
            if twin == NONE {
                // Boundary — fallback to linear scan
                return self.find_containing_triangle_linear(p);
            }
            t = (twin / 3) as usize;
        }

        self.find_containing_triangle_linear(p)
    }

    fn find_containing_triangle_linear(&self, p: Point2) -> Option<usize> {
        for t in 0..self.num_tris() {
            if self.is_deleted(t) {
                continue;
            }
            let [a, b, c] = self.tri_verts(t);
            let pa = self.pt(a);
            let pb = self.pt(b);
            let pc = self.pt(c);
            let d0 = orient2d(pa.x, pa.y, pb.x, pb.y, p.x, p.y);
            let d1 = orient2d(pb.x, pb.y, pc.x, pc.y, p.x, p.y);
            let d2 = orient2d(pc.x, pc.y, pa.x, pa.y, p.x, p.y);
            if !(d0 < 0.0 || d1 < 0.0 || d2 < 0.0) && (d0 > 0.0 || d1 > 0.0 || d2 > 0.0) {
                return Some(t);
            }
            if !(d0 > 0.0 || d1 > 0.0 || d2 > 0.0) && (d0 < 0.0 || d1 < 0.0 || d2 < 0.0) {
                return Some(t);
            }
        }
        None
    }

    // =========================================================================
    // Constraint enforcement
    // =========================================================================

    /// The triangles round point `a`, each with `a`'s corner in it.
    fn fan(&self, a: u32, out: &mut Vec<(usize, usize)>) {
        out.clear();
        let start = self.vert_tri[a as usize];
        if start == NONE {
            return; // a duplicate point: it never joined the triangulation
        }
        let corner = |t: usize| self.tri_verts(t).iter().position(|v| *v == a);
        let Some(i) = corner(start as usize) else { return };
        out.push((start as usize, i));
        let mut next = 0;
        while next < out.len() {
            let (t, i) = out[next];
            next += 1;
            // Over the two edges that meet at `a`.
            for e in [(i + 1) % 3, (i + 2) % 3] {
                let twin = self.adj[t * 3 + e];
                if twin == NONE {
                    continue;
                }
                let t2 = (twin / 3) as usize;
                if !out.iter().any(|(s, _)| *s == t2) {
                    if let Some(j) = corner(t2) {
                        out.push((t2, j));
                    }
                }
            }
        }
    }

    /// Makes `a`-`b` an edge. The triangles the segment crosses are taken out and the two
    /// pseudo-polygons either side of it are triangulated again, each constrained Delaunay
    /// (Anglada's pseudo-polygon triangulation). No flip sequence is involved, so nothing can
    /// cycle: flipping each crossed edge in turn, walking again from `a` after every flip,
    /// flipped one quad's two diagonals back and forth forever when both crossed the segment
    /// (the `%` slash through its rings). The work is the crossed triangles alone. A segment
    /// that runs through a vertex or across another constraint can not be one edge and is left
    /// out (the caller finds it missing).
    fn enforce_constraint(&mut self, a: u32, b: u32) {
        if a == b {
            return;
        }
        let mut fan = Vec::new();
        self.fan(a, &mut fan);
        // Already an edge.
        let found = fan.iter().find_map(|&(t, i)| {
            let v = self.tri_verts(t);
            [(i + 1) % 3, (i + 2) % 3].into_iter().find(|&j| v[j] == b).map(|j| (t, 3 - i - j))
        });
        if let Some((t, e)) = found {
            self.constrained[t * 3 + e] = true;
            let twin = self.adj[t * 3 + e];
            if twin != NONE {
                self.constrained[twin as usize] = true;
            }
            return;
        }
        let (pa, pb) = (self.pt(a), self.pt(b));
        let side = |cdt: &CDT, v: u32| {
            let p = cdt.pt(v);
            orient2d(pa.x, pa.y, pb.x, pb.y, p.x, p.y)
        };
        // The triangle at `a` whose far edge the segment leaves through.
        let Some((t0, i0)) = fan.iter().find_map(|&(t, i)| {
            let v = self.tri_verts(t);
            edges_cross(pa, pb, self.pt(v[(i + 1) % 3]), self.pt(v[(i + 2) % 3])).then_some((t, i))
        }) else {
            return;
        };
        // Walk the crossed triangles, gathering the vertices left and right of a->b in order.
        let v0 = self.tri_verts(t0);
        let (x, y) = (v0[(i0 + 1) % 3], v0[(i0 + 2) % 3]);
        let (mut l, mut r) = if side(self, x) > 0.0 { (x, y) } else { (y, x) };
        let mut left = vec![a, l];
        let mut right = vec![a, r];
        let mut cavity = vec![t0];
        let (mut t, mut e) = (t0, i0);
        loop {
            // Every step crosses the segment further along, so a cavity larger than the
            // triangulation means broken adjacency, not a long segment.
            if self.constrained[t * 3 + e] || cavity.len() > self.num_tris() {
                return;
            }
            let twin = self.adj[t * 3 + e];
            if twin == NONE {
                return;
            }
            let (t2, e2) = ((twin / 3) as usize, (twin % 3) as usize);
            cavity.push(t2);
            let v2 = self.tri_verts(t2);
            let w = v2[e2];
            if w == b {
                break;
            }
            let s = side(self, w);
            if s == 0.0 {
                return; // through a vertex
            }
            let replaced = if s > 0.0 {
                left.push(w);
                std::mem::replace(&mut l, w)
            } else {
                right.push(w);
                std::mem::replace(&mut r, w)
            };
            // On through the edge from `w` to the side it did not replace.
            let Some(j) = v2.iter().position(|&v| v == replaced) else { return };
            (t, e) = (t2, j);
        }
        left.push(b);
        right.push(b);
        // The cavity's outer edges, by their direction in the cavity's (CCW) triangles.
        let mut outer: HashMap<(u32, u32), (u32, bool)> = HashMap::with_capacity_and_hasher(cavity.len() * 3, Default::default());
        for &t in &cavity {
            let v = self.tri_verts(t);
            for e in 0..3 {
                outer.insert((v[(e + 1) % 3], v[(e + 2) % 3]), (self.adj[t * 3 + e], self.constrained[t * 3 + e]));
            }
        }
        // (an edge between two cavity triangles is there both ways round)
        let shared: Vec<(u32, u32)> = outer.keys().filter(|&&(f, to)| outer.contains_key(&(to, f))).copied().collect();
        for k in shared {
            outer.remove(&k);
        }
        // Both sides, each as a chain from its base edge's start to its end: the triangle on
        // the base is (start, end, apex), CCW. Left of a->b the base runs a->b, right b->a.
        right.reverse();
        let mut new_tris: Vec<[u32; 3]> = Vec::with_capacity(cavity.len());
        let mut ranges = Vec::new();
        for chain in [&left, &right] {
            ranges.push((0, chain.len() - 1));
            while let Some((lo, hi)) = ranges.pop() {
                if hi - lo < 2 {
                    continue;
                }
                // The apex whose circle with the base holds no other vertex of the polygon.
                let mut c = lo + 1;
                for i in lo + 2..hi {
                    if self.in_circumcircle_test(chain[lo], chain[hi], chain[c], self.pt(chain[i])) {
                        c = i;
                    }
                }
                new_tris.push([chain[lo], chain[hi], chain[c]]);
                ranges.push((lo, c));
                ranges.push((c, hi));
            }
        }
        if new_tris.len() != cavity.len() {
            return; // (cannot happen: a polygon of n vertices takes n - 2 triangles)
        }
        // Into the cavity's slots, joined to each other and to the outer edges.
        let mut inner: HashMap<(u32, u32), u32> = HashMap::with_capacity_and_hasher(new_tris.len() * 3, Default::default());
        for (&t, v) in cavity.iter().zip(&new_tris) {
            self.tri[t * 3..t * 3 + 3].copy_from_slice(v);
            for e in 0..3 {
                let h = (t * 3 + e) as u32;
                let (f, to) = (v[(e + 1) % 3], v[(e + 2) % 3]);
                if let Some(&(twin, constrained)) = outer.get(&(f, to)) {
                    self.adj[h as usize] = twin;
                    self.constrained[h as usize] = constrained;
                    if twin != NONE {
                        self.adj[twin as usize] = h;
                    }
                } else {
                    let constrained = (f == a && to == b) || (f == b && to == a);
                    self.constrained[h as usize] = constrained;
                    self.adj[h as usize] = NONE;
                    if let Some(&twin) = inner.get(&(to, f)) {
                        self.adj[h as usize] = twin;
                        self.adj[twin as usize] = h;
                    } else {
                        inner.insert((f, to), h);
                    }
                }
            }
            for &p in v {
                self.vert_tri[p as usize] = t as u32;
            }
        }
    }
}

fn edges_cross(a: Point2, b: Point2, c: Point2, d: Point2) -> bool {
    let d1 = orient2d(a.x, a.y, b.x, b.y, c.x, c.y);
    let d2 = orient2d(a.x, a.y, b.x, b.y, d.x, d.y);
    let d3 = orient2d(c.x, c.y, d.x, d.y, a.x, a.y);
    let d4 = orient2d(c.x, c.y, d.x, d.y, b.x, b.y);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_adj(cdt: &CDT) {
        for h in 0..cdt.adj.len() {
            let t = h / 3;
            if cdt.is_deleted(t) {
                continue;
            }
            let tw = cdt.adj[h];
            if tw == NONE {
                continue;
            }
            let tw = tw as usize;
            assert!(
                tw < cdt.adj.len(),
                "twin {} out of bounds (len={})",
                tw,
                cdt.adj.len()
            );
            let t2 = tw / 3;
            assert!(!cdt.is_deleted(t2), "twin points to deleted tri");
            assert_eq!(
                cdt.adj[tw], h as u32,
                "twin symmetry: h={} tw={} tw_tw={}",
                h, tw, cdt.adj[tw]
            );
            // Check shared vertices
            let e1 = h % 3;
            let e2 = tw % 3;
            let v1 = cdt.tri_verts(t);
            let v2 = cdt.tri_verts(t2);
            assert!(
                v1[(e1 + 1) % 3] == v2[(e2 + 2) % 3] && v1[(e1 + 2) % 3] == v2[(e2 + 1) % 3],
                "edge mismatch h={} ({}->{}) tw={} ({}->{})",
                h,
                v1[(e1 + 1) % 3],
                v1[(e1 + 2) % 3],
                tw,
                v2[(e2 + 1) % 3],
                v2[(e2 + 2) % 3]
            );
        }
    }

    #[test]
    fn test_basic_triangle() {
        let mut cdt = CDT::new(Point2 { x: -1.0, y: -1.0 }, Point2 { x: 2.0, y: 2.0 });
        cdt.insert_point(0.0, 0.0);
        check_adj(&cdt);
        cdt.insert_point(1.0, 0.0);
        check_adj(&cdt);
        cdt.insert_point(0.5, 1.0);
        check_adj(&cdt);
        cdt.finalize();
        let tris = cdt.get_triangles();
        assert_eq!(tris.len(), 1, "3 points => 1 triangle, got {}", tris.len());
    }

    #[test]
    fn test_square() {
        let mut cdt = CDT::new(Point2 { x: -1.0, y: -1.0 }, Point2 { x: 2.0, y: 2.0 });
        cdt.insert_point(0.0, 0.0);
        cdt.insert_point(1.0, 0.0);
        cdt.insert_point(1.0, 1.0);
        cdt.insert_point(0.0, 1.0);
        check_adj(&cdt);
        cdt.finalize();
        let tris = cdt.get_triangles();
        assert_eq!(tris.len(), 2, "4 points => 2 triangles, got {}", tris.len());
    }

    #[test]
    fn test_interior_point() {
        let mut cdt = CDT::new(Point2 { x: -1.0, y: -1.0 }, Point2 { x: 3.0, y: 3.0 });
        cdt.insert_point(0.0, 0.0);
        cdt.insert_point(2.0, 0.0);
        cdt.insert_point(1.0, 2.0);
        cdt.insert_point(1.0, 0.5);
        check_adj(&cdt);
        cdt.finalize();
        let tris = cdt.get_triangles();
        assert_eq!(
            tris.len(),
            3,
            "3+1 interior => 3 triangles, got {}",
            tris.len()
        );
    }

    #[test]
    fn test_constraint() {
        let mut cdt = CDT::new(Point2 { x: -1.0, y: -1.0 }, Point2 { x: 2.0, y: 2.0 });
        let a = cdt.insert_point(0.0, 0.0);
        cdt.insert_point(1.0, 0.0);
        let c = cdt.insert_point(1.0, 1.0);
        cdt.insert_point(0.0, 1.0);
        cdt.add_constraint(a, c);
        cdt.finalize();
        let tris = cdt.get_triangles();
        assert_eq!(tris.len(), 2);
        let has_ac = tris.iter().any(|t| {
            [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])]
                .iter()
                .any(|&(u, v)| (u == 0 && v == 2) || (u == 2 && v == 0))
        });
        assert!(has_ac, "constraint edge should exist");
    }

    #[test]
    fn test_area_conservation() {
        let mut cdt = CDT::new(Point2 { x: -1.0, y: -1.0 }, Point2 { x: 3.0, y: 3.0 });
        cdt.insert_point(0.0, 0.0);
        cdt.insert_point(2.0, 0.0);
        cdt.insert_point(2.0, 2.0);
        cdt.insert_point(0.0, 2.0);
        cdt.insert_point(1.0, 1.0);
        cdt.finalize();
        let tris = cdt.get_triangles();
        let pts: [(f64, f64); 5] = [(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0), (1.0, 1.0)];
        let area: f64 = tris
            .iter()
            .map(|t| {
                let (ax, ay) = pts[t[0] as usize];
                let (bx, by) = pts[t[1] as usize];
                let (cx, cy) = pts[t[2] as usize];
                ((bx - ax) * (cy - ay) - (cx - ax) * (by - ay)).abs() / 2.0f64
            })
            .sum();
        assert!(
            (area - 4.0).abs() < 0.01,
            "area should be 4.0, got {}",
            area
        );
    }

    /// Long constraints through a dense cloud: each comes back as one edge, the
    /// adjacency stays consistent and the triangles still tile the hull.
    #[test]
    fn long_constraints_through_a_dense_cloud_come_back() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut cdt = CDT::new(Point2 { x: 0.0, y: 0.0 }, Point2 { x: 1.0, y: 1.0 });
        let mut pts = Vec::new();
        for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            pts.push((x, y));
            cdt.insert_point(x, y);
        }
        for _ in 0..600 {
            let (x, y) = (0.01 + 0.98 * rnd(), 0.01 + 0.98 * rnd());
            pts.push((x, y));
            cdt.insert_point(x, y);
        }
        // Parallel slanted segments (they never cross each other) and the hull.
        let mut want = Vec::new();
        for k in 0..6 {
            let y = 0.08 + 0.15 * k as f64;
            let (a, b) = (pts.len(), pts.len() + 1);
            pts.push((0.02, y));
            pts.push((0.98, y + 0.05));
            let ra = cdt.insert_point(0.02, y);
            let rb = cdt.insert_point(0.98, y + 0.05);
            cdt.add_constraint(ra, rb);
            want.push((a, b));
        }
        for i in 0..4 {
            cdt.add_constraint(3 + i as u32, 3 + ((i + 1) % 4) as u32);
            want.push((i, (i + 1) % 4));
        }
        cdt.finalize();
        check_adj(&cdt);
        let tris = cdt.get_triangles();
        let has = |a: usize, b: usize| {
            tris.iter().any(|t| (0..3).any(|e| {
                let (u, v) = (t[e] as usize, t[(e + 1) % 3] as usize);
                (u, v) == (a, b) || (u, v) == (b, a)
            }))
        };
        for (a, b) in want {
            assert!(has(a, b), "constraint {a}-{b} missing");
        }
        let area: f64 = tris
            .iter()
            .map(|t| {
                let (ax, ay) = pts[t[0] as usize];
                let (bx, by) = pts[t[1] as usize];
                let (cx, cy) = pts[t[2] as usize];
                ((bx - ax) * (cy - ay) - (cx - ax) * (by - ay)).abs() / 2.0
            })
            .sum();
        assert!((area - 1.0).abs() < 1e-9, "the triangles tile the unit square: {area}");
    }

    /// Test CDT with a point on an edge (3 collinear points).
    /// This reproduces Face B[19] from the cube-cylinder case:
    /// Triangle (0,0)→(1,0)→(1,-0.707) with point (0.5,0) on edge 0→1.
    #[test]
    fn test_collinear_point_on_edge() {
        let mut cdt = CDT::new(Point2 { x: -1.1, y: -0.8 }, Point2 { x: 1.1, y: 0.1 });
        let i0 = cdt.insert_point(-1.0, 0.0);
        let i1 = cdt.insert_point(1.0, 0.0);
        let i2 = cdt.insert_point(1.0, -0.707107);
        let i3 = cdt.insert_point(0.0, 0.0); // on edge i0→i1
        check_adj(&cdt);

        // Constraints that split edge 0→1 at point 3
        cdt.add_constraint(i0, i3);
        cdt.add_constraint(i3, i1);
        cdt.add_constraint(i2, i1);
        // Boundary
        cdt.add_constraint(i0, i1);
        cdt.add_constraint(i1, i2);
        cdt.add_constraint(i2, i0);
        cdt.finalize();

        let tris = cdt.get_triangles();
        let pts = [(-1.0f64, 0.0f64), (1.0, 0.0), (1.0, -0.707107), (0.0, 0.0)];

        // Count non-degenerate triangles
        let non_degen: Vec<_> = tris
            .iter()
            .filter(|&&[a, b, c]| {
                let pa = pts[a as usize];
                let pb = pts[b as usize];
                let pc = pts[c as usize];
                let area =
                    ((pb.0 - pa.0) * (pc.1 - pa.1) - (pc.0 - pa.0) * (pb.1 - pa.1)).abs() / 2.0;
                area > 1e-10
            })
            .collect();

        // Should have 2 non-degenerate triangles: [0,3,2] and [3,1,2]
        assert!(
            non_degen.len() >= 2,
            "collinear edge split should produce >= 2 non-degenerate triangles, got {} (total {})",
            non_degen.len(),
            tris.len()
        );

        // Total area should equal the original triangle area
        let total_area: f64 = tris
            .iter()
            .map(|&[a, b, c]| {
                let pa = pts[a as usize];
                let pb = pts[b as usize];
                let pc = pts[c as usize];
                ((pb.0 - pa.0) * (pc.1 - pa.1) - (pc.0 - pa.0) * (pb.1 - pa.1)).abs() / 2.0
            })
            .sum();
        let expected = 0.707107; // area of triangle (0,0)→(2,0)→(2,-0.707)
        assert!(
            (total_area - expected).abs() < 0.01,
            "total area should be ~{}, got {}",
            expected,
            total_area
        );
    }
}
