use makepad_dynamics::{inline, sort, util_blas as blas, util_spatial as spatial};

#[test]
fn seeded_sort_and_partial_prefix() {
    let mut seed = 0x6d2b79f5_u32;
    let mut input = Vec::with_capacity(1000);
    for index in 0..1000 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        input.push(((seed % 13) as i32, index));
    }
    for n in [0, 1, 15, 16, 17, 100, 1000] {
        let mut arr = input[..n].to_vec();
        let mut buf = vec![(0, 0); n];
        let mut cmp = |a: &(i32, usize), b: &(i32, usize)| a.0 - b.0;
        sort::sort(&mut arr, &mut buf, n, &mut cmp);
        for pair in arr.windows(2) {
            assert!(pair[0].0 <= pair[1].0, "keys sorted, n={n}");
            if pair[0].0 == pair[1].0 {
                assert!(pair[0].1 < pair[1].1, "equal keys stable, n={n}");
            }
        }
        // Integers and copied pairs are exact; no numerical tolerance is appropriate.
        let mut expected = input[..n].to_vec();
        expected.sort_by_key(|v| v.0);
        assert_eq!(arr, expected, "full sorted permutation, n={n}");
        for k in [0, 1, n / 2, n] {
            let mut partial = input[..n].to_vec();
            sort::partial_sort(&mut partial, &mut buf, n, k, &mut cmp);
            if k > n {
                assert_eq!(partial, input[..n], "invalid k leaves input unchanged");
                continue;
            }
            // The reference partial heap sort is not stable. Its selected keys must match.
            for i in 0..k {
                assert_eq!(partial[i].0, arr[i].0, "partial key prefix, n={n}, k={k}");
                assert_eq!(
                    partial[i], input[partial[i].1],
                    "selected pair is unmodified"
                );
                assert!(
                    !partial[..i].contains(&partial[i]),
                    "no repeated selected element"
                );
            }
            // Breaking ties by original index makes the selected pair prefix unique.
            let mut partial = input[..n].to_vec();
            sort::partial_sort(&mut partial, &mut buf, n, k, &mut |a, b| {
                if a.0 != b.0 {
                    a.0 - b.0
                } else {
                    a.1 as i32 - b.1 as i32
                }
            });
            assert_eq!(
                partial[..k],
                arr[..k],
                "total-order partial prefix, n={n}, k={k}"
            );
        }
    }
}

#[test]
fn quaternion_rotation_and_symmetric_eigen_round_trips() {
    let mut axis = [2.0, -3.0, 5.0];
    blas::normalize3(&mut axis);
    for angle in [0.0, 0.3, -1.7, 2.8] {
        let mut quat = [0.0; 4];
        spatial::axis_angle2quat(&mut quat, &axis, angle);
        let mut mat = [0.0; 9];
        spatial::quat2mat(&mut mat, &quat);
        let mut recovered = [0.0; 4];
        spatial::mat2quat(&mut recovered, &mat);
        let sign = if blas::dot(&quat, &recovered, 4) < 0.0 {
            -1.0
        } else {
            1.0
        };
        for i in 0..4 {
            assert!(
                (quat[i] - sign * recovered[i]).abs() < 1e-14,
                "matrix quaternion round trip"
            );
        }
        let mut inverse = [0.0; 4];
        let mut product = [0.0; 4];
        spatial::neg_quat(&mut inverse, &quat);
        spatial::mul_quat(&mut product, &quat, &inverse);
        for i in 0..4 {
            assert!(
                (product[i] - if i == 0 { 1.0 } else { 0.0 }).abs() < 1e-14,
                "quaternion inverse"
            );
        }
        let mut vel = [0.0; 3];
        spatial::quat2vel(&mut vel, &quat, 1.0);
        for i in 0..3 {
            assert!(
                (vel[i] - axis[i] * angle).abs() < 1e-14,
                "axis angle velocity round trip"
            );
        }
        let mut inline_mat = [0.0; 9];
        inline::quat2mat(&mut inline_mat, &quat);
        // Both paths execute identical scalar arithmetic, so their bits must agree.
        assert_eq!(
            mat.map(f64::to_bits),
            inline_mat.map(f64::to_bits),
            "inline quaternion matrix parity"
        );
        if angle == 0.0 {
            // Identity takes the exact constant-assignment branch, including positive zeros.
            assert_eq!(
                quat.map(f64::to_bits),
                [1.0_f64, 0.0, 0.0, 0.0].map(f64::to_bits)
            );
            assert_eq!(
                mat.map(f64::to_bits),
                [1.0_f64, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0].map(f64::to_bits)
            );
        }
    }
    let mat = [4.0, 1.0, -2.0, 1.0, 3.0, 0.5, -2.0, 0.5, 6.0];
    let mut values = [0.0; 3];
    let mut vectors = [0.0; 9];
    let mut quat = [0.0; 4];
    spatial::eig3(&mut values, &mut vectors, &mut quat, &mat);
    assert!(
        values[0] >= values[1] && values[1] >= values[2],
        "descending eigenvalues"
    );
    for r in 0..3 {
        for c in 0..3 {
            let mut reconstructed = 0.0;
            for k in 0..3 {
                reconstructed += vectors[r * 3 + k] * values[k] * vectors[c * 3 + k];
            }
            assert!(
                (reconstructed - mat[r * 3 + c]).abs() < 1e-12,
                "symmetric eigen reconstruction at {r},{c}"
            );
        }
    }
}

#[test]
fn miscellaneous_pyramids_halton_spring_and_sparse_maps() {
    use makepad_dynamics::util_misc as misc;

    let force = [10.0, 0.25, -0.5, 0.125, -0.25, 0.5];
    let mu = [0.5, 0.75, 0.25, 0.5, 1.0];
    for dim in [3, 6] {
        let mut pyramid = [0.0; 10];
        let mut decoded = [0.0; 6];
        misc::encode_pyramid(&mut pyramid, &force, &mu, dim);
        misc::decode_pyramid(&mut decoded, &pyramid, &mu, dim);
        for i in 0..dim {
            assert!(
                (decoded[i] - force[i]).abs() < 1e-14,
                "pyramid round trip: dim={dim}, component={i}"
            );
        }
    }

    let base2 = [0.5_f64, 0.25, 0.75, 0.125, 0.625, 0.375, 0.875, 0.0625];
    // Hand trace of C's digit loop: f1=0x1.5555555555555p-2,
    // f2=round(f1/3)=0x1.c71c71c71c71cp-4. For indices 1..8,
    // hn is f1, 2*f1, f2, f1+f2, 2*f1+f2, 2*f2, f1+2*f2, 2*f1+2*f2.
    // Each multiply and then each add rounds separately: index 5 is one ULP
    // below correctly rounded 7/9. Hex values below are IEEE-754 binary64 bits.
    let base3 = [
        0x3fd5_5555_5555_5555,
        0x3fe5_5555_5555_5555,
        0x3fbc_71c7_1c71_c71c,
        0x3fdc_71c7_1c71_c71c,
        0x3fe8_e38e_38e3_8e38,
        0x3fcc_71c7_1c71_c71c,
        0x3fe1_c71c_71c7_1c72,
        0x3fec_71c7_1c71_c71c,
    ];
    for i in 1..=8 {
        assert_eq!(
            misc::halton(i as i32, 2).to_bits(),
            base2[i - 1].to_bits(),
            "base-2 Halton index {i}"
        );
        assert_eq!(
            misc::halton(i as i32, 3).to_bits(),
            base3[i - 1],
            "base-3 Halton index {i}"
        );
    }

    // All closed forms give pos(0)=pos0. Overdamped roots are exactly -1,-2
    // (and 2,1 for negative b), so this also needs no rounding tolerance.
    for (k, b) in [(2.0, 3.0), (2.0, -3.0), (4.0, 4.0), (5.0, 2.0)] {
        assert_eq!(
            misc::spring_damper(2.0, 3.0, k, b, 0.0).to_bits(),
            2.0_f64.to_bits(),
            "spring-damper initial position: k={k}, b={b}"
        );
    }

    // Source pattern (numbers are storage indices):
    // [ 0  .  1  . ]
    // [ 2  3  .  4 ]
    // [ .  5  6  . ]
    // [ 7  .  8  9 ]
    let src_rowadr = [0, 2, 5, 7];
    let src_rownnz = [2, 3, 2, 3];
    let src_colind = [0, 2, 0, 1, 3, 1, 2, 0, 2, 3];
    // Subset pattern:
    // [ .  .  x  . ]
    // [ x  .  .  x ]
    // [ .  x  x  . ]
    // [ .  .  x  . ]
    let mut map = [-1; 6];
    misc::sparse_map(
        &mut map,
        4,
        &[0, 1, 3, 5],
        &[1, 2, 2, 1],
        &[2, 0, 3, 1, 2, 2],
        &src_rowadr,
        &src_rownnz,
        &src_colind,
    );
    assert_eq!(map, [1, 2, 4, 5, 6, 8], "sparse subset gather map");

    // Dense symmetric destination: reflect only the source's lower triangle.
    // Source entries 1 and 4 are above the diagonal and must not be used.
    let mut sym_map = [0; 16];
    let mut cursor = [0; 4];
    misc::lower2_sym_map(
        &mut sym_map,
        4,
        &[0, 4, 8, 12],
        &[4, 4, 4, 4],
        &[0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3],
        &src_rowadr,
        &src_rownnz,
        &src_colind,
        &mut cursor,
    );
    assert_eq!(
        sym_map,
        [0, 2, -1, 7, 2, 3, 5, -1, -1, 5, 6, 8, 7, -1, 8, 9,],
        "symmetric lower-triangle map with masked absent entries"
    );
}
