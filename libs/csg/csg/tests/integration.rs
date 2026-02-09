// Integration tests for the top-level CSG API.
// These test complete workflows: primitives -> booleans -> queries -> I/O.

use makepad_csg::{dvec3, union_all, Solid};
use std::f64::consts::PI;

// --- Primitive creation ---

#[test]
fn test_cube_creation() {
    let c = Solid::cube(2.0, 3.0, 4.0, true);
    assert!(!c.is_empty());
    assert_eq!(c.triangle_count(), 12);
    assert_eq!(c.vertex_count(), 8);

    let vol = c.volume();
    assert!((vol - 24.0).abs() < 1e-10, "cube volume: {}", vol);
    assert!(c.is_valid());
}

#[test]
fn test_cube_uniform() {
    let c = Solid::cube_uniform(2.0, true);
    let vol = c.volume();
    assert!((vol - 8.0).abs() < 1e-10, "2x2x2 cube volume: {}", vol);
}

#[test]
fn test_sphere_creation() {
    let s = Solid::sphere(1.0, 32, 16);
    assert!(s.is_valid());

    let vol = s.volume();
    let expected = 4.0 / 3.0 * PI;
    let error = (vol - expected).abs() / expected;
    assert!(error < 0.02, "sphere volume error: {}%", error * 100.0);
}

#[test]
fn test_cylinder_creation() {
    let c = Solid::cylinder(1.0, 2.0, 64, true);
    assert!(c.is_valid());

    let vol = c.volume();
    let expected = PI * 2.0;
    let error = (vol - expected).abs() / expected;
    assert!(error < 0.01, "cylinder volume error: {}%", error * 100.0);
}

#[test]
fn test_cone_creation() {
    let c = Solid::cone(1.0, 3.0, 64, true);
    assert!(c.is_valid());

    let vol = c.volume();
    let expected = PI * 3.0 / 3.0;
    let error = (vol - expected).abs() / expected;
    assert!(error < 0.01, "cone volume error: {}%", error * 100.0);
}

#[test]
fn test_torus_creation() {
    let t = Solid::torus(2.0, 0.5, 64, 32);
    assert!(t.is_valid());

    let vol = t.volume();
    let expected = 2.0 * PI * PI * 2.0 * 0.25;
    let error = (vol - expected).abs() / expected;
    assert!(error < 0.01, "torus volume error: {}%", error * 100.0);
}

#[test]
fn test_extrude() {
    let hex = hexagon(1.0);
    let e = Solid::extrude(&hex, 2.0);
    assert!(e.is_valid());

    // Regular hexagon area = 3*sqrt(3)/2 * r^2
    let hex_area = 3.0 * 3.0_f64.sqrt() / 2.0;
    let expected_vol = hex_area * 2.0;
    let vol = e.volume();
    assert!(
        (vol - expected_vol).abs() / expected_vol < 0.01,
        "extruded hexagon volume: {} vs {}",
        vol,
        expected_vol
    );
}

fn hexagon(radius: f64) -> Vec<[f64; 2]> {
    (0..6)
        .map(|i| {
            let angle = PI / 3.0 * i as f64;
            [radius * angle.cos(), radius * angle.sin()]
        })
        .collect()
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
fn test_translate() {
    let c = Solid::cube_uniform(1.0, true).translate(10.0, 20.0, 30.0);
    let bb = c.bounding_box();
    assert!((bb.min.x - 9.5).abs() < 1e-10);
    assert!((bb.max.x - 10.5).abs() < 1e-10);
    assert!((bb.min.y - 19.5).abs() < 1e-10);
    assert!((bb.max.y - 20.5).abs() < 1e-10);
}

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
fn test_cube_intersection_compound() {
    // Intersection of two offset cubes
    let a = Solid::cube_uniform(2.0, true);
    let b = Solid::cube_uniform(2.0, true).translate(1.0, 1.0, 0.0);
    let result = a.intersection(&b);

    let vol = result.volume();
    // Overlap region is 1.0 x 1.0 x 2.0 = 2.0
    assert!(
        (vol - 2.0).abs() < 0.2,
        "axis-aligned intersection volume: {} (expected ~2.0)",
        vol
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
fn test_obj_roundtrip() {
    let original = Solid::cube_uniform(2.0, true);
    let path = "/tmp/csg_integration_test.obj";

    original.write_obj(path).unwrap();
    let loaded = Solid::read_obj(path).unwrap();

    assert_eq!(loaded.triangle_count(), 12);
    assert_eq!(loaded.vertex_count(), 8);

    let vol = loaded.volume();
    assert!((vol - 8.0).abs() < 1e-10, "OBJ roundtrip volume: {}", vol);

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
fn test_surface_area() {
    let c = Solid::cube_uniform(1.0, true);
    let area = c.surface_area();
    assert!(
        (area - 6.0).abs() < 1e-10,
        "unit cube surface area: {}",
        area
    );
}

#[test]
fn test_centroid() {
    let c = Solid::cube_uniform(1.0, true);
    let center = c.centroid();
    assert!(center.x.abs() < 1e-10);
    assert!(center.y.abs() < 1e-10);
    assert!(center.z.abs() < 1e-10);
}

#[test]
fn test_centroid_translated() {
    let c = Solid::cube_uniform(1.0, true).translate(5.0, 0.0, 0.0);
    let center = c.centroid();
    assert!((center.x - 5.0).abs() < 1e-10, "centroid x: {}", center.x);
    assert!(center.y.abs() < 1e-10);
    assert!(center.z.abs() < 1e-10);
}

// --- Edge cases ---

#[test]
fn test_flip() {
    let c = Solid::cube_uniform(1.0, true);
    let flipped = c.flip();
    let vol = flipped.volume();
    assert!((vol - (-1.0)).abs() < 1e-10, "flipped volume: {}", vol);
}

#[test]
fn test_merge_no_boolean() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(5.0, 0.0, 0.0);
    let merged = a.merge(&b);

    assert_eq!(merged.vertex_count(), 16);
    assert_eq!(merged.triangle_count(), 24);
}

#[test]
fn test_weld() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(1.0, 0.0, 0.0);
    let merged = a.merge(&b);
    let welded = merged.weld(0.01);

    // Two adjacent unit cubes share 4 vertices
    assert_eq!(welded.vertex_count(), 12);
}

#[test]
fn test_validate_primitive() {
    let s = Solid::sphere(1.0, 16, 8);
    let report = s.validate();
    assert!(report.is_closed);
    assert!(report.is_manifold);
    assert!(report.is_consistently_oriented);
    assert_eq!(report.degenerate_triangles, 0);
}

// --- Corefinement backend tests ---

#[test]
fn test_corefine_union_non_overlapping() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(5.0, 0.0, 0.0);
    let u = a.union_corefine(&b);

    let vol = u.volume();
    assert!(
        (vol - 2.0).abs() < 0.1,
        "corefine non-overlapping union volume: {}",
        vol
    );
}

#[test]
fn test_corefine_union_overlapping() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(0.5, 0.0, 0.0);
    let u = a.union_corefine(&b);

    let vol = u.volume();
    assert!(
        (vol - 1.5).abs() < 0.15,
        "corefine overlapping union volume: {} (expected ~1.5)",
        vol
    );
}

#[test]
fn test_corefine_difference() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(0.5, 0.0, 0.0);
    let d = a.difference_corefine(&b);

    let vol = d.volume();
    assert!(
        (vol - 0.5).abs() < 0.15,
        "corefine difference volume: {} (expected ~0.5)",
        vol
    );
}

#[test]
fn test_corefine_intersection() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(0.5, 0.0, 0.0);
    let isect = a.intersection_corefine(&b);

    let vol = isect.volume();
    assert!(
        (vol - 0.5).abs() < 0.15,
        "corefine intersection volume: {} (expected ~0.5)",
        vol
    );
}

#[test]
fn test_corefine_difference_no_overlap() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(5.0, 0.0, 0.0);
    let d = a.difference_corefine(&b);

    let vol = d.volume();
    assert!(
        (vol - 1.0).abs() < 0.1,
        "corefine no-overlap difference should equal A: {}",
        vol
    );
}

#[test]
fn test_corefine_intersection_no_overlap() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(5.0, 0.0, 0.0);
    let isect = a.intersection_corefine(&b);

    assert!(
        isect.is_empty(),
        "corefine non-overlapping intersection should be empty"
    );
}

// --- Boolean operation tests ---

#[test]
fn test_boolean_union_volume() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(0.5, 0.0, 0.0);
    let result = a.union(&b);
    assert!(
        result.volume() > 1.0 && result.volume() < 2.0,
        "union volume should be between 1.0 and 2.0, got {}",
        result.volume()
    );
}

#[test]
fn test_boolean_difference_volume() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(0.5, 0.0, 0.0);
    let result = a.difference(&b);
    assert!(
        result.volume() > 0.0 && result.volume() < 1.0,
        "difference volume should be between 0.0 and 1.0, got {}",
        result.volume()
    );
}

#[test]
fn test_boolean_intersection_volume() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(0.5, 0.0, 0.0);
    let result = a.intersection(&b);
    assert!(
        result.volume() > 0.0 && result.volume() < 1.0,
        "intersection volume should be between 0.0 and 1.0, got {}",
        result.volume()
    );
}

#[test]
fn test_corefine_identity() {
    // Union with empty = self
    let a = Solid::cube_uniform(1.0, true);
    let u = a.union_corefine(&Solid::empty());
    let vol = u.volume();
    assert!(
        (vol - 1.0).abs() < 1e-10,
        "corefine union with empty: {}",
        vol
    );
}

#[test]
fn test_corefine_cube_intersection_compound() {
    // Intersection of two offset cubes
    let a = Solid::cube_uniform(2.0, true);
    let b = Solid::cube_uniform(2.0, true).translate(1.0, 1.0, 0.0);
    let result = a.intersection_corefine(&b);

    let vol = result.volume();
    // Overlap region is 1.0 x 1.0 x 2.0 = 2.0
    assert!(
        (vol - 2.0).abs() < 0.3,
        "corefine axis-aligned intersection volume: {} (expected ~2.0)",
        vol
    );
}

// --- New feature integration tests ---

#[test]
fn test_tapered_cylinder_frustum() {
    // Frustum: r1=2, r2=1, height=3
    let tc = Solid::tapered_cylinder(2.0, 1.0, 3.0, 64, true);
    assert!(tc.is_valid());
    // V = (pi*h/3) * (r1^2 + r1*r2 + r2^2) = (pi*3/3)*(4+2+1) = 7*pi
    let expected = 7.0 * PI;
    let vol = tc.volume();
    let error = (vol - expected).abs() / expected;
    assert!(
        error < 0.01,
        "frustum volume error: {}% (vol={}, expected={})",
        error * 100.0,
        vol,
        expected
    );
}

#[test]
fn test_tapered_cylinder_is_cone() {
    // r2=0 should produce a cone
    let tc = Solid::tapered_cylinder(1.0, 0.0, 3.0, 64, true);
    let cone = Solid::cone(1.0, 3.0, 64, true);
    let vol_tc = tc.volume();
    let vol_cone = cone.volume();
    assert!(
        (vol_tc - vol_cone).abs() < 0.01,
        "tapered(r2=0) should match cone: {} vs {}",
        vol_tc,
        vol_cone
    );
}

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
fn test_mirror_y() {
    let c = Solid::cube_uniform(1.0, false).translate(0.0, 3.0, 0.0);
    let mirrored = c.mirror(1); // Mirror across XZ plane (Y)
    let bb = mirrored.bounding_box();
    assert!(
        (bb.min.y - (-4.0)).abs() < 1e-10,
        "mirror min.y: {}",
        bb.min.y
    );
    assert!(
        (bb.max.y - (-3.0)).abs() < 1e-10,
        "mirror max.y: {}",
        bb.max.y
    );
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
fn test_symmetric_difference_corefine() {
    let a = Solid::cube_uniform(1.0, true);
    let b = Solid::cube_uniform(1.0, true).translate(0.5, 0.0, 0.0);
    let xor = a.symmetric_difference_corefine(&b);
    let vol = xor.volume();
    assert!(
        (vol - 1.0).abs() < 0.2,
        "corefine symmetric difference volume: {} (expected ~1.0)",
        vol
    );
}

#[test]
fn test_linear_extrude_twist() {
    // Extrude a square with 90 degree twist
    let square = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let twisted = Solid::linear_extrude(&square, 2.0, 90.0, 1.0, 16);
    assert!(!twisted.is_empty());
    let vol = twisted.volume();
    // Twist preserves volume (same cross-section area at each height)
    let expected = 1.0 * 2.0; // area * height
    assert!(
        (vol - expected).abs() / expected < 0.05,
        "twisted extrude volume: {} (expected ~{})",
        vol,
        expected
    );
}

#[test]
fn test_linear_extrude_scale() {
    // Extrude a unit square scaling to 0.5 at top
    let square = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let scaled = Solid::linear_extrude(&square, 3.0, 0.0, 0.5, 8);
    assert!(!scaled.is_empty());
    let vol = scaled.volume();
    // Frustum-like: V = h/3 * (A1 + A2 + sqrt(A1*A2))
    // A1 = 1.0, A2 = 0.25, h = 3.0
    // V = 3/3 * (1.0 + 0.25 + 0.5) = 1.75
    let expected = 1.75;
    assert!(
        (vol - expected).abs() / expected < 0.05,
        "scaled extrude volume: {} (expected ~{})",
        vol,
        expected
    );
}

#[test]
fn test_rotate_extrude_full() {
    // Revolve a small rectangle to make a cylinder-like shape
    let profile = vec![[1.0, 0.0], [2.0, 0.0], [2.0, 1.0], [1.0, 1.0]];
    let lathe = Solid::rotate_extrude(&profile, 360.0, 64);
    assert!(!lathe.is_empty());
    let vol = lathe.volume();
    // Annular cylinder: V = pi * h * (R^2 - r^2) = pi * 1.0 * (4 - 1) = 3*pi
    let expected = 3.0 * PI;
    let error = (vol - expected).abs() / expected;
    assert!(
        error < 0.02,
        "rotate extrude annular volume error: {}% (vol={}, expected={})",
        error * 100.0,
        vol,
        expected
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
