// Repros for rustc tests/ui run-pass failures (conformance lane). Each test names the rustc
// suite file it was reduced from; all of them pass under rustc (`rustc --test conform.rs`).
use std::process::{ExitCode, Termination};
use std::rc::Rc;

// rfcs/rfc-1937-termination-trait/*: `fn main() -> Result<..>` reports through Termination
#[test]
fn c01_termination() {
    assert_eq!(().report(), ExitCode::SUCCESS);
    let ok: Result<(), &'static str> = Ok(());
    assert_eq!(ok.report(), ExitCode::SUCCESS);
    let nested: Result<Result<(), u8>, u8> = Ok(Ok(()));
    assert_eq!(nested.report(), ExitCode::SUCCESS);
}

// statics/static-recursive.rs: statics refer to each other's addresses (cycle)
struct Link {
    next: &'static Link,
    data: i32,
}
static L1: Link = Link { next: &L2, data: 1 };
static L2: Link = Link { next: &L3, data: 2 };
static L3: Link = Link { next: &L1, data: 3 };
static mut SELF_PTR: *const u8 = unsafe { &raw const SELF_PTR as *const u8 };

#[test]
fn c02_recursive_statics() {
    let mut v = Vec::new();
    let mut cur = &L1;
    for _ in 0..4 {
        v.push(cur.data);
        cur = cur.next;
    }
    assert_eq!(v, [1, 2, 3, 1]);
    unsafe {
        assert_eq!(SELF_PTR, &raw const SELF_PTR as *const u8);
    }
}

// cast/cast-to-box-arr.rs, consts/static-raw-pointer-interning2.rs: `[T; N]` -> `[T]` unsizing
// behind raw pointers and smart pointers, also as an `as` cast (a coercion cast)
static mut RAW: *mut i32 = &mut [42, 43] as *mut [i32] as *mut i32;

#[test]
fn c03_array_unsizing() {
    let x = Box::new([1, 2, 3]);
    let y = x as Box<[i32]>;
    assert_eq!(y.len(), 3);
    assert_eq!(y[2], 3);
    let z: Box<[u8]> = Box::new([4u8, 5]);
    assert_eq!(z.len(), 2);
    let r: Rc<[i32]> = Rc::new([9, 8, 7, 6]);
    assert_eq!(r.len(), 4);
    assert_eq!(r[3], 6);
    let a = [1u16, 2, 3];
    let p = &a as *const [u16];
    assert_eq!(unsafe { (&*p).len() }, 3);
    let m = &mut [7, 8] as *mut [i32] as *mut i32;
    assert_eq!(unsafe { *m.add(1) }, 8);
    unsafe {
        assert_eq!(*RAW.add(1), 43);
    }
}

// issues/issue-3559.rs: temporaries of a skipped `||` / `&&` operand are not dropped (each
// operand is a terminating scope; this aborted with a double free)
fn check(a: &str, e: &str) -> bool {
    a == e
}

#[test]
fn c04_lazy_bool_temporaries() {
    let x = 5;
    let b = check(&format!("{}", x), "5") || check(&format!("{}", x), "6");
    assert!(b);
    let b = check(&format!("{}", x), "4") && check(&format!("{}", x), "5");
    assert!(!b);
    let mut hits = 0;
    if check(&format!("{}", x), "5") || check(&format!("{}", x), "6") {
        hits += 1;
    }
    if check(&format!("{}", x), "4") || check(&format!("{}", x), "5") {
        hits += 1;
    }
    assert!(check(&format!("{}", x), "4") || check(&format!("{}", x), "5"));
    assert_eq!(hits, 2);
}

// threads-sendsync/tls-try-with.rs: a closure whose body diverges returns `()` (diverging
// fallback; `!` from edition 2024) -- `try_with(|_| panic!())` must instantiate R
fn call<R>(f: impl FnOnce(bool) -> R, go: bool) -> Option<R> {
    if go {
        Some(f(go))
    } else {
        None
    }
}

#[test]
fn c05_diverging_fallback() {
    let r = call(|_| panic!("never"), false);
    assert!(r.is_none());
}

// self/arbitrary_self_types_{struct,trait}.rs: receivers `self: &Rc<Self>`, `self: Rc<Self>`,
// `self: &Box<Rc<Self>>` are matched by type at the autoderef step they occur at
struct Foo {
    x: i32,
    y: i32,
}

impl Foo {
    fn x(self: &Rc<Self>) -> i32 {
        self.x
    }
    fn y(self: Rc<Self>) -> i32 {
        self.y
    }
}

trait Tr {
    fn deep<'a>(self: &'a Box<Rc<Self>>) -> &'a [i32];
}

impl Tr for Vec<i32> {
    fn deep<'a>(self: &'a Box<Rc<Self>>) -> &'a [i32] {
        &***self
    }
}

#[test]
fn c06_typed_receivers() {
    let foo = Rc::new(Foo { x: 3, y: 4 });
    assert_eq!(foo.x(), 3);
    assert_eq!(foo.y(), 4);
    assert_eq!(Box::new(Rc::new(vec![1, 2, 3])).deep(), &[1, 2, 3]);
}

// binding/match-pipe-binding.rs: or-patterns bind the same names from whichever alternative
// matched (also with moves out of a temporary)
fn swap_or(t: (i32, i32, i32)) -> i32 {
    match t {
        (1, a, b) | (2, b, a) => a * 10 + b,
        _ => 0,
    }
}

#[test]
fn c07_or_pattern_bindings() {
    assert_eq!(swap_or((1, 2, 3)), 23);
    assert_eq!(swap_or((2, 3, 4)), 43);
    assert_eq!(swap_or((5, 3, 4)), 0);
    let r = match ((2, "b".to_string()), (1, "a".to_string())) {
        ((1, a), (2, b)) | ((2, b), (1, a)) => a + &b,
        _ => String::new(),
    };
    assert_eq!(r, "ab");
    let v: Result<u8, u8> = Err(7);
    let (Ok(x) | Err(x)) = v;
    assert_eq!(x, 7);
}

// fn/dyn-fn-alignment.rs, packed/*, derives/deriving-with-repr-packed-3.rs: repr(align(N)) and
// repr(packed(N)) layouts; derives and drops of packed fields go through aligned copies
#[repr(align(256))]
struct Al {
    v: u8,
}

#[repr(packed)]
struct Pk {
    a: u8,
    b: u32,
    c: u16,
}

#[repr(C, packed(2))]
struct Pk2 {
    a: u8,
    b: u32,
}

#[derive(Copy, Clone)]
struct Aligned(usize);

impl PartialEq for Aligned {
    fn eq(&self, other: &Self) -> bool {
        assert_eq!(self as *const Aligned as usize % std::mem::align_of::<Aligned>(), 0);
        assert_eq!(other as *const Aligned as usize % std::mem::align_of::<Aligned>(), 0);
        self.0 == other.0
    }
}

#[repr(packed)]
#[derive(Copy, Clone, PartialEq)]
struct PackedPair(Aligned, Aligned);

#[repr(C)]
#[derive(PartialEq)]
struct Dealigned<T>(u8, T);

struct OddCheck(u16);

impl Drop for OddCheck {
    fn drop(&mut self) {
        assert_eq!(self as *mut OddCheck as usize % 2, 0);
    }
}

#[repr(packed)]
struct Misalign(u8, OddCheck);

#[test]
fn c08_repr_align_packed() {
    assert_eq!(std::mem::align_of::<Al>(), 256);
    assert_eq!(std::mem::size_of::<Al>(), 256);
    let b = Box::new(Al { v: 2 });
    assert_eq!(&*b as *const Al as usize % 256, 0);
    assert_eq!(b.v, 2);
    let a = Al { v: 1 };
    let f: Box<dyn FnOnce() -> u8> = Box::new(move || a.v);
    assert_eq!(f(), 1);
    assert_eq!((std::mem::size_of::<Pk>(), std::mem::align_of::<Pk>()), (7, 1));
    assert_eq!(std::mem::size_of::<Pk2>(), 6);
    let p = Pk { a: 1, b: 0x01020304, c: 7 };
    let (pa, pb, pc) = (p.a, p.b, p.c);
    assert_eq!((pa, pb, pc), (1, 0x01020304, 7));
    let d = Dealigned(0, PackedPair(Aligned(1), Aligned(2)));
    assert!(d == d);
    let m: ([u16; 0], Misalign) = ([], Misalign(0, OddCheck(10)));
    drop(m);
}

// cast/cast-rfc0401.rs, drop/multiple-drop-safe-code-25549.rs: casts that are coercions
// (trivial casts type their literals; struct-tail unsizing)
struct Holder<T: ?Sized>(u8, T);

#[test]
fn c09_coercion_casts() {
    assert_eq!(Some(4u32) as Option<u32>, Some(4u32));
    assert_eq!((1u32, 2u32) as (u32, u32), (1, 2));
    let h = &Holder(1, [5u16, 6, 7]) as &Holder<[u16]>;
    assert_eq!(h.1.len(), 3);
    assert_eq!(h.1[2], 7);
}

// String + &String: the right operand is a coercion site for the impl's Rhs (&String -> &str)
#[test]
fn c10_operator_rhs_coercion() {
    let a = "x".to_string();
    let b = "y".to_string();
    let mut c = a + &b;
    c += &b;
    assert_eq!(c, "xyy");
}

// borrowck/borrowck-box-sensitivity.rs, borrowck-field-sensitivity-rpass.rs: moving fields out
// of a Box and functional record update move only those fields; the rest is dropped once
use std::cell::RefCell;

thread_local!(static LOG: RefCell<Vec<&'static str>> = RefCell::new(Vec::new()));

struct D(&'static str);

impl Drop for D {
    fn drop(&mut self) {
        LOG.with(|l| l.borrow_mut().push(self.0));
    }
}

fn take_log() -> Vec<&'static str> {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

struct Two {
    a: u8,
    b: D,
    c: D,
}

fn mk_two() -> Two {
    Two { a: 1, b: D("tb"), c: D("tc") }
}

#[test]
fn c11_box_field_moves() {
    take_log();
    {
        let bx = Box::new(Two { a: 1, b: D("x"), c: D("y") });
        let moved = bx.b;
        assert_eq!(bx.a, 1);
        drop(moved);
        LOG.with(|l| l.borrow_mut().push("end"));
    }
    assert_eq!(take_log(), ["x", "end", "y"]);
}

#[test]
fn c12_struct_update_moves() {
    take_log();
    {
        let x = Two { a: 1, b: D("xb"), c: D("xc") };
        let y = Two { b: D("yb"), ..x };
        assert_eq!(y.a, 1);
        drop(x.b);
        let z = Two { c: D("zc"), ..mk_two() };
        drop(z);
        LOG.with(|l| l.borrow_mut().push("end"));
    }
    assert_eq!(take_log(), ["xb", "tc", "tb", "zc", "end", "yb", "xc"]);
}

// derives/eq-ord/derive-partial-ord-discriminant*.rs, enum-discrim-*.rs, consts/const-eval/
// enum_discr.rs: discriminants are constants of the repr type (wrapping, `!`, casts, other
// variants), tags are the smallest fitting integer, derived orderings compare discriminants
#[derive(PartialEq, PartialOrd, Clone, Copy)]
enum E1 {
    Pos2 = 2,
    Neg1 = -1,
    Pos1 = 1,
}

#[derive(PartialEq, PartialOrd, Clone, Copy)]
#[repr(i64)]
enum Ei64 {
    Pos2 = 2,
    NegMin = 1 << 63,
    PosMax = !(1 << 63),
}

#[repr(u8)]
#[derive(Clone, Copy)]
enum Eu8 {
    Max = !0 as u8,
}

enum Fwd {
    X = Fwd::Y as isize * 2,
    Y = 9,
}

enum Small {
    A = -1,
    B = 0x80,
}

#[test]
fn c13_discriminants() {
    assert!(E1::Pos2 > E1::Pos1 && E1::Pos1 > E1::Neg1);
    assert!(Ei64::NegMin < Ei64::Pos2 && Ei64::Pos2 < Ei64::PosMax);
    assert_eq!(Ei64::PosMax as i64, i64::MAX);
    assert_eq!(Eu8::Max as u8, 255);
    assert_eq!(Fwd::X as isize, 18);
    assert_eq!(std::mem::size_of::<Small>(), 2);
    assert_eq!(Small::A as i32 + Small::B as i32, 127);
}

// overloaded/overloaded-deref.rs, overloaded-deref-count.rs: `*x` of a Deref type calls deref
// (the guard of `*cell.borrow()` is a statement temporary), `(*v).push(..)` uses DerefMut
use std::cell::Cell;
use std::ops::{Deref, DerefMut};

struct Wrap {
    tag: u64,
    inner: u32,
    muts: Cell<u32>,
}

impl Deref for Wrap {
    type Target = u32;
    fn deref(&self) -> &u32 {
        &self.inner
    }
}

impl DerefMut for Wrap {
    fn deref_mut(&mut self) -> &mut u32 {
        self.muts.set(self.muts.get() + 1);
        &mut self.inner
    }
}

#[test]
fn c14_overloaded_deref_rvalue() {
    let i = RefCell::new(2);
    let a = *i.borrow();
    *i.borrow_mut() = 5;
    assert_eq!((a, *i.borrow()), (2, 5));
    let mut w = Wrap { tag: 99, inner: 7, muts: Cell::new(0) };
    let v = *w;
    assert_eq!((v, w.tag), (7, 99));
    (*w).clone_from(&3);
    assert_eq!((*w, w.muts.get()), (3, 1));
}

// moves/move-3-unique.rs: a variable initialised in a branch lives to the end of its own block
// (its drop flag was registered in the branch: dropped there, read after free)
fn branch_init(c: bool, foo: Box<u32>) -> u32 {
    let y: Box<u32>;
    if c {
        y = foo;
    } else {
        y = Box::new(5);
    }
    *y + 1
}

// drop/repeat-drop.rs: `[v; 0]` drops v at once (a const operand makes no value)
struct Dc<'a>(&'a Cell<u32>);

impl Drop for Dc<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

// autoref-autoderef/autoderef-method-twice-but-not-thrice.rs: by-value receiver under boxes
trait Double {
    fn double(self: Box<Self>) -> usize;
}

impl Double for Box<usize> {
    fn double(self: Box<Box<usize>>) -> usize {
        **self * 2
    }
}

#[test]
fn c15_scopes_and_moves() {
    assert_eq!(branch_init(true, Box::new(1)), 2);
    assert_eq!(branch_init(false, Box::new(1)), 6);
    let n = Cell::new(0);
    let d = Dc(&n);
    let v: [Dc; 0] = [d; 0];
    assert_eq!(n.get(), 1);
    drop(v);
    assert_eq!(n.get(), 1);
    let x: Box<Box<Box<Box<usize>>>> = Box::new(Box::new(Box::new(Box::new(3))));
    assert_eq!(x.double(), 6);
}

// drop/drop-count-assertion-16151.rs, same-alloca-reassigned-match-binding-16151.rs: `while let`
// bindings and its scrutinee temporary are dropped at the end of every iteration (they leaked;
// the last one dropped at function end)
#[test]
fn c16_while_let_drops() {
    take_log();
    {
        let mut v = vec![D("1"), D("2"), D("3")];
        while let Some(x) = v.pop() {
            if x.0 == "0" {
                return;
            }
        }
        LOG.with(|l| l.borrow_mut().push("mid"));
        let mut it = vec![D("6"), D("7")].into_iter();
        while let Some(x) = it.next() {
            if x.0 == "9" {
                break;
            }
        }
        LOG.with(|l| l.borrow_mut().push("end"));
    }
    assert_eq!(take_log(), ["3", "2", "1", "mid", "6", "7", "end"]);
}

// consts/consts-in-patterns.rs, const_in_pattern/*: constant patterns match structurally by
// value (a reference constant compares its pointee; the scrutinee is not auto-dereferenced)
#[derive(PartialEq, Eq)]
enum Shape {
    Dot,
    Line(u8, u8),
}

const LINE: Shape = Shape::Line(1, 2);
const NAME: &str = "abc";
const UNIT_REF: &() = &();
const WORDS: &[&str] = &["a", "bc"];

fn classify(s: &Shape) -> u8 {
    match *s {
        LINE => 1,
        Shape::Line(..) => 2,
        Shape::Dot => 3,
    }
}

#[test]
fn c17_const_patterns() {
    assert_eq!((classify(&Shape::Line(1, 2)), classify(&Shape::Line(1, 3)), classify(&Shape::Dot)), (1, 2, 3));
    let n = match "abc" {
        NAME => 1,
        _ => 2,
    };
    assert_eq!(n, 1);
    let z = match &() {
        UNIT_REF => 9,
    };
    assert_eq!(z, 9);
    let w: &[&str] = &["a", "bc"];
    assert!(matches!(w, WORDS));
    assert!(!matches!(&["a"][..], WORDS));
}

// for-loop-while/for-loop-has-unit-body.rs: a loop body's tail has type `()`
#[test]
fn c18_loop_body_unit() {
    let mut n = 0;
    for _ in 0..3 {
        n += 1;
        unsafe { std::mem::zeroed() }
    }
    assert_eq!(n, 3);
}

// macros/*, offset-of: std's addr_of!/addr_of_mut!/offset_of!/try! macros; array lengths from
// char/bool casts and enum variants
#[repr(C)]
struct Inner {
    x: u8,
    y: u32,
}

#[repr(C)]
struct Outer {
    a: u8,
    b: u64,
    c: Inner,
}

enum Bunny {
    Flopsy = 3,
}

#[test]
fn c19_std_macros_and_lengths() {
    assert_eq!(std::mem::offset_of!(Outer, b), 8);
    assert_eq!(std::mem::offset_of!(Outer, c.y), 20);
    let mut o = Outer { a: 1, b: 2, c: Inner { x: 3, y: 4 } };
    let p = std::ptr::addr_of!(o.b);
    let q = std::ptr::addr_of_mut!(o.c.x);
    unsafe {
        *q = 9;
        assert_eq!(*p, 2);
    }
    assert_eq!((o.a, o.c.x, o.c.y), (1, 9, 4));
    let a = [0u8; '\n' as usize];
    let b = [1u8; true as usize];
    let c = [2u8; Bunny::Flopsy as usize];
    assert_eq!((a.len(), b.len(), c.len()), (10, 1, 3));
}

// structs-enums/struct-aliases.rs: struct literals/patterns through aliases and with explicit
// generic args fix those args (`S4::<u8> { x: 4, .. }` makes x a u8)
struct S3<U, V> {
    x: U,
    y: V,
}

type S4<U> = S3<U, char>;

#[test]
fn c20_struct_path_args() {
    let s = S4 { x: 4, y: 'a' };
    match s {
        S4::<u8> { x, y } => assert_eq!((std::mem::size_of_val(&x), y), (1, 'a')),
    }
    let t = S3::<u16, bool> { x: 5, y: true };
    assert_eq!(std::mem::size_of_val(&t.x), 2);
    assert!(t.y);
}

// for-loop-while/label_break_value.rs: a labeled block whose body diverges has its breaks' type
// (it was `!`: the value was lost, garbage/unreachable)
#[test]
fn c21_labeled_block_value() {
    let v = 5;
    let a = 'b: {
        break 'b v == 5;
    };
    let m = match v {
        w if 'b: {
            break 'b w == 5;
        } => 1,
        _ => 2,
    };
    assert!(a);
    assert_eq!(m, 1);
}

// associated-types/associated-types-doubleendediterator-object.rs: the object type's bindings
// type the coerced value (`vec![1, ..]` as `dyn Iterator<Item = isize>` was a Vec<i32>: SILENT)
#[test]
fn c22_dyn_binding_types_source() {
    let mut t: Box<dyn DoubleEndedIterator<Item = isize>> = Box::new(vec![1, 2, 3, 4, 5, 6].into_iter());
    let mut r = 0;
    while let (Some(f), Some(b)) = (t.next(), t.next_back()) {
        r += b - f;
    }
    assert_eq!(r, 9);
}

// destructuring-assignment/*: `(a, b) = (b, a)`, `..`, `_`, struct assignees; parts of a `let`
// pattern's temporary that are not moved drop at the statement end
struct Pt {
    x: i32,
    y: i32,
}

#[test]
fn c23_destructuring_assignment() {
    let (mut a, mut b);
    (a, b) = (0, 1);
    (b, a) = (a, b);
    assert_eq!((a, b), (1, 0));
    (a, .., b) = (7, 8, 9);
    assert_eq!((a, b), (7, 9));
    (..) = (3, 4);
    (a, _) = (5, 6);
    assert_eq!(a, 5);
    let (mut p, mut q) = (0u8, 0u8);
    (p, q) = (q + 1, p + 2);
    assert_eq!((p, q), (1, 2));
    let (x, y);
    Pt { x, y } = Pt { x: 3, y: 4 };
    assert_eq!((x, y), (3, 4));
    let mut arr = [0; 3];
    [arr[0], .., arr[2]] = [1, 2, 3];
    assert_eq!(arr, [1, 0, 3]);
    take_log();
    {
        let (keep, _) = (D("k"), D("t"));
        LOG.with(|l| l.borrow_mut().push("mid"));
        drop(keep);
    }
    assert_eq!(take_log(), ["t", "mid", "k"]);
}

// traits/default-method/*, traits/to-str.rs, traits/inheritance/visibility.rs: an integer
// literal receiver / argument takes the one numeric type that implements the trait
trait Meow {
    fn meow(&self) -> u32 {
        7
    }
}

impl Meow for isize {}

fn bounded<T: Meow>(x: T) -> u32 {
    x.meow() + std::mem::size_of::<T>() as u32
}

#[test]
fn c24_literal_trait_selection() {
    assert_eq!(3.meow(), 7);
    assert_eq!(bounded(0), 7 + std::mem::size_of::<isize>() as u32);
}

// F37 (bring-up, apple_sys): a discriminant calling a const fn is evaluated like a const
const fn four_cc(s: &str) -> u32 {
    let b = s.as_bytes();
    ((b[0] as u32) << 24) | ((b[1] as u32) << 16) | ((b[2] as u32) << 8) | (b[3] as u32)
}

#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Debug)]
enum Sel {
    Devices = four_cc("dev#"),
    Other = 7,
    Next,
}

#[test]
fn c25_const_fn_discriminant() {
    assert_eq!(Sel::Devices as u32, 0x6465_7623);
    assert_eq!((Sel::Other as u32, Sel::Next as u32), (7, 8));
    assert!(Sel::Devices != Sel::Next);
}

// traits/vtable/empty-supertrait-with-nonempty-supersupertrait.rs, traits/trait-upcasting/*:
// `&dyn Sub` as `&dyn Super` takes Sub's stored vtable for Super (it kept Sub's vtable: the
// wrong method ran, SILENT)
trait UpOne {
    fn one(&self) -> u32 {
        1
    }
}

trait UpTwo {
    fn two(&self) -> u32 {
        2
    }
}

trait UpTwoAgain: UpTwo {}

trait UpOneTwo: UpOne + UpTwoAgain {
    fn both(&self) -> u32 {
        self.one() * 10 + self.two()
    }
}

impl UpOne for () {}
impl UpTwo for () {}
impl<T: UpTwo> UpTwoAgain for T {}
impl UpOneTwo for () {}

#[test]
fn c26_trait_upcasting() {
    let x: &dyn UpOneTwo = &();
    assert_eq!(x.both(), 12);
    let t: &dyn UpTwo = x;
    assert_eq!(t.two(), 2);
    let ta: &dyn UpTwoAgain = x;
    assert_eq!(ta.two(), 2);
    let t2: &dyn UpTwo = ta;
    assert_eq!(t2.two(), 2);
    let o = x as &dyn UpOne;
    assert_eq!(o.one(), 1);
    let b: Box<dyn UpOneTwo> = Box::new(());
    let bo: Box<dyn UpOne> = b;
    assert_eq!(bo.one(), 1);
    let f: Box<dyn Fn() -> u32> = Box::new(|| 5);
    let fm: Box<dyn FnMut() -> u32> = f;
    let mut fm = fm;
    assert_eq!(fm(), 5);
}

// array-slice-vec/slice_is_sorted_by_borrow.rs: ordering of tuples / arrays of primitives is
// lexicographic (it was rejected)
#[test]
fn c27_tuple_ordering() {
    assert!((1, 2) < (1, 3));
    assert!(!((2, 0) < (1, 9)));
    assert!((1, 2) <= (1, 2));
    assert!([3u8, 1] > [3, 0]);
    assert!(!((1.0, f64::NAN) < (1.0, 2.0)));
    assert!(('a', true) >= ('a', false));
}

// closures/2229_closure_analysis/capture-enum-field.rs: `let E(ref mut r, ..) = captured;`
// borrows the captured place (it bound into a copy: the writes were lost, SILENT)
#[derive(Debug, PartialEq)]
enum Color {
    Rgb(u8, u8, u8),
}

#[test]
fn c28_ref_pattern_on_capture() {
    let mut color = Color::Rgb(0, 0, 0);
    let mut red = |v| {
        let Color::Rgb(ref mut r, _, _) = color;
        *r = v;
    };
    red(1);
    let mut blue = |v| {
        let Color::Rgb(_, _, ref mut b) = color;
        *b = v;
    };
    blue(3);
    assert_eq!(color, Color::Rgb(1, 0, 3));
}

// binding/ref-pattern-drop-behavior-8860.rs, for-loop-lifetime-of-unbound-values.rs,
// unboxed-closures-drop.rs: argument / loop items bound by `ref` or `_` drop with their scope;
// `x;` moves x and drops it (all three leaked)
fn takes_ref(ref _d: D) {}

#[test]
fn c29_unbound_values_drop() {
    take_log();
    takes_ref(D("arg"));
    LOG.with(|l| l.borrow_mut().push("after-call"));
    for _ in std::iter::once(D("item")) {
        LOG.with(|l| l.borrow_mut().push("body"));
    }
    let x = D("x");
    x;
    LOG.with(|l| l.borrow_mut().push("end"));
    assert_eq!(take_log(), ["arg", "after-call", "body", "item", "x", "end"]);
}

// derives/clone-copy/misbehaving-clone-impl.rs: a Copy type's derived Clone copies bits
use std::sync::atomic::{AtomicBool, Ordering};

static CLONED: AtomicBool = AtomicBool::new(false);

#[derive(Copy)]
struct Liar;

impl Clone for Liar {
    fn clone(&self) -> Self {
        CLONED.store(true, Ordering::SeqCst);
        *self
    }
}

#[derive(Copy, Clone)]
struct Innocent(#[allow(dead_code)] Liar);

// consts/const_fn_unsize.rs: a smart-pointer unsizing at a function's tail keeps N
fn dangling_slice<T>() -> std::ptr::NonNull<[T]> {
    std::ptr::NonNull::<[T; 1]>::dangling()
}

#[test]
fn c30_copy_clone_and_tail_unsize() {
    let _ = Innocent(Liar).clone();
    assert!(!CLONED.load(Ordering::SeqCst));
    assert_eq!(dangling_slice::<i32>().as_ptr().len(), 1);
}

// std lane core_niche (a): dropping a Box<[T]> / drop_in_place on `*mut [T]` drops every element
// (none or only the first were dropped); (b) `==` on fat raw pointers compares the metadata too
#[test]
fn c31_slice_drop_and_fat_ptr_eq() {
    take_log();
    let b: Box<[D]> = vec![D("1"), D("2"), D("3")].into_boxed_slice();
    drop(b);
    let p: *mut [D] = Box::into_raw(vec![D("4"), D("5")].into_boxed_slice());
    unsafe {
        std::ptr::drop_in_place(p);
        drop(Box::from_raw(p as *mut [std::mem::ManuallyDrop<D>]));
    }
    assert_eq!(take_log(), ["1", "2", "3", "4", "5"]);
    let a = [1u8, 2, 3];
    let p1: *const [u8] = &a[..2];
    let p2: *const [u8] = &a[..3];
    assert!(p1 != p2);
    assert!(p1 == p1);
    assert!(p1 < p2);
}

// destructuring-assignment/nested_destructure.rs, structs-enums/align-enum.rs
struct Pair<S, T>(S, T);

#[repr(align(32))]
#[allow(dead_code)]
enum Align32 {
    Foo,
    Bar,
}

#[test]
fn c32_tuple_struct_assignee_and_enum_align() {
    let (a, b, c);
    Pair((a, b), c) = Pair((0, 1), 2);
    assert_eq!((a, b, c), (0, 1, 2));
    assert_eq!((std::mem::align_of::<Align32>(), std::mem::size_of::<Align32>()), (32, 32));
}

// structs-enums/multiple-reprs.rs, enum-non-c-like-repr-c*.rs, align-struct.rs: repr(C) enums
// are C ints (fieldless) or a tag followed by a union of the variants (FFI layout); hex align
#[repr(C)]
#[allow(dead_code)]
enum CEnum {
    A,
    B,
}

#[repr(C, u8)]
#[allow(dead_code)]
enum CU8 {
    A(u8, u16, u8),
    B(u8, u16, u8),
}

#[repr(u8)]
#[allow(dead_code)]
enum PrimU8 {
    A(u8, u16, u8),
    B(u8, u16, u8),
}

#[repr(C)]
#[allow(dead_code)]
enum CData {
    A(u32),
    B { x: u8, y: u64 },
}

#[repr(align(0x100))]
struct Big {
    _v: u8,
}

#[test]
fn c33_repr_c_enums() {
    use std::mem::{align_of, size_of};
    assert_eq!(size_of::<CEnum>(), 4);
    assert_eq!(size_of::<CU8>(), 8);
    assert_eq!(size_of::<PrimU8>(), 6);
    assert_eq!((size_of::<CData>(), align_of::<CData>()), (24, 8));
    assert_eq!(align_of::<Big>(), 0x100);
    let d = CData::B { x: 7, y: 9 };
    match d {
        CData::B { x, y } => assert_eq!((x, y), (7, 9)),
        _ => unreachable!(),
    }
}

// issues/issue-51655.rs, consts/const_prop_slice_pat_ice.rs: `&[..]` patterns against `&[T]`
// (an internal error: the pointee place lost its length)
fn ref_slice_pat(e: &[u8]) -> u8 {
    match e {
        &[] => 0,
        &[a] => a,
        _ => 9,
    }
}

#[test]
fn c34_ref_slice_patterns() {
    assert_eq!((ref_slice_pat(&[]), ref_slice_pat(&[5]), ref_slice_pat(&[1, 2])), (0, 5, 9));
}

// drop/issue-30018-nopanic.rs: operands already evaluated drop when a later one exits early
// (`f(a, b?)`, `S { a, b: c? }`, `(a, b?)`, `[a, { break }]` leaked them)
struct TwoD {
    _a: D,
    _b: D,
}

fn none_d() -> Option<D> {
    None
}

fn takes_two(_a: D, _b: D) {}

fn early_struct() -> Option<TwoD> {
    Some(TwoD { _a: D("s"), _b: none_d()? })
}

fn early_tuple() -> Option<(D, D)> {
    Some((D("t"), none_d()?))
}

fn early_call() -> Option<()> {
    takes_two(D("c"), none_d()?);
    Some(())
}

#[test]
fn c35_operands_drop_on_early_exit() {
    take_log();
    assert!(early_struct().is_none());
    assert!(early_tuple().is_none());
    assert!(early_call().is_none());
    loop {
        let _r = [D("a0"), D("a1"), {
            break;
        }];
    }
    assert_eq!(take_log(), ["s", "t", "c", "a1", "a0"]);
}

// structs/newtype-struct-drop-run.rs, drop/static-issue-17302.rs: `let Foo(i) = *self;` in a
// Drop impl reads the place (Copy parts); it made a temporary copy that was dropped again:
// infinite drop recursion
struct Setter<'a>(&'a std::cell::Cell<i32>);

impl Drop for Setter<'_> {
    fn drop(&mut self) {
        let Setter(c) = *self;
        c.set(c.get() + 1);
    }
}

#[test]
fn c36_place_pattern_in_drop() {
    let c = std::cell::Cell::new(0);
    {
        let _s = Setter(&c);
    }
    assert_eq!(c.get(), 1);
}

// attributes/inner-attrs-impl-cfg.rs: `impl X { #![cfg(false)] .. }` removes the impl
struct CfgImpl;

impl CfgImpl {
    #![cfg(any())]

    fn which(&self) -> u8 {
        1
    }
}

impl CfgImpl {
    fn which(&self) -> u8 {
        2
    }
}

// autoref-autoderef/autoderef-privacy.rs: a private method not visible at the call site is no
// candidate; the probe continues through Deref
pub struct Outer2 {
    v: Vec<u8>,
}

mod hidden {
    pub struct Wrapper(pub Vec<u8>);

    impl Wrapper {
        fn len(&self) -> usize {
            999
        }
        pub fn inner_len(&self) -> usize {
            self.len()
        }
    }

    impl std::ops::Deref for Wrapper {
        type Target = Vec<u8>;
        fn deref(&self) -> &Vec<u8> {
            &self.0
        }
    }
}

#[test]
fn c37_inner_cfg_and_privacy() {
    assert_eq!(CfgImpl.which(), 2);
    let w = hidden::Wrapper(vec![1, 2, 3]);
    assert_eq!(w.len(), 3);
    assert_eq!(w.inner_len(), 999);
    let _ = Outer2 { v: Vec::new() }.v;
}

// overloaded/overloaded_deref_with_ref_pattern.rs: `ref mut` bindings on a Deref place use
// DerefMut (match scrutinee, let)
struct OnlyMut<T>(T, std::cell::Cell<u32>);

impl<T> std::ops::Deref for OnlyMut<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.1.set(self.1.get() + 1);
        &self.0
    }
}

impl<T> std::ops::DerefMut for OnlyMut<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

#[test]
fn c38_ref_mut_patterns_use_deref_mut() {
    let mut b = OnlyMut(0, std::cell::Cell::new(0));
    match *b {
        ref mut n => *n += 1,
    }
    let ref mut z = *b;
    *z += 1;
    let mut w = OnlyMut((1, 2), std::cell::Cell::new(0));
    let (ref mut p, ref q) = *w;
    *p += *q;
    assert_eq!((b.0, (w.0).0, b.1.get(), w.1.get()), (2, 3, 0, 0));
}

// let-else/let-else-drop-order.rs: a non-matching let-else value and the initializer's
// temporaries drop before the else block, exactly once
struct Holder2(&'static str, Option<u8>);

impl Drop for Holder2 {
    fn drop(&mut self) {
        LOG.with(|l| l.borrow_mut().push(self.0));
    }
}

fn let_else_none() -> u8 {
    let Some(x) = Holder2("tmp", None).1 else {
        LOG.with(|l| l.borrow_mut().push("else"));
        return 0;
    };
    x
}

#[test]
fn c39_let_else_drop_order() {
    take_log();
    assert_eq!(let_else_none(), 0);
    assert_eq!(take_log(), ["tmp", "else"]);
}

// coercion/coerce-unify.rs, coerce-many-with-ty-var.rs, closures/closure_no_cap_coerce_many*:
// if/match arms and array elements meet like rustc's CoerceMany (fn items -> fn pointer,
// `&String` then `&str` -> `&str`)
fn one_fn() -> u8 {
    1
}

fn two_fn() -> u8 {
    2
}

#[test]
fn c40_coerce_many() {
    let c = std::hint::black_box(true);
    let f = if c { one_fn } else { two_fn };
    let g = match std::hint::black_box(1) {
        0 => one_fn,
        _ => two_fn,
    };
    let arr = [one_fn, two_fn, |/**/| 3];
    let s = String::from("x");
    let r = if c { &s } else { "lit" };
    let e: [_; 2] = [one_fn, two_fn];
    assert_eq!((f(), g(), arr[2](), r, e[1]()), (1, 2, 3, "x", 2));
}

// consts/large-zst-array-77062.rs, array-slice-vec/large-zst-array-compilation-time-68010.rs,
// layout/big-type-no-err.rs: `[(); usize::MAX]` is a valid zero-sized type (its length is not a
// type-variable tag) and repeating a zero-sized value stores nothing
static HUGE_ZST: [(); usize::MAX] = [(); usize::MAX];

#[test]
fn c41_usize_max_zst_array() {
    let r = &[(); usize::MAX];
    assert_eq!((r.len(), HUGE_ZST.len(), std::mem::size_of::<[(); usize::MAX]>()), (usize::MAX, usize::MAX, 0));
}

// impl-trait/closure-in-impl-trait.rs: a return-position `impl Trait` whose binding has an array
// length evaluated by running it (the opaque's bounds are lowered again once it is known)
fn blocky() -> impl Iterator<Item = [u8; { let _k = |x: u32| x; 3 }]> {
    std::iter::once([7; 3])
}

#[test]
fn c42_impl_trait_anon_len() {
    let v: Vec<[u8; 3]> = blocky().collect();
    assert_eq!(v, [[7, 7, 7]]);
}

// a method with type params of its own or `where Self: Sized` has no vtable entry (rustc
// vtable_entries); the slots after it still dispatch
trait Area {
    fn area(&self) -> u32;
    fn scaled<T: Into<u32>>(&self, k: T) -> u32
    where
        Self: Sized,
    {
        self.area() * k.into()
    }
    fn boxed(self) -> Box<Self>
    where
        Self: Sized,
    {
        Box::new(self)
    }
    fn name(&self) -> &'static str;
}

struct Sq(u32);

impl Area for Sq {
    fn area(&self) -> u32 {
        self.0 * self.0
    }
    fn name(&self) -> &'static str {
        "sq"
    }
}

#[test]
fn c43_vtable_vacant_methods() {
    let d: &dyn Area = &Sq(3);
    let b = Sq(2).boxed();
    let it: Box<dyn Iterator<Item = u8>> = Box::new([1u8, 2].into_iter());
    assert_eq!((d.area(), d.name(), b.scaled(2u8), it.count()), (9, "sq", 8, 2));
}

// F44 (backend2/promo_autoref.rs, rustybuzz kerning): a shared autoref of a promotable rvalue
// receiver borrows a promoted static (rustc promote_consts), so `[A, B].iter()` outlives the fn
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kern {
    A = 0,
    B = 1,
}

fn kern_iter() -> std::iter::Copied<std::slice::Iter<'static, Kern>> {
    [Kern::A, Kern::B].iter().copied()
}

fn len_of_promoted() -> &'static [u8] {
    [1u8, 2, 3].as_slice()
}

#[inline(never)]
fn clobber_stack(n: u64) -> u64 {
    let a = std::hint::black_box([n; 64]);
    a.iter().sum()
}

#[test]
fn c44_promoted_autoref_receiver() {
    let f: fn() -> std::iter::Copied<std::slice::Iter<'static, Kern>> = std::hint::black_box(kern_iter);
    let g: fn() -> &'static [u8] = std::hint::black_box(len_of_promoted);
    let mut i = f();
    let s = g();
    assert!(clobber_stack(0xffff_ffff) > 0);
    let v: Vec<Kern> = i.by_ref().collect();
    assert_eq!((v, s), (vec![Kern::A, Kern::B], &[1u8, 2, 3][..]));
}

// dst/dst-struct.rs, dst-trait.rs, dst-raw.rs, dst-coercions.rs, issues/issue-23491.rs: unsizing
// through a struct tail behind any pointer (`&Fat<Bar1>` -> `&Fat<dyn ToBar>`, `Box<Fat<[T; N]>>`
// -> `Box<Fat<[T]>>`) and `&T` -> `*const dyn Tr`
struct FatTail<T: ?Sized> {
    f1: isize,
    ptr: T,
}

trait ToNum {
    fn num(&self) -> isize;
}

impl ToNum for u8 {
    fn num(&self) -> isize {
        *self as isize * 10
    }
}

#[test]
fn c45_struct_tail_unsizing() {
    let a = FatTail { f1: 1, ptr: 4u8 };
    let r: &FatTail<dyn ToNum> = &a;
    let b: Box<FatTail<[isize]>> = Box::new(FatTail { f1: 2, ptr: [7, 8, 9] });
    let rc: std::rc::Rc<std::cell::RefCell<[isize]>> = std::rc::Rc::new(std::cell::RefCell::new([1, 2, 3]));
    let p: *const dyn ToNum = &5u8;
    let n = unsafe { (*p).num() };
    assert_eq!((r.f1, r.ptr.num(), b.f1, b.ptr.len(), b.ptr[2], rc.borrow().len(), n), (1, 40, 2, 3, 9, 3, 50));
}

// consts/issue-33537.rs: equal string / byte-string literals are one allocation (rustc
// allocate_bytes_dedup), also across fns
fn lit_ptr() -> *const u8 {
    b"dedup-lit" as *const _ as *const u8
}

#[test]
fn c46_literal_dedup() {
    assert_eq!(lit_ptr(), b"dedup-lit" as *const _ as *const u8);
}

// F45 (rustybuzz GSUB ligature.rs:32): an integer literal passed where a projection whose Self
// is still open is expected (`SmallVec::from_elem(0, 4)`: `A::Item`) waits for the projection
// (rustc: a fresh variable under a projection obligation)
unsafe trait ArrayLike {
    type Item;
}

unsafe impl<T, const N: usize> ArrayLike for [T; N] {
    type Item = T;
}

struct SmallV<A: ArrayLike> {
    v: Vec<A::Item>,
}

impl<A: ArrayLike> SmallV<A> {
    fn from_elem(elem: A::Item, n: usize) -> Self
    where
        A::Item: Clone,
    {
        SmallV { v: vec![elem; n] }
    }
}

fn fill_small(x: &mut SmallV<[usize; 4]>) -> usize {
    x.v[1] = 7;
    x.v.iter().sum::<usize>() + x.v.len()
}

#[test]
fn c47_literal_into_open_projection() {
    let mut m = SmallV::from_elem(0, 4);
    assert_eq!(fill_small(&mut m), 11);
}

// F46 (rustybuzz ot_layout_gsubgpos.rs:588): `&impl Fn(..)` coerces to `&dyn Fn(..)` through the
// opaque type's bound
type MatchF = dyn Fn(u32, u16) -> bool;

fn match_same(g: u32, v: u16) -> bool {
    g == v as u32
}

fn match_in(c: &'static [u16]) -> impl Fn(u32, u16) -> bool {
    move |g, v| c[g as usize] == v
}

fn apply3(fs: [&MatchF; 3]) -> (bool, bool, bool) {
    (fs[0](1, 1), fs[1](1, 5), fs[2](0, 3))
}

#[test]
fn c48_opaque_fn_to_dyn() {
    static CLS: [u16; 2] = [4, 5];
    let one: &MatchF = &match_in(&CLS);
    assert_eq!((apply3([&match_same, &match_in(&CLS), &match_in(&CLS)]), one(0, 4)), ((true, true, false), true));
}

// coercion/coerce-expect-unsized.rs: `vec![..]` is `<[_]>::into_vec(box [..])`, so the expected
// `Vec<T>` reaches the elements; `_` inside `dyn Fn(A) -> _` / `fn(_)` is a type variable
#[test]
fn c49_vec_expectation_and_infer_in_dyn() {
    let v: Vec<Box<dyn Fn(isize) -> _>> = vec![Box::new(|x| x as u8), Box::new(|x| x as i16 as u8)];
    let b: Box<dyn Fn(isize) -> _> = Box::new(|x| x as u16);
    let f: fn(_) -> u8 = |x: u8| x + 1;
    let e: Vec<u8> = vec![];
    let n = vec![String::from("a"), String::from("b")];
    assert_eq!((v[0](300), v[1](300), b(70000), f(1), e.len(), n[1].as_str()), (44, 44, 4464, 2, 0, "b"));
}

// std::env::args() under `makepad run` is the program's argv (it was Relax's own command line on
// macOS: `args().len() > 1` held in every program)
#[test]
fn c50_env_args_are_the_programs() {
    let a: Vec<String> = std::env::args().collect();
    assert!(!a.iter().any(|x| x == "--units" || x == "test"));
}

// runtime/atomic-print.rs: inside `impl Tr for X` the trait Tr is in scope for method calls
// (rustc resolve traits_in_scope: current_trait_ref), without a `use`
mod impl_trait_scope {
    pub struct Wrap(pub u32);
    impl core::fmt::Debug for Wrap {
        fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
            self.0.fmt(f)
        }
    }
}

#[test]
fn c51_impl_trait_in_scope() {
    assert_eq!(format!("{:?}", impl_trait_scope::Wrap(7)), "7");
}

// rustc ast_lowering inline_literals: `{}` of a string / in-range integer literal becomes part of
// the pieces, so `format_args!("x {}", 1).as_str()` is Some and `panic!("t {}", "lit")` has a
// `&'static str` payload
#[test]
fn c52_format_args_inline_literals() {
    let p = std::panic::catch_unwind(|| panic!("thread {}", "lit")).unwrap_err();
    assert_eq!(
        (
            format_args!("x {}", 1).as_str(),
            format_args!("a {} b {}", "lit", 0x10u8).as_str(),
            format_args!("{:?}", "lit").as_str(),
            format_args!("{:>3}", 7).as_str(),
            format_args!("{0} {0} {x}", "q", x = 2_000_000_000).as_str(),
            p.downcast_ref::<&'static str>().copied(),
            format!("{:3}|{}", 2, "z"),
        ),
        (Some("x 1"), Some("a lit b 16"), None, None, Some("q q 2000000000"), Some("thread lit"), "  2|z".to_string())
    );
}

// drop/or-pattern-drop-order.rs: an argument-position `impl Trait<Assoc = X>` is an anonymous
// generic param with that bound (`1..=3` becomes u32)
fn apit_items(x: impl IntoIterator<Item = u32>) -> Vec<u32> {
    x.into_iter().collect()
}

#[test]
fn c53_apit_binding_infers_literals() {
    assert_eq!(apit_items(1..=3), [1u32, 2, 3]);
}

// a scope drops its locals in reverse declaration order, not reverse first-assignment order
// (`let (x, y, z); z = ..; y = ..; x = ..;` drops z, y, x)
#[test]
fn c54_deferred_init_drop_order() {
    take_log();
    {
        let (x, y, z);
        z = Holder2("z", None);
        y = Holder2("y", None);
        x = Holder2("x", None);
        let _ = (&x, &y, &z);
    }
    assert_eq!(take_log(), ["z", "y", "x"]);
}

// issues/issue-39808.rs, threads-sendsync/send_str_hashmap.rs: a value against a projection whose
// Self is still open (`Cow::Owned(s)`: `<?B as ToOwned>::Owned`) waits for the projection
#[test]
fn c55_open_projection_equation() {
    use std::borrow::Cow;
    let c = if std::hint::black_box(false) { Cow::Owned(String::from("o")) } else { Cow::Borrowed("b") };
    assert_eq!(c, "b");
}

// where-clauses on non-param types (`where [T]: Concat<Item>`) take part in inference, and a
// candidate impl whose where-clauses cannot hold is dropped (rustc evaluates nested
// obligations): `[String]::concat()` is the `Concat<str>` impl
#[test]
fn c56_concat_join_inference() {
    let v = vec![String::from("a"), String::from("b")];
    let n = [[1, 2], [3, 4]].concat();
    let j = ["x", "y"].join("-");
    assert_eq!((v.concat(), n, j, v.join("+")), ("ab".to_string(), vec![1, 2, 3, 4], "x-y".to_string(), "a+b".to_string()));
}

// issues/issue-51907.rs, self/objects-owned-object-owned-method.rs: `self: Box<Self>` /
// `self: Rc<Self>` methods called on trait objects (the receiver without its metadata, rustc
// DispatchFromDyn); `for` over `&mut dyn Iterator` (iterators/for-loop-over-mut-iterator-21655.rs)
trait Consume {
    fn consume(self: Box<Self>) -> &'static str;
    fn shared(self: std::rc::Rc<Self>) -> usize;
}

impl Consume for Holder2 {
    fn consume(self: Box<Self>) -> &'static str {
        self.0
    }
    fn shared(self: std::rc::Rc<Self>) -> usize {
        std::rc::Rc::strong_count(&self)
    }
}

fn sum_dyn(it: &mut dyn Iterator<Item = u32>) -> u32 {
    let mut s = 0;
    for x in it {
        s += x;
    }
    s
}

#[test]
fn c57_dyn_box_self_and_for_over_dyn() {
    take_log();
    let b: Box<dyn Consume> = Box::new(Holder2("boxed", None));
    let name = b.consume();
    let r: std::rc::Rc<dyn Consume> = std::rc::Rc::new(Holder2("rc", None));
    let r2 = r.clone();
    let n = r.shared();
    drop(r2);
    let s = sum_dyn(&mut vec![1u32, 2, 3].into_iter());
    assert_eq!((name, n, s, take_log()), ("boxed", 2, 6, vec!["boxed", "rc"]));
}

// generics/mid-path-type-params.rs: `Trait::<A>::m::<B>` puts A on the trait and B on the
// method; generics/generic-default-type-params.rs: `Foo` in a type is `Foo<Defaults..>`
trait MakeWith<T> {
    fn make<U: Default>(x: T) -> (Self, U)
    where
        Self: Sized;
}

#[derive(Debug, PartialEq)]
struct Made(isize);

impl MakeWith<isize> for Made {
    fn make<U: Default>(x: isize) -> (Made, U) {
        (Made(x), U::default())
    }
}

struct Deflt<A = (isize, char)> {
    a: A,
}

fn deflt_first(x: Deflt) -> isize {
    x.a.0
}

#[test]
fn c58_trait_turbofish_and_type_defaults() {
    let (m, u): (Made, u8) = MakeWith::<isize>::make::<u8>(4);
    assert_eq!((m, u, deflt_first(Deflt { a: (7, 'c') })), (Made(4), 0, 7));
}

// array-slice-vec/subslice-patterns-const-eval.rs: a `ref` binding into a promotable let
// initializer borrows a promoted static (also in a const fn / const initializer);
// consts/zst_no_llvm_alloc.rs: a promoted zero-sized value is at its alignment
#[derive(PartialEq, Debug)]
struct Nn(u8);

const SUB_C: &[Nn; 2] = {
    let [_, ref x @ .., _] = [Nn(1), Nn(2), Nn(3), Nn(4)];
    x
};

const fn sub_f() -> &'static [Nn; 2] {
    let [_, ref x @ .., _] = [Nn(1), Nn(2), Nn(3), Nn(4)];
    x
}

#[test]
fn c59_promoted_ref_bindings_and_zst_address() {
    let z: &'static () = &();
    assert_eq!((SUB_C, sub_f(), z as *const () as usize), (&[Nn(2), Nn(3)], &[Nn(2), Nn(3)], 1));
}

// cast/as-cast-unifies-raw-pointer-and-fn-type.rs, cast/cast.rs: a cast that is a coercion is
// one (`&() as *const _` has pointee `()`, `generic as fn(i32)` instantiates it);
// coercion/coerce-unify-return.rs: a method call's expected result types its arguments
fn raw_of<T>(p: *const T) -> usize {
    std::mem::size_of::<T>() + (p as usize & 0)
}

fn takes<T>(_t: T) -> u8 {
    1
}

struct Wrapf;

impl Wrapf {
    fn wrap<T>(self, x: T) -> Option<T> {
        Some(x)
    }
}

fn unit_fn() {}

#[test]
fn c60_cast_coercions_and_method_expectation() {
    let p = &7u64 as *const _;
    let f = takes as fn(i32) -> u8;
    let o: Option<fn()> = Wrapf.wrap(unit_fn);
    assert_eq!((raw_of(p), f(3), o.is_some()), (8, 1, true));
}

// overloaded/overloaded-index.rs: an integer literal index is typed by the selected Index impl
// (`impl Index<isize> for Foo`: `f[1]` is isize), generic ones (`Vec<T>`) take usize; an impl's
// params fixed only by a bound's binding are completed (traits/index-trait-multiple-impls-15734.rs)
struct Pair2 {
    x: isize,
    y: isize,
}

impl std::ops::Index<isize> for Pair2 {
    type Output = isize;
    fn index(&self, z: isize) -> &isize {
        if z == 0 { &self.x } else { &self.y }
    }
}

struct RowOf<M> {
    m: M,
}

impl<T, M: std::ops::Index<usize, Output = T>> std::ops::Index<u8> for RowOf<M> {
    type Output = T;
    fn index(&self, i: u8) -> &T {
        &self.m[i as usize]
    }
}

#[test]
fn c61_literal_index_typed_by_impl() {
    let p = Pair2 { x: 1, y: 2 };
    let v = vec![10, 20, 30];
    let r = RowOf { m: vec![5, 6] };
    assert_eq!((p[1], v[2], r[1]), (2, 30, 6));
}

// `&mut T` is noalias only for T: Unpin (rustc): a !Unpin value may be written through a pointer
// into itself while the `&mut` is live
struct SelfRef {
    x: u32,
    p: *mut u32,
    _pin: std::marker::PhantomPinned,
}

#[inline(never)]
fn write_through_self(s: &mut SelfRef) -> u32 {
    s.x = 1;
    unsafe { *s.p = 2 };
    s.x
}

#[test]
fn c62_not_unpin_mut_ref_may_alias() {
    let mut s = Box::pin(SelfRef { x: 0, p: std::ptr::null_mut(), _pin: std::marker::PhantomPinned });
    let r = unsafe { s.as_mut().get_unchecked_mut() };
    r.p = &mut r.x;
    let f: fn(&mut SelfRef) -> u32 = std::hint::black_box(write_through_self);
    assert_eq!(f(r), 2);
}

// or-patterns/issue-70413-no-unreachable-pat-and-guard.rs, search-via-bindings.rs,
// drop/or-pattern-drop-order.rs: a guarded arm tries each or-alternative combination as its own
// candidate (the guard runs per candidate), its moves happen only once the guard holds (a
// failing guard left moved-out parts that nothing dropped), and array elements moved out by a
// pattern are tracked one by one (the rest drops last range first, as rustc's drop ladder)
#[test]
fn c63_guarded_or_candidates_no_leak() {
    let mut runs = 0;
    let hit = match (3, 42) {
        (a, _) | (_, a)
            if {
                runs += 1;
                a > 10
            } =>
        {
            a
        }
        _ => 0,
    };
    take_log();
    match [Holder2("6", None), Holder2("4", None)] {
        [_, w] | [w, _] if w.0 == "nope" => unreachable!(),
        [_, w] if true => {
            let _ = &w;
        }
        _ => unreachable!(),
    }
    let after_match = take_log();
    match [[D("6"), D("4")], [D("3"), D("2")]] {
        [[_, w], [x, _]] if true => {
            let _ = (&w, &x);
        }
        _ => unreachable!(),
    }
    assert_eq!((hit, runs, after_match, take_log()), (42, 2, vec!["4", "6"], vec!["3", "4", "2", "6"]));
}

// expr/compound-assignment/eval-order.rs: rustc decides built-in vs overloaded `a op= b` on the
// generic body, so in a generic fn where the left operand's type is a param the left operand is
// evaluated first even in the i32 instance (built-in compound assignment: right first)
struct Wrapper2<T>(T);

fn generic_compound<T: std::ops::AddAssign<T> + Default>() -> Vec<&'static str> {
    let mut order = vec![];
    let mut w: Wrapper2<T> = Wrapper2(Default::default());
    {
        order.push("lhs");
        &mut w
    }
    .0 += {
        order.push("rhs");
        Default::default()
    };
    order
}

#[test]
fn c64_generic_compound_assign_order() {
    let mut order = vec![];
    let mut int = Wrapper2(0);
    {
        order.push("lhs");
        &mut int
    }
    .0 += {
        order.push("rhs");
        0
    };
    assert_eq!((order, generic_compound::<i32>()), (vec!["rhs", "lhs"], vec!["lhs", "rhs"]));
}

// let-else/let-else-drop-order.rs: a field of a temporary (`make().1`) is a value the statement
// owns -- a non-matching one is dropped before the else block (it leaked: it was taken for a
// place), and so are lifetime-extended temporaries (`let 0 = &make().1 else`)
enum TwoWay {
    Yes(u8),
    No(u8),
}

impl Drop for TwoWay {
    fn drop(&mut self) {
        LOG.with(|l| l.borrow_mut().push(match self {
            TwoWay::Yes(_) => "yes",
            TwoWay::No(_) => "no",
        }));
    }
}

fn make_two() -> ((), TwoWay) {
    ((), TwoWay::No(1))
}

fn make_held() -> (D, u32) {
    (D("held"), 1)
}

#[test]
fn c65_let_else_field_of_temporary() {
    take_log();
    loop {
        let TwoWay::Yes(_) = make_two().1 else {
            LOG.with(|l| l.borrow_mut().push("else1"));
            break;
        };
        break;
    }
    loop {
        let 0 = &make_held().1 else {
            LOG.with(|l| l.borrow_mut().push("else2"));
            break;
        };
        break;
    }
    assert_eq!(take_log(), ["no", "else1", "held", "else2"]);
}

// hygiene/hygiene.rs, binding/match-var-hygiene.rs, hygiene/hygienic-labels.rs: macro_rules
// locals and labels are hygienic (rustc SyntaxContext marks): a body's bindings do not capture
// or shadow the caller's, and a body's free variables resolve where the macro is defined
macro_rules! shadow_in_body {
    ($ex:expr) => {{
        match 9 {
            _x => $ex,
        }
    }};
}

macro_rules! loop_x {
    ($e:expr) => {
        'x: loop {
            $e
        }
    };
}

#[test]
fn c66_macro_rules_hygiene() {
    let x = 0;
    macro_rules! def_site {
        () => {
            x
        };
    }
    let x = 1;
    let seen = def_site!();
    let caller = match 8 {
        _x => shadow_in_body!(_x),
    };
    let mut broke_outer = true;
    'x: for _ in 0..1 {
        loop_x!(break 'x);
        #[allow(unreachable_code)]
        {
            broke_outer = false;
        }
    }
    assert_eq!((seen, x, caller, broke_outer), (0, 1, 8, true));
}

// traits/trait-upcasting/correct-supertrait-substitution.rs, basic.rs, diamond.rs: a trait
// object's vtable has one entry set per distinct supertrait instantiation (rustc
// vtable_entries: Bar<i32> and Bar<bool> are two), upcasting picks the instantiation, and a
// static call of a trait method with Self = the trait object (`*a == 3` through a supertrait
// PartialEq<i32>) is a virtual call (InstanceKind::Virtual)
trait BarOf<T: Default + ToString> {
    fn bar(&self) -> String {
        T::default().to_string()
    }
}

trait FooOf<T: Default + ToString>: BarOf<i32> + BarOf<T> {}

struct TwoBars;

impl BarOf<i32> for TwoBars {}
impl BarOf<bool> for TwoBars {}
impl FooOf<bool> for TwoBars {}

trait EqFoo: PartialEq<i32> {}

struct Three(i32);

impl PartialEq<i32> for Three {
    fn eq(&self, o: &i32) -> bool {
        self.0 == *o
    }
}

impl EqFoo for Three {}

#[test]
fn c67_generic_supertrait_vtables() {
    let x: &dyn FooOf<bool> = &TwoBars;
    let p = x as &dyn BarOf<i32>;
    let q = x as &dyn BarOf<bool>;
    let e: Box<dyn EqFoo> = Box::new(Three(3));
    assert_eq!((p.bar(), q.bar(), *e == 3, *e == 4), ("0".to_string(), "false".to_string(), true, false));
}

// c68: an integer literal's type variable only unifies with integral types (rustc IntVar), so
// the impl on `W<&i64>` is no candidate for `W({integer})`; a `Mutex<dyn Fn>` reached through
// `Arc` unsizing finds its closure at the sized instance's offset (reordered prefix fields)
struct LitW<T>(T);
trait LitK {
    fn k(&self) -> u32;
}
impl LitK for LitW<i64> {
    fn k(&self) -> u32 {
        1
    }
}
impl<'a> LitK for LitW<&'a i64> {
    fn k(&self) -> u32 {
        2
    }
}

#[test]
fn c68_literal_vars_and_dyn_mutex() {
    let one = 7i64;
    assert_eq!((LitW(5).k(), LitW(&one).k()), (1, 2));
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let h2 = hits.clone();
    let tag = 0x5au8;
    let f: std::sync::Arc<std::sync::Mutex<dyn Fn(u64) -> u64 + Send>> =
        std::sync::Arc::new(std::sync::Mutex::new(move |x: u64| {
            h2.fetch_add(x, std::sync::atomic::Ordering::Relaxed);
            x + tag as u64
        }));
    let r = f.lock().unwrap()(3);
    assert_eq!((r, hits.load(std::sync::atomic::Ordering::Relaxed)), (93, 3));
}

// c69: concat! writes literals by value (rustc concat.rs): integers in decimal without suffix,
// floats as written without suffix and underscores
#[test]
fn c69_concat_literal_values() {
    assert_eq!(concat!(0x1b, "-", 0o17, "-", 0b101, "-", 1_000u32, "-", -0x10i8, "-", 1_0.5e3f64, "-", 2.5, "-", 'c', true), "27-15-5-1000--16-10.5e3-2.5-ctrue");
}
