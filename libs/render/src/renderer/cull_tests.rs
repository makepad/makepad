use super::*;

/// The camera the sandbox actually flies: perspective like scene.rs
/// builds (near 1, far 500), eye at +10z looking at the origin down -z.
fn frustum() -> Frustum {
    let view = Mat4f::look_at(vec3f(0.0, 0.0, 10.0), vec3f(0.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0));
    let projection = Mat4f::perspective(60.0, 1.0, 1.0, 500.0);
    Frustum::from_clip_matrix(&Mat4f::mul(&projection, &view))
}

fn unit_box_at(pos: Vec3f) -> (Vec3f, Vec3f, Mat4f) {
    let mut t = Mat4f::identity();
    t.v[12] = pos.x;
    t.v[13] = pos.y;
    t.v[14] = pos.z;
    (vec3f(-1.0, -1.0, -1.0), vec3f(1.0, 1.0, 1.0), t)
}

/// The plane extraction is validated against the definition of clip
/// space rather than against itself: a point is in the frustum iff its
/// clip coordinates satisfy |x|,|y| <= w and -w <= z <= w, and the
/// extracted planes must agree (via a zero-size box at that point).
/// Margins make the plane side a touch looser — never tighter.
#[test]
fn planes_agree_with_clip_space() {
    let view = Mat4f::look_at(vec3f(3.0, 2.0, 10.0), vec3f(0.0, 1.0, 0.0), vec3f(0.0, 1.0, 0.0));
    let projection = Mat4f::perspective(72.0, 1.6, 1.0, 500.0);
    let clip = Mat4f::mul(&projection, &view);
    let fr = Frustum::from_clip_matrix(&clip);
    for i in 0..125 {
        let p = vec3f(
            ((i % 5) as f32 - 2.0) * 40.0,
            (((i / 5) % 5) as f32 - 2.0) * 40.0,
            ((i / 25) as f32 - 2.0) * 40.0,
        );
        let c = clip.transform_vec4(vec4(p.x, p.y, p.z, 1.0));
        let inside = c.x.abs() <= c.w && c.y.abs() <= c.w && c.z.abs() <= c.w && c.w > 0.0;
        if inside {
            assert!(
                fr.intersects_obb(p, p, &Mat4f::identity()),
                "visible point culled at {:?}",
                p
            );
        }
    }
}

#[test]
fn a_box_behind_the_camera_culls() {
    let fr = frustum();
    // Eye is at z=10 looking toward -z; z=20 is squarely behind it.
    let (min, max, t) = unit_box_at(vec3f(0.0, 0.0, 20.0));
    assert!(!fr.intersects_obb(min, max, &t));
    assert!(!fr.intersects_sphere(vec3f(0.0, 0.0, 20.0), 1.0));
}

#[test]
fn a_box_in_front_does_not_cull() {
    let fr = frustum();
    let (min, max, t) = unit_box_at(vec3f(0.0, 0.0, 0.0));
    assert!(fr.intersects_obb(min, max, &t));
    assert!(fr.intersects_sphere(vec3f(0.0, 0.0, 0.0), 1.0));
}

#[test]
fn a_box_straddling_a_plane_does_not_cull() {
    let fr = frustum();
    // At 10 units depth with a 60-degree fov the left plane sits at
    // x ~= -5.77; a box spanning -100..0 in x straddles it.
    let t = Mat4f::identity();
    assert!(fr.intersects_obb(
        vec3f(-100.0, -1.0, -1.0),
        vec3f(0.0, 1.0, 1.0),
        &t
    ));
    // And one that swallows the whole frustum stays, too.
    assert!(fr.intersects_obb(
        vec3f(-1000.0, -1000.0, -1000.0),
        vec3f(1000.0, 1000.0, 1000.0),
        &t
    ));
}

#[test]
fn a_box_fully_beside_the_frustum_culls() {
    let fr = frustum();
    let (min, max, t) = unit_box_at(vec3f(-100.0, 0.0, 0.0));
    assert!(!fr.intersects_obb(min, max, &t));
    // …and past the far plane (far = 500 from the eye at z=10).
    let (min, max, t) = unit_box_at(vec3f(0.0, 0.0, -600.0));
    assert!(!fr.intersects_obb(min, max, &t));
}

/// The rotation path: bounds are model-space, so the corners must go
/// through the instance transform before the plane test. This box is
/// beside the frustum if only its translation were applied (x 10..500 at
/// ~10 units depth, right plane at ~6.3), but yaw 90 lays it along -z,
/// straddling the far plane dead ahead — it must survive.
#[test]
fn a_rotated_box_is_tested_in_world_space() {
    let fr = frustum();
    let t = Mat4f::rotation(vec3f(0.0, std::f32::consts::FRAC_PI_2, 0.0));
    assert!(fr.intersects_obb(
        vec3f(10.0, -0.5, -0.5),
        vec3f(500.0, 0.5, 0.5),
        &t
    ));
    // Sanity of the premise: unrotated it culls.
    assert!(!fr.intersects_obb(
        vec3f(10.0, -0.5, -0.5),
        vec3f(500.0, 0.5, 0.5),
        &Mat4f::identity()
    ));
}

#[test]
fn aabb_test_matches_the_corner_test() {
    let fr = frustum();
    // In front stays, behind culls, straddler stays — the same contract
    // the OBB corners give, via the positive-vertex shortcut.
    assert!(fr.intersects_aabb(vec3f(-1.0, -1.0, -1.0), vec3f(1.0, 1.0, 1.0)));
    assert!(!fr.intersects_aabb(vec3f(-1.0, -1.0, 19.0), vec3f(1.0, 1.0, 21.0)));
    assert!(fr.intersects_aabb(vec3f(-100.0, -1.0, -1.0), vec3f(0.0, 1.0, 1.0)));
    assert!(!fr.intersects_aabb(vec3f(-101.0, -1.0, -1.0), vec3f(-99.0, 1.0, 1.0)));
}
