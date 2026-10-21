// diff_fast corpus program: dense integer `match`es (jump tables, bit tests, binary search, ranges, or-patterns, guards),
// with constants near the 32-bit and 64-bit limits (the back ends encode only signed 32-bit compare immediates),
// signed and unsigned scrutinees. The first round runs the quick register allocation (O3 not yet installed).
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

#[inline(never)]
fn dense(x: u32) -> u32 {
    match x { 0 => 11, 1 => 22, 2 => 33, 3 | 4 => 44, 5..=9 => 55, 10 => 66, 12 => 77, 13 => 88, 14 => 99, 15..=20 => 110, 22 => 121, 23 => 132, 25 => 143, 26 => 154, _ => 7 }
}
#[inline(never)]
fn wide_u(x: u64) -> u64 {
    match x {
        0..=9 => 1,
        0x7fff_ffff => 2,
        0x8000_0000 => 3,
        0xffff_ffff => 4,
        0x1_0000_0000 => 5,
        0x7fff_ffff_ffff_ffa7..=0x7fff_ffff_ffff_ffd7 => 6,
        0x8000_0000_0000_0000..=0x8000_0000_0000_0010 => 7,
        0xffff_ffff_ffff_fff0..=u64::MAX => 8,
        _ => 0,
    }
}
#[inline(never)]
fn wide_i(x: i64) -> i64 {
    match x {
        i64::MIN..=-0x8000_0001 => -1,
        -0x8000_0000..=-1 => -2,
        0 => 10,
        1..=100 => 20,
        0x7fff_ffff => 30,
        0x8000_0000..=0x1_0000_0000 => 40,
        0x7fff_ffff_ffff_ff00..=i64::MAX => 50,
        _ => 60,
    }
}
#[inline(never)]
fn kind(c: u8) -> u8 {
    match c { b'0'..=b'9' => 1, b'a'..=b'z' => 2, b'A'..=b'Z' => 3, b' ' | b'\t' | b'\n' | b'\r' => 4, b'_' | b'-' => 5, 0x80..=0xbf => 6, 0xc0..=0xdf => 7, 0xe0..=0xef => 8, 0xf0..=0xff => 9, _ => 0 }
}
#[inline(never)]
fn guarded(x: i32, y: i32) -> i32 {
    match x { 0 if y > 0 => 1, 0 => 2, 1 | 2 | 3 if y < 0 => 3, 1..=3 => 4, 4..=8 => 5, 9 => 6, 100..=200 if y == 7 => 7, 100..=200 => 8, i32::MIN..=-1 => 9, _ => 10 }
}
fn work() {
    let mut h = 0u64;
    for x in 0..40u32 { h = h.wrapping_mul(31).wrapping_add(dense(x) as u64); }
    d!("dense {}", h);
    let mut probes: Vec<u64> = vec![0, 1, 9, 10, 0x7fff_fffe, 0x7fff_ffff, 0x8000_0000, 0x8000_0001, 0xffff_fffe, 0xffff_ffff, 0x1_0000_0000, 0x1_0000_0001, 0x7fff_ffff_ffff_ffa6, 0x7fff_ffff_ffff_ffa7, 0x7fff_ffff_ffff_ffd7, 0x7fff_ffff_ffff_ffd8, 0x8000_0000_0000_0000, 0x8000_0000_0000_0010, 0x8000_0000_0000_0011, 0xffff_ffff_ffff_ffef, 0xffff_ffff_ffff_fff0, u64::MAX];
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..200 { s ^= s << 13; s ^= s >> 7; s ^= s << 17; probes.push(s); }
    let (mut hu, mut hi) = (0u64, 0i64);
    for &p in &probes { hu = hu.wrapping_mul(131).wrapping_add(wide_u(p)); hi = hi.wrapping_mul(131).wrapping_add(wide_i(p as i64)); }
    d!("wide_u {} wide_i {}", hu, hi);
    for p in [0u64, 9, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff, 0x1_0000_0000, 0x7fff_ffff_ffff_ffb0, 0x8000_0000_0000_0008, u64::MAX] { d!("{:#x} -> {} {}", p, wide_u(p), wide_i(p as i64)); }
    let mut hk = 0u64;
    for c in 0..=255u8 { hk = hk.wrapping_mul(7).wrapping_add(kind(c) as u64); }
    d!("kind {}", hk);
    let mut hg = 0i64;
    for x in [-5, -1, 0, 1, 2, 3, 4, 8, 9, 10, 99, 100, 150, 200, 201, i32::MIN, i32::MAX] { for y in [-3, 0, 7, 9] { hg = hg.wrapping_mul(17).wrapping_add(guarded(x, y) as i64); } }
    d!("guarded {}", hg);
}
