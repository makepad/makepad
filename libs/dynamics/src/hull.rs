//! Our own implementation, not a translation: a quickhull providing the model builder what the reference
//! compiler takes from its hull library for a vertex-only mesh, built the way that library builds it.
//! Distances are held to that library's bound on the rounding of a distance to a plane: a point is new to
//! the hull only if it is farther out than four times that, a facet is seen from it beyond twice that,
//! and in between the point is coplanar with the hull.
//! As the hull grows, triangles that are coplanar within that tolerance, or not clearly convex, are merged
//! into one facet with one plane, so a face a hair off flat is seen whole: a point is outside all of it or
//! none of it. A new triangle on a facet the new point is coplanar with joins that facet. Of two facets
//! that are not clearly convex, the one that fits its nearest neighbour's plane better is merged into that
//! neighbour, which keeps its plane. A flipped facet, and one with fewer than three neighbours, is merged
//! into its nearest neighbour; a flipped new triangle folded back over its horizon facet joins that one.
//! The output is triangulated with coplanar non-corner points dropped (a point is a corner where three or
//! more facets meet), except where the line between two facets bends: facets a hair off one plane can
//! meet along a bent line, and a facet can run out in a sliver along another's edge. Such a facet's
//! outline is not convex; it is cut into ears, the slivers first.

use std::collections::VecDeque;

/// The convex hull of a point set.
pub struct Hull {
    /// The hull's corner points, as point ids (indices into the input), ascending.
    pub vertices: Vec<i32>,
    /// Its triangles as point ids, counter-clockwise seen from outside.
    pub faces: Vec<[i32; 3]>,
    /// For each entry of `vertices`, the triangles (indices into `faces`) that contain it, ascending.
    pub neighbors: Vec<Vec<usize>>,
}

type Point = [f64; 3];

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: Point, b: Point) -> Point {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(a: Point) -> f64 {
    a[0].hypot(a[1]).hypot(a[2])
}

fn unit(a: Point) -> Point {
    let size = length(a);
    [a[0] / size, a[1] / size, a[2] / size]
}

// A triangle of the growing surface.
struct Face {
    vertices: [usize; 3],
    // Entry i is across the directed edge vertices[i] -> vertices[(i + 1) % 3].
    adjacent: [usize; 3],
    facet: usize,
    removed: bool,
}

// Triangles that are one face of the hull, with one plane: that of the triangle it started from, kept
// through every merge into it.
struct Facet {
    normal: Point,
    origin: Point,
    faces: Vec<usize>,
    outside: Vec<usize>,
    // Twice its area, and the sum of its triangles' centroids weighted by twice their area; for a facet
    // without area, the plain sum of their centroids and their count. These place its centrum.
    area: f64,
    moment: Point,
    centroids: Point,
    count: usize,
    removed: bool,
}

impl Facet {
    fn new(face: usize, vertices: [usize; 3], points: &[Point]) -> Self {
        let [a, b, c] = vertices.map(|i| points[i]);
        let area = length(cross(sub(b, a), sub(c, a)));
        let centroid = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0];
        Self {
            normal: unit(cross(unit(sub(b, a)), unit(sub(c, a)))),
            origin: a,
            faces: vec![face],
            outside: Vec::new(),
            area,
            moment: centroid.map(|v| v * area),
            centroids: centroid,
            count: 1,
            removed: false,
        }
    }

    fn distance(&self, point: Point) -> f64 {
        dot(self.normal, sub(point, self.origin))
    }

    fn centrum(&self) -> Point {
        if self.area > 0.0 {
            self.moment.map(|v| v / self.area)
        } else {
            self.centroids.map(|v| v / self.count as f64)
        }
    }
}

struct Surface<'a> {
    points: &'a [Point],
    eps: f64,
    // The rounding error of a distance to a plane, as the reference library bounds it.
    rounding: f64,
    // A point strictly inside every hull of the build: the centroid of the first simplex.
    interior: Point,
    faces: Vec<Face>,
    facets: Vec<Facet>,
}

impl Surface<'_> {
    fn add_face(&mut self, vertices: [usize; 3], adjacent: [usize; 3]) -> usize {
        let id = self.faces.len();
        self.faces.push(Face { vertices, adjacent, facet: self.facets.len(), removed: false });
        self.facets.push(Facet::new(id, vertices, self.points));
        id
    }

    // Its plane faces inward, or is not a plane (a triangle without area).
    fn flipped(&self, facet: usize) -> bool {
        !(self.facets[facet].distance(self.interior) < -self.rounding)
    }

    // Each centrum is clearly below the other's plane: by more than the rounding of a centrum and of a
    // distance to a plane.
    fn convex(&self, a: usize, b: usize) -> bool {
        let (a, b) = (&self.facets[a], &self.facets[b]);
        a.distance(b.centrum()) < -2.0 * self.rounding && b.distance(a.centrum()) < -2.0 * self.rounding
    }

    // The distinct facets across its sides, ascending.
    fn neighbours(&self, facet: usize) -> Vec<usize> {
        let mut found: Vec<usize> = self.facets[facet].faces.iter()
            .flat_map(|&t| self.faces[t].adjacent.map(|u| self.faces[u].facet))
            .filter(|&g| g != facet)
            .collect();
        found.sort_unstable();
        found.dedup();
        found
    }

    // Whether three distinct facets are across its sides.
    fn three_neighbours(&self, facet: usize) -> bool {
        let mut found = [usize::MAX; 2];
        for &t in &self.facets[facet].faces {
            for u in self.faces[t].adjacent {
                let g = self.faces[u].facet;
                if g != facet && g != found[0] && g != found[1] {
                    if found[0] == usize::MAX {
                        found[0] = g;
                    } else if found[1] == usize::MAX {
                        found[1] = g;
                    } else {
                        return true;
                    }
                }
            }
        }
        false
    }

    // The neighbour whose plane best fits the facet, a flipped one only if all are, and how well: the
    // largest distance of the facet's points from that plane, or of its centrum where it has many points.
    fn nearest(&self, facet: usize) -> Option<(usize, f64)> {
        // A facet is a disk of triangles: one of fifty or more has more than 26 points.
        let faces = &self.facets[facet].faces;
        let mut points = Vec::new();
        if faces.len() < 50 {
            points.extend(faces.iter().flat_map(|&t| self.faces[t].vertices));
            points.sort_unstable();
            points.dedup();
        }
        let many = faces.len() >= 50 || points.len() > 26;
        let centrum = self.facets[facet].centrum();
        let fit = |g: usize| {
            let plane = &self.facets[g];
            let far = if many {
                plane.distance(centrum).abs()
            } else {
                points.iter().map(|&v| plane.distance(self.points[v]).abs()).fold(0.0, f64::max)
            };
            (self.flipped(g), if far.is_nan() { f64::INFINITY } else { far })
        };
        let mut best: Option<(usize, (bool, f64))> = None;
        for g in self.neighbours(facet) {
            let w = fit(g);
            let better = match best {
                None => true,
                Some((_, b)) => !w.0 && b.0 || w.0 == b.0 && w.1 < b.1,
            };
            if better {
                best = Some((g, w));
            }
        }
        best.map(|(g, (_, far))| (g, far))
    }

    // Merge `from` into `into`, which keeps its plane.
    fn merge(&mut self, from: usize, into: usize, orphaned: &mut Vec<usize>) {
        let moved = std::mem::take(&mut self.facets[from].faces);
        for &t in &moved {
            self.faces[t].facet = into;
        }
        let source = &mut self.facets[from];
        source.removed = true;
        orphaned.append(&mut source.outside);
        let (area, moment, centroids, count) = (source.area, source.moment, source.centroids, source.count);
        let target = &mut self.facets[into];
        target.faces.extend(moved);
        target.area += area;
        target.moment = [target.moment[0] + moment[0], target.moment[1] + moment[1], target.moment[2] + moment[2]];
        target.centroids = [target.centroids[0] + centroids[0], target.centroids[1] + centroids[1], target.centroids[2] + centroids[2]];
        target.count += count;
    }

    // Merge, then test every side of the facet merged into again: its centrum moved.
    fn merge_retest(&mut self, from: usize, into: usize, retest: &mut Vec<usize>, orphaned: &mut Vec<usize>) {
        self.merge(from, into, orphaned);
        retest.extend(self.facets[into].faces.iter().rev());
    }

    // Merge the new triangles `cone` of the point `eye` into their neighbours: into a horizon facet the eye
    // is coplanar with, a flipped one into its nearest neighbour, then wherever two facets are not clearly
    // convex and every facet left with fewer than three neighbours, until none is. Returns the facets
    // merged into.
    fn merge_cone(&mut self, cone: &[usize], eye: usize, first_new: usize, orphaned: &mut Vec<usize>) -> Result<Vec<usize>, String> {
        let mut retest: Vec<usize> = Vec::new();
        let mut kept = Vec::new();
        // A new triangle on a horizon facet that the eye is coplanar with becomes part of it, which keeps
        // its plane.
        for &t in cone {
            let horizon = self.faces[self.faces[t].adjacent[0]].facet;
            if self.facets[horizon].distance(self.points[eye]) >= -2.0 * self.rounding {
                self.merge(self.faces[t].facet, horizon, orphaned);
                kept.push(horizon);
            }
        }
        // A flipped new triangle that faces against its horizon facet lies over it, folded back across their
        // edge: it joins that facet. Any other goes into its nearest neighbour.
        for &t in cone {
            let facet = self.faces[t].facet;
            if facet >= first_new && self.flipped(facet) {
                let horizon = self.faces[self.faces[t].adjacent[0]].facet;
                let into = if dot(self.facets[facet].normal, self.facets[horizon].normal) < 0.0 {
                    horizon
                } else {
                    self.nearest(facet).ok_or("convex hull: the surface merged into one facet")?.0
                };
                self.merge(facet, into, orphaned);
                kept.push(into);
            }
        }
        // Every side of the facets merged into so far is tested once, after the new triangles.
        let mut seen = Vec::new();
        for &facet in &kept {
            if !seen.contains(&facet) {
                seen.push(facet);
                retest.extend(self.facets[facet].faces.iter().rev());
            }
        }
        retest.extend(cone.iter().rev());
        loop {
            while let Some(t) = retest.pop() {
                for edge in 0..3 {
                    let a = self.faces[t].facet;
                    let b = self.faces[self.faces[t].adjacent[edge]].facet;
                    if a != b && !self.convex(a, b) {
                        // Of the newer facet and the older, the one that fits its nearest neighbour better
                        // is merged into that neighbour.
                        let (newer, older) = (a.max(b), a.min(b));
                        let (newer_into, newer_fit) = self.nearest(newer).ok_or("convex hull: the surface merged into one facet")?;
                        let (older_into, older_fit) = self.nearest(older).ok_or("convex hull: the surface merged into one facet")?;
                        let (from, into) = if newer_fit < older_fit { (newer, newer_into) } else { (older, older_into) };
                        self.merge_retest(from, into, &mut retest, orphaned);
                        kept.push(into);
                    }
                }
            }
            // Only a merge can leave a facet with fewer than three neighbours: the facet merged into, or one
            // around it. A new triangle has three, and an old facet gains one for each it lost to the cone.
            let mut merged_into = kept.clone();
            merged_into.sort_unstable();
            merged_into.dedup();
            let mut check = Vec::new();
            for facet in merged_into {
                if !self.facets[facet].removed {
                    check.push(facet);
                    check.extend(self.neighbours(facet));
                }
            }
            check.sort_unstable();
            check.dedup();
            for facet in check.into_iter().filter(|&f| !self.facets[f].removed && !self.three_neighbours(f)) {
                let (into, _) = self.nearest(facet).ok_or("convex hull: the surface merged into one facet")?;
                self.merge_retest(facet, into, &mut retest, orphaned);
                kept.push(into);
                break;
            }
            if retest.is_empty() {
                return Ok(kept);
            }
        }
    }
}

fn simplex(points: &[Point], eps: f64) -> Result<[usize; 4], String> {
    let mut low = [0; 3];
    let mut high = [0; 3];
    for (i, point) in points.iter().enumerate() {
        for axis in 0..3 {
            if point[axis] < points[low[axis]][axis] {
                low[axis] = i;
            }
            if point[axis] > points[high[axis]][axis] {
                high[axis] = i;
            }
        }
    }
    let mut axis = 0;
    for candidate in 1..3 {
        if points[high[candidate]][candidate] - points[low[candidate]][candidate]
            > points[high[axis]][axis] - points[low[axis]][axis]
        {
            axis = candidate;
        }
    }
    let (a, b) = (low[axis], high[axis]);
    if length(sub(points[b], points[a])) <= eps {
        return Err("convex hull: colocated points".into());
    }
    let line = unit(sub(points[b], points[a]));
    let mut c = a;
    let mut farthest = eps;
    for (i, point) in points.iter().enumerate() {
        let distance = length(cross(sub(*point, points[a]), line));
        if distance > farthest {
            c = i;
            farthest = distance;
        }
    }
    if c == a {
        return Err("convex hull: collinear points".into());
    }
    let plane = Facet::new(0, [a, b, c], points);
    let mut d = a;
    farthest = eps;
    for (i, point) in points.iter().enumerate() {
        let distance = plane.distance(*point).abs();
        if distance > farthest {
            d = i;
            farthest = distance;
        }
    }
    if d == a {
        return Err("convex hull: coplanar points".into());
    }
    Ok([a, b, c, d])
}

// Assign a point to only its most distant facet, if it is farther out than `outside`; interior points,
// and those coplanar with the hull, need no storage.
fn assign(point: usize, candidates: &[usize], facets: &mut [Facet], points: &[Point], outside: f64) {
    let mut distance = outside;
    let mut owner = None;
    for &id in candidates {
        let candidate = facets[id].distance(points[point]);
        if candidate > distance {
            distance = candidate;
            owner = Some(id);
        }
    }
    if let Some(id) = owner {
        facets[id].outside.push(point);
    }
}

// A convex patch has one directed boundary cycle. Sorting avoids hash order
// dependence and lets the same traversal serve both horizons and facets.
fn boundary_order(edges: &[(usize, usize)]) -> Result<Vec<usize>, String> {
    let mut sorted: Vec<_> = edges.iter().enumerate().map(|(i, e)| (e.0, i)).collect();
    sorted.sort_unstable();
    if sorted.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err("convex hull: boundary branches at a point".into());
    }
    let mut order = Vec::with_capacity(edges.len());
    let mut current = sorted.first().ok_or("convex hull: empty boundary")?.1;
    let start = current;
    loop {
        order.push(current);
        let next = edges[current].1;
        let entry = sorted
            .binary_search_by_key(&next, |e| e.0)
            .map_err(|_| "convex hull: open boundary")?;
        current = sorted[entry].1;
        if current == start {
            break;
        }
        if order.len() == edges.len() {
            return Err("convex hull: boundary does not close".into());
        }
    }
    if order.len() != edges.len() {
        return Err("convex hull: disconnected boundary".into());
    }
    Ok(order)
}

// The farthest point of `chain` between its ends from the line through them is kept if farther than `tol`,
// and each side of it in turn: the points left out lie on the kept polyline within `tol`.
fn keep_bends(chain: &[usize], points: &[Point], tol: f64, keep: &mut [bool]) {
    if chain.len() < 3 {
        return;
    }
    let (a, b) = (points[chain[0]], points[chain[chain.len() - 1]]);
    let line = sub(b, a);
    let distance = |p: Point| if length(line) > tol { length(cross(sub(p, a), unit(line))) } else { length(sub(p, a)) };
    let mut far = 0;
    let mut farthest = tol;
    for (k, &v) in chain.iter().enumerate().take(chain.len() - 1).skip(1) {
        let d = distance(points[v]);
        if d > farthest {
            far = k;
            farthest = d;
        }
    }
    if far > 0 {
        keep[chain[far]] = true;
        keep_bends(&chain[..=far], points, tol, keep);
        keep_bends(&chain[far..], points, tol, keep);
    }
}

// Triangles covering the facet's polygon (its kept boundary points, counter-clockwise about its normal).
fn triangulate_polygon(polygon: &[usize], facet: &Facet, points: &[Point], eps: f64, triangles: &mut Vec<[i32; 3]>) {
    let count = polygon.len();
    // A facet's points lie off its plane by up to `off`: a corner turning by less than twice that is straight.
    let off = polygon.iter().map(|&v| facet.distance(points[v]).abs()).fold(0.0, f64::max);
    let tol = eps.max(2.0 * off);
    let mut previous: Vec<_> = (0..count).map(|i| (i + count - 1) % count).collect();
    let mut next: Vec<_> = (0..count).map(|i| (i + 1) % count).collect();
    let mut remaining = count;
    let mut first = (0..count).min_by_key(|&i| polygon[i]).unwrap();
    // How far a corner lies outside the chord between its neighbours: negative where the polygon turns back.
    let turn = |i: usize, previous: &[usize], next: &[usize]| {
        let chord = sub(points[polygon[next[i]]], points[polygon[previous[i]]]);
        let offset = sub(points[polygon[i]], points[polygon[previous[i]]]);
        dot(cross(offset, unit(chord)), facet.normal)
    };
    // A corner on the line through its neighbours but not between them: the boundary runs out along a line
    // and back, round a sliver of this facet along another facet's edge.
    let tip = |i: usize, previous: &[usize], next: &[usize]| {
        let chord = sub(points[polygon[next[i]]], points[polygon[previous[i]]]);
        let along = dot(sub(points[polygon[i]], points[polygon[previous[i]]]), unit(chord));
        length(chord) <= tol || (turn(i, previous, next).abs() <= tol && !(along > 0.0 && along < length(chord)))
    };
    let mut clip = |i: usize, previous: &mut [usize], next: &mut [usize], first: &mut usize, remaining: &mut usize| {
        let (a, b) = (previous[i], next[i]);
        triangles.push([polygon[a] as i32, polygon[i] as i32, polygon[b] as i32]);
        next[a] = b;
        previous[b] = a;
        *remaining -= 1;
        if i == *first {
            *first = b;
        }
    };
    let corners = |first: usize, next: &[usize]| {
        let mut order = vec![first];
        while next[order[order.len() - 1]] != first {
            order.push(next[order[order.len() - 1]]);
        }
        order
    };
    // Each sliver is cut off first, as a triangle without area.
    while remaining > 3 {
        let Some(i) = corners(first, &next).into_iter().find(|&i| tip(i, &previous, &next)) else { break };
        clip(i, &mut previous, &mut next, &mut first, &mut remaining);
    }
    if corners(first, &next).into_iter().all(|i| turn(i, &previous, &next) >= -tol) {
        // A convex polygon. A corner of another facet can lie on a straight side of this one. Cut off the
        // turning corners beside such points first, so no triangle has three points on one line.
        let straight = |i: usize, previous: &[usize], next: &[usize]| turn(i, previous, next).abs() <= tol;
        while remaining > 3 {
            let mut ear = None;
            let mut i = first;
            for _ in 0..remaining {
                if !straight(i, &previous, &next) && (straight(previous[i], &previous, &next) || straight(next[i], &previous, &next)) {
                    ear = Some(i);
                    break;
                }
                i = next[i];
            }
            let Some(i) = ear else { break };
            clip(i, &mut previous, &mut next, &mut first, &mut remaining);
        }
        let mut second = next[first];
        while next[second] != first {
            let third = next[second];
            triangles.push([polygon[first] as i32, polygon[second] as i32, polygon[third] as i32]);
            second = third;
        }
        return;
    }
    // The polygon turns back somewhere (two facets that are nearly one plane meet along a bent line): cut
    // ears, corners turning outward whose triangle holds no other point, those beside a straight point first.
    let inside = |a: usize, b: usize, c: usize, p: usize| {
        let side = |from: usize, to: usize| {
            let edge = sub(points[polygon[to]], points[polygon[from]]);
            dot(cross(unit(edge), sub(points[polygon[p]], points[polygon[from]])), facet.normal) >= -tol
        };
        side(a, b) && side(b, c) && side(c, a)
    };
    while remaining > 3 {
        let mut ear = None;
        let mut any = None;
        let mut sharpest = first;
        let mut i = first;
        for _ in 0..remaining {
            let bend = turn(i, &previous, &next);
            if bend > turn(sharpest, &previous, &next) {
                sharpest = i;
            }
            let (a, b) = (previous[i], next[i]);
            let mut j = next[b];
            let mut clear = bend > tol;
            while clear && j != a {
                clear = turn(j, &previous, &next) > tol || !inside(a, i, b, j);
                j = next[j];
            }
            if clear {
                if turn(a, &previous, &next).abs() <= tol || turn(b, &previous, &next).abs() <= tol {
                    ear = Some(i);
                    break;
                }
                any = any.or(Some(i));
            }
            i = next[i];
        }
        let i = ear.or(any).unwrap_or(sharpest);
        clip(i, &mut previous, &mut next, &mut first, &mut remaining);
    }
    triangles.push([polygon[previous[first]] as i32, polygon[first] as i32, polygon[next[first]] as i32]);
}

fn triangulate(surface: &Surface) -> Result<Hull, String> {
    let (faces, facets, points, eps) = (&surface.faces, &surface.facets, surface.points, surface.eps);
    // Each facet's boundary edges, with the facet across each.
    let mut boundaries = vec![Vec::new(); facets.len()];
    let mut facets_at = vec![Vec::new(); points.len()];
    for face in faces.iter().filter(|face| !face.removed) {
        for edge in 0..3 {
            let across = faces[face.adjacent[edge]].facet;
            if across != face.facet {
                boundaries[face.facet].push((face.vertices[edge], face.vertices[(edge + 1) % 3], across));
            }
            facets_at[face.vertices[edge]].push(face.facet);
        }
    }
    // A point where three or more facets meet is a corner. A point where only two meet lies on the line
    // between them and is dropped from both, unless their shared boundary bends there by more than the
    // tolerance; that is decided once for each stretch between two corners, so both facets agree.
    let corner: Vec<bool> = facets_at.iter_mut().map(|facets| {
        facets.sort_unstable();
        facets.dedup();
        facets.len() >= 3
    }).collect();
    let mut keep = corner.clone();
    let mut cycles = vec![Vec::new(); facets.len()];
    for (facet, edges) in boundaries.iter().enumerate().filter(|(_, edges)| !edges.is_empty()) {
        let pairs: Vec<_> = edges.iter().map(|&(a, b, _)| (a, b)).collect();
        let cycle: Vec<_> = boundary_order(&pairs)?.into_iter().map(|i| (edges[i].0, edges[i].2)).collect();
        if let Some(start) = cycle.iter().position(|&(v, _)| corner[v]) {
            let mut k = start;
            loop {
                let mut chain = vec![cycle[k].0];
                let mut j = (k + 1) % cycle.len();
                while !corner[cycle[j].0] {
                    chain.push(cycle[j].0);
                    j = (j + 1) % cycle.len();
                }
                chain.push(cycle[j].0);
                if cycle[k].1 > facet {
                    keep_bends(&chain, points, eps, &mut keep);
                }
                k = j;
                if k == start {
                    break;
                }
            }
        }
        cycles[facet] = cycle;
    }
    let mut triangles = Vec::new();
    for (facet, cycle) in cycles.iter().enumerate().filter(|(_, cycle)| !cycle.is_empty()) {
        let polygon: Vec<_> = cycle.iter().map(|&(v, _)| v).filter(|&v| keep[v]).collect();
        if polygon.len() < 3 {
            return Err("convex hull: a facet has fewer than three corners".into());
        }
        triangulate_polygon(&polygon, &facets[facet], points, eps, &mut triangles);
    }
    // Two facets can each run out in a sliver along the same line: the two triangles cut off there are
    // one triangle, once each way round, and both are dropped (the reference library drops such mirrors).
    let mut keyed: Vec<([i32; 3], usize)> = triangles.iter().enumerate().map(|(i, t)| {
        let mut key = *t;
        key.sort_unstable();
        (key, i)
    }).collect();
    keyed.sort_unstable();
    let mut mirrored = vec![false; triangles.len()];
    for pair in keyed.windows(2) {
        let ((key, i), (other, j)) = (pair[0], pair[1]);
        let turned = |t: [i32; 3]| {
            let first = (0..3).min_by_key(|&k| t[k]).unwrap();
            [t[first], t[(first + 1) % 3], t[(first + 2) % 3]]
        };
        if key == other && turned(triangles[i]) != turned(triangles[j]) && !mirrored[i] && !mirrored[j] {
            mirrored[i] = true;
            mirrored[j] = true;
        }
    }
    let mut triangles: Vec<_> = triangles.into_iter().zip(mirrored).filter(|&(_, m)| !m).map(|(t, _)| t).collect();
    triangles.sort_unstable();
    let mut vertices: Vec<_> = triangles.iter().flatten().copied().collect();
    vertices.sort_unstable();
    vertices.dedup();
    let mut neighbors = vec![Vec::new(); vertices.len()];
    for (id, face) in triangles.iter().enumerate() {
        for vertex in face {
            neighbors[vertices.binary_search(vertex).unwrap()].push(id);
        }
    }
    Ok(Hull { vertices, faces: triangles, neighbors })
}

/// The hull of the `n` points `vert[3 * i..3 * i + 3]`.
pub fn convex_hull(vert: &[f64], n: usize) -> Result<Hull, String> {
    if n < 4 {
        return Err("convex hull: fewer than 4 points".into());
    }
    if n - 1 > i32::MAX as usize || n > vert.len() / 3 {
        return Err("convex hull: invalid point count".into());
    }
    let vert = &vert[..3 * n];
    let mut maxima = [0.0_f64; 3];
    for point in vert.chunks_exact(3) {
        for axis in 0..3 {
            if !point[axis].is_finite() {
                return Err("convex hull: non-finite coordinate".into());
            }
            maxima[axis] = maxima[axis].max(point[axis].abs());
        }
    }
    // Scale all distances together to keep cross products and the tolerance
    // finite even for very large or very small coordinate units.
    let scale = maxima[0].max(maxima[1]).max(maxima[2]);
    if scale == 0.0 {
        return Err("convex hull: colocated points".into());
    }
    let sum = maxima[0] / scale + maxima[1] / scale + maxima[2] / scale;
    let eps = 3.0 * f64::EPSILON * sum;
    // As the reference library: a point is new to the hull only if it is farther out than twice what makes
    // a facet visible from it, which is twice the rounding of a distance; between the two it is coplanar.
    let rounding = f64::EPSILON * (3.0 * 3.0_f64.sqrt().min(sum) * 1.01 + 1.0);
    let (visible_from, outside) = (2.0 * rounding, 4.0 * rounding);
    let points: Vec<Point> = vert.chunks_exact(3).map(|p| [p[0] / scale, p[1] / scale, p[2] / scale]).collect();
    let initial = simplex(&points, eps)?;
    let corners = initial.map(|i| points[i]);
    let interior = [0, 1, 2].map(|axis| (corners[0][axis] + corners[1][axis] + corners[2][axis] + corners[3][axis]) / 4.0);
    let mut surface = Surface { points: &points, eps, rounding, interior, faces: Vec::new(), facets: Vec::new() };
    for opposite in 0..4 {
        let mut vertices = [initial[(opposite + 1) % 4], initial[(opposite + 2) % 4], initial[(opposite + 3) % 4]];
        if Facet::new(0, vertices, &points).distance(points[initial[opposite]]) > 0.0 {
            vertices.swap(1, 2);
        }
        surface.add_face(vertices, [usize::MAX; 3]);
    }
    for id in 0..4 {
        for edge in 0..3 {
            let a = surface.faces[id].vertices[edge];
            let b = surface.faces[id].vertices[(edge + 1) % 3];
            surface.faces[id].adjacent[edge] = (0..4).find(|&other| other != id && (0..3).any(|j| surface.faces[other].vertices[j] == b && surface.faces[other].vertices[(j + 1) % 3] == a)).unwrap();
        }
    }
    for point in 0..n {
        if !initial.contains(&point) {
            assign(point, &[0, 1, 2, 3], &mut surface.facets, &points, outside);
        }
    }
    let mut pending: VecDeque<_> = (0..4).filter(|&i| !surface.facets[i].outside.is_empty()).collect();
    let mut visible = vec![false; 4];
    let mut tested = vec![0; 4];
    let mut round = 0;
    let mut patch = Vec::new();
    let mut horizon = Vec::new();
    let mut horizon_neighbors = Vec::new();
    let mut orphaned = Vec::new();
    let mut cone = Vec::new();
    let mut candidates = Vec::new();
    while let Some(id) = pending.pop_front() {
        let facet = &surface.facets[id];
        if facet.removed || facet.outside.is_empty() {
            continue;
        }
        let mut eye = facet.outside[0];
        let mut farthest = facet.distance(points[eye]);
        for &point in &facet.outside[1..] {
            let distance = facet.distance(points[point]);
            if distance > farthest || (distance == farthest && point < eye) {
                eye = point;
                farthest = distance;
            }
        }
        round += 1;
        patch.clear();
        patch.push(id);
        visible[id] = true;
        let mut cursor = 0;
        while cursor < patch.len() {
            for &face in &surface.facets[patch[cursor]].faces {
                for &other in &surface.faces[face].adjacent {
                    let facet = surface.faces[other].facet;
                    if !visible[facet] && tested[facet] != round {
                        tested[facet] = round;
                        if surface.facets[facet].distance(points[eye]) > visible_from {
                            visible[facet] = true;
                            patch.push(facet);
                        }
                    }
                }
            }
            cursor += 1;
        }
        horizon.clear();
        horizon_neighbors.clear();
        for &old in &patch {
            for &face in &surface.facets[old].faces {
                for edge in 0..3 {
                    let other = surface.faces[face].adjacent[edge];
                    if !visible[surface.faces[other].facet] {
                        horizon.push((surface.faces[face].vertices[edge], surface.faces[face].vertices[(edge + 1) % 3]));
                        let back = surface.faces[other].adjacent.iter().position(|&i| i == face).unwrap();
                        horizon_neighbors.push((other, back));
                    }
                }
            }
        }
        orphaned.clear();
        for &old in &patch {
            for &face in &surface.facets[old].faces {
                surface.faces[face].removed = true;
            }
            let facet = &mut surface.facets[old];
            facet.faces.clear();
            facet.removed = true;
            orphaned.append(&mut facet.outside);
        }
        let order = boundary_order(&horizon)?;
        let base = surface.faces.len();
        let first_new = surface.facets.len();
        cone.clear();
        for (i, &edge) in order.iter().enumerate() {
            let (a, b) = horizon[edge];
            let (other, back) = horizon_neighbors[edge];
            let face = surface.add_face([a, b, eye], [other, base + (i + 1) % order.len(), base + (i + order.len() - 1) % order.len()]);
            surface.faces[other].adjacent[back] = face;
            cone.push(face);
        }
        visible.resize(surface.facets.len(), false);
        tested.resize(surface.facets.len(), 0);
        for &old in &patch {
            visible[old] = false;
        }
        let kept = surface.merge_cone(&cone, eye, first_new, &mut orphaned)?;
        // The facets the cone ended in, in its order, then any other merged into: the points that were
        // outside the visible and merged facets can only be outside these.
        round += 1;
        candidates.clear();
        for facet in cone.iter().map(|&t| surface.faces[t].facet).chain(kept) {
            if !surface.facets[facet].removed && tested[facet] != round {
                tested[facet] = round;
                candidates.push(facet);
            }
        }
        for &point in &orphaned {
            if point != eye {
                assign(point, &candidates, &mut surface.facets, &points, outside);
            }
        }
        pending.extend(candidates.iter().copied().filter(|&i| !surface.facets[i].outside.is_empty()));
    }
    triangulate(&surface)
}
