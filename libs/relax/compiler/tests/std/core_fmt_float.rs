// Differential Grisu/Dragon float formatting.
#[test]
fn a01_f64() {
    let mut state = 0x9e3779b97f4a7c15u64;
    let mut i = 0;
    while i < 256 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        observe_f64(f64::from_bits(state));
        i += 1;
    }
}
fn observe_f64(v: f64) {
    println!("| {} {:?} {:e} {:E} {:+} {:10.3}", v, v, v, v, v, v);
    println!("| {:.0} {:.0e}", v, v);
    println!("| {:.1} {:.1e}", v, v);
    println!("| {:.2} {:.2e}", v, v);
    println!("| {:.3} {:.3e}", v, v);
    println!("| {:.4} {:.4e}", v, v);
    println!("| {:.5} {:.5e}", v, v);
    println!("| {:.6} {:.6e}", v, v);
    println!("| {:.7} {:.7e}", v, v);
    println!("| {:.8} {:.8e}", v, v);
    println!("| {:.9} {:.9e}", v, v);
    println!("| {:.10} {:.10e}", v, v);
    println!("| {:.11} {:.11e}", v, v);
    println!("| {:.12} {:.12e}", v, v);
    println!("| {:.13} {:.13e}", v, v);
    println!("| {:.14} {:.14e}", v, v);
    println!("| {:.15} {:.15e}", v, v);
    println!("| {:.16} {:.16e}", v, v);
    println!("| {:.17} {:.17e}", v, v);
    println!("| {:.40} {:.40e}", v, v);
}
#[test]
fn a02_f32() {
    let mut state = 0x12345678u32;
    let mut i = 0;
    while i < 256 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        observe_f32(f32::from_bits(state));
        i += 1;
    }
}
fn observe_f32(v: f32) {
    println!("| {} {:?} {:e} {:E} {:+} {:10.3}", v, v, v, v, v, v);
    println!("| {:.0} {:.0e}", v, v);
    println!("| {:.1} {:.1e}", v, v);
    println!("| {:.2} {:.2e}", v, v);
    println!("| {:.3} {:.3e}", v, v);
    println!("| {:.4} {:.4e}", v, v);
    println!("| {:.5} {:.5e}", v, v);
    println!("| {:.6} {:.6e}", v, v);
    println!("| {:.7} {:.7e}", v, v);
    println!("| {:.8} {:.8e}", v, v);
    println!("| {:.9} {:.9e}", v, v);
    println!("| {:.10} {:.10e}", v, v);
    println!("| {:.11} {:.11e}", v, v);
    println!("| {:.12} {:.12e}", v, v);
    println!("| {:.13} {:.13e}", v, v);
    println!("| {:.14} {:.14e}", v, v);
    println!("| {:.15} {:.15e}", v, v);
    println!("| {:.16} {:.16e}", v, v);
    println!("| {:.17} {:.17e}", v, v);
    println!("| {:.40} {:.40e}", v, v);
}
#[test]
fn a03_edges() {
    for v in [0.0f64, -0.0, f64::from_bits(1), f64::MIN_POSITIVE, f64::MAX, f64::INFINITY, f64::NAN, 0.1 + 0.2, 0.5, 1.5, 2.5, 9.5, 99.5, 1e-4, 1e16, 1e23] { observe_f64(v); }
    for v in [0.0f32, -0.0, f32::from_bits(1), f32::MIN_POSITIVE, f32::MAX, f32::INFINITY, f32::NAN, 0.1 + 0.2, 0.5, 1.5, 2.5, 9.5, 99.5, 1e-4, 1e16, 1e23] { observe_f32(v); }
}
