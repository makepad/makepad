// Port of box3d/test/test_math.c

use makepad_box3d::math_functions::*;
use makepad_box3d::math_internal::*;
use makepad_box3d::test_utils::random_float;
use makepad_box3d::{ensure, ensure_small};

// 0.0023 degrees
const ATAN_TOL: f32 = 0.00004;

#[test]
fn math_test() {
    let mut t = -10.0f32;
    while t < 10.0 {
        let angle = PI * t;
        let cs = compute_cos_sin(angle);
        let c = angle.cos();
        let s = angle.sin();

        // The cosine and sine approximations are accurate to about 0.1 degrees (0.002 radians)
        ensure_small!(cs.cosine - c, 0.002);
        ensure_small!(cs.sine - s, 0.002);

        let xn = unwind_angle(angle);
        let a = atan2(s, c);
        ensure!(is_valid_float(a));

        let mut diff = abs_float(a - xn);

        // The two results can be off by 360 degrees (-pi and pi)
        if diff > PI {
            diff -= 2.0 * PI;
        }

        // The approximate atan2 is quite accurate
        ensure_small!(diff, ATAN_TOL);

        t += 0.01;
    }

    let mut y = -1.0f32;
    while y <= 1.0 {
        let mut x = -1.0f32;
        while x <= 1.0 {
            let a1 = atan2(y, x);
            let a2 = y.atan2(x);
            let diff = abs_float(a1 - a2);
            ensure!(is_valid_float(a1));
            ensure_small!(diff, ATAN_TOL);
            x += 0.01;
        }
        y += 0.01;
    }

    for (yy, xx) in [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0), (0.0, 0.0)] {
        let a1 = atan2(yy, xx);
        let a2 = yy.atan2(xx);
        let diff = abs_float(a1 - a2);
        ensure!(is_valid_float(a1));
        ensure_small!(diff, ATAN_TOL);
    }

    let zero = Vec3::ZERO;
    let one = vec3(1.0, 1.0, 1.0);
    let two = vec3(2.0, 2.0, 2.0);

    let mut v = add(one, two);
    ensure!(v.x == 3.0 && v.y == 3.0);

    v = sub(zero, two);
    ensure!(v.x == -2.0 && v.y == -2.0);

    v = add(two, two);
    ensure!(v.x != 5.0 && v.y != 5.0);

    let axis = normalize(vec3(-0.75, 0.5, 1.0));
    let transform1 = Transform { p: vec3(-2.0, 3.0, 0.0), q: Quat::IDENTITY };
    let transform2 = Transform { p: vec3(1.0, 0.0, 0.0), q: make_quat_from_axis_angle(axis, PI) };

    let transform = mul_transforms(transform2, transform1);

    v = transform_point(transform2, transform_point(transform1, two));

    let mut u = transform_point(transform, two);

    ensure_small!(u.x - v.x, 10.0 * f32::EPSILON);
    ensure_small!(u.y - v.y, 10.0 * f32::EPSILON);

    v = transform_point(transform1, two);
    v = inv_transform_point(transform1, v);

    ensure_small!(v.x - two.x, 8.0 * f32::EPSILON);
    ensure_small!(v.y - two.y, 8.0 * f32::EPSILON);

    let rel_transform = inv_mul_transforms(transform1, transform2);
    v = inv_transform_point(transform1, transform_point(transform2, two));
    u = transform_point(rel_transform, two);
    ensure_small!(u.x - v.x, 10.0 * f32::EPSILON);
    ensure_small!(u.y - v.y, 10.0 * f32::EPSILON);

    {
        let axis = vec3(0.0, 0.0, 1.0);
        let q1 = make_quat_from_axis_angle(axis, -0.5 * PI);
        let q2 = compute_quat_between_unit_vectors(vec3(1.0, 0.0, 0.0), vec3(0.0, -1.0, 0.0));

        ensure_small!(q1.v.x - q2.v.x, f32::EPSILON);
        ensure_small!(q1.v.y - q2.v.y, f32::EPSILON);
        ensure_small!(q1.v.z - q2.v.z, f32::EPSILON);
        ensure_small!(q1.s - q2.s, f32::EPSILON);

        let q3 = normalize_quat(Quat { v: vec3(1.0, -2.0, 3.0), s: 4.0 });
        let q4 = inv_mul_quat(q3, q1);
        let q5 = mul_quat(q3, q4);
        ensure_small!(q1.v.x - q5.v.x, f32::EPSILON);
        ensure_small!(q1.v.y - q5.v.y, f32::EPSILON);
        ensure_small!(q1.v.z - q5.v.z, f32::EPSILON);
        ensure_small!(q1.s - q5.s, f32::EPSILON);

        let q6 = compute_quat_between_unit_vectors(vec3(0.0, 1.0, 0.0), vec3(0.0, -1.0, 0.0));
        ensure_small!(q6.s, f32::EPSILON);
    }

    let v = normalize(vec3(0.2, -0.5, 3.0));
    let mut z = -1.0f32;
    while z <= 1.0 {
        let mut y = -1.0f32;
        while y <= 1.0 {
            let mut x = -1.0f32;
            while x <= 1.0 {
                if x == 0.0 && y == 0.0 && z == 0.0 {
                    x += 0.02;
                    continue;
                }

                let u = normalize(vec3(x, y, z));

                let r = compute_quat_between_unit_vectors(v, u);
                ensure!(is_valid_quat(r));

                let w = rotate_vector(r, v);

                ensure_small!(dot(r.v, cross(u, w)) - scalar_triple_product(r.v, u, w), f32::EPSILON);

                // The quaternion between vectors can have lots of round off error at large angles.
                ensure_small!(w.x - u.x, 0.001);
                ensure_small!(w.y - u.y, 0.001);
                ensure_small!(w.z - u.z, 0.001);

                // Twist angle testing
                let mut twist = if r.s < 0.0 { atan2(-r.v.z, -r.s) } else { atan2(r.v.z, r.s) };
                twist *= 2.0;
                ensure!(-PI <= twist && twist <= PI);

                x += 0.02;
            }
            y += 0.02;
        }
        z += 0.02;
    }

    {
        // More twist angle testing
        let q = Quat { v: vec3(-0.0558656752, -0.188799798, 0.00689807534), s: -0.980401039 };
        let mut twist = if q.s < 0.0 { atan2(-q.v.z, -q.s) } else { atan2(q.v.z, q.s) };
        twist *= 2.0;
        ensure!(-PI <= twist && twist <= PI);
    }

    {
        let m = Matrix3 {
            cx: vec3(3.0, 1.0, -1.0),
            cy: vec3(-1.0, 3.0, 1.0),
            cz: vec3(1.0, -1.0, 3.0),
        };
        let inv_m = invert_matrix(m);
        let a = mul_mm(m, inv_m);
        ensure_small!(a.cx.x - 1.0, f32::EPSILON);
        ensure_small!(a.cx.y, f32::EPSILON);
        ensure_small!(a.cx.z, f32::EPSILON);
        ensure_small!(a.cy.x, f32::EPSILON);
        ensure_small!(a.cy.y - 1.0, f32::EPSILON);
        ensure_small!(a.cy.z, f32::EPSILON);
        ensure_small!(a.cz.x, f32::EPSILON);
        ensure_small!(a.cz.y, f32::EPSILON);
        ensure_small!(a.cz.z - 1.0, f32::EPSILON);

        let v = vec3(1.0, -2.0, 3.0);
        let u = mul_mv(inv_m, mul_mv(m, v));
        ensure_small!(v.x - u.x, f32::EPSILON);
        ensure_small!(v.y - u.y, f32::EPSILON);
        ensure_small!(v.z - u.z, f32::EPSILON);

        let w = mul_mv(inv_m, v);
        let u = solve3(m, v);
        ensure_small!(w.x - u.x, f32::EPSILON);
        ensure_small!(w.y - u.y, f32::EPSILON);
        ensure_small!(w.z - u.z, f32::EPSILON);
    }

    {
        let m = Matrix2 { cx: vec2(3.0, 1.0), cy: vec2(-1.0, 3.0) };
        let inv_m = invert2(m);
        let a = mul_mm2(m, inv_m);
        ensure_small!(a.cx.x - 1.0, f32::EPSILON);
        ensure_small!(a.cx.y, f32::EPSILON);
        ensure_small!(a.cy.x, f32::EPSILON);
        ensure_small!(a.cy.y - 1.0, f32::EPSILON);

        let v2 = vec2(1.0, -2.0);
        let mut u2 = mul_mv2(inv_m, mul_mv2(m, v2));
        ensure_small!(v2.x - u2.x, f32::EPSILON);
        ensure_small!(v2.y - u2.y, f32::EPSILON);

        let w = mul_mv2(inv_m, v2);
        u2 = solve2(m, v2);
        ensure_small!(w.x - u2.x, f32::EPSILON);
        ensure_small!(w.y - u2.y, f32::EPSILON);

        let w = mul_mv2(m, u2);
        ensure_small!(w.x - v2.x, 10.0 * f32::EPSILON);
        ensure_small!(w.y - v2.y, 10.0 * f32::EPSILON);
    }

    for _ in 0..100 {
        let a = random_float();
        let b = a as f64;
        let c = b as f32;
        ensure!(c == a);
    }

    let q1 = Quat::IDENTITY;
    let q2 = make_quat_from_axis_angle(Vec3::AXIS_Z, 0.5 * PI);
    let n = 100;
    for i in 0..=n {
        let alpha = i as f32 / n as f32;
        let q = nlerp(q1, q2, alpha);
        let angle = get_twist_angle(q);
        ensure_small!(alpha * 0.5 * PI - angle, 1.0 * DEG_TO_RAD);
    }

    {
        let normal = vec3(0.504055440, 0.621548057, 0.599671543);
        let perp = arbitrary_perp(normal);
        ensure_small!(dot(normal, perp), 2.0 * f32::EPSILON);
    }

    {
        // World position boundary helpers. The query agrees with the built type sizes.
        ensure!(
            makepad_box3d::core::is_double_precision()
                == (std::mem::size_of::<Pos>() > std::mem::size_of::<Vec3>())
        );

        // Deltas and offsets round trip exactly for representable inputs in both modes.
        let a = vec3(3.0, -5.0, 2.0);
        let b = vec3(1.0, 4.0, -6.0);
        let pa = to_pos(a);
        let pb = to_pos(b);

        let d = sub_pos(pa, pb);
        let s = sub(a, b);
        ensure!(d.x == s.x && d.y == s.y && d.z == s.z);

        let back = sub_pos(offset_pos(pb, s), pa);
        ensure!(back.x == 0.0 && back.y == 0.0 && back.z == 0.0);

        let r = to_vec3(pa);
        ensure!(r.x == a.x && r.y == a.y && r.z == a.z);

        ensure!(is_valid_position(pa));

        // World transform relative ops match the pure float transform ops.
        let axis = normalize(vec3(0.3, -0.7, 0.5));
        let t_a = Transform { p: a, q: make_quat_from_axis_angle(axis, 0.4) };
        let t_b = Transform { p: b, q: make_quat_from_axis_angle(axis, -1.1) };
        let w_a = make_world_transform(t_a);
        let w_b = make_world_transform(t_b);
        ensure!(is_valid_world_transform(w_a));

        let rel_ref = inv_mul_transforms(t_a, t_b);
        let rel = inv_mul_world_transforms(w_a, w_b);
        ensure_small!(rel.p.x - rel_ref.p.x, 1.0e-5);
        ensure_small!(rel.p.y - rel_ref.p.y, 1.0e-5);
        ensure_small!(rel.p.z - rel_ref.p.z, 1.0e-5);
        ensure_small!(rel.q.s - rel_ref.q.s, 1.0e-5);

        // Local point to world and back.
        let local = vec3(0.5, -0.25, 1.5);
        let back2 = inv_transform_world_point(w_a, transform_world_point(w_a, local));
        ensure_small!(back2.x - local.x, 1.0e-5);
        ensure_small!(back2.y - local.y, 1.0e-5);
        ensure_small!(back2.z - local.z, 1.0e-5);

        // Compose with a local transform, then strip it back off.
        let rel_ab = inv_mul_world_transforms(w_a, mul_world_transforms(w_a, t_b));
        ensure_small!(rel_ab.p.x - t_b.p.x, 1.0e-5);
        ensure_small!(rel_ab.p.y - t_b.p.y, 1.0e-5);
        ensure_small!(rel_ab.p.z - t_b.p.z, 1.0e-5);
    }
}
