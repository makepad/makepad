// min/max/mul_add semantics vs rustc
fn mm(a: f64, b: f64) -> (f64, f64) {
    (a.min(b), a.max(b))
}

#[test]
fn min_max_nan() {
    let nan = f64::NAN;
    let cases = [(1.0, 2.0), (2.0, 1.0), (nan, 1.0), (1.0, nan), (-3.5, -3.5), (1e300, -1e300)];
    let mut h = 0u64;
    for i in 0..cases.len() {
        let (a, b) = cases[i];
        let (lo, hi) = mm(a, b);
        h = (h ^ lo.to_bits()).wrapping_mul(0x100000001b3);
        h = (h ^ hi.to_bits()).wrapping_mul(0x100000001b3);
        let lf = (a as f32).min(b as f32);
        h = (h ^ lf.to_bits() as u64).wrapping_mul(0x100000001b3);
    }
    println!("minmax hash {:x}", h);
    assert!(nan.min(1.0) == 1.0 && 1.0f64.min(nan) == 1.0);
    let f = 0.1f64.mul_add(10.0, -1.0);
    println!("fma {:e}", f);
}
