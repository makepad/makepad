// Integration tests for the top-level CSG API.
// These test complete workflows: primitives -> booleans -> queries -> I/O.

use makepad_csg::{dvec3, union_all, Solid};
use std::f64::consts::PI;

// --- Primitive creation ---

#[test]
fn test_cube_uniform() {
    let c = Solid::cube_uniform(2.0, true);
    let vol = c.volume();
    assert!((vol - 8.0).abs() < 1e-10, "2x2x2 cube volume: {}", vol);
}

// --- Boolean operations ---

#[test]
fn test_union_non_overlapping() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(5.0, 0.0, 0.0);
    let u = a.union(&b);

    let vol = u.volume();
    assert!(
        (vol - 2.0).abs() < 0.1,
        "non-overlapping union volume: {}",
        vol
    );
}

#[test]
fn test_union_overlapping() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(0.5, 0.0, 0.0);
    let u = a.union(&b);

    let vol = u.volume();
    // Union of two overlapping unit cubes shifted by 0.5 on X
    // Volume = 2.0 - overlap = 2.0 - 0.5 = 1.5
    assert!(
        (vol - 1.5).abs() < 0.1,
        "overlapping union volume: {} (expected ~1.5)",
        vol
    );
    assert!(vol > 1.0 && vol < 2.0, "union volume in range: {}", vol);
}

#[test]
fn test_difference_basic() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(0.5, 0.0, 0.0);
    let d = a.difference(&b);

    let vol = d.volume();
    // A - B where B overlaps 0.5 of A => volume = 1.0 - 0.5 = 0.5
    assert!(
        (vol - 0.5).abs() < 0.1,
        "difference volume: {} (expected ~0.5)",
        vol
    );
    assert!(
        vol > 0.0 && vol < 1.0,
        "difference volume in range: {}",
        vol
    );
}

#[test]
fn test_difference_no_overlap() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(5.0, 0.0, 0.0);
    let d = a.difference(&b);

    let vol = d.volume();
    assert!(
        (vol - 1.0).abs() < 0.1,
        "no-overlap difference should equal A: {}",
        vol
    );
}

#[test]
fn test_intersection_basic() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(0.5, 0.0, 0.0);
    let isect = a.intersection(&b);

    let vol = isect.volume();
    // Intersection of two cubes shifted by 0.5 => 0.5 x 1.0 x 1.0 = 0.5
    assert!(
        (vol - 0.5).abs() < 0.1,
        "intersection volume: {} (expected ~0.5)",
        vol
    );
}

#[test]
fn test_intersection_no_overlap() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(5.0, 0.0, 0.0);
    let isect = a.intersection(&b);

    assert!(
        isect.is_empty(),
        "non-overlapping intersection should be empty"
    );
}

#[test]
fn test_union_identity() {
    // Union with empty = self
    let a = Solid::cube_uniform(1.0, true);
    let u = a.union(&Solid::empty());
    let vol = u.volume();
    assert!((vol - 1.0).abs() < 1e-10, "union with empty: {}", vol);
}

#[test]
fn test_difference_from_empty() {
    // Empty - anything = empty
    let d = Solid::empty().difference(&Solid::cube_uniform(1.0, true));
    assert!(d.is_empty());
}

// --- Transforms ---

#[test]
fn test_scale() {
    let c = Solid::cube_uniform(1.0, true).scale(2.0, 3.0, 4.0);
    let vol = c.volume();
    // Original volume 1.0 * scale = 2 * 3 * 4 = 24
    assert!((vol - 24.0).abs() < 1e-10, "scaled volume: {}", vol);
}

#[test]
fn test_rotate_y() {
    let c = Solid::cube_uniform(1.0, true).rotate_y(45.0);
    let vol = c.volume();
    // Rotation preserves volume
    assert!((vol - 1.0).abs() < 1e-10, "rotated volume: {}", vol);
}

#[test]
fn test_volume_preserved_by_transform() {
    let s = Solid::sphere(1.0, 16, 8);
    let vol_before = s.volume();

    let transformed = s.translate(5.0, 0.0, 0.0).rotate_z(30.0).scale_uniform(2.0);

    let vol_after = transformed.volume();
    // Scale of 2 in all axes -> volume * 8
    let expected = vol_before * 8.0;
    assert!(
        (vol_after - expected).abs() / expected < 1e-10,
        "transform volume: {} vs {}",
        vol_after,
        expected
    );
}

// --- Compound operations (OpenSCAD-style) ---

#[test]
fn test_sphere_with_hole() {
    // Drill a cylinder through a sphere (very low res for debug-mode speed)
    let sphere = Solid::sphere(1.0, 8, 4);
    let drill = Solid::cylinder(0.3, 3.0, 8, true);
    let result = sphere.difference(&drill);

    let sphere_vol = sphere.volume();
    let vol = result.volume();
    // Result volume should be less than sphere
    assert!(
        vol < sphere_vol,
        "drilled sphere should be smaller: {}",
        vol
    );
    assert!(
        vol > 0.0,
        "drilled sphere should have positive volume: {}",
        vol
    );
}

#[test]
fn test_showcase_rook_is_manifold() {
    // Reproduce the showcase rook build path exactly.
    let n = 12;
    let eps = 0.01;
    let base = Solid::tapered_cylinder(6.0, 5.0, 2.0, n, false);
    let body = Solid::cylinder(4.0, 10.0 + 2.0 * eps, n, false).translate(0.0, 2.0 - eps, 0.0);
    let neck = Solid::tapered_cylinder(4.0, 5.0, 2.0 + 2.0 * eps, n, false).translate(
        0.0,
        12.0 - eps,
        0.0,
    );
    let crown = Solid::cylinder(5.0, 2.0 + 2.0 * eps, n, false).translate(0.0, 14.0 - eps, 0.0);
    let hollow = Solid::cylinder(3.0, 14.0, n, false).translate(0.0, 2.5, 0.0);

    let mut rook = base.union(&body).union(&neck).union(&crown);
    rook = rook.difference(&hollow);
    // 4 crenellation notches — each cuts a gap in the crown wall.
    // The notch is offset in z so each 90° rotation cuts a distinct region.
    // (A centered 12-unit-wide notch at 0° and 180° occupies the same space,
    // causing cascaded boolean imprecision on the second redundant cut.)
    for i in 0..4 {
        let angle = (i as f64) * 90.0;
        let notch = Solid::cube(3.0, 2.5, 6.0, true)
            .translate(0.0, 15.25, 3.0)
            .rotate_y(angle);
        rook = rook.difference(&notch);
    }

    let report = rook.validate();
    assert!(
        report.is_closed && report.is_manifold && report.is_consistently_oriented,
        "rook should be valid, report={:?}",
        report
    );
}

#[test]
fn test_three_way_union() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(2.0, 0.0, 0.0);
    let c = Solid::cube_uniform(1.0, true).translate(4.0, 0.0, 0.0);

    let result = union_all(&[a, b, c]);
    let vol = result.volume();
    assert!((vol - 3.0).abs() < 0.2, "three-cube union volume: {}", vol);
}

#[test]
fn test_cross_shape() {
    // Three perpendicular beams forming a cross/jack shape
    let beam_x = Solid::cube(3.0, 0.5, 0.5, true);
    let beam_y = Solid::cube(0.5, 3.0, 0.5, true);
    let beam_z = Solid::cube(0.5, 0.5, 3.0, true);

    let cross = beam_x.union(&beam_y).union(&beam_z);
    let vol = cross.volume();

    // Each beam = 3 * 0.5 * 0.5 = 0.75
    // Three beams = 2.25, minus overlapping center cube (0.5^3 = 0.125) counted twice
    // Actually: 3 * 0.75 - 3 * overlap + 1 * triple_overlap
    // Overlap of two beams in the center = 0.5 * 0.5 * 0.5 = 0.125
    // 2.25 - 3*0.125 + 0.125 = 2.25 - 0.375 + 0.125 = 2.0
    assert!(
        vol > 1.5 && vol < 2.5,
        "cross volume: {} (expected ~2.0)",
        vol
    );
}

// --- I/O roundtrip ---

#[test]
fn test_stl_roundtrip() {
    let original = Solid::cube_uniform(2.0, true);
    let path = "/tmp/csg_integration_test.stl";

    original.write_stl(path).unwrap();
    let loaded = Solid::read_stl(path).unwrap();

    assert_eq!(loaded.triangle_count(), 12);
    let vol = loaded.volume();
    // STL uses f32, so some precision loss
    assert!((vol - 8.0).abs() < 0.01, "STL roundtrip volume: {}", vol);

    std::fs::remove_file(path).ok();
}

#[test]
fn test_boolean_result_stl_export() {
    // Full pipeline: create, boolean, export (cube-cube for debug speed)
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(0.8, true).translate(0.3, 0.3, 0.3);
    let result = a.difference(&b);

    let path = "/tmp/csg_boolean_result.stl";
    result.write_stl(path).unwrap();

    let file_size = std::fs::metadata(path).unwrap().len();
    assert!(file_size > 0, "STL file should not be empty");

    let loaded = Solid::read_stl(path).unwrap();
    assert!(loaded.triangle_count() > 0);
    assert!(loaded.volume() > 0.0);

    std::fs::remove_file(path).ok();
}

// --- Queries ---

#[test]
fn test_bounding_box() {
    let c = Solid::cube(2.0, 3.0, 4.0, false);
    let bb = c.bounding_box();
    assert!((bb.min.x).abs() < 1e-10);
    assert!((bb.min.y).abs() < 1e-10);
    assert!((bb.min.z).abs() < 1e-10);
    assert!((bb.max.x - 2.0).abs() < 1e-10);
    assert!((bb.max.y - 3.0).abs() < 1e-10);
    assert!((bb.max.z - 4.0).abs() < 1e-10);
}

#[test]
fn test_centroid_translated() {
    let c = Solid::cube_uniform(1.0, true).translate(5.0, 0.0, 0.0);
    let center = c.centroid();
    assert!((center.x - 5.0).abs() < 1e-10, "centroid x: {}", center.x);
    assert!(center.y.abs() < 1e-10);
    assert!(center.z.abs() < 1e-10);
}

// --- New feature integration tests ---

#[test]
fn test_polyhedron_tetrahedron() {
    // Regular tetrahedron with edge length sqrt(2)
    let verts = [
        dvec3(1.0, 1.0, 1.0),
        dvec3(1.0, -1.0, -1.0),
        dvec3(-1.0, 1.0, -1.0),
        dvec3(-1.0, -1.0, 1.0),
    ];
    let faces = [[0u32, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]];
    let tet = Solid::polyhedron(&verts, &faces);
    assert_eq!(tet.triangle_count(), 4);
    assert_eq!(tet.vertex_count(), 4);
    let vol = tet.volume().abs();
    // Edge = sqrt(8), V = edge^3 / (6*sqrt(2)) = 8*sqrt(8)/(6*sqrt(2)) = 8/3 * sqrt(4/1) ...
    // V = |det([b-a, c-a, d-a])| / 6 = |det([[0,-2,-2],[−2,0,−2],[−2,−2,0]])| / 6
    // det = 0+8+8 - (0+0+0) = -16, so V = 16/6 = 8/3
    let expected = 8.0 / 3.0;
    assert!(
        (vol - expected).abs() < 0.01,
        "tetrahedron volume: {} (expected {})",
        vol,
        expected
    );
}

#[test]
fn test_mirror_x() {
    let c = Solid::cube_uniform(1.0, false).translate(2.0, 0.0, 0.0);
    let mirrored = c.mirror(0); // Mirror across YZ plane (X)
    let bb = mirrored.bounding_box();
    // Original was at x=[2,3], mirrored should be at x=[-3,-2]
    assert!(
        (bb.min.x - (-3.0)).abs() < 1e-10,
        "mirror min.x: {}",
        bb.min.x
    );
    assert!(
        (bb.max.x - (-2.0)).abs() < 1e-10,
        "mirror max.x: {}",
        bb.max.x
    );
    // Volume should be preserved (positive after flip)
    let vol = mirrored.volume();
    assert!((vol - 1.0).abs() < 1e-10, "mirrored volume: {}", vol);
}

#[test]
fn test_resize_uniform() {
    // A 2x2x2 cube resized to 4x4x4
    let c = Solid::cube_uniform(2.0, true);
    let resized = c.resize(4.0, 4.0, 4.0);
    let vol = resized.volume();
    assert!(
        (vol - 64.0).abs() < 1e-10,
        "resized volume: {} (expected 64)",
        vol
    );
}

#[test]
fn test_resize_auto_axis() {
    // Resize only X to 4.0, Y and Z should auto-scale uniformly
    let c = Solid::cube_uniform(2.0, true);
    let resized = c.resize(4.0, 0.0, 0.0);
    let bb = resized.bounding_box();
    // X should be 4.0 wide
    assert!(
        ((bb.max.x - bb.min.x) - 4.0).abs() < 1e-10,
        "resized X width: {}",
        bb.max.x - bb.min.x
    );
    // Y and Z should also be 4.0 (auto from X scale factor of 2)
    assert!(
        ((bb.max.y - bb.min.y) - 4.0).abs() < 1e-10,
        "resized Y width: {}",
        bb.max.y - bb.min.y
    );
    assert!(
        ((bb.max.z - bb.min.z) - 4.0).abs() < 1e-10,
        "resized Z width: {}",
        bb.max.z - bb.min.z
    );
}

#[test]
fn test_symmetric_difference() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(0.5, 0.0, 0.0);
    let xor = a.symmetric_difference(&b);
    let vol = xor.volume();
    // XOR = union - intersection = 1.5 - 0.5 = 1.0
    assert!(
        (vol - 1.0).abs() < 0.2,
        "symmetric difference volume: {} (expected ~1.0)",
        vol
    );
}

#[test]
fn test_rotate_extrude_partial() {
    // Revolve 180 degrees
    let profile = vec![[1.0, 0.0], [2.0, 0.0], [2.0, 1.0], [1.0, 1.0]];
    let half = Solid::rotate_extrude(&profile, 180.0, 32);
    assert!(!half.is_empty());
    let vol = half.volume();
    // Half of 3*pi
    let expected = 1.5 * PI;
    let error = (vol - expected).abs() / expected;
    assert!(
        error < 0.05,
        "half-revolution volume error: {}% (vol={}, expected={})",
        error * 100.0,
        vol,
        expected
    );
}

#[test]
fn test_difference_all() {
    use makepad_csg::difference_all;
    let a = Solid::cube_uniform(2.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(0.0, 0.75, 0.0);
    let c = Solid::cube_uniform(1.0, true).translate(0.0, -0.75, 0.0);
    let result = difference_all(&[a.clone(), b, c]);
    let vol = result.volume();
    // Each subtraction removes up to 0.5 of the overlapping region
    assert!(vol > 0.0 && vol < 8.0, "difference_all volume: {}", vol);
}

#[test]
fn test_screw_hole_workflow() {
    // OpenSCAD-style: drill a hole through a block (screw hole)
    let block = Solid::cube(4.0, 4.0, 2.0, true);
    let hole = Solid::cylinder(0.5, 3.0, 32, true);
    let result = block.difference(&hole);

    let block_vol = block.volume();
    let result_vol = result.volume();

    // The hole passes through the entire block (height 2 < hole height 3)
    // So the removed volume is pi * 0.25 * 2.0 = 0.5*pi
    let removed = PI * 0.25 * 2.0;
    let expected = block_vol - removed;
    assert!(
        (result_vol - expected).abs() / expected < 0.1,
        "screw hole volume: {} (expected ~{})",
        result_vol,
        expected
    );
}

#[test]
fn test_countersunk_screw_hole() {
    // Block with a simple countersink (tapered cylinder subtraction only)
    let block = Solid::cube(4.0, 4.0, 2.0, true);
    let countersink = Solid::tapered_cylinder(0.8, 0.3, 0.5, 8, true);
    let result = block.difference(&countersink);

    assert!(result.volume() > 0.0);
    assert!(
        result.volume() < block.volume(),
        "countersink should reduce volume: result={}, block={}",
        result.volume(),
        block.volume()
    );
}

// --- Regression tests ---

#[test]
fn test_cube_sphere_union() {
    let cube = Solid::cube(4.0, 4.0, 2.0, true);
    let sphere = Solid::sphere(2.5, 8, 4);
    let result = cube.union(&sphere);
    assert!(result.triangle_count() > 0);
    let vol = result.volume();
    // Union must be at least as big as the larger operand
    assert!(
        vol >= cube.volume() - 0.1,
        "union volume too small: {}",
        vol
    );
}

#[test]
fn test_cube_sphere_difference() {
    let cube = Solid::cube(4.0, 4.0, 2.0, true);
    let sphere = Solid::sphere(1.0, 8, 4).translate(2.5, 0.0, 0.0);
    let result = cube.difference(&sphere);
    assert!(result.triangle_count() > 0);
    assert!(result.volume() < cube.volume());
    assert!(result.volume() > 0.0);
}

#[test]
fn test_cube_sphere_intersection() {
    let cube = Solid::cube(4.0, 4.0, 2.0, true);
    let sphere = Solid::sphere(1.5, 8, 4);
    let result = cube.intersection(&sphere);
    assert!(result.triangle_count() > 0);
    let vol = result.volume();
    // Intersection must be smaller than both operands
    assert!(
        vol < cube.volume(),
        "intersection should be smaller than cube"
    );
    assert!(
        vol < sphere.volume(),
        "intersection should be smaller than sphere"
    );
    assert!(vol > 0.0);
}

// --- Coincident faces that are coplanar only up to rounding ---

/// A rounded rectangle (centred, CCW), `seg` facets per quarter round.
fn rounded_rect(w: f64, d: f64, r: f64, seg: u32) -> Vec<[f64; 2]> {
    let (hx, hz) = (w / 2.0 - r, d / 2.0 - r);
    let mut out = Vec::new();
    for (k, (cx, cz)) in [(hx, -hz), (hx, hz), (-hx, hz), (-hx, -hz)].into_iter().enumerate() {
        let start = -PI / 2.0 + k as f64 * PI / 2.0;
        for i in 0..=seg {
            let a = start + PI / 2.0 * i as f64 / seg as f64;
            out.push([cx + r * a.cos(), cz + r * a.sin()]);
        }
    }
    out
}

/// A shelf and the wall standing on its back edge, both with rounded vertical edges, each
/// built centred and moved into place: their bottoms, back faces and the facets of the two
/// shared rounded edges coincide, but only up to floating-point rounding (not bit-exactly).
/// The union is one closed, manifold solid.
#[test]
fn test_union_rounded_coincident_faces_is_closed() {
    let shelf = Solid::extrude(&rounded_rect(64.0, 46.0, 1.2, 3), 6.0);
    let wall = Solid::extrude(&rounded_rect(64.0, 6.0, 1.2, 3), 52.0).translate(0.0, 0.0, -20.0);
    let u = shelf.union(&wall);
    let report = u.validate();
    assert!(report.is_closed && report.is_manifold, "rounded shelf + wall: {report:?}");
    let expected = shelf.volume() + wall.volume() - Solid::extrude(&rounded_rect(64.0, 6.0, 1.2, 3), 6.0).volume();
    assert!((u.volume() - expected).abs() < 1e-6 * expected, "volume {} vs {expected}", u.volume());
}

/// Two boxes sharing three faces, both turned by an oblique rotation: the shared faces are
/// coplanar only up to rounding. The union is one closed, manifold solid.
#[test]
fn test_union_rotated_boxes_sharing_faces_is_closed() {
    let axis = dvec3(1.0, 2.0, 3.0).normalize();
    let shelf = Solid::cube(64.0, 6.0, 46.0, false).rotate(axis, 33.0);
    let wall = Solid::cube(64.0, 52.0, 6.0, false).rotate(axis, 33.0);
    let u = shelf.union(&wall);
    let report = u.validate();
    assert!(report.is_closed && report.is_manifold, "rotated shelf + wall: {report:?}");
    let expected = 64.0 * 6.0 * 46.0 + 64.0 * 52.0 * 6.0 - 64.0 * 6.0 * 6.0;
    assert!((u.volume() - expected).abs() < 1e-6 * expected, "volume {} vs {expected}", u.volume());
}

// --- Solids touching face to face (coplanar faces with opposite normals) ---

/// A box standing on a plate, a box against the plate's side, and a pocket
/// flush with the plate's top: the touching faces are interior to a union,
/// stay the plate's surface in a difference, and enclose nothing in an
/// intersection. Every result is one closed, manifold solid of exact volume.
#[test]
fn test_touching_solids_union_difference_intersection() {
    let plate = Solid::cube(80.0, 60.0, 8.0, false);
    let check = |name: &str, s: &Solid, expected: f64| {
        let r = s.validate();
        assert!(r.is_closed && r.is_manifold, "{name}: {r:?}");
        assert!((s.volume() - expected).abs() < 1e-6 * expected.max(1.0), "{name}: volume {} vs {expected}", s.volume());
    };
    // On top: the box's bottom touches the plate's top.
    let on_top = Solid::cube(20.0, 20.0, 5.0, false).translate(5.3, 7.1, 8.0);
    check("box on plate", &plate.union(&on_top), 80.0 * 60.0 * 8.0 + 20.0 * 20.0 * 5.0);
    check("plate minus box on top", &plate.difference(&on_top), 80.0 * 60.0 * 8.0);
    assert!(plate.intersection(&on_top).volume().abs() < 1e-6, "touching boxes enclose nothing");
    // Side by side: half of the box's face against the plate's side.
    let beside = Solid::cube(10.0, 10.0, 8.0, false).translate(80.0, 55.0, 0.0);
    check("box beside plate", &plate.union(&beside), 80.0 * 60.0 * 8.0 + 10.0 * 10.0 * 8.0);
    // A pocket whose top is flush with the plate's top.
    let pocket = Solid::cube(10.0, 10.0, 3.0, false).translate(35.0, 25.0, 5.0);
    check("flush pocket", &plate.difference(&pocket), 80.0 * 60.0 * 8.0 - 300.0);
}
