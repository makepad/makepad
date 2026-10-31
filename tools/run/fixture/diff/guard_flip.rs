// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
// Guards (libs/relax/compiler/src/guards.rs): every indirect call site here changes its target while it
// runs -- monomorphic, then a different single target, then bimorphic, then megamorphic, per round and inside a
// round -- so the profile, the guard, the guard's failure path, the failure limit's relink, the re-profile and the
// despecialisation all run (diff_fast's O3 tier sets small MAKEPAD_RUN_GUARDS_READY / _FAILS thresholds).
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn(u32)) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..6 { f(r); if r < 5 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

trait Shape { fn area(&self, k: u64) -> u64; fn name(&self) -> &'static str; }
struct Sq(u64);
struct Rect(u64, u64);
struct Tri(u64, u64);
impl Shape for Sq { fn area(&self, k: u64) -> u64 { self.0 * self.0 + k } fn name(&self) -> &'static str { "sq" } }
impl Shape for Rect { fn area(&self, k: u64) -> u64 { self.0 * self.1 ^ k } fn name(&self) -> &'static str { "rect" } }
impl Shape for Tri { fn area(&self, k: u64) -> u64 { (self.0 * self.1 / 2).wrapping_sub(k) } fn name(&self) -> &'static str { "tri" } }

// the dyn site: one call per element
#[inline(never)]
fn total(v: &[&dyn Shape], k: u64) -> u64 {
    let mut s = 0u64;
    for (i, x) in v.iter().enumerate() { s = s.wrapping_mul(31).wrapping_add(x.area(k + i as u64)); }
    s
}

fn f_add(x: u64) -> u64 { x.wrapping_add(7) }
fn f_mul(x: u64) -> u64 { x.wrapping_mul(3) }
fn f_xor(x: u64) -> u64 { x ^ 0x5a5a }

// the fn-pointer site
#[inline(never)]
fn apply_n(f: fn(u64) -> u64, n: u32, x: u64) -> u64 {
    let mut x = x;
    for _ in 0..n { x = f(x); }
    x
}

// a boxed closure site whose closure changes
#[inline(never)]
fn drive(f: &mut dyn FnMut(u64) -> u64, n: u64) -> u64 {
    let mut a = 0u64;
    for i in 0..n { a = a.wrapping_add(f(i)); }
    a
}

fn work(round: u32) {
    let (sq, re, tr) = (Sq(3 + round as u64), Rect(4, 5 + round as u64), Tri(6, 7));
    let only_a: Vec<&dyn Shape> = (0..200).map(|_| &sq as &dyn Shape).collect();
    let only_b: Vec<&dyn Shape> = (0..200).map(|_| &re as &dyn Shape).collect();
    let mixed2: Vec<&dyn Shape> = (0..200).map(|i| if i % 3 == 0 { &re as &dyn Shape } else { &sq as &dyn Shape }).collect();
    let mixed3: Vec<&dyn Shape> = (0..200).map(|i| match i % 3 { 0 => &sq as &dyn Shape, 1 => &re, _ => &tr }).collect();
    // round-dependent phase order: the guard built on one round's profile meets another target
    let phases: [&Vec<&dyn Shape>; 4] = match round % 3 { 0 => [&only_a, &only_a, &only_b, &mixed2], 1 => [&only_b, &mixed2, &only_a, &mixed3], _ => [&mixed3, &only_a, &only_a, &only_b] };
    let mut h = 0u64;
    for (p, v) in phases.iter().enumerate() {
        for rep in 0..50u64 { h = h.wrapping_mul(1_000_003).wrapping_add(total(v, rep + p as u64)); }
        d!("round {} phase {} {} {}", round, p, v[0].name(), h);
    }
    // fn pointer: the same site sees one function, then another, then alternates
    let fs: [fn(u64) -> u64; 3] = [f_add, f_mul, f_xor];
    let mut x = round as u64;
    for step in 0..60u32 {
        let f = match (round, step) { (0, _) => fs[0], (1, s) if s < 30 => fs[0], (1, _) => fs[1], (_, s) => fs[(s % 3) as usize] };
        x = apply_n(f, 100, x);
    }
    d!("round {} fnptr {}", round, x);
    // closures behind &mut dyn FnMut: the captured state differs, the closure type flips mid-run
    let mut acc = 1u64;
    let mut c1 = |i: u64| { acc = acc.wrapping_mul(3).wrapping_add(i); acc };
    let a1 = drive(&mut c1, 500);
    let mut cnt = 0u64;
    let mut c2 = |i: u64| { cnt += 1; i * cnt };
    let a2 = drive(&mut c2, 500);
    let a3 = drive(&mut c1, 500);
    d!("round {} closures {} {} {}", round, a1, a2, a3);
}
