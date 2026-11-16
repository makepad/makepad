// 2D Constrained Delaunay Triangulation (CDT).
//
// Used to re-triangulate triangle faces split by intersection segments
// during mesh corefinement.
//
// Algorithm:
// 1. Super-triangle containing all points.
// 2. Incremental insertion by split and flip (Lawson): the point splits the triangle it lies in
//    (or the two at the edge it lies on), then each edge opposite it flips while the corner across
//    lies strictly inside the circle through the edge and the point. With exact signs that is the
//    triangulation Bowyer-Watson's cavity builds, ties included.
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

/// The predicates a triangulation decides by.
pub trait Predicates {
    /// > 0 when a, b, c turn counterclockwise, < 0 clockwise, 0 collinear (only the sign counts).
    fn orient(&self, points: &[Point2], a: u32, b: u32, c: u32) -> f64;
    /// > 0 when d lies inside the circle through the counterclockwise a, b, c (only the sign counts).
    fn in_circle(&self, points: &[Point2], a: u32, b: u32, c: u32, d: u32) -> f64;
    /// Whether a and b are one point.
    fn same(&self, points: &[Point2], a: u32, b: u32) -> bool;
    /// Whether edge a-b of the counterclockwise (a, b, p) flips to p-d, d being the corner across it:
    /// d strictly inside the circle through a, b, p (a cocircular d does not flip).
    fn flips(&self, points: &[Point2], a: u32, b: u32, p: u32, d: u32) -> bool {
        self.in_circle(points, a, b, p, d) > 0.0
    }
}

/// The predicates on the float positions themselves (what every caller had before).
pub struct FloatPredicates;

impl Predicates for FloatPredicates {
    fn orient(&self, points: &[Point2], a: u32, b: u32, c: u32) -> f64 {
        let [a, b, c] = [a, b, c].map(|i| points[i as usize]);
        orient2d(a.x, a.y, b.x, b.y, c.x, c.y)
    }

    fn in_circle(&self, points: &[Point2], a: u32, b: u32, c: u32, d: u32) -> f64 {
        let [a, b, c, d] = [a, b, c, d].map(|i| points[i as usize]);
        in_circle(a.x, a.y, b.x, b.y, c.x, c.y, d.x, d.y)
    }

    fn same(&self, points: &[Point2], a: u32, b: u32) -> bool {
        let [a, b] = [a, b].map(|i| points[i as usize]);
        a.x == b.x && a.y == b.y
    }
}

pub struct CDT<P: Predicates = FloatPredicates> {
    points: Vec<Point2>,
    predicates: P,
    tri: Vec<u32>,          // tri[3*t+i] = vertex index for corner i
    adj: Vec<u32>,          // adj[3*t+e] = twin half-edge (3*t2+e2) or NONE
    constrained: Vec<bool>, // constrained[3*t+e] = edge is a constraint
    constraints: Vec<(u32, u32)>,
    num_super: u32,
    /// A live triangle at each point, while constraints go in.
    vert_tri: Vec<u32>,
    /// The triangles whose edge opposite the point going in is still to be tested (kept across
    /// insertions for its allocation).
    legalize: Vec<u32>,
}

impl CDT {
    pub fn new(bounds_min: Point2, bounds_max: Point2) -> CDT {
        CDT::with_predicates(bounds_min, bounds_max, FloatPredicates)
    }
}

impl<P: Predicates> CDT<P> {
    pub fn with_predicates(bounds_min: Point2, bounds_max: Point2, predicates: P) -> CDT<P> {
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
            predicates,
            tri: vec![0, 1, 2],
            adj: vec![NONE, NONE, NONE],
            constrained: vec![false, false, false],
            constraints: Vec::new(),
            num_super: 3,
            vert_tri: Vec::new(),
            legalize: Vec::new(),
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

    pub fn insert_point(&mut self, x: f64, y: f64) -> u32 {
        // Flips do not look at constraints: every point goes in before `finalize`.
        debug_assert!(self.vert_tri.is_empty(), "points go in before the constraints");
        let idx = self.points.len() as u32;
        self.points.push(Point2 { x, y });
        // (cancellation is looked at once per point and once per constraint: each is bounded
        // work, and the thread-local read per walk or flood step was the inner loop's cost)
        if !thread_pool::cancelled() {
            self.insert(idx);
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

    /// The constrained edges of the triangulation, without the super triangle's points, each once.
    pub fn constrained_edges(&self) -> Vec<(u32, u32)> {
        let mut edges = Vec::new();
        for t in 0..self.num_tris() {
            if self.is_deleted(t) {
                continue;
            }
            let v = self.tri_verts(t);
            for e in 0..3 {
                let (a, b) = (v[(e + 1) % 3], v[(e + 2) % 3]);
                if self.constrained[t * 3 + e] && a < b && a >= self.num_super {
                    edges.push((a - self.num_super, b - self.num_super));
                }
            }
        }
        edges
    }

    /// Flood from super-triangle points across edges that are not walls.
    /// Return unreached triangles in the same order and indices as `get_triangles`.
    pub fn inside(&self, walls: &[(u32, u32)]) -> Vec<[u32; 3]> {
        let mut walls: Vec<_> = walls.iter().map(|&(a, b)| (a.min(b), a.max(b))).collect();
        walls.sort_unstable();
        walls.dedup();
        let mut outside = vec![false; self.num_tris()];
        let mut stack = Vec::new();
        for (t, reached) in outside.iter_mut().enumerate() {
            if !self.is_deleted(t) && self.tri_verts(t).iter().any(|&v| v < self.num_super) {
                *reached = true;
                stack.push(t);
            }
        }
        while let Some(t) = stack.pop() {
            let v = self.tri_verts(t);
            for e in 0..3 {
                let twin = self.adj[t * 3 + e];
                if twin == NONE {
                    continue;
                }
                let a = v[(e + 1) % 3];
                let b = v[(e + 2) % 3];
                if walls.binary_search(&(a.min(b), a.max(b))).is_ok() {
                    continue;
                }
                let next = (twin / 3) as usize;
                if !outside[next] && !self.is_deleted(next) {
                    outside[next] = true;
                    stack.push(next);
                }
            }
        }
        let mut result = Vec::new();
        for (t, reached) in outside.into_iter().enumerate() {
            if reached || self.is_deleted(t) {
                continue;
            }
            let v = self.tri_verts(t);
            if v.iter().all(|&p| p >= self.num_super) {
                result.push(v.map(|p| p - self.num_super));
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
    // Insertion: split, then flip (Lawson)
    // =========================================================================

    /// Inserts point `p`. A triangle the split or a flip makes always has `p` as its corner 2, so
    /// the edge to test is its edge 2. Every stored triangle runs counterclockwise (orient is exact,
    /// and a flip is made only when the corner across is strictly inside, which makes its quad
    /// strictly convex), and nothing is deleted: the split reuses its triangles' slots and appends
    /// the rest, a flip rewrites its two.
    fn insert(&mut self, p: u32) {
        let Some((t, sides)) = self.find_containing_triangle(p) else { return };
        // Only identical points are duplicates; nearby points keep their own ids.
        if self.tri_verts(t).iter().any(|&v| self.predicates.same(&self.points, p, v)) {
            return;
        }
        let mut stack = std::mem::take(&mut self.legalize);
        let mut zeros = (0..3).filter(|&e| sides[e] == 0.0);
        match (zeros.next(), zeros.next()) {
            (None, _) => self.split_triangle(t, p, &mut stack),
            (Some(e), None) => self.split_edge(t, e, p, &mut stack),
            // On two edges is at their corner, which `same` found: nothing goes in.
            (Some(_), Some(_)) => {}
        }
        while let Some(t) = stack.pop() {
            self.flip_if_inside(t as usize, &mut stack);
        }
        self.legalize = stack;
    }

    fn new_triangle(&mut self) -> usize {
        let t = self.num_tris();
        self.tri.extend_from_slice(&[NONE; 3]);
        self.adj.extend_from_slice(&[NONE; 3]);
        self.constrained.extend_from_slice(&[false; 3]);
        t
    }

    /// Triangle `t` becomes `v`, its edge e (opposite corner e) joined to half-edge `twins[e]`,
    /// and each twin joined back.
    fn put(&mut self, t: usize, v: [u32; 3], twins: [u32; 3]) {
        self.tri[t * 3..t * 3 + 3].copy_from_slice(&v);
        for (e, &twin) in twins.iter().enumerate() {
            self.adj[t * 3 + e] = twin;
            if twin != NONE {
                self.adj[twin as usize] = (t * 3 + e) as u32;
            }
        }
    }

    /// `p` strictly inside `t` = (a, b, c): (a, b, p), (b, c, p), (c, a, p).
    fn split_triangle(&mut self, t: usize, p: u32, stack: &mut Vec<u32>) {
        let [a, b, c] = self.tri_verts(t);
        let [bc, ca, ab] = [0, 1, 2].map(|e| self.adj[t * 3 + e]);
        let (t1, t2) = (self.new_triangle(), self.new_triangle());
        let h = |t: usize, e: usize| (t * 3 + e) as u32;
        self.put(t, [a, b, p], [h(t1, 1), h(t2, 0), ab]);
        self.put(t1, [b, c, p], [h(t2, 1), h(t, 0), bc]);
        self.put(t2, [c, a, p], [h(t, 1), h(t1, 0), ca]);
        stack.extend([t, t1, t2].map(|t| t as u32));
    }

    /// `p` strictly inside edge `e` of `t` (from a to b, opposite c) and of the triangle across it
    /// (opposite d): (b, c, p), (c, a, p), (a, d, p), (d, b, p).
    fn split_edge(&mut self, t: usize, e: usize, p: u32, stack: &mut Vec<u32>) {
        let v = self.tri_verts(t);
        let (c, a, b) = (v[e], v[(e + 1) % 3], v[(e + 2) % 3]);
        let (bc, ca, across) = (self.adj[t * 3 + (e + 1) % 3], self.adj[t * 3 + (e + 2) % 3], self.adj[t * 3 + e]);
        let h = |t: usize, e: usize| (t * 3 + e) as u32;
        let t1 = self.new_triangle();
        if across == NONE {
            self.put(t, [b, c, p], [h(t1, 1), NONE, bc]);
            self.put(t1, [c, a, p], [NONE, h(t, 0), ca]);
            stack.extend([t as u32, t1 as u32]);
            return;
        }
        let (u, f) = ((across / 3) as usize, (across % 3) as usize);
        let d = self.tri_verts(u)[f];
        let (ad, db) = (self.adj[u * 3 + (f + 1) % 3], self.adj[u * 3 + (f + 2) % 3]);
        let u1 = self.new_triangle();
        self.put(t, [b, c, p], [h(t1, 1), h(u1, 0), bc]);
        self.put(t1, [c, a, p], [h(u, 1), h(t, 0), ca]);
        self.put(u, [a, d, p], [h(u1, 1), h(t1, 0), ad]);
        self.put(u1, [d, b, p], [h(t, 1), h(u, 0), db]);
        stack.extend([t, t1, u, u1].map(|t| t as u32));
    }

    /// `t` = (a, b, p): flips a-b to p-d when d, across it, is strictly inside the circle of (a, b, p).
    /// Then d lies in the cap beyond the chord, inside the cone from p through a and b, so the quad is
    /// strictly convex and both new triangles run counterclockwise.
    fn flip_if_inside(&mut self, t: usize, stack: &mut Vec<u32>) {
        let across = self.adj[t * 3 + 2];
        if across == NONE {
            return;
        }
        debug_assert!(!self.constrained[t * 3 + 2], "no constraint while points go in");
        let [a, b, p] = self.tri_verts(t);
        let (u, f) = ((across / 3) as usize, (across % 3) as usize);
        let w = self.tri_verts(u);
        let d = w[f];
        debug_assert!(w[(f + 1) % 3] == b && w[(f + 2) % 3] == a, "twins run opposite ways");
        if !self.predicates.flips(&self.points, a, b, p, d) {
            return;
        }
        let (bp, pa) = (self.adj[t * 3], self.adj[t * 3 + 1]);
        let (ad, db) = (self.adj[u * 3 + (f + 1) % 3], self.adj[u * 3 + (f + 2) % 3]);
        let h = |t: usize, e: usize| (t * 3 + e) as u32;
        self.put(t, [a, d, p], [h(u, 1), pa, ad]);
        self.put(u, [d, b, p], [bp, h(t, 0), db]);
        stack.extend([t as u32, u as u32]);
    }

    fn in_circumcircle_test(&self, a: u32, b: u32, c: u32, p: u32) -> bool {
        let orient = self.predicates.orient(&self.points, a, b, c);
        if orient > 0.0 {
            self.predicates.in_circle(&self.points, a, b, c, p) > 0.0
        } else if orient < 0.0 {
            self.predicates.in_circle(&self.points, a, c, b, p) > 0.0
        } else {
            true // degenerate
        }
    }

    /// The triangle `p` lies in or on, with p's side of each of its edges (edge e opposite corner
    /// e: 0 is on it).
    fn find_containing_triangle(&self, p: u32) -> Option<(usize, [f64; 3])> {
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
            let d0 = self.predicates.orient(&self.points, a, b, p);
            let d1 = self.predicates.orient(&self.points, b, c, p);
            let d2 = self.predicates.orient(&self.points, c, a, p);

            // Check if p is inside this triangle (works for both CCW and CW)
            // Stored triangles run counterclockwise: inside or on its edges, p is on no edge's
            // right.
            if d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0 {
                return Some((t, [d1, d2, d0]));
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

    fn find_containing_triangle_linear(&self, p: u32) -> Option<(usize, [f64; 3])> {
        for t in 0..self.num_tris() {
            if self.is_deleted(t) {
                continue;
            }
            let [a, b, c] = self.tri_verts(t);
            let d0 = self.predicates.orient(&self.points, a, b, p);
            let d1 = self.predicates.orient(&self.points, b, c, p);
            let d2 = self.predicates.orient(&self.points, c, a, p);
            // Stored triangles run counterclockwise: inside or on its edges, p is on no edge's
            // right.
            if d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0 {
                return Some((t, [d1, d2, d0]));
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
        let side = |cdt: &Self, v: u32| cdt.predicates.orient(&cdt.points, a, b, v);
        // The triangle at `a` whose far edge the segment leaves through.
        let Some((t0, i0)) = fan.iter().find_map(|&(t, i)| {
            let v = self.tri_verts(t);
            self.edges_cross(a, b, v[(i + 1) % 3], v[(i + 2) % 3]).then_some((t, i))
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
                    if self.in_circumcircle_test(chain[lo], chain[hi], chain[c], chain[i]) {
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

    fn edges_cross(&self, a: u32, b: u32, c: u32, d: u32) -> bool {
        let d1 = self.predicates.orient(&self.points, a, b, c);
        let d2 = self.predicates.orient(&self.points, a, b, d);
        let d3 = self.predicates.orient(&self.points, c, d, a);
        let d4 = self.predicates.orient(&self.points, c, d, b);
        ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
            && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    }
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
