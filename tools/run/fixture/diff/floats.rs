// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

#[inline(never)]
fn sc(x: f64) -> (f64, f64) { (x.sin(), x.cos()) }
#[inline(never)]
fn sc32(x: f32) -> (f32, f32) { (x.sin(), x.cos()) }
fn work() {
    let mut acc = 0.0f64;
    let mut h = 0u64;
    for i in 0..2000 {
        let x = i as f64 * 0.0137 - 9.0;
        let (s, c) = sc(x);
        let (s2, c2) = sc32(x as f32);
        acc += s * c + (x * 0.5).tan().atan2(c) + (x.abs() + 1.0).ln() * s2 as f64 + c2 as f64;
        acc += (x * 0.01).exp().powf(1.3) + x.abs().sqrt().mul_add(c, s) + x.hypot(3.0).cbrt();
        h = h.rotate_left(5) ^ s.to_bits() ^ c.to_bits() ^ (s2.to_bits() as u64) ^ ((c2.to_bits() as u64) << 32);
    }
    d!("acc={:e} bits={:016x}", acc, acc.to_bits());
    d!("h={:016x}", h);
    let n = [f64::NAN, f64::INFINITY, -0.0, 0.0, 1e308, 5e-324, -1.5, 2.5, 0.49999999999999994];
    for v in n { d!("{:?} {} {} {} {} {:?} {:?}", v, v.floor(), v.ceil(), v.round(), v.trunc(), v.max(1.0), v.min(-1.0)); }
    for v in [300.7f64, -1.0, f64::NAN, 1e20, -1e20] { d!("{} {} {} {}", v as u8, v as i32, v as u64, v as i64); }
    let m = [[1.5f32, 2.0, -0.5], [0.25, -3.0, 4.0], [7.0, 0.125, 1.0]];
    let mut v = [1.0f32, 2.0, 3.0];
    for _ in 0..50 { let mut o = [0.0f32; 3]; for r in 0..3 { for c in 0..3 { o[r] += m[r][c] * v[c]; } } let n = (o[0]*o[0]+o[1]*o[1]+o[2]*o[2]).sqrt(); v = [o[0]/n, o[1]/n, o[2]/n]; }
    d!("{:?} {:08x}", v, v[0].to_bits());
    d!("{} {} {}", (0.1f32 + 0.2f32) as f64, 16777217i32 as f32, u64::MAX as f32);
    d!("{} {}", f64::from_bits(0x400921FB54442D18).sin(), (1e22f64).cos());
}
