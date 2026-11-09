use makepad_dynamics::hull::{convex_hull, Hull};
use std::collections::BTreeMap;

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn checked_hull(points: &[[f64; 3]]) -> Hull {
    let coordinates: Vec<_> = points.iter().flatten().copied().collect();
    let hull = convex_hull(&coordinates, points.len()).unwrap();
    let repeated = convex_hull(&coordinates, points.len()).unwrap();
    assert_eq!(hull.vertices, repeated.vertices, "deterministic vertices");
    assert_eq!(hull.faces, repeated.faces, "deterministic faces");
    assert_eq!(hull.neighbors, repeated.neighbors, "deterministic neighbors");
    assert!(hull.vertices.windows(2).all(|p| p[0] < p[1]), "ascending unique vertices");
    let mut maxima = [0.0_f64; 3];
    for point in points {
        for axis in 0..3 {
            maxima[axis] = maxima[axis].max(point[axis].abs());
        }
    }
    let eps = 3.0 * f64::EPSILON * maxima.iter().sum::<f64>();
    let mut centroid = [0.0; 3];
    for &vertex in &hull.vertices {
        assert!(vertex >= 0 && (vertex as usize) < points.len(), "valid vertex id");
        for (axis, coordinate) in centroid.iter_mut().enumerate() {
            *coordinate += points[vertex as usize][axis] / hull.vertices.len() as f64;
        }
    }
    let mut edges = BTreeMap::<(i32, i32), (usize, i32)>::new();
    let mut expected_neighbors = vec![Vec::new(); hull.vertices.len()];
    for (id, face) in hull.faces.iter().enumerate() {
        let [a, b, c] = face.map(|v| points[v as usize]);
        let normal = cross(sub(b, a), sub(c, a));
        let length = normal[0].hypot(normal[1]).hypot(normal[2]);
        assert!(length > 0.0, "nondegenerate face {id}");
        let normal = normal.map(|v| v / length);
        assert!(dot(normal, sub(centroid, a)) < 0.0, "outward face {id}");
        for (point, &position) in points.iter().enumerate() {
            let distance = dot(normal, sub(position, a));
            assert!(distance <= eps, "point {point} outside face {id}: {distance} > {eps}");
        }
        for i in 0..3 {
            let (a, b) = (face[i], face[(i + 1) % 3]);
            let edge = edges.entry((a.min(b), a.max(b))).or_default();
            edge.0 += 1;
            edge.1 += if a < b { 1 } else { -1 };
            expected_neighbors[hull.vertices.binary_search(&a).unwrap()].push(id);
        }
    }
    assert!(edges.values().all(|&edge| edge == (2, 0)), "each edge has two opposite uses");
    assert_eq!(2 * edges.len(), 3 * hull.faces.len(), "triangular edge count");
    assert_eq!(hull.vertices.len() + hull.faces.len(), edges.len() + 2, "Euler characteristic");
    assert_eq!(hull.neighbors, expected_neighbors, "ascending incident triangles");
    assert!(hull.neighbors.iter().all(|n| n.len() >= 3), "every corner has incident faces");
    hull
}

#[test]
fn cube_and_grid_drop_non_corners() {
    let mut cube = Vec::new();
    for x in [-1.0, 1.0] {
        for y in [-1.0, 1.0] {
            for z in [-1.0, 1.0] {
                cube.push([x, y, z]);
            }
        }
    }
    cube.push([0.0; 3]);
    let hull = checked_hull(&cube);
    assert_eq!(hull.vertices, (0..8).collect::<Vec<_>>(), "cube corners");
    assert_eq!(hull.faces.len(), 12, "cube triangulation");

    let mut grid = Vec::new();
    let mut corners = Vec::new();
    for x in [-1.0, 0.0, 1.0] {
        for y in [-1.0, 0.0, 1.0] {
            for z in [-1.0, 0.0, 1.0] {
                if x != 0.0 && y != 0.0 && z != 0.0 {
                    corners.push(grid.len() as i32);
                }
                grid.push([x, y, z]);
            }
        }
    }
    let hull = checked_hull(&grid);
    assert_eq!(hull.vertices, corners, "grid keeps only the eight corners");
    assert_eq!(hull.faces.len(), 12, "grid triangulation");
    // Rotate and translate the grid so planar merging is not axis-dependent.
    let rotated: Vec<_> = grid.iter().map(|&[x, y, z]| [3.0 + (2.0 * x - 2.0 * y + z) / 3.0, -2.0 + (x + 2.0 * y + 2.0 * z) / 3.0, (2.0 * x + y - 2.0 * z) / 3.0]).collect();
    let hull = checked_hull(&rotated);
    assert_eq!(hull.vertices, corners, "rotated grid corners");
    assert_eq!(hull.faces.len(), 12, "rotated grid triangulation");
}

#[test]
fn tetrahedron_and_octahedron() {
    let tetrahedron = [[1.0, 1.0, 1.0], [1.0, -1.0, -1.0], [-1.0, 1.0, -1.0], [-1.0, -1.0, 1.0]];
    let hull = checked_hull(&tetrahedron);
    assert_eq!((hull.vertices.len(), hull.faces.len()), (4, 4));
    let octahedron = [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0]];
    let hull = checked_hull(&octahedron);
    assert_eq!((hull.vertices.len(), hull.faces.len()), (6, 8));
}

struct Generator(u64);

impl Generator {
    fn coordinate(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        2.0 * ((self.0 >> 11) as f64 / 9007199254740992.0) - 1.0
    }

    fn ball(&mut self) -> [f64; 3] {
        loop {
            let point = [self.coordinate(), self.coordinate(), self.coordinate()];
            let radius = dot(point, point);
            if radius > 0.0 && radius <= 1.0 {
                return point;
            }
        }
    }
}

#[test]
fn irregular_ellipsoid() {
    let mut generator = Generator(19);
    let points: Vec<_> = (0..20).map(|i| {
        let mut point = generator.ball();
        if i % 2 == 0 {
            let radius = dot(point, point).sqrt();
            point = point.map(|v| v / radius);
        }
        [2.0 * point[0], 0.7 * point[1], 1.3 * point[2]]
    }).collect();
    checked_hull(&points);
}

#[test]
fn two_thousand_points_in_ball() {
    let mut generator = Generator(1234567);
    let points: Vec<_> = (0..2000).map(|_| generator.ball()).collect();
    checked_hull(&points);
}

#[test]
fn degenerate_and_invalid_inputs() {
    let cases = [
        (vec![[0.0; 3]; 3], "fewer than 4"),
        (vec![[2.0; 3]; 4], "colocated"),
        (vec![[0.0; 3]; 4], "colocated"),
        (vec![[0.0; 3], [1.0; 3], [2.0; 3], [-1.0; 3]], "collinear"),
        (vec![[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0], [1.0, 1.0, 1.0]], "coplanar"),
    ];
    for (points, expected) in cases {
        let coordinates: Vec<_> = points.iter().flatten().copied().collect();
        let error = convex_hull(&coordinates, points.len()).err().expect("degenerate hull must fail");
        assert!(error.contains(expected), "expected {expected}, got {error}");
    }
    assert!(convex_hull(&[], 0).is_err(), "empty point set");
    assert!(convex_hull(&[0.0; 11], 4).is_err(), "truncated coordinates");
    assert!(convex_hull(&[f64::NAN; 12], 4).is_err(), "non-finite coordinates");
    assert!(convex_hull(&[], usize::MAX).is_err(), "point ids must fit in i32");
}

// Points of a dense f32 surface mesh: 0, 3 and 5 lie on one straight edge, where two triangle groups
// that are not quite coplanar meet. Each group must keep point 0 as the other does, so every edge has
// its twin.
#[test]
fn corner_on_a_straight_side_stays_in_every_face() {
    let points: Vec<[f64; 3]> = [
        [27.773853_f32, -3.376929, 27.82456],
        [-30.002691, -6.219191, 25.569668],
        [-30.002691, -5.741691, 25.092169],
        [-28.571148, -3.376929, 27.82456],
        [29.206352, -4.3091908, 27.829668],
        [28.728853, -3.376929, 27.82456],
        [-1.353647, -6.654191, 1.6946682],
        [12.016353, 7.452918, 4.559668],
        [1.9893743, 8.261117, 18.18285],
    ].iter().map(|p| p.map(|v| v as f64)).collect();
    checked_hull(&points);
}
