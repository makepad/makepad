use makepad_dynamics::collision_continuous::{pt_tri, seg_seg};

#[test]
fn closest_points_on_axis_aligned_features() {
    let mut cp1 = [0.0; 3];
    let mut cp2 = [0.0; 3];
    let mut st = [0.0; 2];
    let distance = seg_seg(
        &[-1.0, 0.0, 0.0], &[1.0, 0.0, 0.0],
        &[0.0, -1.0, 2.0], &[0.0, 1.0, 2.0],
        &mut cp1, &mut cp2, &mut st,
    );
    assert_eq!(distance, 2.0, "perpendicular segment distance");
    assert_eq!(cp1, [0.0, 0.0, 0.0], "first segment closest point");
    assert_eq!(cp2, [0.0, 0.0, 2.0], "second segment closest point");
    assert_eq!(st, [0.5, 0.5], "interior segment parameters");

    let distance = seg_seg(
        &[0.0, 0.0, 0.0], &[1.0, 0.0, 0.0],
        &[2.0, 1.0, 0.0], &[2.0, -1.0, 0.0],
        &mut cp1, &mut cp2, &mut st,
    );
    assert_eq!(distance, 1.0, "endpoint-clamped segment distance");
    assert_eq!(cp1, [1.0, 0.0, 0.0], "clamped closest endpoint");
    assert_eq!(cp2, [2.0, 0.0, 0.0], "opposite closest point");
    assert_eq!(st, [1.0, 0.5], "clamped segment parameters");

    let a = [0.0, 0.0, 0.0];
    let b = [2.0, 0.0, 0.0];
    let c = [0.0, 2.0, 0.0];
    let mut cp = [0.0; 3];
    let mut w = [0.0; 3];
    let distance = pt_tri(&[0.5, 0.5, 3.0], &a, &b, &c, &mut cp, &mut w);
    assert_eq!(distance, 3.0, "point above triangle distance");
    assert_eq!(cp, [0.5, 0.5, 0.0], "interior triangle closest point");
    assert_eq!(w, [0.5, 0.25, 0.25], "interior barycentric weights");

    let distance = pt_tri(&[1.0, -2.0, 0.0], &a, &b, &c, &mut cp, &mut w);
    assert_eq!(distance, 2.0, "point outside triangle distance");
    assert_eq!(cp, [1.0, 0.0, 0.0], "closest point on triangle edge");
    assert_eq!(w, [0.5, 0.5, 0.0], "edge barycentric weights");
}
