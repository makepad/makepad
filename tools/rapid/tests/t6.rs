// float -> int `as` conversions (saturating, NaN -> 0) vs rustc
#[test]
fn conversions() {
    let vals = [0.0f64, -0.0, 1.5, -1.5, 2.5e9, -2.5e9, 9.3e18, -9.3e18, 1e300, -1e300, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 255.9, -1.0, 65535.7];
    let mut h = 0u64;
    for i in 0..vals.len() {
        let v = vals[i];
        h = (h ^ (v as i64) as u64).wrapping_mul(0x100000001b3);
        h = (h ^ (v as u64)).wrapping_mul(0x100000001b3);
        h = (h ^ (v as i32) as u64).wrapping_mul(0x100000001b3);
        h = (h ^ (v as u8) as u64).wrapping_mul(0x100000001b3);
        h = (h ^ ((v as f32) as i64) as u64).wrapping_mul(0x100000001b3);
        h = (h ^ (v as u16) as u64).wrapping_mul(0x100000001b3);
    }
    println!("conv hash {:x}", h);
}
