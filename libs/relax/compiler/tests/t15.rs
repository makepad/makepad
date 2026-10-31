// std only (MAKEPAD_RUN_STD=libs/relax/compiler/std): format machinery on the S2 ABI, std derives
// expanded by the parser, bit intrinsics, statics as places, stdout buffering, slice patterns
use std::fmt;
use std::fmt::Write as _;
use std::sync::OnceLock;

struct P { x: i32, y: i32 }
impl fmt::Display for P {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}
#[derive(Debug)]
struct D { a: u8, b: &'static str }

fn side(n: &mut i32) -> i32 { *n += 1; *n }

#[test]
fn simple() {
    let s = format!("a{}b{}c", 1, "two");
    assert!(s == "a1btwoc");
    let x = 5;
    assert_eq!(format!("{x}"), "5");
    assert_eq!(format!("{} {}", P { x: 1, y: -2 }, 3u8), "(1, -2) 3");
    assert_eq!(format!("{{}}"), "{}");
    assert_eq!(format!("no args"), "no args");
}

#[test]
fn specs() {
    assert_eq!(format!("{:>5}|", 42), "   42|");
    assert_eq!(format!("{:<5}|", 42), "42   |");
    assert_eq!(format!("{:^6}|", "ab"), "  ab  |");
    assert_eq!(format!("{:05}", -42), "-0042");
    assert_eq!(format!("{:x} {:X} {:#x} {:b} {:o}", 255, 255, 255, 5, 8), "ff FF 0xff 101 10");
    assert_eq!(format!("{:+}", 3), "+3");
    let w = 4;
    assert_eq!(format!("{:w$}|", 1), "   1|");
    assert_eq!(format!("{:1$}|", 1, 3), "  1|");
    assert_eq!(format!("{:*>4}", 7), "***7");
    assert_eq!(format!("{0} {0} {1}", "a", "b"), "a a b");
    assert_eq!(format!("{n} {m}", n = 1, m = 2), "1 2");
}

#[test]
fn debug() {
    let d = D { a: 1, b: "hi" };
    assert_eq!(format!("{:?}", d), "D { a: 1, b: \"hi\" }");
    assert_eq!(format!("{:?}", Some(3)), "Some(3)");
    assert_eq!(format!("{:?}", "q\""), "\"q\\\"\"");
}

#[test]
fn once() {
    let mut n = 0;
    let s = format!("{0}-{0}", side(&mut n));
    assert_eq!(s, "1-1");
    assert_eq!(n, 1);
}

#[test]
fn writes() {
    let mut s = String::new();
    write!(s, "{}+{}", 1, 2).unwrap();
    writeln!(s, "!").unwrap();
    assert_eq!(s, "1+2!\n");
    let v = vec![1, 2, 3];
    assert_eq!(v.len(), 3);
    let z = vec![0u8; 4];
    assert_eq!(z.len(), 4);
    let e: Vec<i32> = vec![];
    assert!(e.is_empty());
    println!("println works {}", 7);
}

#[test]
fn bits() {
    assert_eq!(0u32.leading_zeros(), 32);
    assert_eq!(1u32.leading_zeros(), 31);
    assert_eq!(0x8000_0000u32.leading_zeros(), 0);
    assert_eq!(1u64.leading_zeros(), 63);
    assert_eq!(0u8.leading_zeros(), 8);
    assert_eq!((-1i16).leading_zeros(), 0);
    assert_eq!(0u32.trailing_zeros(), 32);
    assert_eq!(8u32.trailing_zeros(), 3);
    assert_eq!(0u64.trailing_zeros(), 64);
    assert_eq!((1u64 << 63).trailing_zeros(), 63);
    assert_eq!(0xffu8.count_ones(), 8);
    assert_eq!((-1i64).count_ones(), 64);
    assert_eq!(0x1234_5678u32.swap_bytes(), 0x7856_3412);
    assert_eq!(0x12u8.swap_bytes(), 0x12);
    assert_eq!((0x0102i16).swap_bytes(), 0x0201);
    assert_eq!((0x0080i16).swap_bytes(), -32768);
    assert_eq!(1u8.reverse_bits(), 0x80);
    assert_eq!(1u32.reverse_bits(), 0x8000_0000);
    assert_eq!(0x0123_4567_89ab_cdefu64.reverse_bits(), 0xf7b3_d591_e6a2_c480);
    assert_eq!(1u128.leading_zeros(), 127);
    assert_eq!((1u128 << 70).leading_zeros(), 57);
    assert_eq!(0u128.leading_zeros(), 128);
    assert_eq!((1u128 << 70).trailing_zeros(), 70);
    assert_eq!(0u128.trailing_zeros(), 128);
    assert_eq!(u128::MAX.count_ones(), 128);
    assert!(1u128.swap_bytes() == 1u128 << 120);
    assert!(1u128.reverse_bits() == 1u128 << 127);
    println!("{} {:.3} {:e}", 1.5f64, 2.0f64 / 3.0, 1234.5f64);
}

static mut ODROPS: u32 = 0;
struct OD(u32);
impl Drop for OD { fn drop(&mut self) { unsafe { ODROPS += 1; } } }
static S: OnceLock<OD> = OnceLock::new();
fn odrops() -> u32 { unsafe { ODROPS } }
#[test]
fn static_receiver_not_copied() {
    let v = S.get_or_init(|| OD(5));
    assert_eq!(v.0, 5);
    assert_eq!(odrops(), 0);
    let v = S.get_or_init(|| OD(6));
    assert_eq!(v.0, 5);
    assert_eq!(odrops(), 0);
    let g = S.get();
    assert!(g.is_some());
    assert_eq!(odrops(), 0);
}

#[test]
fn print_sequences() {
    print!("x");
    print!("y\n");
    println!("{} {:?} {:>4}|", 1, "s", 2);
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum E2 { #[default] A, B(u8, String), C { x: i32 } }
#[derive(Debug, Default, PartialEq, PartialOrd)]
struct G<T: Clone> { t: T, v: Vec<T> }
#[test]
fn derives() {
    assert_eq!(format!("{:?}", E2::B(1, "s".to_string())), "B(1, \"s\")");
    assert_eq!(format!("{:?}", E2::C { x: -1 }), "C { x: -1 }");
    assert_eq!(format!("{:?}", E2::default()), "A");
    assert!(E2::A < E2::B(0, String::new()));
    assert!(E2::B(1, "a".to_string()) < E2::B(1, "b".to_string()));
    assert_eq!(E2::C { x: 1 }.cmp(&E2::C { x: 2 }), std::cmp::Ordering::Less);
    let g: G<u8> = G::default();
    assert_eq!(format!("{:?}", g), "G { t: 0, v: [] }");
    assert!(G { t: 1u8, v: vec![] } < G { t: 2, v: vec![] });
}
#[test]
fn slice_patterns() {
    let v = [1, 2, 3, 4];
    let [a, .., d] = v;
    assert_eq!(a + d, 5);
    let s: &[i32] = &v;
    match s {
        [] => panic!(),
        [x] => panic!("{}", x),
        [first, rest @ ..] => {
            assert_eq!(*first, 1);
            assert_eq!(rest.len(), 3);
        }
    }
    if let [.., y, z] = s { assert_eq!(y + z, 7); } else { panic!(); }
    let e: &[i32] = &[];
    assert!(matches!(e, []));
}
fn bound_as_ref<K: AsRef<std::ffi::OsStr>>(key: K) -> usize {
    // the bound picks the impl (AsRef<OsStr>), not the first AsRef impl of the instance type
    key.as_ref().as_encoded_bytes().len()
}
#[test]
fn generic_bound_method() {
    assert_eq!(bound_as_ref("hello"), 5);
    assert_eq!(bound_as_ref(String::from("hi")), 2);
}
trait Dq { fn f(&self) -> u32; }
trait Dq2: Dq { fn h(&self) -> u32 { 1 } }
struct DqS;
impl Dq for DqS { fn f(&self) -> u32 { 7 } }
impl Dq2 for DqS {}
fn dq1(x: &dyn Dq) -> u32 { Dq::f(x) }
fn dq2(x: &dyn Dq) -> u32 { <dyn Dq>::f(x) }
fn dq3(x: &dyn Dq2) -> u32 { <dyn Dq2>::f(x) + <dyn Dq2 as Dq2>::h(x) }
#[test]
fn ufcs_on_trait_objects() {
    // virtual calls (were a trap: the trait declaration has no body)
    assert_eq!(dq1(&DqS) + dq2(&DqS) + dq3(&DqS), 22);
}

// B26: temporaries are dropped at the end of their statement / condition / arm / closure body;
// `let` extends `&temp`; moving a field out of a temporary does not drop it again
#[test]
fn a_two_pushes() {
    let c: std::cell::RefCell<Vec<u32>> = std::cell::RefCell::new(Vec::new());
    c.borrow_mut().push(1);
    c.borrow_mut().push(2);
}
#[test]
fn b_len_then_push() {
    let c: std::cell::RefCell<Vec<u32>> = std::cell::RefCell::new(Vec::new());
    let n = c.borrow().len();
    c.borrow_mut().push(n as u32);
}
#[test]
fn c_if_cond() {
    let c: std::cell::RefCell<Vec<u32>> = std::cell::RefCell::new(Vec::new());
    if c.borrow().len() == 0 { c.borrow_mut().push(3); }
}
#[test]
fn d_match_scrut() {
    let c: std::cell::RefCell<Vec<u32>> = std::cell::RefCell::new(Vec::new());
    match c.borrow().len() { 0 => {} _ => panic!() }
    c.borrow_mut().push(4);
}
#[test]
fn e_assert() {
    let c: std::cell::RefCell<Vec<u32>> = std::cell::RefCell::new(Vec::new());
    assert_eq!(c.borrow().len(), 0);
    c.borrow_mut().push(4);
}
static mut TDROPS: u32 = 0;
struct TG(u32, String);
impl Drop for TG { fn drop(&mut self) { unsafe { TDROPS += 1; } } }
fn tmk(x: u32) -> TG { TG(x, "s".to_string()) }
fn td() -> u32 { unsafe { TDROPS } }
struct TP { s: String, n: u32 }
fn tmp_mk() -> TP { TP { s: "abc".to_string(), n: 2 } }
#[test]
fn let_extension() {
    unsafe { TDROPS = 0; }
    {
        let r = &tmk(1);
        assert_eq!(r.0, 1);
        assert_eq!(td(), 0);
        let t = (&tmk(2), 5);
        assert_eq!(t.0 .0 + t.1, 7);
        assert_eq!(td(), 0);
    }
    assert_eq!(td(), 2);
}
#[test]
fn arm_temps() {
    unsafe { TDROPS = 0; }
    for i in 0..3 {
        let n = match i { 0 => tmk(1).0, 1 => 5, _ => tmk(2).0 + tmk(3).0 };
        assert!(n > 0);
    }
    assert_eq!(td(), 3);
}
#[test]
fn field_out_of_temp() {
    let s = tmp_mk().s;
    assert_eq!(s, "abc");
    let n = tmp_mk().n;
    assert_eq!(n, 2);
}
#[test]
fn closure_body() {
    let c: std::cell::RefCell<Option<u32>> = std::cell::RefCell::new(None);
    let h = |c: &std::cell::RefCell<Option<u32>>| *c.borrow_mut() = Some(3);
    h(&c);
    h(&c);
    let k = |c: &std::cell::RefCell<Option<u32>>| c.borrow().unwrap();
    assert_eq!(k(&c) + k(&c), 6);
}

// c26: a non-move closure that consumes a capture captures it by value (rustc upvar.rs ByValue):
// the enclosing fn does not drop it again
fn c26_with<F: FnOnce(&std::cell::UnsafeCell<Option<std::sync::Arc<u32>>>)>(c: &std::cell::UnsafeCell<Option<std::sync::Arc<u32>>>, f: F) { f(c) }
fn c26_set(c: &std::cell::UnsafeCell<Option<std::sync::Arc<u32>>>, t: std::sync::Arc<u32>) { c26_with(c, |c| unsafe { *c.get() = Some(t) }); }
#[test]
fn closure_consumes_capture() {
    let a = std::sync::Arc::new(1u32);
    let cell = std::cell::UnsafeCell::new(None);
    c26_set(&cell, a.clone());
    assert_eq!(std::sync::Arc::strong_count(&a), 2);
    drop(cell);
    assert_eq!(std::sync::Arc::strong_count(&a), 1);
}
#[test]
fn threads_spawn_join_panic() {
    let mut hs = Vec::new();
    for i in 0..4u32 { hs.push(std::thread::spawn(move || i * 2)); }
    let mut s = 0;
    for h in hs { s += h.join().unwrap(); }
    assert_eq!(s, 12);
    let h = std::thread::spawn(|| { if true { panic!("boom"); } 1u32 });
    assert!(h.join().is_err());
    let mut v: Vec<Vec<u8>> = Vec::new();
    for i in 0..300 { let mut w = Vec::new(); w.push(i as u8); v.push(w); }
}

// F30: catch_unwind(AssertUnwindSafe(..)) learns R from AssertUnwindSafe's FnOnce impl (projection
// normalized through the selected impl, as rustc's project.rs)
#[test]
fn catch_unwind_assert_unwind_safe() {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| 1u32 + 1));
    assert_eq!(r.unwrap(), 2);
    let p = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> u32 { if true { panic!("x"); } 3 }));
    assert!(p.is_err());
}

// B40 const generics (rustc ty::Const): const params on types, impls and fns, `N` as a value, [x; N],
// impls for [T; N], N inferred from arguments, the expected type and turbofish
struct Buf<const N: usize> { data: [u8; N], len: usize }
impl<const N: usize> Buf<N> {
    fn new() -> Self { Buf { data: [0; N], len: 0 } }
    fn cap(&self) -> usize { N }
    fn push(&mut self, b: u8) { if self.len < N { self.data[self.len] = b; self.len += 1; } }
}
fn sum<const N: usize>(a: [u32; N]) -> u32 { let mut s = 0; for i in 0..N { s += a[i]; } s }
fn first<T: Copy, const N: usize>(a: &[T; N]) -> T { a[0] }
trait Lenny { fn lenny(&self) -> usize; }
impl<T, const N: usize> Lenny for [T; N] { fn lenny(&self) -> usize { N } }
const CG_SIZE: usize = 3;
fn dbg_arr<T: std::fmt::Debug, const N: usize>(a: [T; N]) -> String { format!("{:?}", a) }
#[test]
fn const_generics() {
    let mut b: Buf<4> = Buf::new();
    b.push(1); b.push(2);
    assert_eq!(b.cap(), 4);
    assert_eq!(b.len, 2);
    let c = Buf::<CG_SIZE>::new();
    assert_eq!(c.cap(), 3);
    assert_eq!(sum([1, 2, 3]), 6);
    assert_eq!(sum([5u32; 4]), 20);
    assert_eq!(first(&[7u8, 8]), 7);
    assert_eq!([1u8, 2, 3, 4, 5].lenny(), 5);
    assert_eq!(dbg_arr([1, 2]), "[1, 2]");
}
fn from_fn2<T: Copy + Default, const N: usize, F: FnMut(usize) -> T>(mut f: F) -> [T; N] {
    let mut a = [T::default(); N];
    for i in 0..N { a[i] = f(i); }
    a
}
#[test]
fn n_from_expected() {
    let a: [u16; 3] = from_fn2(|i| i as u16 * 2);
    assert_eq!(a, [0u16, 2, 4]);
    let b = from_fn2::<u8, 2, _>(|i| i as u8);
    assert_eq!(b, [0u8, 1]);
}

// Self::Target from a supertrait (`impl DerefMut`: <Self as Deref>::Target); deref through types with
// still-unknown args when one impl applies
struct StM { v: Vec<u32> }
impl std::ops::Deref for StM { type Target = Vec<u32>; fn deref(&self) -> &Self::Target { &self.v } }
impl std::ops::DerefMut for StM { fn deref_mut(&mut self) -> &mut Self::Target { &mut self.v } }
struct StG<T> { v: Vec<T> }
impl<T> std::ops::Deref for StG<T> { type Target = Vec<T>; fn deref(&self) -> &Vec<T> { &self.v } }
impl<T> std::ops::DerefMut for StG<T> { fn deref_mut(&mut self) -> &mut Self::Target { &mut self.v } }
#[test]
fn self_target_via_supertrait() {
    let mut m = StM { v: Vec::new() };
    m.push(3);
    std::ops::DerefMut::deref_mut(&mut m).push(4);
    assert_eq!(m.len(), 2);
    let mut g = StG { v: Vec::new() };
    g.push(1u8);
    assert_eq!(std::ops::DerefMut::deref_mut(&mut g).len(), 1);
}
#[test]
fn deref_with_unknown_args() {
    let c = std::cell::RefCell::new(Vec::new());
    c.borrow_mut().push(5u8);
    assert_eq!(c.borrow().len(), 1);
}

// F-CLO1: closure values drop their by-value captures; an FnOnce closure's state carries a drop flag per
// capture (cleared when its body moves the capture out); calling a Box<dyn FnOnce> consumes it
static mut CDROPS: u32 = 0;
struct CD(u32);
impl Drop for CD { fn drop(&mut self) { unsafe { CDROPS += 1; } } }
fn cd() -> u32 { unsafe { CDROPS } }
fn clo_call_once<F: FnOnce()>(f: F) { f() }
#[test]
fn a_direct_consuming() {
    unsafe { CDROPS = 0; }
    { let a = CD(1); let c = move || { drop(a); }; c(); }
    assert_eq!(cd(), 1);
}
#[test]
fn b_generic_consuming() {
    unsafe { CDROPS = 0; }
    { let a = CD(1); clo_call_once(move || { drop(a); }); }
    assert_eq!(cd(), 1);
}
#[test]
fn c_boxed_consuming() {
    unsafe { CDROPS = 0; }
    { let a = CD(1); let b: Box<dyn FnOnce()> = Box::new(move || { drop(a); }); b(); }
    assert_eq!(cd(), 1);
}
#[test]
fn d_not_consuming() {
    unsafe { CDROPS = 0; }
    { let a = CD(1); let c = move || { let _ = a.0; }; c(); c(); }
    assert_eq!(cd(), 1);
}
fn clo2_call_once<F: FnOnce()>(f: F) { f() }
#[test]
fn rc_direct() {
    let a = std::rc::Rc::new(5);
    let keep = a.clone();
    let c = move || { drop(a); };
    c();
    assert_eq!(std::rc::Rc::strong_count(&keep), 1);
}
#[test]
fn rc_generic() {
    let a = std::rc::Rc::new(5);
    let keep = a.clone();
    clo2_call_once(move || { drop(a); });
    assert_eq!(std::rc::Rc::strong_count(&keep), 1);
}
#[test]
fn rc_boxed() {
    let a = std::rc::Rc::new(5);
    let keep = a.clone();
    let b: Box<dyn FnOnce()> = Box::new(move || { drop(a); });
    b();
    assert_eq!(std::rc::Rc::strong_count(&keep), 1);
}
#[test]
fn rc_not_consuming_boxed() {
    let a = std::rc::Rc::new(5);
    let keep = a.clone();
    let b: Box<dyn FnOnce() -> usize> = Box::new(move || std::rc::Rc::strong_count(&a));
    assert_eq!(b(), 2);
    assert_eq!(std::rc::Rc::strong_count(&keep), 1);
}
#[test]
fn rc_not_consuming_generic() {
    let a = std::rc::Rc::new(5);
    let keep = a.clone();
    clo2_call_once(move || { let _ = std::rc::Rc::strong_count(&a); });
    assert_eq!(std::rc::Rc::strong_count(&keep), 1);
}

// cfg_attr expanded while parsing (derives see it; rustc_expand config.rs); let _ = place evaluates the place (index panics)
#[cfg_attr(any(target_os = "linux", target_os = "macos"), derive(Debug, Default))]
struct CaP { x: u32 }
#[cfg_attr(target_os = "nonexistent", derive(Debug))]
struct CaQ;
#[cfg_attr(all(), cfg_attr(all(), derive(Debug)))]
struct CaR(u8);
#[test]
fn cfg_attr_derive() {
    assert_eq!(format!("{:?}", CaP::default()), "CaP { x: 0 }");
    assert_eq!(format!("{:?}", CaR(3)), "CaR(3)");
    let _ = CaQ;
}
#[test]
fn let_underscore_index_panics() {
    let v = vec![1, 2, 3];
    let r = std::panic::catch_unwind(|| { let _ = v[3]; });
    assert!(r.is_err());
    let _ = v[2];
}

// FnOnce closures drop the captures their body left at the end of the call (rustc order)
static mut FTLOG: Vec<u32> = Vec::new();
struct FtD(u32);
impl Drop for FtD { fn drop(&mut self) { unsafe { FTLOG.push(self.0); } } }
fn ftlog() -> Vec<u32> { unsafe { FTLOG.clone() } }
fn ft_call_once<F: FnOnce()>(f: F) { f(); unsafe { FTLOG.push(100); } }
#[test]
fn fnonce_drops_at_call_end() {
    unsafe { FTLOG.clear(); }
    {
        let a = FtD(1);
        let b = FtD(2);
        let c = move || { drop(a); let _keep = &b; };
        c();
        unsafe { FTLOG.push(50); }
    }
    // rustc: a dropped in the body, b at the end of the call, then 50
    assert_eq!(ftlog(), vec![1, 2, 50]);
    unsafe { FTLOG.clear(); }
    let a = FtD(3);
    let b = FtD(4);
    ft_call_once(move || { drop(a); let _keep = &b; });
    assert_eq!(ftlog(), vec![3, 4, 100]);
}
