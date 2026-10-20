// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (O1, baseline, O3) must print identical D lines.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

// dense matches (switch / jump tables) of every shape the baseline (O1, quick register allocation) lowers
#[derive(Clone, Copy)]
enum Kind { A, B(u8), C(i32, i32), D { x: u16 }, E, F(bool), G, H(u8, u8) }
#[inline(never)]
fn dense(n: u32) -> u32 {
    match n { 0 => 17, 1 => 3, 2 => 99, 3 => 5, 4 => 1000, 5 => 0, 6 => 41, 7 => 7, 8 => 64, 9 => 12, 10 => 8, 11 => 301, _ => 1 }
}
#[inline(never)]
fn holes(n: u8) -> i64 {
    match n { 1 => -1, 3 => 30, 4 => 40, 7 => -70, 9 | 10 => 90, 12 => 120, 15..=17 => 150, 20 => 200, 255 => i64::MAX, _ => 0 }
}
#[inline(never)]
fn signed(n: i32) -> &'static str {
    match n { -3 => "m3", -2 => "m2", -1 => "m1", 0 => "z", 1 => "p1", 2 => "p2", 3 => "p3", 4 => "p4", 5 => "p5", i32::MIN => "min", _ => "other" }
}
#[inline(never)]
fn wide(n: u64) -> u64 {
    match n { 0 => 1, 1 => 2, 2 => 4, 3 => 8, 4 => 16, 5 => 32, 6 => 64, 7 => 128, 1099511627776 => 9, u64::MAX => 11, _ => n ^ 0xABCD }
}
#[inline(never)]
fn kind(k: Kind, salt: u32) -> u32 {
    match k {
        Kind::A => salt, Kind::B(b) => b as u32 + salt, Kind::C(a, b) => (a.wrapping_mul(b)) as u32, Kind::D { x } => x as u32 * 3,
        Kind::E => 0xE, Kind::F(t) => if t { 1 } else { 2 }, Kind::G => salt.rotate_left(4), Kind::H(a, b) => (a as u32) << 8 | b as u32,
    }
}
#[inline(never)]
fn chars(c: char) -> u8 { match c { 'a' => 1, 'b' => 2, 'c' => 3, 'd' => 4, 'e' => 5, 'f' => 6, 'g' => 7, 'h' => 8, '0'..='9' => 9, ' ' => 10, '\n' => 11, _ => 0 } }
#[inline(never)]
fn float_in_arm(n: u8, x: f64) -> f64 {
    match n { 0 => x.sin(), 1 => x.cos(), 2 => x * 2.0, 3 => x.sqrt(), 4 => -x, 5 => x.abs(), 6 => x + 1.5, 7 => x.floor(), _ => 0.0 }
}
fn work() {
    let mut a = 0u64;
    for i in 0..200u32 { a = a.wrapping_mul(31).wrapping_add(dense(i % 16) as u64); }
    d!("dense {}", a);
    d!("holes {:?}", (0..=255u8).step_by(5).map(holes).collect::<Vec<_>>());
    d!("signed {:?}", [-4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6, i32::MIN, i32::MAX].map(signed));
    d!("wide {:?}", [0u64, 1, 2, 3, 4, 5, 6, 7, 8, 1099511627776, u64::MAX, 12345].map(wide));
    let ks = [Kind::A, Kind::B(200), Kind::C(-7, 99999), Kind::D { x: 65535 }, Kind::E, Kind::F(true), Kind::F(false), Kind::G, Kind::H(3, 250)];
    d!("kind {:?}", ks.iter().map(|k| kind(*k, 0xF00D)).collect::<Vec<_>>());
    d!("chars {:?}", "ab9 \nzh0c".chars().map(chars).collect::<Vec<_>>());
    d!("fl {:?}", (0..10u8).map(|n| float_in_arm(n, 2.5)).collect::<Vec<_>>());
    let mut st = 0u8; let mut out = Vec::new();
    for i in 0..60u32 { st = match st { 0 => 1, 1 => 2, 2 => if i % 3 == 0 { 3 } else { 0 }, 3 => 4, 4 => 5, 5 => 6, 6 => if i % 2 == 0 { 7 } else { 1 }, _ => 0 }; out.push(st); }
    d!("fsm {:?}", out);
}
