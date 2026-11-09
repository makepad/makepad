use makepad_dynamics::{types::MINVAL, util_blas, util_solve::*, util_sparse};

#[test]
fn seeded_linear_systems_and_constrained_quadratics() {
    let mut seed = 0x6d2b79f5_u32;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f64 / u32::MAX as f64) - 0.5
    };
    let near = |actual: &[f64], expected: &[f64], property: &str| {
        for (i, (&a, &b)) in actual.iter().zip(expected).enumerate() {
            assert!((a - b).abs() < 1e-12, "{property}, entry {i}: {a} != {b}");
        }
    };

    // A = B*B' with positive diagonal B and a seeded sparse pattern.
    let mut b_mat = [0.0; 49];
    for i in 0..7 {
        b_mat[i * 7 + i] = 2.0 + random();
        for j in i.saturating_sub(2)..i {
            b_mat[i * 7 + j] = random();
        }
    }
    // This extra edge requires fill-in during reverse sparse factorization.
    b_mat[6 * 7] = random();
    let mut a = [0.0; 49];
    for i in 0..7 {
        for j in 0..7 {
            for k in 0..7 {
                a[i * 7 + j] += b_mat[i * 7 + k] * b_mat[j * 7 + k];
            }
        }
    }
    assert_eq!(a[3], 0.0, "sparse test includes structural zeros");
    let mut expected = [0.0; 7];
    for x in &mut expected {
        *x = random();
    }
    let mut rhs = [0.0; 7];
    util_blas::mul_mat_vec(&mut rhs, &a, &expected, 7, 7);
    let mut factor = a;
    assert_eq!(chol_factor(&mut factor, 7, MINVAL), 7, "dense SPD rank");
    let mut dense = [0.0; 7];
    chol_solve(&mut dense, &factor, &rhs, 7);
    near(&dense, &expected, "dense Cholesky solution");

    let mut values = [0.0; 49];
    let mut rownnz = [0; 7];
    let mut rowadr = [0; 7];
    let mut colind = [0; 49];
    assert_eq!(
        util_sparse::dense2sparse(
            &mut values,
            &a,
            7,
            7,
            &mut rownnz,
            &mut rowadr,
            &mut colind,
            49
        ),
        0,
        "sparse conversion fits"
    );
    let mut l_rownnz = [0; 7];
    let mut l_rowadr = [0; 7];
    let mut l_colind = [0; 49];
    let mut lt_rownnz = [0; 7];
    let mut lt_rowadr = [0; 7];
    let mut lt_colind = [0; 49];
    let mut lt_map = [0; 49];
    let mut symbolic_work = [0; 21];
    let nnz = chol_factor_symbolic(
        None,
        &mut l_rownnz,
        &mut l_rowadr,
        &mut lt_colind,
        &mut lt_rownnz,
        &mut lt_rowadr,
        &mut lt_map,
        &rownnz,
        &rowadr,
        &colind,
        7,
        None,
        &mut symbolic_work,
    );
    assert!(nnz >= 7 && nnz < 28, "sparse symbolic count retains zeros");
    assert_eq!(
        chol_factor_symbolic(
            Some(&mut l_colind),
            &mut l_rownnz,
            &mut l_rowadr,
            &mut lt_colind,
            &mut lt_rownnz,
            &mut lt_rowadr,
            &mut lt_map,
            &rownnz,
            &rowadr,
            &colind,
            7,
            None,
            &mut symbolic_work,
        ),
        0,
        "sparse symbolic fill"
    );
    let mut l = [0.0; 49];
    let mut work = [0.0; 7];
    assert_eq!(
        chol_factor_numeric(
            &mut l, 7, MINVAL, &l_rownnz, &l_rowadr, &l_colind, &lt_rownnz, &lt_rowadr, &lt_colind,
            &lt_map, &values, &rownnz, &rowadr, &colind, &mut work,
        ),
        7,
        "sparse SPD rank"
    );
    let mut sparse = [0.0; 7];
    chol_solve_sparse(&mut sparse, &l, &rhs, 7, &l_rownnz, &l_rowadr, &l_colind);
    near(&sparse, &dense, "sparse and dense Cholesky solutions");

    let mut update = [0.0; 7];
    for x in &mut update {
        *x = random() * 0.1;
    }
    let mut updated = factor;
    assert_eq!(
        chol_update(&mut updated, &mut update.clone(), 7, 1),
        7,
        "update rank"
    );
    assert_eq!(
        chol_update(&mut updated, &mut update.clone(), 7, 0),
        7,
        "downdate rank"
    );
    near(&updated, &factor, "Cholesky plus/minus restores factor");

    // The first five rows are banded; two dense rows exercise the arrowhead tail.
    let mut band = [0.0; 29]; // (7-2)*3 + 2*7
    dense2band(&mut band, &a, 7, 3, 2);
    assert!(
        chol_factor_band(&mut band, 7, 3, 2, 0.0, 0.0) > 0.0,
        "band SPD rank"
    );
    let mut band_solution = [0.0; 7];
    chol_solve_band(&mut band_solution, &band, &rhs, 7, 3, 2);
    near(&band_solution, &dense, "band and dense Cholesky solutions");

    let mut a6 = [0.0; 36];
    for i in 0..6 {
        for j in 0..6 {
            a6[i * 6 + j] = random();
        }
        a6[i * 6 + i] += 3.0;
    }
    let mut expected6 = [0.0; 6];
    for x in &mut expected6 {
        *x = random();
    }
    let mut rhs6 = [0.0; 6];
    util_blas::mul_mat_vec(&mut rhs6, &a6, &expected6, 6, 6);
    let mut lu = a6;
    let mut lu6 = a6;
    let mut pivot = [0; 6];
    let mut pivot6 = [0; 6];
    assert_eq!(factor_lu(&mut lu, 6, &mut pivot), 1, "LU nonsingular");
    assert_eq!(factor_lu6(&mut lu6, &mut pivot6), 1, "LU6 nonsingular");
    let mut x = [0.0; 6];
    let mut x6 = [0.0; 6];
    solve_lu(&mut x, &lu, &rhs6, &pivot, 6);
    solve_lu6(&mut x6, &lu6, &rhs6, &pivot6);
    near(&x, &expected6, "LU solution");
    near(&x6, &expected6, "LU6 solution");

    let d2 = [1.5, 0.75];
    let mut q2 = [0.0; 2];
    assert_eq!(
        qcqp2(&mut q2, &[2.0, 0.2, 0.2, 3.0], &[-4.0, 3.0], &d2, 0.8),
        1,
        "QCQP2 constrained"
    );
    let radius2 = (q2[0] / d2[0]).powi(2) + (q2[1] / d2[1]).powi(2);
    assert!(radius2 <= 0.8 * 0.8 + 1e-10, "QCQP2 ellipsoid: {radius2}");
    let a3 = [2.0, 0.2, 0.1, 0.2, 3.0, 0.3, 0.1, 0.3, 4.0];
    let b3 = [-4.0, 3.0, -5.0];
    let d3 = [1.5, 0.75, 2.0];
    let mut q3 = [0.0; 3];
    assert_eq!(qcqp3(&mut q3, &a3, &b3, &d3, 0.8), 1, "QCQP3 constrained");
    let radius3 = (q3[0] / d3[0]).powi(2) + (q3[1] / d3[1]).powi(2) + (q3[2] / d3[2]).powi(2);
    assert!(radius3 <= 0.8 * 0.8 + 1e-10, "QCQP3 ellipsoid: {radius3}");
    let mut qn = [0.0; 3];
    assert_eq!(qcqp(&mut qn, &a3, &b3, &d3, 0.8, 3), 1, "QCQPN constrained");
    near(&qn, &q3, "QCQPN and QCQP3 solutions");

    let h = [
        2.0, 0.0, 0.0, 0.0, 0.0, 4.0, 0.0, 0.0, 0.0, 0.0, 6.0, 0.0, 0.0, 0.0, 0.0, 8.0,
    ];
    let g = [-6.0, 8.0, -1.5, -24.0];
    let lower = [0.0, -1.0, 0.0, 0.0];
    let upper = [1.0, 1.0, 1.0, 2.0];
    let mut qp = [0.0; 4];
    let mut r = [0.0; 44];
    let mut index = [0; 4];
    let mut integer_work = [0; 12];
    assert_eq!(
        box_qp(
            &mut qp,
            &mut r,
            Some(&mut index),
            &h,
            &g,
            4,
            Some(&lower),
            Some(&upper),
            &mut integer_work
        ),
        1,
        "one free QP variable"
    );
    near(&qp, &[1.0, -1.0, 0.25, 2.0], "box QP hand-computed optimum");
    assert_eq!(index[0], 2, "free QP dimension");
}
