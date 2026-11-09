use makepad_dynamics::{
    core_smooth::{factor_i, solve_ld},
    types::MINVAL,
    util_solve::{chol_factor, chol_solve},
};

#[test]
fn chain_inertia_factorization_and_solves() {
    // C's ascending-column lower CSR for dof_parentid = [-1, 0, 1].
    let rownnz = [1, 2, 3];
    let rowadr = [0, 1, 3];
    let colind = [0, 0, 1, 0, 1, 2];
    let mut mat = [4.0, 1.0, 3.0, 0.5, -0.25, 2.0];
    let mut dense = [4.0, 1.0, 0.5, 1.0, 3.0, -0.25, 0.5, -0.25, 2.0];
    let mut diaginv = [0.0; 3];
    assert_eq!(
        factor_i(
            &mut mat,
            Some(&mut diaginv),
            3,
            &rownnz,
            &rowadr,
            &colind,
            None
        ),
        -1,
        "SPD chain needs no pivot clamp"
    );
    assert_eq!(chol_factor(&mut dense, 3, MINVAL), 3, "dense Cholesky rank");
    let rhs = [1.0, -2.0, 3.0, 0.0, 4.0, 0.0];
    let mut multiple = rhs;
    solve_ld(
        &mut multiple,
        &mat,
        &diaginv,
        3,
        2,
        &rownnz,
        &rowadr,
        &colind,
        None,
    );
    for column in 0..2 {
        let mut expected = [0.0; 3];
        chol_solve(&mut expected, &dense, &rhs[3 * column..], 3);
        let mut single = [rhs[3 * column], rhs[3 * column + 1], rhs[3 * column + 2]];
        solve_ld(
            &mut single,
            &mat,
            &diaginv,
            3,
            1,
            &rownnz,
            &rowadr,
            &colind,
            None,
        );
        for i in 0..3 {
            assert!(
                (single[i] - expected[i]).abs() < 1e-12,
                "single RHS {column}, dof {i}"
            );
            assert!(
                (multiple[3 * column + i] - expected[i]).abs() < 1e-12,
                "multiple RHS {column}, dof {i}"
            );
        }
    }
}
