// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
// 128-bit products of sign- and zero-extended 64-bit values (the high word of `a as i128 * b as i128` is one
// signed high multiply: backend passes.rs signed_mulhi), next to products whose high words are not those
// signs, and geometry's i128 cross products, orientations and sort keys.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Pt { x: i64, y: i64 }
#[inline(never)]
fn cross(a: Pt, b: Pt) -> i128 { a.x as i128 * b.y as i128 - a.y as i128 * b.x as i128 }
#[inline(never)]
fn orient(a: Pt, b: Pt, c: Pt) -> i128 {
    let (abx, aby) = ((b.x - a.x) as i128, (b.y - a.y) as i128);
    let (acx, acy) = ((c.x - a.x) as i128, (c.y - a.y) as i128);
    abx * acy - aby * acx
}
#[inline(never)]
fn smul(a: i64, b: i64) -> i128 { a as i128 * b as i128 }
#[inline(never)]
fn umul(a: u64, b: u64) -> u128 { a as u128 * b as u128 }
#[inline(never)]
fn square(a: i64) -> i128 { a as i128 * a as i128 }
// one side a full i128: its high word is not the low word's sign
#[inline(never)]
fn wide_times(a: i128, b: i64) -> i128 { a.wrapping_mul(b as i128) }
// a signed times an unsigned extension
#[inline(never)]
fn mixed(a: i64, b: u64) -> i128 { (a as i128).wrapping_mul(b as i128) }

fn work() {
    let vals: [i64; 12] = [0, 1, -1, 2, -3, i64::MAX, i64::MIN, i64::MIN + 1, 0x1234_5678_9abc_def0, -0x0fed_cba9_8765_4321, 1 << 32, -(1 << 31)];
    let mut h = 0u128;
    for &a in &vals {
        for &b in &vals {
            let (s, u, m) = (smul(a, b), umul(a as u64, b as u64), mixed(a, b as u64));
            d!("{} {} | {} {} {} {}", a, b, s, u, m, wide_times(s, b));
            h = h.wrapping_mul(31).wrapping_add(s as u128 ^ u);
        }
        d!("sq {} {}", a, square(a));
    }
    let pts: Vec<Pt> = (0..40i64).map(|i| Pt { x: (i * 7919 % 101 - 50) * 1_000_003, y: (i * 104729 % 97 - 48) * 999_983 }).collect();
    let (mut c, mut o) = (0i128, 0i128);
    for w in pts.windows(3) {
        c = c.wrapping_add(cross(w[0], w[1]));
        o = o.wrapping_add(orient(w[0], w[1], w[2]));
    }
    // a sort keyed by an i128 dot product, ties by point
    let dir = Pt { x: 3, y: -7 };
    let mut sorted = pts.clone();
    sorted.sort_unstable_by_key(|p| (p.x as i128 * dir.x as i128 + p.y as i128 * dir.y as i128, *p));
    let big = Pt { x: i64::MAX / 2, y: i64::MIN / 2 };
    d!("{} {} {} {:?} {}", c, o, h, &sorted[..3], cross(big, Pt { x: -3, y: i64::MAX }));
}
