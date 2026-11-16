//! Relax provided-crate fixture (loader-core.md section 8, step 3). rustc builds this crate into
//! the `makepad` loader with its shims (feature `host-shims`); under `makepad run` Relax loads its
//! source for the bin but binds the C-shaped non-generic fns below to the loader's code.
//! `bound()` tells which side ran: only the loader's build has the feature.
//! `provided_static.sh` covers marked shared statics (public, private, function-local, mutable,
//! Cell-backed and arrays), per-side memo storage and thread-locals. Unmarked PLAIN_COUNT and
//! layout-incompatible SHARED_LOG must be refused when reached from JIT code.
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use relax_api::{relax_api, string_from_words, string_to_words, vec_from_words, vec_to_words, Words};

/// true when this call ran the loader's rustc-built code
#[relax_api(run)]
pub fn bound() -> bool {
    cfg!(feature = "host-shims")
}

/// Only in the loader's build of the fixture (feature `wide`, which its own bins leave off): a
/// fixture run builds the crate with fewer features than the loader, and runs with the loader's.
#[cfg(feature = "wide")]
#[relax_api(run)]
pub fn wide_marker() -> u32 {
    7
}

#[relax_api(run)]
pub fn add(a: i32, b: i64) -> i64 {
    #[cfg(feature = "wide")]
    debug_assert_eq!(wide_marker(), 7);
    a as i64 + b
}

#[relax_api(run)]
pub fn mix(a: f32, b: f64, c: u8, d: bool, e: char) -> f64 {
    a as f64 * b + c as f64 + if d { 0.5 } else { 0.0 } + e as u32 as f64
}

#[relax_api(plain, run)]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct P {
    pub x: f32,
    pub y: f32,
}

#[relax_api(run)]
pub fn scale(p: P, k: f32) -> P {
    P { x: p.x * k, y: p.y * k }
}

/// passed and returned in memory
#[relax_api(plain, run)]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Big {
    pub a: [u64; 6],
}

#[relax_api(run)]
pub fn big_make(x: u64) -> Big {
    Big { a: [x, x + 1, x + 2, x + 3, x + 4, x + 5] }
}

#[relax_api(run)]
pub fn big_sum(b: Big) -> u64 {
    b.a.iter().sum()
}

#[relax_api(run)]
pub fn text_len(s: &str) -> usize {
    s.chars().count()
}

#[relax_api(run)]
pub fn sum(v: &[u32]) -> u64 {
    v.iter().map(|x| *x as u64).sum()
}

#[relax_api(plain, run)]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    A = 1,
    B = 7,
}

#[relax_api(plain, run)]
#[repr(align(16))]
#[derive(Clone, Copy)]
pub struct V(pub [f32; 4]);

#[relax_api(plain, run)]
#[derive(Clone, Copy)]
pub struct Id(pub u64);

// Concrete ops, so each Option spelling is exported and bound independently. The bin has
// its own constructors and checksums; neither side uses the other's expected-value code.
macro_rules! tagged_option_ops {
    ($module:ident, $ty:ty, $seed:ident => $value:expr, $x:ident => $sum:expr) => {
        pub mod $module {
            use super::*;

            fn make(present: bool, $seed: u64) -> Option<$ty> {
                if present { Some($value) } else { None }
            }

            fn sum(value: &Option<$ty>) -> u64 {
                match value { None => 17, Some($x) => 29u64.wrapping_add($sum) }
            }

            #[relax_api(handle, run)]
            pub struct Holder {
                pub guard: u64,
                pub value: Option<$ty>,
            }

            #[relax_api(run)]
            pub fn exchange(value: Option<$ty>, expected: u64, present: bool, seed: u64) -> Option<$ty> {
                assert_eq!(sum(&value), expected, "native by-value input");
                make(present, seed)
            }

            #[relax_api(run)]
            pub fn by_ref(value: &Option<$ty>, expected: u64) -> &Option<$ty> {
                assert_eq!(sum(value), expected, "native shared-reference input");
                value
            }

            #[relax_api(run)]
            pub fn by_mut(value: &mut Option<$ty>, expected: u64, present: bool, seed: u64) -> &mut Option<$ty> {
                assert_eq!(sum(value), expected, "native mutable-reference input");
                *value = make(present, seed);
                value
            }

            #[relax_api(run)]
            pub fn field(value: Holder, expected: u64, present: bool, seed: u64) -> Holder {
                assert_eq!(value.guard, 0x1234_5678_9abc_def0, "native handle guard");
                assert_eq!(sum(&value.value), expected, "native handle Option field");
                Holder { guard: value.guard, value: make(present, seed) }
            }

            /// Reverse calls use the same memory-slot transport as an op shim, avoiding any
            /// dependence on rustc's Rust ABI for Option or an improper C ABI for aggregates.
            #[relax_api(run)]
            pub fn reverse(callback: usize, present: bool, seed: u64) -> u64 {
                // SAFETY: the bin supplies a live callback with exactly this C-unwind signature.
                let call: extern "C-unwind" fn(*mut u8, u32, u64, bool, u64) -> u64 = unsafe { std::mem::transmute(callback) };
                let mut total = 0u64;
                for mode in 0..4 {
                    let mut value = Holder { guard: 0x1234_5678_9abc_def0, value: make(present, seed) };
                    let before = sum(&value.value);
                    let data = if mode == 3 { (&mut value as *mut Holder).cast::<u8>() }
                        else { (&mut value.value as *mut Option<$ty>).cast::<u8>() };
                    let got = call(data, mode, before, !present, seed + 1);
                    let expected = if mode == 1 { before } else { sum(&make(!present, seed + 1)) };
                    assert_eq!(got, expected, "native checks caller checksum, mode {mode}");
                    assert_eq!(sum(&value.value), expected, "native checks caller output bytes, mode {mode}");
                    assert_eq!(value.guard, 0x1234_5678_9abc_def0, "caller preserves handle guard");
                    total = total.wrapping_add(got.wrapping_mul(mode as u64 + 1));
                }
                total
            }
        }
    };
}

tagged_option_ops!(option_u32, u32, seed => 0xfeed_0000 | seed as u32, x => *x as u64);
tagged_option_ops!(option_f32, f32, seed => seed as f32 + 0.25, x => x.to_bits() as u64);
tagged_option_ops!(option_u64, u64, seed => (seed << 40) | 0xabcd, x => x.rotate_left(13));
tagged_option_ops!(option_u128, u128,
    seed => ((seed as u128) << 100) | ((seed as u128) << 52) | 0xcafe,
    x => ((*x >> 64) as u64).wrapping_mul(31).wrapping_add(*x as u64));
tagged_option_ops!(option_v, V,
    seed => V([seed as f32 + 0.25, seed as f32 + 0.5, seed as f32 + 0.75, seed as f32 + 1.0]),
    x => x.0.iter().enumerate().map(|(i, v)| (2 * i as u64 + 1) * v.to_bits() as u64).sum::<u64>());
tagged_option_ops!(option_id, Id, seed => Id((seed << 44) | 0xbeef), x => x.0.rotate_left(7));

/// Primitive repr keeps Byte's u8 immediately after the tag, even with a u64 variant.
/// The attribute above the mark must be visible to the layout exporter too.
#[repr(u8)]
#[relax_api(plain, run)]
pub enum PlainEnumLayout {
    Empty = 3,
    Byte(u8),
    Named { byte: u8, wide: u64 } = 1 << 4,
    Last,
}

/// Owning payloads use the same export for handles. The loader also enables `wide`,
/// checking both sides of the variant/field cfgs when it builds this fixture.
#[relax_api(handle, run)]
#[repr(u8)]
pub enum OwnedEnumLayout {
    Empty,
    Words(Vec<u32>) = 5,
    Boxed { byte: u8, value: Box<Site> },
    Tuple(#[cfg(feature = "wide")] u64, u8),
    // (a field cfg'd out always: a `not(feature)` cfg would make `wide` non-additive, provided.rs)
    Named { #[cfg(any())] byte: u8, wide: u64 },
    #[cfg(feature = "wide")]
    Extra(u32),
    Last,
}

#[relax_api(plain, run)]
#[repr(u16, align(8))]
pub enum WideEnumLayout {
    Empty = 0x100,
    Byte(u8),
    Named { value: u64 } = 0x200,
}

/// repr(C, u8) shares one payload offset across variants: Byte starts after the
/// padding for the payload union's u64 alignment, unlike PlainEnumLayout::Byte.
#[relax_api(plain, run)]
#[repr(C, u8)]
pub enum CEnumLayout {
    Empty,
    Byte(u8),
    Wide(u64),
}

/// Lifetime-only mirrors must keep lifetimes used even on unit variants or when a
/// field is cfg'd out. Signed discriminants are sign-extended in the exported table.
#[repr(i8)]
#[relax_api(handle, run)]
pub enum BorrowedEnumLayout<'a, 'b: 'a> {
    Empty = -7,
    Pair(&'a u8, &'b u64),
    Named { #[cfg(feature = "wide")] text: &'a str, byte: u8 },
    // (cfg_attr whose condition holds: removes the variant)
    #[cfg_attr(all(), cfg(any()))]
    Extra(&'b str),
    Last,
}

/// ScriptPodTy's shape: plain enum payloads and a cycle through Vec/Box of a handle.
#[relax_api(handle, run)]
#[repr(u8)]
#[derive(Default)]
pub enum PodTy {
    #[default]
    Void,
    Bool,
    Vec(Kind),
    Struct { align_of: usize, size_of: usize, fields: Vec<PodField> },
    FixedArray { align_of: usize, size_of: usize, len: usize, ty: Box<PodField> },
    /// Two owned fields let a partial move leave a separately counted payload behind.
    Split { fields: Vec<PodField>, rest: Box<PodField> },
}

#[relax_api(handle, run)]
pub struct PodField {
    pub name: usize,
    pub ty: PodTy,
    pub token: PodDrop,
}

#[relax_api(handle, run)]
pub struct PodDrop {
    pub value: u32,
}

static POD_DROPS: AtomicU32 = AtomicU32::new(0);

impl Drop for PodDrop {
    fn drop(&mut self) {
        POD_DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

#[relax_api(run)]
pub fn pod_drops() -> u32 {
    POD_DROPS.load(Ordering::Relaxed)
}

#[relax_api(run)]
pub fn pod_build(i: u32) -> PodTy {
    match i {
        0 => PodTy::Void,
        1 => PodTy::Bool,
        2 => PodTy::Vec(Kind::B),
        3 => PodTy::Struct {
            align_of: 8,
            size_of: 64,
            fields: vec![
                PodField { name: 11, ty: PodTy::Vec(Kind::A), token: PodDrop { value: 101 } },
                PodField { name: 13, ty: PodTy::Bool, token: PodDrop { value: 103 } },
            ],
        },
        4 => PodTy::FixedArray {
            align_of: 16,
            size_of: 96,
            len: 3,
            ty: Box::new(PodField { name: 17, ty: PodTy::Vec(Kind::B), token: PodDrop { value: 107 } }),
        },
        5 => PodTy::Split {
            fields: vec![
                PodField { name: 11, ty: PodTy::Vec(Kind::A), token: PodDrop { value: 101 } },
                PodField { name: 13, ty: PodTy::Bool, token: PodDrop { value: 103 } },
            ],
            rest: Box::new(PodField { name: 19, ty: PodTy::Bool, token: PodDrop { value: 109 } }),
        },
        _ => panic!("unknown pod variant"),
    }
}

#[relax_api(run)]
pub fn pod_field_checksum(field: &PodField) -> u64 {
    17 * field.name as u64 + 19 * field.token.value as u64 + 23 * pod_checksum(&field.ty)
}

/// rustc reads every field, including every element's fields and nested enum tag.
#[relax_api(run)]
pub fn pod_checksum(value: &PodTy) -> u64 {
    match value {
        PodTy::Void => 3,
        PodTy::Bool => 5,
        PodTy::Vec(kind) => 7 + match kind { Kind::A => 11, Kind::B => 13 },
        PodTy::Struct { align_of, size_of, fields } => {
            let mut sum = 29 + 31 * *align_of as u64 + 37 * *size_of as u64 + 41 * fields.len() as u64;
            for (i, field) in fields.iter().enumerate() {
                sum += (i as u64 + 1) * 43 * pod_field_checksum(field);
            }
            sum
        }
        PodTy::FixedArray { align_of, size_of, len, ty } => {
            47 + 53 * *align_of as u64 + 59 * *size_of as u64 + 61 * *len as u64 + 67 * pod_field_checksum(ty)
        }
        PodTy::Split { fields, rest } => {
            let mut sum = 71 + 73 * fields.len() as u64 + 83 * pod_field_checksum(rest);
            for (i, field) in fields.iter().enumerate() {
                sum += (i as u64 + 1) * 79 * pod_field_checksum(field);
            }
            sum
        }
    }
}

#[relax_api(run)]
pub fn pod_take(value: PodTy) -> u64 {
    pod_checksum(&value)
}

#[relax_api(run)]
pub fn pod_maybe(i: u32, present: bool) -> Option<PodTy> {
    if present { Some(pod_build(i)) } else { None }
}

#[relax_api(run)]
pub fn pod_option_take(value: Option<PodTy>) -> u64 {
    match value { Some(value) => 1 + pod_checksum(&value), None => 0 }
}

#[relax_api(run)]
pub fn pod_slice_checksum(values: &[PodTy]) -> u64 {
    let mut sum = 0;
    for (i, value) in values.iter().enumerate() {
        sum += (i as u64 + 1) * pod_checksum(value);
    }
    sum
}

#[relax_api(run)]
pub fn pod_field_build(i: u32) -> PodField {
    PodField { name: 23, ty: pod_build(i), token: PodDrop { value: 113 } }
}

/// No primitive repr: construction must still be refused when this reaches a bound op.
#[relax_api(handle, run)]
pub enum RefusedPodTy {
    Void,
    Field { value: usize },
}

#[relax_api(run)]
pub fn refused_pod_take(value: RefusedPodTy) -> usize {
    match value { RefusedPodTy::Void => 0, RefusedPodTy::Field { value } => value }
}

#[relax_api(run)]
pub fn flip(k: Kind) -> Kind {
    match k {
        Kind::A => Kind::B,
        Kind::B => Kind::A,
    }
}

static DROPS: AtomicU32 = AtomicU32::new(0);

/// How many Registry values rustc's code dropped.
#[relax_api(run)]
pub fn drops() -> u32 {
    DROPS.load(Ordering::Relaxed)
}

/// Opaque to Relax: rustc's Vec/HashMap inside.
#[relax_api(handle, run)]
pub struct Registry {
    names: Vec<String>,
    by_name: HashMap<String, u32>,
}

#[relax_api(run)]
impl Registry {
    #[relax_api(run)]
    pub fn new() -> Registry {
        Registry { names: Vec::new(), by_name: HashMap::new() }
    }

    #[relax_api(run)]
    pub fn add(&mut self, name: &str) -> u32 {
        if let Some(id) = self.by_name.get(name) {
            return *id;
        }
        let id = self.names.len() as u32;
        self.names.push(name.to_string());
        self.by_name.insert(name.to_string(), id);
        id
    }

    #[relax_api(run)]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    #[relax_api(run)]
    pub fn name_len(&self, id: u32) -> usize {
        self.names.get(id as usize).map_or(0, |n| n.len())
    }

    #[relax_api(run)]
    pub fn boom(&self, why: u32) -> u32 {
        if why > 0 {
            panic!("boom {} from the provided side", why);
        }
        0
    }

    /// generic: compiled by Relax from this source, calling the bound `add`
    pub fn add_all<'a, I: IntoIterator<Item = &'a str>>(&mut self, names: I) -> usize {
        let mut n = 0;
        for s in names {
            self.add(s);
            n += 1;
        }
        n
    }

    /// compiled on the caller's side (only calls methods)
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[relax_api(run)]
impl Default for Registry {
    fn default() -> Registry {
        Registry::new()
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

/// generic, so compiled by Relax: reads an opaque type's field whose bytes are rustc's alone (its
/// std's HashMap), which Relax refuses (provided.rs)
pub fn by_name_peek<T>(r: &Registry, _t: T) -> usize {
    r.by_name.len()
}

/// One strong reference to a registry rustc shares, handed over as an address (`Arc::into_raw`).
#[relax_api(run)]
pub fn shared_registry() -> usize {
    std::sync::Arc::into_raw(std::sync::Arc::new(Registry::new())) as usize
}

/// Not an op: compiled by Relax, it would count rustc's `ArcInner` by Relax's layout of it. A
/// `Registry` crosses the boundary (rustc made this one), so Relax refuses it (provided.rs
/// check_provided_ownership; relax-provided-fixture-arc-guard). The strong count after one more.
pub fn shared_registry_retain(at: usize) -> usize {
    let p = at as *const Registry;
    unsafe {
        std::sync::Arc::increment_strong_count(p);
        let a = std::mem::ManuallyDrop::new(std::sync::Arc::from_raw(p));
        std::sync::Arc::strong_count(&a)
    }
}

/// Option<usize> by value (tag + payload in registers on Relax's side)
#[relax_api(run)]
pub fn maybe(x: u32) -> Option<usize> {
    (x > 0).then_some(x as usize * 3)
}

#[relax_api(plain, run)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Io {
    pub kind: u8,
    pub name: u64,
    pub ty: u32,
    pub buffer_index: Option<usize>,
}

#[relax_api(run)]
impl Io {
    pub fn buffer_index(&self) -> Option<usize> {
        self.buffer_index
    }
}

#[relax_api(run)]
pub fn ios() -> &'static [Io] {
    static IOS: [Io; 2] = [Io { kind: 1, name: 7, ty: 2, buffer_index: None }, Io { kind: 2, name: 8, ty: 3, buffer_index: Some(3) }];
    &IOS
}

/// opaque (a String field: rustc's layout): its Hash / Debug / Display run rustc's impls, also
/// when Relax code hashes or formats it (jit.rs provided_fmt_hash; Stage's LiteralSite)
#[relax_api(handle, clone, eq, hash, debug, display, run)]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Site {
    pub file: String,
    pub line: u32,
}

#[relax_api(run)]
pub fn site(file: &str, line: u32) -> Site {
    Site { file: file.to_string(), line }
}

impl std::fmt::Display for Site {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}:{}", self.file, self.line)
    }
}

/// A closure of the caller's side run on a native thread of the provided crate (the shape of
/// `makepad_hal::dispatch_main`): `data` is the boxed closure, `run` calls and frees it,
/// `discard` frees it unrun. Only plain values cross.
#[relax_api(run)]
pub fn spawn_raw(data: usize, run: usize, discard: usize, refuse: bool) -> bool {
    // SAFETY: `run` and `discard` are `extern "C" fn(usize)` thunks of the caller (`spawn`).
    let (run, discard) = unsafe { (std::mem::transmute::<usize, extern "C" fn(usize)>(run), std::mem::transmute::<usize, extern "C" fn(usize)>(discard)) };
    if refuse {
        discard(data);
        return false;
    }
    std::thread::spawn(move || run(data)).join().is_ok()
}

/// Generic, so the caller's side compiles it: boxes `f` and hands the native side the pointer and
/// two thunks. False when the native side refused (the closure was dropped unrun).
pub fn spawn<F: FnOnce() + Send + 'static>(f: F, refuse: bool) -> bool {
    let data = Box::into_raw(Box::new(f)) as usize;
    spawn_raw(data, thunk_run::<F> as extern "C" fn(usize) as usize, thunk_discard::<F> as extern "C" fn(usize) as usize, refuse)
}

extern "C" fn thunk_run<F: FnOnce()>(data: usize) {
    let f = unsafe { Box::from_raw(data as *mut F) };
    f()
}

extern "C" fn thunk_discard<F>(data: usize) {
    drop(unsafe { Box::from_raw(data as *mut F) })
}

/// A handle that is returned inside an `Option` and moved into a closure that drops it on the
/// native thread (`keep_main_loop_alive`'s shape).
#[relax_api(handle, run)]
pub struct Token {
    _private: (),
}

static LIVE_TOKENS: AtomicU32 = AtomicU32::new(0);

impl Drop for Token {
    fn drop(&mut self) {
        LIVE_TOKENS.fetch_sub(1, Ordering::Relaxed);
    }
}

#[relax_api(run)]
pub fn token(grant: bool) -> Option<Token> {
    grant.then(|| {
        LIVE_TOKENS.fetch_add(1, Ordering::Relaxed);
        Token { _private: () }
    })
}

#[relax_api(run)]
pub fn live_tokens() -> u32 {
    LIVE_TOKENS.load(Ordering::Relaxed)
}

// ---- the program's environment and the host's, read by rustc's code

/// A variable as the program has it (relax-api `env::var`), after the side that read it (`rustc`:
/// the loader's build): a package's run-env reaches it (modules_harness
/// a_packages_run_env_is_its_programs_and_grants_nothing).
#[relax_api(run)]
pub fn program_var_raw(name: &str) -> Words {
    string_to_words(format!("{} {}", side(), relax_api::env::var(name).unwrap_or_else(|_| "none".into())))
}

pub fn program_var(name: &str) -> String {
    unsafe { string_from_words(program_var_raw(name)) }
}

/// The same variable as the host process has it (`std::env`): a run-env does not reach it.
#[relax_api(run)]
pub fn host_var_raw(name: &str) -> Words {
    string_to_words(format!("{} {}", side(), std::env::var(name).unwrap_or_else(|_| "none".into())))
}

pub fn host_var(name: &str) -> String {
    unsafe { string_from_words(host_var_raw(name)) }
}

fn side() -> &'static str {
    if cfg!(feature = "host-shims") { "rustc" } else { "jit" }
}

// ---- owned heap values (String, Vec, structs of them) crossing the boundary
//
// An op moves the buffer as three words; the wrapper (not an op: compiled by the caller's side)
// rebuilds the value with that side's own String/Vec. Both sides use the system allocator.

#[relax_api(run)]
pub fn owned_text_raw(n: usize) -> Words {
    string_to_words((0..n).map(|i| char::from(b'a' + (i % 26) as u8)).collect::<String>() + "-é")
}

pub fn owned_text(n: usize) -> String {
    unsafe { string_from_words(owned_text_raw(n)) }
}

#[relax_api(run)]
pub fn owned_list_raw(n: u32) -> Words {
    vec_to_words((0..n).map(|i| Item { a: i, b: i as u64 * 3, c: i % 2 == 0 }).collect::<Vec<_>>())
}

pub fn owned_list(n: u32) -> Vec<Item> {
    unsafe { vec_from_words(owned_list_raw(n)) }
}

#[relax_api(plain, run)]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub a: u32,
    pub b: u64,
    pub c: bool,
}

/// The caller's String goes in; the native side frees it (a buffer the caller's side allocated).
#[relax_api(run)]
pub fn owned_consume_raw(w: Words) -> usize {
    let s = unsafe { string_from_words(w) };
    s.len() + s.capacity() * 0
}

pub fn owned_consume(s: String) -> usize {
    owned_consume_raw(string_to_words(s))
}

/// A struct holding a String and a Vec (the shape of the script VM's fn tokens), by its plain
/// transport form.
pub struct Doc {
    pub name: String,
    pub items: Vec<Item>,
    pub closure: bool,
}

#[relax_api(plain, run)]
#[repr(C)]
pub struct DocRaw {
    pub name: Words,
    pub items: Words,
    pub closure: bool,
}

#[relax_api(run)]
pub fn owned_doc_raw(n: u32, present: bool, out: &mut DocRaw) -> bool {
    if !present { return false; }
    out.name = string_to_words(format!("doc{n}"));
    out.items = vec_to_words((0..n).map(|i| Item { a: i, b: 7, c: false }).collect::<Vec<_>>());
    out.closure = n % 2 == 1;
    true
}

pub fn owned_doc(n: u32, present: bool) -> Option<Doc> {
    let mut raw = DocRaw { name: [0; 3], items: [0; 3], closure: false };
    if !owned_doc_raw(n, present, &mut raw) { return None; }
    Some(unsafe { Doc { name: string_from_words(raw.name), items: vec_from_words(raw.items), closure: raw.closure } })
}

/// A handle of owned values (Strings, Vecs, an `Option<String>`, a plain enum) that JIT code builds
/// itself (a struct literal, a struct update over `Default::default()`) and hands to rustc's code,
/// which reads, grows, frees or returns it: rustc's layout is makepad run's own on both sides
/// (relax-compiler provided.rs same_layout_handles), so the bytes cross as they are.
#[relax_api(handle, run)]
#[derive(Debug)]
pub struct Note {
    pub title: String,
    pub tags: Vec<String>,
    pub items: Vec<Item>,
    pub alias: Option<String>,
    pub limit: Option<u32>,
    pub kind: Kind,
    pub weight: u32,
}

impl Default for Note {
    fn default() -> Note {
        Note { title: String::from("untitled"), tags: Vec::new(), items: Vec::new(), alias: None, limit: Some(3), kind: Kind::A, weight: 1 }
    }
}

/// rustc reads every field of a Note JIT code built
#[relax_api(run)]
pub fn note_digest(n: &Note) -> String {
    format!("{}|{}|{}|{:?}|{:?}|{:?}|{}", n.title, n.tags.join(","), n.items.iter().map(|i| i.a as u64 + i.b).sum::<u64>(), n.alias, n.limit, n.kind, n.weight)
}

/// rustc takes a Note by value and frees what JIT code allocated
#[relax_api(run)]
pub fn note_take(n: Note) -> usize {
    n.title.len() + n.tags.iter().map(|t| t.len()).sum::<usize>() + n.items.len() + n.alias.map_or(0, |a| a.len())
}

/// rustc grows a Note past its capacities and hands it back: JIT code reads and frees what rustc allocated
#[relax_api(run)]
pub fn note_grow(mut n: Note) -> Note {
    n.title.push_str(" -- grown by rustc past its capacity");
    n.tags.extend((0..20).map(|i| format!("t{i}")));
    n.items.push(Item { a: 5, b: 6, c: true });
    n.alias = n.alias.map(|a| a + "!").or_else(|| Some(String::from("named")));
    n.limit = None;
    n.kind = flip(n.kind);
    n.weight *= 10;
    n
}

/// rustc builds a Note
#[relax_api(run)]
pub fn note_new(title: &str) -> Note {
    Note { title: title.to_string(), tags: vec![String::from("x")], alias: None, ..Note::default() }
}

/// A plain struct holding a String (the shape of the script VM's `ScriptLoc`: caller-owned, its
/// methods stay on the constructing side): Rust's repr and not Copy. JIT code builds it by a
/// literal and reads it; no bound fn takes it (one that did would refuse it: a String is not
/// C-shaped).
#[relax_api(plain, run)]
#[derive(Clone, Debug, PartialEq)]
pub struct Loc {
    pub file: String,
    pub col: u32,
    pub line: u32,
}

impl Loc {
    /// compiled from source on the caller's side
    pub fn next_line(&self) -> Loc {
        Loc { file: self.file.clone(), col: 1, line: self.line + 1 }
    }
}

/// Peak resident size so far, in the OS's unit (bytes on macOS and Windows, KiB on Linux).
#[cfg(unix)]
pub fn peak_rss() -> i64 {
    extern "C" { fn getrusage(who: i32, usage: *mut [i64; 18]) -> i32; }
    let mut u = [0i64; 18];
    unsafe { getrusage(0, &mut u) };
    u[4]
}

/// Peak working set so far, in bytes (PROCESS_MEMORY_COUNTERS.PeakWorkingSetSize).
#[cfg(windows)]
pub fn peak_rss() -> i64 {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut u8, cb: u32) -> i32;
    }
    // cb + PageFaultCount (u32 each), PeakWorkingSetSize, WorkingSetSize, ..
    let mut c = [0usize; 9];
    c[0] = std::mem::size_of_val(&c);
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), c.as_mut_ptr().cast(), std::mem::size_of_val(&c) as u32) };
    c[1] as i64
}

/// Inline assembly is compiled by rustc only: an op of the provided crate, JIT code calls it.
#[cfg(target_arch = "aarch64")]
#[relax_api(run)]
pub fn asm_inc(v: u64) -> u64 {
    let out: u64;
    unsafe { std::arch::asm!("add {0}, {1}, #1", out(reg) out, in(reg) v, options(nomem, nostack, preserves_flags)) };
    out
}

#[cfg(not(target_arch = "aarch64"))]
#[relax_api(run)]
pub fn asm_inc(v: u64) -> u64 {
    v + 1
}

/// Calls the asm op from an inline(always) fn, as `FpEnv::pin` does.
#[inline(always)]
pub fn asm_twice(v: u64) -> u64 {
    asm_inc(asm_inc(v))
}

// ---- statics reached from JIT-compiled code (provided_static.sh)

/// Each side keeps its own memo (a per-side cache of values that side computes): JIT code that
/// compiles `memo_square` from source gets its own instance.
#[relax_api(per_side)]
static MEMO: std::sync::Mutex<std::collections::BTreeMap<u64, u64>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Not an op: JIT code compiles it from source and reaches the per-side MEMO.
pub fn memo_square(x: u64) -> u64 {
    if let Some(v) = MEMO.lock().unwrap().get(&x) {
        return *v;
    }
    let v = x * x;
    MEMO.lock().unwrap().insert(x, v);
    v
}

/// Entries in this side's memo.
pub fn memo_len() -> usize {
    MEMO.lock().unwrap().len()
}

/// rustc's one copy (not marked): JIT code reaching it is refused with a clear error.
static SHARED_LOG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Not an op: compiled from source by JIT callers, which cannot read rustc's SHARED_LOG.
pub fn shared_log(line: &str) -> usize {
    let mut l = SHARED_LOG.lock().unwrap();
    l.push(line.to_string());
    l.len()
}

/// One marked instance: ops, plain fns compiled by the JIT and direct bin access all share it.
#[relax_api]
pub static SHARED_COUNT: AtomicU64 = AtomicU64::new(0);

/// An op: rustc's code adds `n` to SHARED_COUNT; the count after it.
#[relax_api(run)]
pub fn shared_count_add(n: u64) -> u64 {
    SHARED_COUNT.fetch_add(n, Ordering::Relaxed) + n
}

/// Not an op: JIT code compiles it from source and bumps SHARED_COUNT.
pub fn shared_count_bump(n: u64) -> u64 {
    SHARED_COUNT.fetch_add(n, Ordering::Relaxed) + n
}

/// What rustc's code reads of SHARED_COUNT after either side writes it.
#[relax_api(run)]
pub fn shared_count() -> u64 {
    SHARED_COUNT.load(Ordering::Relaxed)
}

#[relax_api(run)]
pub fn shared_count_addr() -> usize {
    (&raw const SHARED_COUNT) as usize
}

/// The loader's address and value together, for a JIT static holding a reference to it.
#[relax_api(run)]
pub fn shared_count_snapshot() -> [u64; 2] {
    [(&raw const SHARED_COUNT) as usize as u64, SHARED_COUNT.load(Ordering::Relaxed)]
}

/// Private storage: the generated address accessor must be able to name it in this module.
#[relax_api]
static HIDDEN: AtomicU64 = AtomicU64::new(0);

#[relax_api(run)]
pub fn hidden_count_add(n: u64) -> u64 {
    HIDDEN.fetch_add(n, Ordering::Relaxed) + n
}

pub fn hidden_count_bump(n: u64) -> u64 {
    HIDDEN.fetch_add(n, Ordering::Relaxed) + n
}

#[relax_api(run)]
pub fn hidden_count() -> u64 {
    HIDDEN.load(Ordering::Relaxed)
}

pub fn hidden_count_read() -> u64 {
    HIDDEN.load(Ordering::Relaxed)
}

#[relax_api(run)]
pub fn hidden_count_addr() -> usize {
    (&raw const HIDDEN) as usize
}

pub fn hidden_count_plain_addr() -> usize {
    (&raw const HIDDEN) as usize
}

/// The op twin calls this same helper: one LOCAL, whose key includes `local_count_bump`.
/// Calling with zero also reads it without changing the count.
pub fn local_count_bump(n: u64) -> u64 {
    #[relax_api]
    static LOCAL: AtomicU64 = AtomicU64::new(0);
    LOCAL.fetch_add(n, Ordering::Relaxed) + n
}

#[relax_api(run)]
pub fn local_count_add(n: u64) -> u64 {
    local_count_bump(n)
}

#[relax_api(run)]
pub fn local_count() -> u64 {
    local_count_bump(0)
}

/// Plain mutable data: the single-threaded fixture reads and writes the loader's one instance.
#[relax_api]
pub static mut RAW: u64 = 0;

#[relax_api(run)]
pub fn raw_set(v: u64) {
    unsafe { RAW = v }
}

#[relax_api(run)]
pub fn raw_count() -> u64 {
    unsafe { RAW }
}

pub fn raw_read() -> u64 {
    unsafe { RAW }
}

#[relax_api(run)]
pub fn raw_addr() -> usize {
    (&raw const RAW) as usize
}

pub struct Cells {
    pub c: Cell<u64>,
}

// SAFETY: this fixture's Cell access is confined to the single thread running the statics mode.
unsafe impl Sync for Cells {}

#[relax_api]
pub static CELLS: Cells = Cells { c: Cell::new(0) };

#[relax_api(run)]
pub fn cells_add(n: u64) -> u64 {
    CELLS.c.set(CELLS.c.get() + n);
    CELLS.c.get()
}

pub fn cells_bump(n: u64) -> u64 {
    CELLS.c.set(CELLS.c.get() + n);
    CELLS.c.get()
}

#[relax_api(run)]
pub fn cells_count() -> u64 {
    CELLS.c.get()
}

#[relax_api(run)]
pub fn cells_addr() -> usize {
    (&raw const CELLS) as usize
}

#[relax_api]
pub static ARR: [AtomicU32; 4] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];

#[relax_api(run)]
pub fn arr_add(i: usize, n: u32) -> u32 {
    ARR[i].fetch_add(n, Ordering::Relaxed) + n
}

pub fn arr_bump(i: usize, n: u32) -> u32 {
    ARR[i].fetch_add(n, Ordering::Relaxed) + n
}

#[relax_api(run)]
pub fn arr_count(i: usize) -> u32 {
    ARR[i].load(Ordering::Relaxed)
}

#[relax_api(run)]
pub fn arr_addr() -> usize {
    (&raw const ARR) as usize
}

/// Layout-compatible but unmarked: direct JIT access still needs an explicit sharing mark.
pub static PLAIN_COUNT: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// A `const` thread-local without drop glue (a `#[thread_local]` static under Relax).
    static TLS_COUNT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// A lazily made one with drop glue (a runtime key under Relax).
    static TLS_LIST: std::cell::RefCell<Vec<u64>> = std::cell::RefCell::new(Vec::new());
}

/// Not an op: JIT code compiles it from source and reaches both thread-locals (each side keeps
/// its own per-thread storage).
pub fn tls_bump(v: u64) -> (u32, usize) {
    let n = TLS_COUNT.with(|c| {
        c.set(c.get() + 1);
        c.get()
    });
    let len = TLS_LIST.with(|l| {
        l.borrow_mut().push(v);
        l.borrow().len()
    });
    (n, len)
}

// ---- a panic of the caller's side through rustc's frames (relax-unwind-through)

static GUARD_DROPS: AtomicU32 = AtomicU32::new(0);

/// A drop guard of rustc's frame: counts its drops.
struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        GUARD_DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

/// rustc's frame holding a drop guard around a call of the caller's side (`run` is an
/// `extern "C-unwind" fn(usize) -> u32` thunk of the caller, `through_guard`).
#[relax_api(run)]
pub fn guarded_call(run: usize, data: usize) -> u32 {
    // SAFETY: `run` is the caller's thunk (through_guard)
    let run = unsafe { std::mem::transmute::<usize, extern "C-unwind" fn(usize) -> u32>(run) };
    let _guard = Guard;
    run(data) + 1
}

/// rustc's catch_unwind around a call of the caller's side: true when it caught a panic.
#[relax_api(run)]
pub fn caught_call(run: usize, data: usize) -> bool {
    // SAFETY: `run` is the caller's thunk (catch_through)
    let run = unsafe { std::mem::transmute::<usize, extern "C-unwind" fn(usize) -> u32>(run) };
    std::panic::catch_unwind(|| run(data)).is_err()
}

/// How many of rustc's guards dropped.
#[relax_api(run)]
pub fn guard_drops() -> u32 {
    GUARD_DROPS.load(Ordering::Relaxed)
}

extern "C-unwind" fn thunk_call<F: FnMut() -> u32>(data: usize) -> u32 {
    // SAFETY: `data` is the `&mut F` of through_guard / catch_through, live for the call
    unsafe { (*(data as *mut F))() }
}

/// Generic, so the caller's side compiles it: `f` runs inside rustc's guarded frame.
pub fn through_guard<F: FnMut() -> u32>(f: &mut F) -> u32 {
    guarded_call(thunk_call::<F> as extern "C-unwind" fn(usize) -> u32 as usize, f as *mut F as usize)
}

/// Generic, so the caller's side compiles it: `f` runs inside rustc's catch_unwind.
pub fn catch_through<F: FnMut() -> u32>(f: &mut F) -> bool {
    caught_call(thunk_call::<F> as extern "C-unwind" fn(usize) -> u32 as usize, f as *mut F as usize)
}
