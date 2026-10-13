// partial moves: drop only what is left (B4), field reassignment after a move, pattern moves
static mut DROPS: u32 = 0;
static mut LOG: u64 = 0;
struct D(u32);
impl Drop for D {
    fn drop(&mut self) {
        unsafe { DROPS += 1; LOG = LOG * 10 + self.0 as u64; }
    }
}
struct W { a: D, b: D }
struct N { w: W, c: D }
enum E { One(D, D), Two { x: D }, Zero }
fn reset() { unsafe { DROPS = 0; LOG = 0; } }
fn drops() -> u32 { unsafe { DROPS } }
fn log() -> u64 { unsafe { LOG } }

fn take_tail(w: W) -> D { w.a }
fn take_b(w: W) -> D { let x = w.b; x }

#[test]
fn let_field() {
    reset();
    { let w = W { a: D(1), b: D(2) }; let x = w.a; drop(x); }
    assert_eq!(drops(), 2);
    assert_eq!(log(), 12);
}
#[test]
fn tail_of_param() {
    reset();
    let d = take_tail(W { a: D(1), b: D(2) });
    assert_eq!(drops(), 1);
    assert_eq!(d.0, 1);
    drop(d);
    assert_eq!(drops(), 2);
    reset();
    let d = take_b(W { a: D(1), b: D(2) });
    assert_eq!(log(), 1);
    drop(d);
    assert_eq!(drops(), 2);
}
#[test]
fn nested() {
    reset();
    {
        let n = N { w: W { a: D(1), b: D(2) }, c: D(3) };
        let x = n.w.b;
        assert_eq!(x.0, 2);
    }
    // x dropped first (declared later), then n.w.a, n.c
    assert_eq!(log(), 213);
}
#[test]
fn let_pattern() {
    reset();
    {
        let w = W { a: D(1), b: D(2) };
        let W { a, .. } = w;
        assert_eq!(a.0, 1);
    }
    assert_eq!(drops(), 2);
    assert_eq!(log(), 12);
}
#[test]
fn match_moves() {
    reset();
    {
        let w = W { a: D(1), b: D(2) };
        match w {
            W { b, .. } => assert_eq!(b.0, 2),
        }
        assert_eq!(drops(), 1);
    }
    assert_eq!(log(), 21);
}
#[test]
fn enum_moves() {
    reset();
    {
        let e = E::One(D(1), D(2));
        if let E::One(x, _) = e {
            assert_eq!(x.0, 1);
        }
    }
    assert_eq!(log(), 12);
    reset();
    {
        let e = E::Two { x: D(5) };
        match e {
            E::One(a, _) => drop(a),
            E::Two { x } => drop(x),
            E::Zero => {}
        }
    }
    assert_eq!(log(), 5);
    reset();
    { let e = E::Zero; if let E::One(a, _) = e { drop(a); } }
    assert_eq!(drops(), 0);
}
#[test]
fn reassign_field() {
    reset();
    {
        let mut w = W { a: D(1), b: D(2) };
        let x = w.a;
        w.a = D(3);
        drop(x);
        assert_eq!(drops(), 1);
        w.b = D(4);
        assert_eq!(log(), 12);
    }
    assert_eq!(log(), 1234);
}
#[test]
fn in_loop() {
    reset();
    for i in 0..3 {
        let w = W { a: D(i), b: D(10) };
        if i == 1 {
            let x = w.a;
            drop(x);
        }
    }
    assert_eq!(drops(), 6);
}
#[test]
fn conditional_move() {
    reset();
    for i in 0..2 {
        let w = W { a: D(1), b: D(2) };
        if i == 0 {
            let _x = w.a;
        }
    }
    assert_eq!(drops(), 4);
}
#[test]
fn strings() {
    struct S { name: String, v: Vec<String> }
    let mut sv = Vec::new();
    sv.push("a".to_string());
    let s = S { name: "n".to_string(), v: sv };
    let name = s.name;
    assert_eq!(name.len(), 1);
    let v = s.v;
    assert_eq!(v.len(), 1);
}

// F26: a diverging first branch does not type the whole if / else-if chain as `!`
fn pick(a: Option<u32>, b: Option<u32>) -> u32 {
    let n = if let Some(_t) = a {
        return 0;
    } else if let Some(x) = b {
        x
    } else {
        return 1;
    };
    n + 10
}
#[test]
fn if_first_branch_diverges() {
    assert!(pick(None, Some(5)) == 15);
    assert!(pick(Some(1), None) == 0);
    assert!(pick(None, None) == 1);
}

// F27: an associated const through a generic bound, checked per instance (`T::ZERO`, T = u64)
trait Zr: Copy + PartialEq { const ZERO: Self; }
impl Zr for u64 { const ZERO: u64 = 0; }
impl Zr for u8 { const ZERO: u8 = 0; }
fn nonzero<T: Zr>(n: T) -> Option<T> { if n == T::ZERO { None } else { Some(n) } }
#[test]
fn assoc_const_via_bound() {
    assert!(nonzero(0u64).is_none());
    assert!(nonzero(3u64).is_some());
    assert!(nonzero(0u8).is_none());
    assert!(u64::ZERO == 0);
}

// `Self::Bits::EMPTY` in a trait default method: the prefix is a projection normalized per instance
mod bflib {
    pub trait Bits: Copy { const EMPTY: Self; const ALL: Self; }
    impl Bits for u32 { const EMPTY: u32 = 0; const ALL: u32 = !0; }
    pub trait Flags: Sized {
        type Bits: Bits;
        fn from_bits_retain(b: Self::Bits) -> Self;
        fn empty() -> Self { Self::from_bits_retain(Self::Bits::EMPTY) }
        fn all() -> Self { Self::from_bits_retain(<Self::Bits as Bits>::ALL) }
    }
}
use bflib::Flags;
struct BF(u32);
impl Flags for BF { type Bits = u32; fn from_bits_retain(b: u32) -> BF { BF(b) } }
#[test]
fn projection_assoc_const_in_default_method() { assert!(BF::empty().0 == 0); assert!(BF::all().0 == !0); }

// R4: #[cfg_attr(pred, attr)] is expanded before any attribute is read (rustc_expand config.rs)
extern "C" {
    #[cfg_attr(target_os = "macos", link_name = "abs")]
    #[cfg_attr(target_os = "linux", link_name = "abs")]
    fn my_abs(x: i32) -> i32;
}
#[test]
fn cfg_attr_link_name_applied() { assert!(unsafe { my_abs(-4) } == 4); }
