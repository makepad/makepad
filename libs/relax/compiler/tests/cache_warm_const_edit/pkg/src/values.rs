// What tests/cache_warm_const_edit/run.py edits, one item per probe.
#![allow(dead_code)]
use std::sync::atomic::AtomicU64;
pub const CONST_DIRECT: u64 = 1;
pub const CONST_THROUGH: u64 = 1;
pub const CONST_INLINED: u64 = 1;
pub const CONST_BASE: u64 = 1;
pub const CONST_DERIVED: u64 = CONST_BASE * 2;
pub const CONST_PATTERN: u64 = 1;
pub const CONST_LEN: usize = 1;
pub const CONST_LATE: u64 = 1;
pub const CONST_INCLUDE: &str = include_str!("data.txt");
// a table of included texts, one through a macro's `concat!` path (an app's demo table)
pub struct Doc {
    pub key: &'static str,
    pub text: &'static str,
}
macro_rules! doc {
    ($key:literal) => {
        Doc { key: $key, text: include_str!(concat!($key, ".txt")) }
    };
}
pub const DOCS: &[Doc] = &[doc!("doc_a"), Doc { key: "doc_b", text: include_str!("doc_b.txt") }];
// an include whose path no text names (`env!`): its save asks for a restart
pub const DOC_E: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/doc_e.txt"));
pub const CONST_SAME: u64 = 1;
pub const CONST_MACRO: u64 = 1;
pub static STATIC_DIRECT: u64 = 1;
pub static STATIC_THROUGH: u64 = 1;
pub static STATIC_LATE: u64 = 1;

pub trait Kind {
    const K: u64;
}
pub struct A;
impl Kind for A {
    const K: u64 = 1;
}
pub struct B;
impl Kind for B {
    const K: u64 = 1;
}

// The value of an impl, reached only through a type argument of generic code: the code's text names
// a parameter, and the argument may be wrapped, foreign, inferred or hidden.
impl Kind for warm_const_dep::DepT {
    const K: u64 = 1;
}
pub struct E1;
impl warm_const_dep::DepKind for E1 {
    const DK: u64 = 1;
}
pub struct W3;
impl Kind for Vec<W3> {
    const K: u64 = 1;
}
impl Kind for u32 {
    const K: u64 = 1;
}
pub trait Mark {}
pub trait Blank {
    const BL: u64;
}
impl<T: Mark> Blank for T {
    const BL: u64 = 1;
}
pub struct M1;
impl Mark for M1 {}
pub struct Cl;
impl Kind for Cl {
    const K: u64 = 1;
}
pub struct Wrap<T>(pub T);
impl<T: Kind> Kind for Wrap<T> {
    const K: u64 = T::K + 1;
}
pub struct A4;
impl Kind for A4 {
    const K: u64 = 1;
}
pub struct Op;
impl Kind for Op {
    const K: u64 = 1;
}
pub fn make_op() -> impl Kind {
    Op
}
pub struct Inf;
impl Kind for Inf {
    const K: u64 = 1;
}
pub fn make_inf() -> Inf {
    Inf
}

pub trait Dflt {
    const D: u64 = 1;
    fn m() -> u64 {
        1
    }
}
pub struct Df;
impl Dflt for Df {}

pub trait Shape {
    type Out;
}
pub struct C;
impl Shape for C {
    type Out = u8;
}
pub struct C2;
impl Shape for C2 {
    type Out = u8;
}
#[repr(C)]
pub struct Holder<T: Shape> {
    pub v: T::Out,
}

// Consts in a type: an array's length, a const argument. (repr(C): a JIT's struct keeps room for
// fields an edit adds, and size_of would show that)
pub const CONST_GEN: usize = 1;
pub const CONST_GEN_TY: usize = 1;
pub const CONST_ARR: usize = 1;
pub const CONST_ARR_TY: usize = 1;
pub struct G<const N: usize>;
impl<const N: usize> Kind for G<N> {
    const K: u64 = N as u64;
}
#[repr(C)]
pub struct Buf {
    pub data: [u8; CONST_ARR],
}
// Its own name: a same-named const that only values read would restart too (by name).
pub trait LenKind {
    const LK: u64;
}
pub struct D1;
impl LenKind for D1 {
    const LK: u64 = 1;
}
#[repr(C)]
pub struct BufK {
    pub data: [u8; <D1 as LenKind>::LK as usize],
}

// An impl comes or goes. A type's Drop decides what generic code folds and which glue it runs, and
// a type's Copy which of std's specializations an instance takes.
pub struct Plain;
// impl_added
pub struct Droppy;
impl Drop for Droppy {
    fn drop(&mut self) {}
}
pub static GLUE_DROPS: AtomicU64 = AtomicU64::new(0);
#[derive(Default)]
pub struct Glue;
// impl_added_glue
pub struct Sp(pub u64);
impl Clone for Sp {
    fn clone(&self) -> Sp {
        Sp(self.0 + 1)
    }
}
// impl_added_spec

#[inline(never)]
pub fn same_file() -> u64 {
    CONST_SAME
}

#[inline]
pub fn tiny() -> u64 {
    1
}

#[inline]
pub fn late_tiny() -> u64 {
    1
}

#[inline(always)]
pub fn late_always() -> u64 {
    1
}
// dyn_method_add: a trait object made once, called through its vtable on every row; the edit adds
// a method ahead of the one called (its vtable entry moves) and changes what it returns
pub trait Dm {
    fn a(&self) -> u64;
}
pub struct DmX;
impl Dm for DmX {
    fn a(&self) -> u64 {
        1
    }
}
pub static DM: std::sync::OnceLock<Box<dyn Dm + Send + Sync>> = std::sync::OnceLock::new();
#[inline(never)]
pub fn dm_held() -> u64 {
    DM.get_or_init(|| Box::new(DmX)).a()
}
// (dyn_method_add_late: the same call, first made after the edit)
#[inline(never)]
pub fn dm_late() -> u64 {
    DM.get_or_init(|| Box::new(DmX)).a()
}
// dyn_vtable_fresh: the trait object is made by code whose text the edit leaves; the edit adds a
// method ahead of the one called and a reader calls both
pub trait Dv {
    fn a(&self) -> u64;
}
pub struct DvX;
impl Dv for DvX {
    fn a(&self) -> u64 {
        1
    }
}
pub fn dv_read() -> u64 {
    dv_held().a()
}
pub static DV: std::sync::OnceLock<Box<dyn Dv + Send + Sync>> = std::sync::OnceLock::new();
#[inline(never)]
pub fn dv_held() -> &'static (dyn Dv + Send + Sync) {
    &**DV.get_or_init(|| Box::new(DvX))
}
// dyn_impl_removed: an impl removed while a trait object of it lives, the code that made it edited
// to make another one: the edit lands, the object keeps its vtable
pub struct Ri(pub u64);
impl std::fmt::Display for Ri {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
pub const RI_K: u64 = 0;
pub fn ri_make() -> Box<dyn std::fmt::Display + Send + Sync> {
    Box::new(Ri(7))
}
pub static RI: std::sync::OnceLock<Box<dyn std::fmt::Display + Send + Sync>> = std::sync::OnceLock::new();
#[inline(never)]
pub fn ri_read() -> u64 {
    RI.get_or_init(ri_make).to_string().len() as u64 + RI_K
}

// enum_insert: a value held from the first run, a variant inserted before it (a running variant
// keeps its tag by name) and the reader edited to build the new one and order it
#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub enum Ev {
    A,
    B(u32),
    C { level: u8 },
}
pub static EV: std::sync::Mutex<Option<Ev>> = std::sync::Mutex::new(None);
#[inline(never)]
pub fn ev_read() -> u64 {
    let mut g = EV.lock().unwrap();
    let held = g.get_or_insert(Ev::C { level: 4 });
    let level = match held {
        Ev::C { level } => *level as u64,
        _ => 0,
    };
    level + if Ev::A < *held { 10 } else { 0 }
}
// enum_insert_cast: a fieldless enum read as a number (`as`) by running code; a variant inserted
// before a running one moves its number: the edit restarts
#[derive(Clone, Copy)]
pub enum Lv {
    Low,
    High,
}
#[inline(never)]
pub fn lv_index(l: Lv) -> u64 {
    l as u64
}
pub fn lv_read() -> u64 {
    lv_index(Lv::High)
}
// enum_insert_hash: a derived `Hash` ran (a set holds a value's hash, made from the variant's
// position); a variant inserted before the held one moves it: the edit restarts
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hk {
    P,
    Q,
}
pub const HK_K: u64 = 0;
pub static HK: std::sync::Mutex<Option<std::collections::HashSet<Hk>>> = std::sync::Mutex::new(None);
#[inline(never)]
pub fn hk_read() -> u64 {
    let mut g = HK.lock().unwrap();
    let set = g.get_or_insert_with(|| [Hk::Q].into_iter().collect());
    set.contains(&Hk::Q) as u64 + HK_K
}

pub const CONST_NONLIT: u64 = 1 + 0;

pub const CONST_DISCR: isize = 1;
#[repr(isize)]
pub enum Dsc {
    One = CONST_DISCR,
}

pub const CONST_ARR_BASE: usize = 1;
pub const CONST_ARR_DER: usize = CONST_ARR_BASE * 2;

pub const CONST_FOR_STATIC: u64 = 1;
pub static STATIC_FROM_CONST: u64 = CONST_FOR_STATIC;

pub const CONST_FOR_FN: u64 = 1;
pub const fn through_fn() -> u64 {
    CONST_FOR_FN
}
pub static STATIC_THROUGH_FN: u64 = through_fn();

// Array lengths whose text does not name the const.
pub const CONST_LEN_FN: usize = 1;
pub const fn len_fn() -> usize {
    CONST_LEN_FN
}
#[repr(C)]
pub struct BufFn {
    pub data: [u8; len_fn()],
}
pub const CONST_LEN_MAC: usize = 1;
macro_rules! len_mac {
    () => {
        CONST_LEN_MAC
    };
}
#[repr(C)]
pub struct BufMac {
    pub data: [u8; len_mac!()],
}
pub const CONST_LEN_AL: usize = 1;
mod al {
    pub use super::CONST_LEN_AL as RENAMED_LEN;
}
#[repr(C)]
pub struct BufAl {
    pub data: [u8; al::RENAMED_LEN],
}

// A static's initializer names another type's `new`; and one that does read the edit through its own.
pub const CONST_FOR_NEW: u64 = 1;
pub struct ForNew(pub u64);
impl ForNew {
    pub const fn new() -> Self {
        ForNew(CONST_FOR_NEW)
    }
}
pub struct OtherNew(pub u64);
impl OtherNew {
    pub const fn new() -> Self {
        OtherNew(7)
    }
}
pub static STATIC_OTHER_NEW: OtherNew = OtherNew::new();
pub const CONST_THROUGH_NEW: u64 = 1;
pub struct ViaNew(pub u64);
impl ViaNew {
    pub const fn new() -> Self {
        ViaNew(CONST_THROUGH_NEW)
    }
}
pub static STATIC_THROUGH_NEW: ViaNew = ViaNew::new();

// A Copy struct no running value holds (a frame of the loop never sees one): a field added
// before another moves it, and the reader whose text stays builds an outer struct holding it.
#[derive(Clone, Copy)]
pub struct Cp {
    pub a: u32,
    pub b: u64,
}
#[inline(never)]
pub fn cp_make(i: u32) -> Cp {
    Cp { a: i, b: (i as u64) << 8 }
}
pub struct CpOuter {
    pub tag: u8,
    pub cs: [Cp; 2],
}
#[inline(never)]
pub fn cp_read() -> u64 {
    let o = CpOuter { tag: 1, cs: [cp_make(1), cp_make(2)] };
    o.cs.iter().map(|c| c.a as u64 + c.b).sum::<u64>() + o.tag as u64
}

// A Copy struct the loop's frame holds: a field added restarts (the frame may hold one in the
// old layout).
#[derive(Clone, Copy)]
pub struct CpF {
    pub a: u32,
    pub b: u64,
}
#[inline(never)]
pub fn cpf_new() -> CpF {
    CpF { a: 1, b: 1 }
}
