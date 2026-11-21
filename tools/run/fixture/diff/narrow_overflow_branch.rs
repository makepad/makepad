// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
// A narrow signed add or sub checked with `?`: its overflow test is the sign of (x ^ r) & (y ^ r)
// (or (x ^ y) & (x ^ r)), an and / xor chain ending in a compare-and-branch, which reads the whole
// register. Relax once left the narrow result unextended there (backend low_bits_only), so two
// negative i32 operands came out as an overflow (None).
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

type Pt = [i32; 2];
const DIRS: [Pt; 4] = [[1, 0], [0, 1], [-1, 0], [0, -1]];

#[inline(never)]
fn add32(x: i32, y: i32) -> Option<i32> { Some(x.checked_add(y)?) }
#[inline(never)]
fn sub32(x: i32, y: i32) -> Option<i32> { Some(x.checked_sub(y)?) }
#[inline(never)]
fn add16(x: i16, y: i16) -> Option<i16> { Some(x.checked_add(y)?) }
#[inline(never)]
fn add8(x: i8, y: i8) -> Option<i8> { Some(x.checked_add(y)?) }
/// A grid step (a router's): both coordinates checked.
#[inline(never)]
fn moved([x, y]: Pt, dir: usize) -> Option<Pt> {
    let [dx, dy] = DIRS[dir];
    Some([x.checked_add(dx)?, y.checked_add(dy)?])
}

fn work() {
    use std::hint::black_box as bb;
    for (x, y) in [(-3, -1), (-1, -1), (3, -1), (-3, 1), (i32::MIN, -1), (i32::MIN + 1, -1), (i32::MAX, 1), (-1_000_000, -2_000_000)] {
        d!("i32 {x} {y}: {:?} {:?}", add32(bb(x), bb(y)), sub32(bb(x), bb(-y)));
    }
    for (x, y) in [(-3i16, -1i16), (i16::MIN, -1), (i16::MAX, 1), (-100, -200)] {
        d!("i16 {x} {y}: {:?}", add16(bb(x), bb(y)));
    }
    for (x, y) in [(-3i8, -1i8), (i8::MIN, -1), (i8::MAX, 1), (-60, -60), (-64, -64)] {
        d!("i8 {x} {y}: {:?}", add8(bb(x), bb(y)));
    }
    for p in [[-3, -7], [0, -1], [2, 3], [i32::MIN, 0]] {
        let steps: Vec<Option<Pt>> = (0..4).map(|dir| moved(bb(p), bb(dir))).collect();
        d!("moved {p:?}: {steps:?}");
    }
}
