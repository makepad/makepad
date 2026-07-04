// Not a C test port: verifies the wide (4-lane) contact solver ops and the V32
// geometry ops produce results identical to the scalar reference on the active
// path (NEON on aarch64, SSE2 on x86_64, trivial self-check on scalar builds).
//
// The C paths are engineered to be bit-compatible: b3MulAddW is deliberately
// non-fused on every path ("Cannot use real FMA because it doesn't match the
// non-SIMD path"), and add/sub/mul/div/sqrt/min/max are IEEE operations.
// Comparison ops differ in mask REPRESENTATION (all-bits vs 1.0f) by design,
// so they are checked semantically through blend_w/all_zero_w.

use makepad_box3d::contact_solver::{
    add_w, all_zero_w, blend_w, div_w, equals_w, greater_than_w, max_w, mul_add_w, mul_w, neg_w, or_w, set_w, splat_w,
    sqrt_w, sub_w, sym_clamp_w, zero_w, FloatW,
};
use makepad_box3d::test_utils::{random_float, set_random_seed, RAND_SEED};

fn lanes(a: FloatW) -> [f32; 4] {
    [a.get(0), a.get(1), a.get(2), a.get(3)]
}

fn assert_lanes_bits(actual: FloatW, expected: [f32; 4], what: &str) {
    let a = lanes(actual);
    for i in 0..4 {
        assert!(
            a[i].to_bits() == expected[i].to_bits(),
            "{}: lane {} differs: {} ({:#010x}) vs {} ({:#010x})",
            what,
            i,
            a[i],
            a[i].to_bits(),
            expected[i],
            expected[i].to_bits()
        );
    }
}

// Scalar reference implementations (the C B3_SIMD_NONE math, per lane).
fn ref_sym_clamp(a: f32, b: f32) -> f32 {
    let mut r = if a <= b { a } else { b };
    r = if r <= -b { -b } else { r };
    r
}

#[test]
fn wide_ops_match_scalar_reference() {
    set_random_seed(RAND_SEED);

    for iter in 0..1000 {
        // Random lane values in [-4, 4], plus fixed edge values on the first
        // iterations. Bounds for sym_clamp are non-negative like the solver's.
        let mut av = [0.0f32; 4];
        let mut bv = [0.0f32; 4];
        for i in 0..4 {
            av[i] = 4.0 * random_float();
            bv[i] = 4.0 * random_float();
        }
        if iter == 0 {
            av = [0.0, 1.0, -1.0, 3.5];
            bv = [0.0, 0.0, 2.0, 3.5];
        }

        let a = set_w(av[0], av[1], av[2], av[3]);
        let b = set_w(bv[0], bv[1], bv[2], bv[3]);

        // set_w/get round trip
        assert_lanes_bits(a, av, "set_w/get");

        // Arithmetic: exact bit equality with the scalar computation
        assert_lanes_bits(
            add_w(a, b),
            [av[0] + bv[0], av[1] + bv[1], av[2] + bv[2], av[3] + bv[3]],
            "add_w",
        );
        assert_lanes_bits(
            sub_w(a, b),
            [av[0] - bv[0], av[1] - bv[1], av[2] - bv[2], av[3] - bv[3]],
            "sub_w",
        );
        assert_lanes_bits(
            mul_w(a, b),
            [av[0] * bv[0], av[1] * bv[1], av[2] * bv[2], av[3] * bv[3]],
            "mul_w",
        );
        if bv.iter().all(|x| *x != 0.0) {
            assert_lanes_bits(
                div_w(a, b),
                [av[0] / bv[0], av[1] / bv[1], av[2] / bv[2], av[3] / bv[3]],
                "div_w",
            );
        }
        assert_lanes_bits(neg_w(a), [-av[0], -av[1], -av[2], -av[3]], "neg_w");

        let abs_a = set_w(av[0].abs(), av[1].abs(), av[2].abs(), av[3].abs());
        assert_lanes_bits(
            sqrt_w(abs_a),
            [av[0].abs().sqrt(), av[1].abs().sqrt(), av[2].abs().sqrt(), av[3].abs().sqrt()],
            "sqrt_w",
        );

        // Non-fused a + b * c on every path
        let c = splat_w(1.5);
        assert_lanes_bits(
            mul_add_w(a, b, c),
            [av[0] + bv[0] * 1.5, av[1] + bv[1] * 1.5, av[2] + bv[2] * 1.5, av[3] + bv[3] * 1.5],
            "mul_add_w",
        );

        assert_lanes_bits(
            max_w(a, b),
            [
                if av[0] >= bv[0] { av[0] } else { bv[0] },
                if av[1] >= bv[1] { av[1] } else { bv[1] },
                if av[2] >= bv[2] { av[2] } else { bv[2] },
                if av[3] >= bv[3] { av[3] } else { bv[3] },
            ],
            "max_w",
        );

        // sym_clamp with non-negative bounds (solver usage).
        // VALUE equality, not bit equality: at an exact a == b == 0 tie the C
        // scalar branch chain produces -0 while the C SIMD min/max path
        // produces +0 (same divergence exists between the C paths). The values
        // are equal so downstream arithmetic is unaffected.
        let bounds = set_w(bv[0].abs(), bv[1].abs(), bv[2].abs(), bv[3].abs());
        let sc = lanes(sym_clamp_w(a, bounds));
        for i in 0..4 {
            let expected = ref_sym_clamp(av[i], bv[i].abs());
            assert!(
                sc[i] == expected,
                "sym_clamp_w lane {}: {} vs {}",
                i,
                sc[i],
                expected
            );
        }

        // Comparisons: mask representation differs per path (all-bits vs 1.0),
        // so check them semantically through blend_w.
        let gt = greater_than_w(a, b);
        let sel = lanes(blend_w(a, b, gt));
        for i in 0..4 {
            let expected = if av[i] > bv[i] { bv[i] } else { av[i] };
            assert!(sel[i].to_bits() == expected.to_bits(), "greater_than/blend lane {}", i);
        }

        let eq = equals_w(a, a);
        let sel = lanes(blend_w(a, b, eq));
        for i in 0..4 {
            assert!(sel[i].to_bits() == bv[i].to_bits(), "equals/blend lane {}", i);
        }

        // or of two masks selects where either condition holds
        let m = or_w(greater_than_w(a, b), equals_w(a, b));
        let sel = lanes(blend_w(a, b, m));
        for i in 0..4 {
            let expected = if av[i] >= bv[i] { bv[i] } else { av[i] };
            assert!(sel[i].to_bits() == expected.to_bits(), "or/blend lane {}", i);
        }

        // all_zero
        assert!(all_zero_w(zero_w()));
        assert!(all_zero_w(splat_w(0.0)));
        if av.iter().any(|x| *x != 0.0) {
            assert!(!all_zero_w(a));
        }

        // lane set
        let mut d = a;
        d.set(2, 42.5);
        assert!(d.get(0) == av[0] && d.get(1) == av[1] && d.get(2) == 42.5 && d.get(3) == av[3]);
    }
}

#[test]
fn v32_ops_match_scalar_reference() {
    use makepad_box3d::simd::*;

    set_random_seed(RAND_SEED + 1);

    for _ in 0..1000 {
        let ax = 4.0 * random_float();
        let ay = 4.0 * random_float();
        let az = 4.0 * random_float();
        let bx = 4.0 * random_float();
        let by = 4.0 * random_float();
        let bz = 4.0 * random_float();

        let a = load_v(&[ax, ay, az]);
        let b = load_v(&[bx, by, bz]);

        // Lane accessors
        assert!(get_x_v(a) == ax && get_y_v(a) == ay && get_z_v(a) == az);
        assert!(get_v(a, 0) == ax && get_v(a, 1) == ay && get_v(a, 2) == az);

        // Arithmetic: bit equality per lane
        let s = add_v(a, b);
        assert!(get_x_v(s).to_bits() == (ax + bx).to_bits());
        assert!(get_y_v(s).to_bits() == (ay + by).to_bits());
        assert!(get_z_v(s).to_bits() == (az + bz).to_bits());

        let d = sub_v(a, b);
        assert!(get_x_v(d).to_bits() == (ax - bx).to_bits());

        let m = mul_v(a, b);
        assert!(get_y_v(m).to_bits() == (ay * by).to_bits());

        let n = neg_v(a);
        assert!(get_z_v(n).to_bits() == (0.0f32 - az).to_bits());

        // cross product per the scalar formula
        let cr = cross_v(a, b);
        assert!(get_x_v(cr).to_bits() == (ay * bz - az * by).to_bits());
        assert!(get_y_v(cr).to_bits() == (az * bx - ax * bz).to_bits());
        assert!(get_z_v(cr).to_bits() == (ax * by - ay * bx).to_bits());

        let mc = modified_cross_v(a, b);
        assert!(get_x_v(mc).to_bits() == (ay * bz + az * by).to_bits());

        // Comparisons (low 3 lanes)
        assert!(any_less_3v(a, b) == (ax < bx || ay < by || az < bz));
        assert!(any_greater_3v(a, b) == (ax > bx || ay > by || az > bz));
        assert!(all_less_eq_3v(a, b) == (ax <= bx && ay <= by && az <= bz));
        assert!(any_less_eq_3v(a, b) == (ax <= bx || ay <= by || az <= bz));

        // abs/min/max
        let ab = abs_v(a);
        assert!(get_x_v(ab) == ax.abs() && get_y_v(ab) == ay.abs() && get_z_v(ab) == az.abs());
        let mn = min_v(a, b);
        assert!(get_x_v(mn) == if ax < bx { ax } else { bx });
        let mx = max_v(a, b);
        assert!(get_x_v(mx) == if ax > bx { ax } else { bx });
    }
}
