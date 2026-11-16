use makepad_csg_math::{dvec3, robust::orient3d, Vec3d};
use makepad_csg_mesh::mesh::TriMesh;
use std::collections::{HashMap, HashSet};

/// The closed convex hull of points, with outward winding and flat faces.
/// Duplicate and interior points are dropped; coplanar points need not be kept.
/// The seed tetrahedron costs O(n); insertion costs about O(n log h) expected with conflict lists,
/// where h is the largest number of hull triangles during construction.
pub fn hull(points: &[Vec3d]) -> Result<TriMesh, String> {
    let invalid = || "a hull needs four points that are not in one plane".to_string();
    if points.len() < 4 {
        return Err(invalid());
    }
    let mut seen = HashSet::new();
    let mut unique = Vec::new();
    for &p in points {
        if !p.is_finite() {
            return Err("a hull needs finite points".into());
        }
        // Positive and negative zero describe the same point. Preserve input order.
        let key = [p.x, p.y, p.z].map(|v| if v == 0.0 { 0 } else { v.to_bits() });
        if seen.insert(key) {
            unique.push(p);
        }
    }
    let points = unique;
    if points.len() < 4 {
        return Err(invalid());
    }

    // The least and greatest point in each axis' lexicographic order (that axis, then the next
    // two): six corners of the hull, never a point inside an edge or face. The farthest pair of
    // them seeds the hull, a long chord found in O(n) where the farthest pair of all costs O(n²).
    let mut extremes = [0; 6];
    for axis in 0..3 {
        let key = |p: Vec3d| [p.component(axis), p.component((axis + 1) % 3), p.component((axis + 2) % 3)];
        for (i, &p) in points.iter().enumerate() {
            if key(p) < key(points[extremes[2 * axis]]) {
                extremes[2 * axis] = i;
            }
            if key(p) > key(points[extremes[2 * axis + 1]]) {
                extremes[2 * axis + 1] = i;
            }
        }
    }
    let (mut a, mut b, mut longest) = (0, 0, 0.0);
    for (k, &i) in extremes.iter().enumerate() {
        for &j in &extremes[k + 1..] {
            let d = points[j] - points[i];
            let length = d.x.hypot(d.y).hypot(d.z);
            if length > longest {
                (a, b, longest) = (i, j, length);
            }
        }
    }

    let (mut c, mut widest) = (a, 0.0);
    for (i, &p) in points.iter().enumerate() {
        // The three projected areas give distance from the line, up to its length.
        // Exact predicates keep an almost-collinear point distinct from the line.
        let area = |u, v| {
            orient3d(
                dvec3(points[a].component(u), points[a].component(v), 0.0),
                dvec3(points[b].component(u), points[b].component(v), 0.0),
                dvec3(p.component(u), p.component(v), 0.0),
                Vec3d::Z,
            )
        };
        let distance = area(0, 1).hypot(area(1, 2)).hypot(area(2, 0));
        if distance > widest {
            (c, widest) = (i, distance);
        }
    }
    if widest == 0.0 {
        return Err(invalid());
    }

    let (mut d, mut deepest) = (a, 0.0);
    for (i, &p) in points.iter().enumerate() {
        let distance = orient3d(points[a], points[b], points[c], p).abs();
        if distance > deepest {
            (d, deepest) = (i, distance);
        }
    }
    if deepest == 0.0 {
        return Err(invalid());
    }

    struct Face {
        vertices: [usize; 3],
        alive: bool,
        neighbours: [usize; 3],
        conflicts: Vec<usize>,
    }

    let mut faces = Vec::with_capacity(4);
    for (mut face, opposite) in [([a, b, c], d), ([a, d, b], c), ([b, d, c], a), ([c, d, a], b)] {
        // orient3d is positive on the CCW normal side; the opposite vertex is inside.
        if orient3d(points[face[0]], points[face[1]], points[face[2]], points[opposite]) > 0.0 {
            face.swap(1, 2);
        }
        faces.push(Face {
            vertices: face,
            alive: true,
            neighbours: [usize::MAX; 3],
            conflicts: Vec::new(),
        });
    }

    let mut edges: HashMap<(usize, usize), (usize, usize)> = HashMap::new();
    for f in 0..faces.len() {
        let [u, v, w] = faces[f].vertices;
        for (edge, [from, to]) in [[u, v], [v, w], [w, u]].into_iter().enumerate() {
            let key = (from.min(to), from.max(to));
            if let Some((neighbour, neighbour_edge)) = edges.remove(&key) {
                faces[f].neighbours[edge] = neighbour;
                faces[neighbour].neighbours[neighbour_edge] = f;
            } else {
                edges.insert(key, (f, edge));
            }
        }
    }

    let mut owner = vec![None; points.len()];
    for (i, &p) in points.iter().enumerate() {
        if [a, b, c, d].contains(&i) {
            continue;
        }
        for (f, face) in faces.iter_mut().enumerate() {
            let [u, v, w] = face.vertices;
            if orient3d(points[u], points[v], points[w], p) > 0.0 {
                face.conflicts.push(i);
                owner[i] = Some(f);
                break;
            }
        }
    }

    // Stamp visibility with the point index so each flood only touches nearby faces.
    let mut visibility = vec![(usize::MAX, false); faces.len()];
    let mut visible = Vec::new();
    let mut horizon = Vec::new();
    let mut conflicts = Vec::new();
    let mut outgoing = HashMap::new();
    for i in 0..points.len() {
        let Some(f) = owner[i] else {
            continue;
        };
        if !faces[f].alive {
            continue;
        }
        let p = points[i];
        owner[i] = None;
        visible.clear();
        visible.push(f);
        visibility[f] = (i, true);
        horizon.clear();
        let mut cursor = 0;
        while cursor < visible.len() {
            let f = visible[cursor];
            cursor += 1;
            let [u, v, w] = faces[f].vertices;
            for (edge, [from, to]) in [[u, v], [v, w], [w, u]].into_iter().enumerate() {
                let neighbour = faces[f].neighbours[edge];
                if visibility[neighbour].0 != i {
                    let [u, v, w] = faces[neighbour].vertices;
                    let sees = orient3d(points[u], points[v], points[w], p) > 0.0;
                    visibility[neighbour] = (i, sees);
                    if sees {
                        visible.push(neighbour);
                    }
                }
                if !visibility[neighbour].1 {
                    let neighbour_edge = faces[neighbour]
                        .neighbours
                        .iter()
                        .position(|&other| other == f)
                        .unwrap();
                    horizon.push((from, to, neighbour, neighbour_edge));
                }
            }
        }

        conflicts.clear();
        for &f in &visible {
            faces[f].alive = false;
            conflicts.extend(std::mem::take(&mut faces[f].conflicts).into_iter().filter(|&j| j != i));
        }

        let first_new = faces.len();
        outgoing.clear();
        // Visit edges in flood face order, never hash-table order. Each edge keeps the
        // removed face's winding, opposite its surviving neighbour. A point that
        // sees a face cannot be collinear with any of that face's edges.
        for &(from, to, neighbour, neighbour_edge) in &horizon {
            let f = faces.len();
            faces.push(Face {
                vertices: [from, to, i],
                alive: true,
                neighbours: [neighbour, usize::MAX, usize::MAX],
                conflicts: Vec::new(),
            });
            visibility.push((usize::MAX, false));
            faces[neighbour].neighbours[neighbour_edge] = f;
            outgoing.insert(from, f);
        }
        for f in first_new..faces.len() {
            let next = outgoing[&faces[f].vertices[1]];
            faces[f].neighbours[1] = next;
            faces[next].neighbours[2] = f;
        }

        // Merge the removed faces' lists in point order, then file under the first new face.
        conflicts.sort_unstable();
        for &j in &conflicts {
            owner[j] = None;
            for f in first_new..faces.len() {
                let [u, v, w] = faces[f].vertices;
                if orient3d(points[u], points[v], points[w], points[j]) > 0.0 {
                    faces[f].conflicts.push(j);
                    owner[j] = Some(f);
                    break;
                }
            }
        }
    }

    let faces: Vec<_> = faces.into_iter().filter(|face| face.alive).map(|face| face.vertices).collect();
    let mut used = vec![false; points.len()];
    for face in &faces {
        for &i in face {
            used[i] = true;
        }
    }
    let mut mesh = TriMesh::with_capacity(used.iter().filter(|&&v| v).count(), faces.len());
    let mut indices = vec![0; points.len()];
    for (i, &p) in points.iter().enumerate() {
        if used[i] {
            indices[i] = mesh.add_vertex(p);
        }
    }
    mesh.triangles = faces.into_iter().map(|f| f.map(|i| indices[i])).collect();
    Ok(mesh)
}
