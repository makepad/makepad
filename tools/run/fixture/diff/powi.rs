// diff_fast corpus program: f32/f64 powi with constant and variable exponents (the exponents 2, 3, 4, -1, -2 take
// straight-line paths in Relax's core), bits compared against rustc's powi over normal, subnormal, huge, NaN, inf and zero values.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

#[inline(never)]
fn consts64(x: f64) -> [u64; 8] { [x.powi(2).to_bits(), x.powi(3).to_bits(), x.powi(4).to_bits(), x.powi(-1).to_bits(), x.powi(-2).to_bits(), x.powi(0).to_bits(), x.powi(1).to_bits(), x.powi(5).to_bits()] }
#[inline(never)]
fn consts32(x: f32) -> [u32; 8] { [x.powi(2).to_bits(), x.powi(3).to_bits(), x.powi(4).to_bits(), x.powi(-1).to_bits(), x.powi(-2).to_bits(), x.powi(0).to_bits(), x.powi(1).to_bits(), x.powi(5).to_bits()] }
#[inline(never)]
fn var64(x: f64, n: i32) -> u64 { x.powi(n).to_bits() }
#[inline(never)]
fn var32(x: f32, n: i32) -> u32 { x.powi(n).to_bits() }
fn work() {
    let mut xs: Vec<f64> = vec![0.0, -0.0, 1.0, -1.0, 2.0, -3.5, 0.1, 1e-310, -1e-320, 1e154, 1e200, f64::MAX, f64::MIN_POSITIVE, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, 3.0000000000000004];
    let mut s = 0x2545f4914f6cdd1du64;
    for _ in 0..300 { s ^= s << 13; s ^= s >> 7; s ^= s << 17; xs.push(f64::from_bits(s)); }
    let (mut h, mut g, mut hv, mut gv) = (0u64, 0u32, 0u64, 0u32);
    for &x in &xs {
        for b in consts64(x) { h = h.wrapping_mul(1000003).wrapping_add(b); }
        for b in consts32(x as f32) { g = g.wrapping_mul(1000003).wrapping_add(b); }
        for n in -6..=7 { hv = hv.wrapping_mul(1000003).wrapping_add(var64(x, n)); gv = gv.wrapping_mul(1000003).wrapping_add(var32(x as f32, n) as u32 as u64 as u32); }
    }
    d!("consts f64 {:#x} f32 {:#x}", h, g);
    d!("variable f64 {:#x} f32 {:#x}", hv, gv);
}
