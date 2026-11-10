// run.py edits this file: each marked line is replaced by its edit, one scenario at a time.

pub fn stable_before() -> u32 { 7 }

pub struct Shape { pub a: u32, pub b: u32 } // EDIT layout

pub const SCALE: u32 = 3; // EDIT const

pub static LEVEL: u32 = 5; // EDIT static

pub trait Speak {
    fn speak(&self) -> u32;
    fn loud(&self) -> u32 { 1 }
}

pub struct Dog;

impl Speak for Dog {
    fn speak(&self) -> u32 { 10 } // EDIT body
    // EDIT impl
}

#[inline]
pub fn leaf() -> u32 { 100 } // EDIT inline

/// leaf's caller in this crate (the app crate's code is lowered before its check's verdict here:
/// baseline code, which inlines no user-crate fn)
#[inline(never)]
pub fn leaf_user() -> u32 { leaf() + 1 }

/// The size of `T` as code instantiated with `T` has it (no constant the caller folded).
#[inline(never)]
pub fn size_in<T>(v: &[T]) -> usize { std::mem::size_of::<T>() * v.len() }

pub fn local_size() -> usize {
    struct Local { a: u8 } // EDIT local
    let l = vec![Local { a: 1 }];
    size_in(&l) + l[0].a as usize
}

pub fn untouched_user() -> u32 { stable_before() + 1 }

pub fn stable_after() -> u32 { 9 }

// Macro-expanded content: an edit inside an invocation must reach the code it feeds.
macro_rules! mk_fn { ($n:ident = $v:expr) => { pub fn $n() -> u32 { $v } } }
macro_rules! mk_static { ($n:ident = $v:expr) => { pub static $n: u32 = $v; } }
macro_rules! plus_one { ($v:expr) => { $v + 1 } }
mk_fn!(from_mac = 7); // EDIT macro_item
mk_static!(MAC_LEVEL = 70); // EDIT macro_static
pub fn mac_body() -> u32 { plus_one!(20) } // EDIT macro_body
