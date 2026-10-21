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
fn mix(a: u64, b: u64) -> u64 { (a ^ b).wrapping_mul(0x9E3779B97F4A7C15).rotate_left(23) }
fn work() {
    let vals: [i64; 8] = [0, 1, -1, i64::MAX, i64::MIN, 12345678901, -987654321, 255];
    let mut h = 0u64;
    for &a in &vals { for &b in &vals {
        d!("{} {} | {:?} {:?} {:?} {:?} {} {}", a, b, a.checked_add(b), a.checked_mul(b), a.overflowing_sub(b), b.checked_div(a), a.saturating_add(b), a.wrapping_mul(b));
        h = mix(h, a.wrapping_add(b) as u64);
    } }
    for s in [0u32, 1, 7, 31, 63, 64, 65] { d!("{} {} {} {}", 1u64.wrapping_shl(s), (-8i64).wrapping_shr(s), u64::MAX.checked_shl(s).is_some(), 0x8000u16.rotate_right(s)); }
    for x in [0u32, 1, 0x80, 0xF0F0, u32::MAX] { d!("{} {} {} {} {}", x.leading_zeros(), x.trailing_zeros(), x.count_ones(), x.swap_bytes(), x.reverse_bits()); }
    let (a, b) = (0xFEDC_BA98_7654_3210_0123_4567_89AB_CDEFu128, 0x1234_5678_9ABC_DEF0u128);
    d!("{} {} {} {}", a.wrapping_mul(a), a / b, a % b, (a as i128).wrapping_neg() >> 7);
    d!("{} {}", (a as f64), (u128::MAX as f32));
    d!("{} {} {}", i32::MIN.wrapping_abs(), (i32::MIN).wrapping_div(-1), (-7i32).rem_euclid(3));
    d!("{} {} {}", 200u8 as i8, -1i32 as u64, 3_000_000_000u32 as i32);
    d!("{:?} {:?}", 5u8.checked_sub(6), "99999999999".parse::<i32>().is_err());
    let mut x = 1u32; let mut s = 0u64; for i in 0..20000u32 { x = x.wrapping_mul(1103515245).wrapping_add(12345); s = s.wrapping_add((x >> 16) as u64 * (i as u64 | 1)); }
    d!("{} {}", s, h);
    let r = std::panic::catch_unwind(|| { let v = std::hint::black_box(200u8); v.checked_add(100).unwrap() }); d!("{}", r.is_err());
}
