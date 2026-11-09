//! Our own implementation, not a translation: a quickhull providing the model
//! builder what the reference compiler takes from its hull library for a
//! vertex-only mesh: triangulated output with coplanar non-corner points dropped
//! (a point is a corner where three or more planar groups of triangles meet).

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

struct Face {
    vertices: [usize; 3],
    // Entry i is across the directed edge vertices[i] -> vertices[(i + 1) % 3].
    adjacent: [usize; 3],
    normal: Point,
    outside: Vec<usize>,
}

impl Face {
    fn new(vertices: [usize; 3], points: &[Point]) -> Self {
        let [a, b, c] = vertices.map(|i| points[i]);
        Self {
            vertices,
            adjacent: [usize::MAX; 3],
            normal: unit(cross(unit(sub(b, a)), unit(sub(c, a)))),
            outside: Vec::new(),
        }
    }

    fn distance(&self, point: Point, points: &[Point]) -> f64 {
        dot(self.normal, sub(point, points[self.vertices[0]]))
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
    let plane = Face::new([a, b, c], points);
    let mut d = a;
    farthest = eps;
    for (i, point) in points.iter().enumerate() {
        let distance = plane.distance(*point, points).abs();
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

// Assign a point to only its most distant face; interior points need no storage.
fn assign(point: usize, candidates: &[usize], faces: &mut [Face], points: &[Point], eps: f64) {
    let mut distance = eps;
    let mut owner = None;
    for &id in candidates {
        let candidate = faces[id].distance(points[point], points);
        if candidate > distance {
            distance = candidate;
            owner = Some(id);
        }
    }
    if let Some(id) = owner {
        faces[id].outside.push(point);
    }
}

fn root(parents: &mut [usize], mut id: usize) -> usize {
    while parents[id] != id {
        parents[id] = parents[parents[id]];
        id = parents[id];
    }
    id
}

// A convex patch has one directed boundary cycle. Sorting avoids hash order
// dependence and lets the same traversal serve both horizons and planar patches.
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

fn triangulate(faces: &[Face], removed: &[bool], points: &[Point], eps: f64) -> Result<Hull, String> {
    let mut parents: Vec<_> = (0..faces.len()).collect();
    for (id, face) in faces.iter().enumerate().filter(|(i, _)| !removed[*i]) {
        for &other in &face.adjacent {
            if other <= id {
                continue;
            }
            let neighbor = &faces[other];
            if face.vertices.iter().all(|&v| neighbor.distance(points[v], points).abs() <= eps)
                && neighbor.vertices.iter().all(|&v| face.distance(points[v], points).abs() <= eps)
            {
                let a = root(&mut parents, id);
                let b = root(&mut parents, other);
                parents[a.max(b)] = a.min(b);
            }
        }
    }
    let mut boundaries = vec![Vec::new(); faces.len()];
    // The planar groups meeting at each point: a point where fewer than three meet lies on the edge
    // between two of them and is dropped from both, so neighbouring groups agree on their corners.
    let mut groups_at = vec![Vec::new(); points.len()];
    for (id, face) in faces.iter().enumerate().filter(|(i, _)| !removed[*i]) {
        let group = root(&mut parents, id);
        for edge in 0..3 {
            if root(&mut parents, face.adjacent[edge]) != group {
                boundaries[group].push((face.vertices[edge], face.vertices[(edge + 1) % 3]));
            }
            groups_at[face.vertices[edge]].push(group);
        }
    }
    let corner: Vec<bool> = groups_at.iter_mut().map(|groups| {
        groups.sort_unstable();
        groups.dedup();
        groups.len() >= 3
    }).collect();
    let mut triangles = Vec::new();
    for edges in boundaries.iter().filter(|edges| !edges.is_empty()) {
        let order = boundary_order(edges)?;
        let polygon: Vec<_> = order.iter().map(|&i| edges[i].0).filter(|&v| corner[v]).collect();
        let count = polygon.len();
        if count < 3 {
            return Err("convex hull: a planar group has fewer than three corners".into());
        }
        let mut previous: Vec<_> = (0..count).map(|i| (i + count - 1) % count).collect();
        let mut next: Vec<_> = (0..count).map(|i| (i + 1) % count).collect();
        let mut remaining = count;
        // A corner of another group can lie on a straight side of this one. Cut off the turning
        // corners beside such points first, so no triangle has three points on one line.
        let straight = |i: usize, previous: &[usize], next: &[usize]| {
            let chord = sub(points[polygon[next[i]]], points[polygon[previous[i]]]);
            let offset = sub(points[polygon[i]], points[polygon[previous[i]]]);
            length(cross(offset, unit(chord))) <= eps
        };
        let mut first = (0..count).min_by_key(|&i| polygon[i]).unwrap();
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
            let (a, b) = (previous[i], next[i]);
            triangles.push([polygon[a] as i32, polygon[i] as i32, polygon[b] as i32]);
            next[a] = b;
            previous[b] = a;
            remaining -= 1;
            if i == first {
                first = b;
            }
        }
        let mut second = next[first];
        while next[second] != first {
            let third = next[second];
            triangles.push([polygon[first] as i32, polygon[second] as i32, polygon[third] as i32]);
            second = third;
        }
    }
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
    let eps = 3.0 * f64::EPSILON * (maxima[0] / scale + maxima[1] / scale + maxima[2] / scale);
    let points: Vec<Point> = vert.chunks_exact(3).map(|p| [p[0] / scale, p[1] / scale, p[2] / scale]).collect();
    let initial = simplex(&points, eps)?;
    let mut faces = Vec::new();
    for opposite in 0..4 {
        let mut vertices = [initial[(opposite + 1) % 4], initial[(opposite + 2) % 4], initial[(opposite + 3) % 4]];
        if Face::new(vertices, &points).distance(points[initial[opposite]], &points) > 0.0 {
            vertices.swap(1, 2);
        }
        faces.push(Face::new(vertices, &points));
    }
    for id in 0..4 {
        for edge in 0..3 {
            let a = faces[id].vertices[edge];
            let b = faces[id].vertices[(edge + 1) % 3];
            faces[id].adjacent[edge] = (0..4).find(|&other| other != id && (0..3).any(|j| faces[other].vertices[j] == b && faces[other].vertices[(j + 1) % 3] == a)).unwrap();
        }
    }
    for point in 0..n {
        if !initial.contains(&point) {
            assign(point, &[0, 1, 2, 3], &mut faces, &points, eps);
        }
    }
    let mut pending: VecDeque<_> = (0..4).filter(|&i| !faces[i].outside.is_empty()).collect();
    let mut removed = vec![false; 4];
    let mut visible = vec![false; 4];
    let mut patch = Vec::new();
    let mut horizon = Vec::new();
    let mut horizon_neighbors = Vec::new();
    let mut orphaned = Vec::new();
    let mut cone = Vec::new();
    while let Some(id) = pending.pop_front() {
        if removed[id] {
            continue;
        }
        let face = &faces[id];
        let mut eye = face.outside[0];
        let mut farthest = face.distance(points[eye], &points);
        for &point in &face.outside[1..] {
            let distance = face.distance(points[point], &points);
            if distance > farthest || (distance == farthest && point < eye) {
                eye = point;
                farthest = distance;
            }
        }
        patch.clear();
        patch.push(id);
        visible[id] = true;
        let mut cursor = 0;
        while cursor < patch.len() {
            for &other in &faces[patch[cursor]].adjacent {
                if !visible[other] && faces[other].distance(points[eye], &points) > eps {
                    visible[other] = true;
                    patch.push(other);
                }
            }
            cursor += 1;
        }
        horizon.clear();
        horizon_neighbors.clear();
        orphaned.clear();
        for &old in &patch {
            for edge in 0..3 {
                let other = faces[old].adjacent[edge];
                if !visible[other] {
                    horizon.push((faces[old].vertices[edge], faces[old].vertices[(edge + 1) % 3]));
                    let back = faces[other].adjacent.iter().position(|&i| i == old).unwrap();
                    horizon_neighbors.push((other, back));
                }
            }
            orphaned.append(&mut faces[old].outside);
            removed[old] = true;
        }
        let order = boundary_order(&horizon)?;
        let base = faces.len();
        cone.clear();
        for (i, &edge) in order.iter().enumerate() {
            let (a, b) = horizon[edge];
            let (other, back) = horizon_neighbors[edge];
            let mut face = Face::new([a, b, eye], &points);
            face.adjacent = [other, base + (i + 1) % order.len(), base + (i + order.len() - 1) % order.len()];
            faces[other].adjacent[back] = base + i;
            faces.push(face);
            cone.push(base + i);
        }
        removed.resize(faces.len(), false);
        visible.resize(faces.len(), false);
        for &old in &patch {
            visible[old] = false;
        }
        for &point in &orphaned {
            if point != eye {
                assign(point, &cone, &mut faces, &points, eps);
            }
        }
        pending.extend(cone.iter().copied().filter(|&i| !faces[i].outside.is_empty()));
    }
    triangulate(&faces, &removed, &points, eps)
}
