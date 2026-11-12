// std::f64::consts and std::f32::consts have GOLDEN_RATIO and EULER_GAMMA (relax issue 70: missing,
// E0425), with the values real core gives them.

#[test]
fn golden_ratio_and_euler_gamma_are_float_consts() {
    use std::f32::consts as c32;
    use std::f64::consts as c64;
    // the issue's
    assert_eq!(format!("{} {}", c64::GOLDEN_RATIO, c32::GOLDEN_RATIO), "1.618033988749895 1.618034");
    assert_eq!(format!("{} {}", c64::EULER_GAMMA, c32::EULER_GAMMA), "0.5772156649015329 0.5772157");
    assert!((c64::GOLDEN_RATIO * c64::GOLDEN_RATIO - c64::GOLDEN_RATIO - 1.0).abs() < 1e-15);
    assert_eq!(c64::GOLDEN_RATIO.to_bits(), ((1.0 + 5f64.sqrt()) / 2.0).to_bits());
    assert_eq!(c32::GOLDEN_RATIO, c64::GOLDEN_RATIO as f32);
    assert_eq!(c32::EULER_GAMMA, c64::EULER_GAMMA as f32);
}
