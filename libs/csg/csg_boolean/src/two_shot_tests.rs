use crate::boolean::{mesh_boolean_report, mesh_boolean_with, BoolOp, BooleanReport, FinishParams};
use crate::thread_pool::with_serial;
use crate::tri_tri::plane_tolerance;
use makepad_csg_math::{dvec3, Vec3d};
use makepad_csg_mesh::{validate::{is_flat, validate_mesh}, volume::mesh_volume, TriMesh};

const TURNS: [[f64; 3]; 4] = [
    [0.0, 0.0, 0.0],
    [10.0, 0.0, 0.0],
    [0.0, 0.0, 33.0],
    [23.7, -61.3, 141.9],
];

const CORNER_WALLS: [[usize; 2]; 4] = [[2, 3], [0, 3], [0, 1], [2, 1]];

fn rotation_xyz(deg: [f64; 3]) -> [[f64; 3]; 3] {
    let (sx, cx) = deg[0].to_radians().sin_cos();
    let (sy, cy) = deg[1].to_radians().sin_cos();
    let (sz, cz) = deg[2].to_radians().sin_cos();
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    mul3(&rz, &mul3(&ry, &rx))
}

fn mul3(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn turn(p: [f64; 3], rotation: &[[f64; 3]; 3]) -> Vec3d {
    let q = rotation.map(|row| {
        ((row[0] * p[0] + row[1] * p[1] + row[2] * p[2]) as f32) as f64
    });
    dvec3(q[0], q[1], q[2])
}

fn turn_f64(p: [f64; 3], rotation: &[[f64; 3]; 3]) -> Vec3d {
    let q = rotation.map(|row| row[0] * p[0] + row[1] * p[1] + row[2] * p[2]);
    dvec3(q[0], q[1], q[2])
}

fn rectangle(center: [f64; 2], size: [f64; 2], z: f64) -> [[f64; 3]; 4] {
    [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]].map(|[x, y]| {
        [center[0] + x * size[0] * 0.5, center[1] + y * size[1] * 0.5, z]
    })
}

fn hull(sections: [[[f64; 3]; 4]; 2], rotation: &[[f64; 3]; 3]) -> TriMesh {
    let points: Vec<_> = sections.into_iter().flatten().map(|p| turn(p, rotation)).collect();
    makepad_csg_primitives::hull(&points).expect("two rectangles form a solid hull")
}

fn section(z: f64) -> [[f64; 3]; 4] {
    let t = (z - 5.0) / (9.0 - 5.0);
    rectangle(
        [1.08, 3.16],
        [3.6 + (4.0 - 3.6) * t, 1.6 + (2.0 - 1.6) * t],
        z,
    )
}

fn scene(rotation: &[[f64; 3]; 3], precise: bool) -> (TriMesh, TriMesh, TriMesh, f64) {
    let solid = |sections: [[[f64; 3]; 4]; 2]| {
        if precise {
            let points: Vec<_> = sections.into_iter().flatten()
                .map(|p| turn_f64(p, rotation)).collect();
            makepad_csg_primitives::hull(&points).expect("two rectangles form a solid hull")
        } else {
            hull(sections, rotation)
        }
    };
    let body = solid([
        rectangle([0.0, 0.0], [18.0, 18.0], 0.0),
        rectangle([0.0, 0.0], [14.0, 14.0], 8.0),
    ]);
    let window = solid([section(9.0), section(5.0)]);
    let a = mesh_boolean_with(&body, &window, BoolOp::Difference, FinishParams::default());
    let inside = mesh_boolean_with(&body, &window, BoolOp::Intersection, FinishParams::default());
    let volume = mesh_volume(&body) - mesh_volume(&inside);
    (body, window, a, volume)
}

#[derive(Clone, Copy)]
struct Plane {
    normal: Vec3d,
    point: Vec3d,
}

impl Plane {
    fn through(a: Vec3d, b: Vec3d, c: Vec3d) -> Self {
        Self { normal: (b - a).cross(c - a).normalize(), point: a }
    }

    fn distance(self, point: Vec3d) -> f64 {
        self.normal.dot(point - self.point)
    }

    fn offset(self, distance: f64) -> Self {
        Self { point: self.point + self.normal * distance, ..self }
    }
}

fn intersection(a: Plane, b: Plane, c: Plane) -> Vec3d {
    // Cramer's rule solves the three plane equations without a hull moving the corners.
    (b.normal.cross(c.normal) * a.normal.dot(a.point)
        + c.normal.cross(a.normal) * b.normal.dot(b.point)
        + a.normal.cross(b.normal) * c.normal.dot(c.point))
        / a.normal.dot(b.normal.cross(c.normal))
}

fn wall_planes(bottom: [Vec3d; 4], top: [Vec3d; 4]) -> [Plane; 4] {
    [1, 2, 3, 0].map(|i| Plane::through(bottom[i], bottom[(i + 1) % 4], top[(i + 1) % 4]))
}

fn designed_shot(rotation: &[[f64; 3]; 3], tol: f64, past: bool) -> TriMesh {
    let bottom = section(5.0).map(|p| dvec3(p[0], p[1], p[2]));
    let top = section(9.0).map(|p| dvec3(p[0], p[1], p[2]));
    let walls = wall_planes(bottom, top);
    let gaps = [0.3, -0.3, 0.3, -0.3];
    let shifted: [Plane; 4] = std::array::from_fn(|i| walls[i].offset(gaps[i] * tol));
    let caps = [
        Plane { normal: dvec3(0.0, 0.0, -1.0), point: dvec3(0.0, 0.0, 5.5) },
        Plane { normal: dvec3(0.0, 0.0, 1.0), point: dvec3(0.0, 0.0, 8.0) }.offset(0.3 * tol),
    ];
    let mut mesh = TriMesh::new();
    for cap in caps {
        for [x, y] in CORNER_WALLS {
            mesh.vertices.push(intersection(shifted[x], shifted[y], cap));
        }
    }
    if past {
        mesh.vertices[2] = mesh.vertices[2]
            + walls[0].normal * (3.0 * tol - walls[0].distance(mesh.vertices[2]));
    }
    let corner = mesh.vertices[6];
    mesh.vertices.push(corner + (mesh.vertices[4] - corner).normalize() * (3.0 * tol));
    mesh.triangles.extend([[0, 2, 1], [0, 3, 2]]);
    for i in 0..4u32 {
        let j = (i + 1) % 4;
        mesh.triangles.extend([[i, j, j + 4], [i, j + 4, i + 4], [8, i + 4, j + 4]]);
    }
    for p in &mut mesh.vertices {
        *p = turn_f64([p.x, p.y, p.z], rotation);
    }
    mesh
}

fn nearest_vertex(mesh: &TriMesh, point: Vec3d) -> Vec3d {
    mesh.vertices.iter().copied()
        .min_by(|a, b| (*a - point).length_sq().total_cmp(&(*b - point).length_sq()))
        .unwrap_or(dvec3(f64::NAN, f64::NAN, f64::NAN))
}

fn reference_planes(body: &TriMesh, window: &TriMesh, rotation: &[[f64; 3]; 3]) -> [Plane; 5] {
    let corners = |mesh: &TriMesh, points: [[f64; 3]; 4]| {
        points.map(|p| nearest_vertex(mesh, turn_f64(p, rotation)))
    };
    let walls = wall_planes(corners(window, section(5.0)), corners(window, section(9.0)));
    let top = corners(body, rectangle([0.0, 0.0], [14.0, 14.0], 8.0));
    [walls[0], walls[1], walls[2], walls[3], Plane::through(top[0], top[1], top[2])]
}

fn record(failures: &mut Vec<String>, context: &str, good: bool, property: String) {
    if !good {
        failures.push(format!("{context}: {property}"));
    }
}

struct Flat {
    triangle: usize,
    points: [Vec3d; 3],
    midpoint: Vec3d,
    shortest: f64,
}

fn check_mesh(mesh: &TriMesh, context: &str, failures: &mut Vec<String>, coincident: usize) -> Vec<Flat> {
    let report = validate_mesh(mesh);
    record(failures, context, report.is_closed,
        format!("closed: boundary edges={}", report.boundary_edges));
    record(failures, context, report.is_manifold,
        format!("manifold: non-manifold edges={}", report.non_manifold_edges));
    record(failures, context, report.is_consistently_oriented,
        format!("consistently oriented={}", report.is_consistently_oriented));
    record(failures, context, report.near_duplicate_pairs == 0,
        format!("near duplicate pairs={}", report.near_duplicate_pairs));
    record(failures, context, report.coincident_pairs <= coincident,
        format!("coincident pairs={} at most {coincident}", report.coincident_pairs));
    let mut flats = Vec::new();
    for (triangle, &[a, b, c]) in mesh.triangles.iter().enumerate() {
        if a == b || b == c || c == a {
            record(failures, context, false,
                format!("distinct indices: triangle {triangle} has [{a}, {b}, {c}]"));
            continue;
        }
        let points = [a, b, c].map(|i| mesh.vertices[i as usize]);
        if is_flat(points[0], points[1], points[2]) {
            let mut edge = (points[0], points[1]);
            for (p, q) in [(points[1], points[2]), (points[2], points[0])] {
                if (p - q).length_sq() < (edge.0 - edge.1).length_sq() {
                    edge = (p, q);
                }
            }
            flats.push(Flat {
                triangle, points, midpoint: (edge.0 + edge.1) * 0.5,
                shortest: (edge.0 - edge.1).length(),
            });
        }
    }
    flats
}

fn check_a(a: &TriMesh, context: &str, failures: &mut Vec<String>) {
    let context = format!("{context} orders A+B and B+A operand A");
    let flats = check_mesh(a, &context, failures, 0);
    for flat in flats {
        record(failures, &context, false,
            format!("unexpected flat: triangle {} shortest={:.17e}", flat.triangle, flat.shortest));
    }
}

fn check_union(
    left: &TriMesh, right: &TriMesh, expected: f64, context: &str,
    last: bool, failures: &mut Vec<String>, coincident: usize,
) -> (Vec<Flat>, BooleanReport) {
    let run = || mesh_boolean_report(left, right, BoolOp::Union, FinishParams::default());
    let (result, report) = run();
    let flats = check_mesh(&result, context, failures, coincident);
    record(failures, context, report.joined_open == 0,
        format!("joined_open={}", report.joined_open));
    record(failures, context, report.refined_open == [0, 0],
        format!("refined_open={:?}", report.refined_open));
    record(failures, context, report.cut == [0, 0, 0], format!("cut={:?}", report.cut));
    record(failures, context, report.label_conflicts == 0,
        format!("label_conflicts={}", report.label_conflicts));
    let volume = mesh_volume(&result);
    let error = (volume - expected).abs() / expected.abs();
    record(failures, context, error <= 1e-6,
        format!("volume={volume:.17} expected={expected:.17} relative error={error:.17e}"));
    if last {
        let bits = |mesh: &TriMesh| {
            mesh.vertices.iter().map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()])
                .collect::<Vec<_>>()
        };
        let original = bits(&result);
        for (name, other) in [("repeat", run().0), ("serial", with_serial(run).0)] {
            record(failures, context, original == bits(&other),
                format!("{name} determinism: vertex bits differ"));
            record(failures, context, result.triangles == other.triangles,
                format!("{name} determinism: triangles differ"));
        }
    }
    (flats, report)
}

fn check_gaps(
    a: &TriMesh, b: &TriMesh, planes: &[Plane; 5], tol: f64,
    past: bool, context: &str, failures: &mut Vec<String>,
) {
    let bounds = a.bounding_box().union(b.bounding_box());
    let union_tol = plane_tolerance(bounds.min, bounds.max, bounds.min, bounds.max, bounds.min, bounds.max);
    record(failures, context, tol == union_tol,
        format!("tolerance: A={tol:.17e} union={union_tol:.17e}"));
    let faces: [&[usize]; 5] = [
        &[1, 2, 5, 6], &[2, 3, 6, 7], &[0, 3, 4, 7], &[0, 1, 4, 5], &[4, 5, 6, 7, 8],
    ];
    for (face, vertices) in faces.into_iter().enumerate() {
        for &vertex in vertices {
            if past && face == 0 && vertex == 2 {
                continue;
            }
            let distance = planes[face].distance(b.vertices[vertex]);
            record(failures, context, distance.abs() <= 0.31 * tol,
                format!("inside plane {face} vertex {vertex}: distance={distance:.17e} tol units={:.17e}", distance / tol));
        }
    }
    // The corner pairs lie past the old corefinement point merge (1e-10), so no position merge joins
    // them, but inside the weld: that uses the same plane tolerance, and three near-orthogonal
    // 0.3 tol gaps separate corners by about 0.5 tol. The index-only join exposes what the weld hides.
    for (corner, [x, y]) in CORNER_WALLS.into_iter().enumerate() {
        let point_b = b.vertices[corner + 4];
        let point_a = nearest_vertex(a, point_b);
        let distance = (point_a - point_b).length();
        record(failures, context, distance > 1e-10 && distance < tol,
            format!("corner pair {corner}: separation={distance:.17e} tol units={:.17e}", distance / tol));
        for face in [x, y, 4] {
            for (operand, point) in [("A", point_a), ("B", point_b)] {
                let distance = planes[face].distance(point);
                record(failures, context, distance.abs() <= 0.31 * tol,
                    format!("corner {corner} {operand} plane {face}: distance={distance:.17e} tol units={:.17e}", distance / tol));
            }
        }
    }
    let separation = (b.vertices[8] - b.vertices[6]).length();
    record(failures, context, (2.9..=3.1).contains(&(separation / tol)),
        format!("cap pair: separation={separation:.17e} tol units={:.17e}", separation / tol));
    if past {
        let distance = planes[0].distance(b.vertices[2]);
        record(failures, context, (2.9..=3.1).contains(&(distance / tol)),
            format!("past corner: distance={distance:.17e} tol units={:.17e}", distance / tol));
    }
}

/// How far from the moved corner its crossings with A's wall can lie: the gap over the sine of the
/// flattest edge at the corner that crosses the wall (its far end on the other side).
fn crossing_footprint(b: &TriMesh, plane: Plane) -> (f64, f64, f64) {
    let corner = b.vertices[2];
    let distance = plane.distance(corner);
    let mut sine = 1.0f64;
    for triangle in &b.triangles {
        if triangle.contains(&2) {
            for &index in triangle {
                let end = b.vertices[index as usize];
                if index != 2 && plane.distance(end) < 0.0 {
                    let edge = end - corner;
                    sine = sine.min(plane.normal.dot(edge).abs() / edge.length());
                }
            }
        }
    }
    let phi = sine.clamp(0.0, 1.0).asin();
    (distance, phi, distance / phi.sin())
}

#[test]
fn two_shots_with_designed_gaps_unite_by_identity() {
    let mut failures = Vec::new();
    let mut summaries = Vec::new();
    for (index, turn) in TURNS.into_iter().enumerate() {
        let rotation = rotation_xyz(turn);
        let context = format!("case A turn {turn:?}");
        let (body, window, a, volume) = scene(&rotation, true);
        check_a(&a, &context, &mut failures);
        let bounds = a.bounding_box();
        let tol = plane_tolerance(bounds.min, bounds.max, bounds.min, bounds.max, bounds.min, bounds.max);
        let planes = reference_planes(&body, &window, &rotation);
        let mut summary = context.clone();
        for (variant, past) in [("plain", false), ("past", true)] {
            let b = designed_shot(&rotation, tol, past);
            let context = format!("{context} variant {variant}");
            let both = format!("{context} orders A+B and B+A");
            check_gaps(&a, &b, &planes, tol, past, &both, &mut failures);
            let (distance, phi, footprint) = if past {
                crossing_footprint(&b, planes[0])
            } else {
                (0.0, 0.0, 0.0)
            };
            if past {
                record(&mut failures, &both, footprint.is_finite() && footprint > 0.0,
                    format!("crossing footprint: d={distance:.17e} phi={phi:.17e} f={footprint:.17e}"));
            }
            let expected = volume + mesh_volume(&b);
            for (order, left, right) in [("A+B", &a, &b), ("B+A", &b, &a)] {
                let context = format!("{context} order {order}");
                let (flats, report) = check_union(left, right, expected, &context,
                    index + 1 == TURNS.len(), &mut failures, 0);
                record(&mut failures, &context, report.classes > 0,
                    format!("classes={}", report.classes));
                record(&mut failures, &context, report.aliases > 0,
                    format!("aliases={}", report.aliases));
                summary.push_str(&format!("; {variant} {order} classes={} aliases={} refused={}",
                    report.classes, report.aliases, report.refused));
                for flat in flats {
                    let nearest = flat.points.iter().map(|p| (*p - b.vertices[2]).length())
                        .fold(f64::INFINITY, f64::min);
                    let allowed = past && footprint.is_finite()
                        && nearest <= footprint + distance && flat.shortest <= footprint;
                    record(&mut failures, &context, allowed,
                        format!("unexpected flat: triangle {} nearest={nearest:.17e} shortest={:.17e} d={distance:.17e} phi={phi:.17e} f={footprint:.17e}",
                            flat.triangle, flat.shortest));
                }
            }
        }
        summaries.push(summary);
    }
    assert!(failures.is_empty(), "{}\n{}", failures.join("\n"), summaries.join("\n"));
}

#[test]
fn two_shots_through_f32_keep_the_diagnosed_needles() {
    // The f32 pass puts corners about 1e-8 off each other's planes; this is the diagnosed set, so any new flat fails.
    // Once operands declare their precision, this table becomes no flats.
    let sites: [&[[f64; 3]]; 4] = [
        &[],
        &[[2.905, 3.985, 5.5], [-0.745, 3.985, 5.5], [0.03, 4.11, 8.0]],
        &[],
        &[[-0.745, 3.985, 5.5]],
    ];
    // Coincident vertex pairs (contact copies kept apart, no edge) each turn's union keeps; a count may only fall.
    let contacts = [12, 0, 2, 0];
    let mut failures = Vec::new();
    for (index, turn) in TURNS.into_iter().enumerate() {
        let rotation = rotation_xyz(turn);
        let inverse = std::array::from_fn(|i| std::array::from_fn(|j| rotation[j][i]));
        let context = format!("case B turn {turn:?}");
        let (body, window, a, volume) = scene(&rotation, false);
        check_a(&a, &context, &mut failures);
        let b = hull([section(8.0), section(5.5)], &rotation);
        // The case's own precondition, so it keeps testing near contact past the tolerance whatever
        // the generators become: B's corners off A's walls by more than the tolerance and by at most
        // 100 times it.
        let bounds = a.bounding_box().union(b.bounding_box());
        let tol = plane_tolerance(bounds.min, bounds.max, bounds.min, bounds.max, bounds.min, bounds.max);
        let planes = reference_planes(&body, &window, &rotation);
        let near = b.vertices.iter().filter(|&&p| planes.iter().any(|plane| {
            let distance = plane.distance(p).abs();
            distance > tol && distance <= 100.0 * tol
        })).count();
        record(&mut failures, &context, near > 0,
            format!("precondition: B corners past the tolerance and within 100x of A's walls={near} tol={tol:.3e}"));
        let expected = volume + mesh_volume(&b);
        for (order, left, right) in [("A+B", &a, &b), ("B+A", &b, &a)] {
            let context = format!("{context} order {order}");
            let (flats, report) = check_union(left, right, expected, &context,
                index + 1 == TURNS.len(), &mut failures, contacts[index]);
            record(&mut failures, &context, report.refused == 0,
                format!("refused={}: under cells no class forms past the tolerance, so nothing is refused", report.refused));
            let mut counts = vec![0; sites[index].len()];
            for flat in flats {
                let p = flat.midpoint;
                let midpoint = turn_f64([p.x, p.y, p.z], &inverse);
                let site = sites[index].iter().position(|p| {
                    (midpoint - dvec3(p[0], p[1], p[2])).length() <= 1e-6
                });
                if let Some(site) = site {
                    counts[site] += 1;
                }
                record(&mut failures, &context, site.is_some() && flat.shortest <= 1e-6,
                    format!("needle triangle {}: midpoint={midpoint:?} site={site:?} shortest={:.17e}",
                        flat.triangle, flat.shortest));
            }
            for (site, count) in sites[index].iter().zip(counts) {
                record(&mut failures, &context, count == 1,
                    format!("needle site {site:?}: count={count}, expected one"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Edges that do not have exactly one face each way, as (edge, faces from the lower index, back).
fn not_one_each_way(mesh: &TriMesh) -> Vec<((u32, u32), usize, usize)> {
    let mut counts = std::collections::BTreeMap::<(u32, u32), (usize, usize)>::new();
    for &[a, b, c] in &mesh.triangles {
        for (u, v) in [(a, b), (b, c), (c, a)] {
            let count = counts.entry((u.min(v), u.max(v))).or_default();
            if u < v { count.0 += 1 } else { count.1 += 1 }
        }
    }
    counts.into_iter().filter(|(_, count)| *count != (1, 1)).map(|(edge, (ab, ba))| (edge, ab, ba)).collect()
}

#[test]
fn finishing_keeps_thin_two_sheet_cells_one_face_each_way() {
    // Thin cells of exact geometry: two sheets on one rim, each triangulated with different chords.
    // The finishing passes take a mesh with one face each way on every edge to another such mesh.
    let mesh = |points: &[[f64; 3]], triangles: &[[u32; 3]]| {
        let mut mesh = TriMesh::new();
        for p in points {
            mesh.add_vertex(dvec3(p[0], p[1], p[2]));
        }
        for t in triangles {
            mesh.add_triangle(t[0], t[1], t[2]);
        }
        mesh
    };
    // Slivers: a hexagon, the top sheet on chords 1-3, 0-3, 0-4 and the bottom on 2-4, 1-4, 1-5. The
    // top's two triangles on 0-3 are slivers (1e-7 across, under the 1e-5 tolerance); flipping
    // 0-3 would make 1-4, which the bottom has. The other triangles are wide.
    let w = 1e-7;
    let rim = [[-1.0, 0.0, 0.0], [-0.2, -w, 0.0], [0.5, -1.0, 0.0], [1.0, 0.0, 0.0], [0.2, w, 0.0], [-0.5, 1.0, 0.0]];
    let mut cell = mesh(&rim, &[[0, 1, 3], [1, 2, 3], [0, 3, 4], [0, 4, 5], [1, 4, 2], [2, 4, 3], [4, 1, 5], [5, 1, 0]]);
    assert!(not_one_each_way(&cell).is_empty(), "the made-up cell is closed, one face each way");
    let mut active = vec![false; cell.vertex_count()];
    crate::boolean::remove_slivers(&mut cell, 1e-5, &mut active);
    let slivers = not_one_each_way(&cell);
    // Weld: each sheet is a fan round its own centre, the centres 1e-9 apart under a 1e-8 radius.
    // Joining them would put four faces on every spoke.
    let mut points = vec![[-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    points.extend([[0.0, 0.0, 1e-9], [0.0, 0.0, 0.0]]);
    let mut fans = mesh(&points, &[[0, 1, 4], [1, 2, 4], [2, 3, 4], [3, 0, 4], [1, 0, 5], [2, 1, 5], [3, 2, 5], [0, 3, 5]]);
    assert!(not_one_each_way(&fans).is_empty(), "the made-up fans are closed, one face each way");
    let (_, refused) = crate::weld::weld_keeping_edges(&mut fans, 0, 1e-8);
    let weld = not_one_each_way(&fans);
    assert!(slivers.is_empty() && weld.is_empty() && refused == 1 && fans.triangle_count() == 8,
        "sliver flips make no edge that is already there: {slivers:?}; the weld refuses the join of the two centres: \
         refused {refused}, {} triangles, {weld:?}", fans.triangle_count());
}
