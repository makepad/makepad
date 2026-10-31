//! The framework crate two apps share (compiler/tests/cache_shared_apps/run.py).

macro_rules! mk_fn {
    ($name:ident = $v:expr) => {
        pub fn $name() -> u32 { $v + 1 } // EDIT mk_fn
    };
}

mk_fn!(from_macro = 40); // EDIT macro

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shape { pub w: u32, pub h: u32 } // EDIT shape

impl Shape { pub fn new(w: u32, h: u32) -> Shape { Shape { w, h, ..Default::default() } } } // EDIT new

pub fn add(a: u32, b: u32) -> u32 { a + b } // EDIT add

pub fn scaled<T: Into<u64>>(t: T) -> u64 { t.into() * 3 } // EDIT scaled

pub fn area(s: &Shape) -> u32 { s.w * s.h }

pub fn describe(s: &Shape) -> usize { format!("{:?}", s).len() }

pub fn cfg_macro() -> u32 { if cfg!(feature = "fast") { 1 } else { 0 } }

#[cfg(feature = "fast")]
pub fn fast_only() -> u32 { 77 }

pub fn gated() -> u32 {
    #[cfg(feature = "fast")]
    { 5 }
    #[cfg(not(feature = "fast"))]
    { 6 }
}

pub fn unrelated() -> u32 { 1234 }

pub fn uses_unrelated() -> u32 { unrelated() + 1 }

pub fn sum_to(n: u32) -> u32 { (1..=n).sum() }

pub fn pow_std(a: u32) -> u32 { a.pow(3) }

pub fn flavor() -> u32 {
    #[cfg(flavor_b)]
    { 2 }
    #[cfg(not(flavor_b))]
    { 1 }
}

pub fn envv() -> usize { option_env!("SC_ENVV").unwrap_or("none").len() } // EDIT envv

pub fn data() -> usize { include_str!("../data.txt").len() }

#[path = "alt.rs"]
mod alt;
pub fn alt_val() -> u32 { alt::val() + alt::alt_made() }

// What a code key must see of other items through names (cache/deps.rs): a glob import shadowed by a
// new item, a method a macro's expansion calls (`write!` -> `write_fmt`), a macro's definition, a trait
// impl's items, a generic fn's body with an `impl Trait` argument.
mod glob_src { pub fn which() -> u32 { 1 } }
use glob_src::*;
// EDIT shadow
pub fn uses_which() -> u32 { which() }

pub struct Sink(pub u32);
impl std::fmt::Write for Sink { fn write_str(&mut self, s: &str) -> std::fmt::Result { self.0 += s.len() as u32; Ok(()) } }
// EDIT inherent
pub fn sink_len() -> u32 { use std::fmt::Write as _; let mut s = Sink(0); let _ = write!(s, "abc"); s.0 }

macro_rules! seven { () => { 7 } } // EDIT seven
pub fn uses_seven() -> u32 { seven!() }

pub trait Greet { fn greet(&self) -> u32 { 1 } }
pub struct Plain;
impl Greet for Plain {} // EDIT greet
pub fn greet_plain() -> u32 { Plain.greet() }

pub fn apply(f: impl Fn(u32) -> u32) -> u32 { f(2) } // EDIT apit

pub fn wrap<T: Default + std::fmt::Debug>() -> usize { format!("{:?}", T::default()).len() }

// Values a static's or constant's initializer computes through const fns of this crate (cache.rs
// record_const: the value entry is certified by the bodies CTFE ran)
pub const fn cval(x: u32) -> u32 { x * 3 } // EDIT cval
pub const fn cval_outer(x: u32) -> u32 { cval(x) + 1 }
pub const FW_CONST: u32 = cval_outer(5);
pub static FW_STATIC: u32 = cval(7) + 1;

// an exported macro another crate's file uses (JITCOLD7: that file's image is taken after an edit
// here only while its lookup replays to the same rules)
#[macro_export]
macro_rules! fw_twice { ($e:expr) => { $e * 2 } } // EDIT fw_twice
