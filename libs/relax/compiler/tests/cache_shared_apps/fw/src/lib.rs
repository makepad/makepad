//! The framework crate two apps share (compiler/tests/cache_shared_apps/run.py).

macro_rules! mk_fn {
    ($name:ident = $v:expr) => {
        pub fn $name() -> u32 { $v + 1 }
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
pub fn alt_val() -> u32 { alt::val() + 1 }

pub fn wrap<T: Default + std::fmt::Debug>() -> usize { format!("{:?}", T::default()).len() }
