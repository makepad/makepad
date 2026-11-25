/// The edit target: `hot` reaches it only through the call in `f`'s loop, `direct` calls it.
#[inline(never)]
pub fn g(x: u32) -> u32 { x.wrapping_mul(3).wrapping_add(1) }

/// Never inlined into a test: the test's own O3 code (and its cache entry) must not hold the call to `g`.
#[inline(never)]
pub fn f(n: u32) -> u32 {
    let mut sum = 0u32;
    for i in 0..n {
        sum = sum.wrapping_add(g(i));
    }
    sum
}

#[inline(never)]
pub fn g2(x: u32) -> u32 { x.wrapping_add(7) }

#[inline(never)]
pub fn g2_alt(x: u32) -> u32 { x.wrapping_mul(2) }

/// The same loop through a fn pointer table, a call O3 cannot devirtualise.
#[inline(never)]
pub fn f2(n: u32, pick: &[fn(u32) -> u32; 2]) -> u32 {
    let mut sum = 0u32;
    for i in 0..n {
        sum = sum.wrapping_add(pick[(i & 1) as usize](i));
    }
    sum
}

/// A value `hot` also checks: its edit to an equal value reaches `hot` and changes no outcome.
pub fn h() -> u32 { 10 }

/// Enough work in `f`'s own code per run (about 10M RIR instructions) for PGO to build it at O3
/// rather than park it in its counted code (PGO_MIN_WORK: 1M of them a second over a window).
#[test]
fn hot() {
    for _ in 0..2000 {
        assert_eq!(f(1000), 1499500);
    }
    assert_eq!(h(), 10);
}

#[test]
fn hot_indirect() {
    let pick: [fn(u32) -> u32; 2] = [g2, g2_alt];
    for _ in 0..100 {
        assert_eq!(f2(1000, &pick), 753000);
    }
    assert_eq!(h(), 10);
}

/// Reaches `g` through its entry wrapper: an edit of `g` is reached by some test, so the session
/// selects by reach (with no test reaching it, it would rerun every test).
#[test]
fn direct() { assert_eq!(g(5), 16); }

#[test]
fn other() { assert_eq!(2 + 2, 4); }
