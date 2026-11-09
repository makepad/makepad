use makepad_dynamics::derivative::d_sub_quat;
use makepad_dynamics::util_spatial::{quat_integrate, sub_quat};

#[test]
fn sub_quat_derivatives_match_spatial_difference() {
    let qa = [0.6, 0.0, 0.0, 0.8];
    let qb = [1.0, 0.0, 0.0, 0.0];
    let mut da = [0.0; 9];
    let mut db = [0.0; 9];
    d_sub_quat(&qa, &qb, Some(&mut da), Some(&mut db));
    // C hand trace: the normalized axis is [0, 0, 1], so K[8] and
    // (K*K)[8] are zero. Da_tmp[8] starts at 1, and both additions are zero.
    assert_eq!(da[8].to_bits(), 1.0_f64.to_bits(), "C axial derivative");
    let eps = 1e-7;
    for input in 0..2 {
        for col in 0..3 {
            let mut direction = [0.0; 3];
            direction[col] = 1.0;
            let mut plus = if input == 0 { qa } else { qb };
            let mut minus = plus;
            quat_integrate(&mut plus, &direction, eps);
            quat_integrate(&mut minus, &direction, -eps);
            let mut y_plus = [0.0; 3];
            let mut y_minus = [0.0; 3];
            if input == 0 {
                sub_quat(&mut y_plus, &plus, &qb);
                sub_quat(&mut y_minus, &minus, &qb);
            } else {
                sub_quat(&mut y_plus, &qa, &plus);
                sub_quat(&mut y_minus, &qa, &minus);
            }
            let analytic = if input == 0 { &da } else { &db };
            for row in 0..3 {
                let numerical = (y_plus[row] - y_minus[row]) / (2.0 * eps);
                assert!(
                    (analytic[3 * row + col] - numerical).abs() < 1e-6,
                    "input {input}, row {row}, column {col}: analytic {}, numerical {numerical}",
                    analytic[3 * row + col]
                );
            }
        }
    }
}
