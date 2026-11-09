use makepad_dynamics::core_constraint::{getimpedance, power};

#[test]
fn impedance_and_power() {
    let solimp = [0.9, 0.95, 0.001, 0.5, 2.0];
    // x = pos / .001. At x=0: (d0, 0); at x>=1: (d1, 0).
    // At x=.5, a=1/.5=2, y=2*.5^2=.5, y'=2*2*.5=2:
    // imp=.9+.5*(.95-.9)=.925; imp'=2*(.95-.9)/.001=100.
    // The C double subtraction .95-.9 rounds the derivative to the value below.
    for (pos, expected_imp, expected_derivative) in [
        (0.0, 0.9, 0.0),
        (0.0005, 0.925, 99.99999999999986),
        (0.001, 0.95, 0.0),
        (0.01, 0.95, 0.0),
    ] {
        let (mut imp, mut derivative) = (0.0, 0.0);
        getimpedance(&solimp, pos, 0.0, &mut imp, &mut derivative);
        assert!(
            (imp - expected_imp).abs() <= 1e-15,
            "impedance at {pos}: {imp}"
        );
        assert!(
            (derivative - expected_derivative).abs() <= 1e-15,
            "derivative at {pos}: {derivative}"
        );
    }
    assert_eq!(power(3.0, 1.0), 3.0, "linear shortcut");
    assert_eq!(power(3.0, 2.0), 9.0, "quadratic shortcut");
    assert_eq!(power(2.0, 3.0), 8.0, "general power");
}
