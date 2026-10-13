//! Relax provided-crate fixture (loader-core.md section 8, step 3). rustc builds this crate into
//! the `makepad` loader with its shims (feature `host-shims`); under `makepad run` Relax loads its
//! source for the bin but binds the C-shaped non-generic fns below to the loader's code.
//! `bound()` tells which side ran: only the loader's build has the feature.
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

use makepad_relax_api::{relax_api, string_from_words, string_to_words, vec_from_words, vec_to_words, Words};

/// true when this call ran the loader's rustc-built code
#[relax_api]
pub fn bound() -> bool {
    cfg!(feature = "host-shims")
}

#[relax_api]
pub fn add(a: i32, b: i64) -> i64 {
    a as i64 + b
}

#[relax_api]
pub fn mix(a: f32, b: f64, c: u8, d: bool, e: char) -> f64 {
    a as f64 * b + c as f64 + if d { 0.5 } else { 0.0 } + e as u32 as f64
}

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct P {
    pub x: f32,
    pub y: f32,
}

#[relax_api]
pub fn scale(p: P, k: f32) -> P {
    P { x: p.x * k, y: p.y * k }
}

/// passed and returned in memory
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Big {
    pub a: [u64; 6],
}

#[relax_api]
pub fn big_make(x: u64) -> Big {
    Big { a: [x, x + 1, x + 2, x + 3, x + 4, x + 5] }
}

#[relax_api]
pub fn big_sum(b: Big) -> u64 {
    b.a.iter().sum()
}

#[relax_api]
pub fn text_len(s: &str) -> usize {
    s.chars().count()
}

#[relax_api]
pub fn sum(v: &[u32]) -> u64 {
    v.iter().map(|x| *x as u64).sum()
}

#[relax_api(plain)]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    A = 1,
    B = 7,
}

#[relax_api]
pub fn flip(k: Kind) -> Kind {
    match k {
        Kind::A => Kind::B,
        Kind::B => Kind::A,
    }
}

static DROPS: AtomicU32 = AtomicU32::new(0);

/// How many Registry values rustc's code dropped.
#[relax_api]
pub fn drops() -> u32 {
    DROPS.load(Ordering::Relaxed)
}

/// Opaque to Relax: rustc's Vec/HashMap inside.
#[relax_api(handle)]
pub struct Registry {
    names: Vec<String>,
    by_name: HashMap<String, u32>,
}

#[relax_api]
impl Registry {
    #[relax_api]
    pub fn new() -> Registry {
        Registry { names: Vec::new(), by_name: HashMap::new() }
    }

    #[relax_api]
    pub fn add(&mut self, name: &str) -> u32 {
        if let Some(id) = self.by_name.get(name) {
            return *id;
        }
        let id = self.names.len() as u32;
        self.names.push(name.to_string());
        self.by_name.insert(name.to_string(), id);
        id
    }

    #[relax_api]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    #[relax_api]
    pub fn name_len(&self, id: u32) -> usize {
        self.names.get(id as usize).map_or(0, |n| n.len())
    }

    #[relax_api]
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

#[relax_api]
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

/// generic, so compiled by Relax: reads an opaque type's field, which Relax refuses (provided.rs)
pub fn names_peek<T>(r: &Registry, _t: T) -> usize {
    r.names.len()
}

/// Option<usize> by value (tag + payload in registers on Relax's side)
#[relax_api]
pub fn maybe(x: u32) -> Option<usize> {
    (x > 0).then_some(x as usize * 3)
}

#[relax_api(plain)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Io {
    pub kind: u8,
    pub name: u64,
    pub ty: u32,
    pub buffer_index: Option<usize>,
}

#[relax_api]
impl Io {
    pub fn buffer_index(&self) -> Option<usize> {
        self.buffer_index
    }
}

#[relax_api]
pub fn ios() -> &'static [Io] {
    static IOS: [Io; 2] = [Io { kind: 1, name: 7, ty: 2, buffer_index: None }, Io { kind: 2, name: 8, ty: 3, buffer_index: Some(3) }];
    &IOS
}

/// opaque (a String field: rustc's layout): its Hash / Debug / Display run rustc's impls, also
/// when Relax code hashes or formats it (jit.rs provided_fmt_hash; Stage's LiteralSite)
#[relax_api(handle, clone, eq, hash, debug, display)]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Site {
    pub file: String,
    pub line: u32,
}

#[relax_api]
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
#[relax_api]
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
#[relax_api(handle)]
pub struct Token {
    _private: (),
}

static LIVE_TOKENS: AtomicU32 = AtomicU32::new(0);

impl Drop for Token {
    fn drop(&mut self) {
        LIVE_TOKENS.fetch_sub(1, Ordering::Relaxed);
    }
}

#[relax_api]
pub fn token(grant: bool) -> Option<Token> {
    grant.then(|| {
        LIVE_TOKENS.fetch_add(1, Ordering::Relaxed);
        Token { _private: () }
    })
}

#[relax_api]
pub fn live_tokens() -> u32 {
    LIVE_TOKENS.load(Ordering::Relaxed)
}

// ---- owned heap values (String, Vec, structs of them) crossing the boundary
//
// An op moves the buffer as three words; the wrapper (not an op: compiled by the caller's side)
// rebuilds the value with that side's own String/Vec. Both sides use the system allocator.

#[relax_api]
pub fn owned_text_raw(n: usize) -> Words {
    string_to_words((0..n).map(|i| char::from(b'a' + (i % 26) as u8)).collect::<String>() + "-é")
}

pub fn owned_text(n: usize) -> String {
    unsafe { string_from_words(owned_text_raw(n)) }
}

#[relax_api]
pub fn owned_list_raw(n: u32) -> Words {
    vec_to_words((0..n).map(|i| Item { a: i, b: i as u64 * 3, c: i % 2 == 0 }).collect::<Vec<_>>())
}

pub fn owned_list(n: u32) -> Vec<Item> {
    unsafe { vec_from_words(owned_list_raw(n)) }
}

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub a: u32,
    pub b: u64,
    pub c: bool,
}

/// The caller's String goes in; the native side frees it (a buffer the caller's side allocated).
#[relax_api]
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

#[relax_api(plain)]
#[repr(C)]
pub struct DocRaw {
    pub name: Words,
    pub items: Words,
    pub closure: bool,
}

#[relax_api]
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
#[relax_api]
pub fn asm_inc(v: u64) -> u64 {
    let out: u64;
    unsafe { std::arch::asm!("add {0}, {1}, #1", out(reg) out, in(reg) v, options(nomem, nostack, preserves_flags)) };
    out
}

#[cfg(not(target_arch = "aarch64"))]
#[relax_api]
pub fn asm_inc(v: u64) -> u64 {
    v + 1
}

/// Calls the asm op from an inline(always) fn, as `FpEnv::pin` does.
#[inline(always)]
pub fn asm_twice(v: u64) -> u64 {
    asm_inc(asm_inc(v))
}
