//! `#[relax_api]`: the interface a provided crate (a crate rustc builds into the `makepad` loader)
//! offers to code Relax compiles.
//!
//! The attribute marks, on the code itself:
//! - `#[relax_api]` on a free fn, or on an impl block (every fn in it, or only the fns inside
//!   marked `#[relax_api]` when there are any; `#[relax_api(marked)]` on the impl: only those,
//!   for a macro's impl whose fns each invocation marks): an *op* JIT code calls on the compiled
//!   side;
//! - `#[relax_api(plain)]` on a struct/enum: plain data JIT code reads, builds and drops itself
//!   (Relax checks rustc's size, align and field offsets against its own);
//! - `#[relax_api(handle[, clone][, eq][, hash][, debug][, display])]` on a struct/enum: a handle --
//!   rustc's size/align/drop (and the listed trait ops) cross. A struct's field offsets cross too:
//!   when Relax lays it out the same and every field is the same bytes on both sides (plain data,
//!   `String`/`Vec`/`Option<String>` while the two stds agree, other such handles), JIT code builds it
//!   and reads its fields itself; otherwise its fields never cross.
//! - `#[relax_api]` on a static: one instance, the loader's, read and written by JIT code at
//!   the loader's address when its type is the same bytes on both sides.
//!   JIT code writes the loader's memory, so a pointer it stores in a shared static (`AtomicPtr`,
//!   `Cell<*const T>`) dangles after the run unless the module's run-end reset clears it.
//! - `#[relax_api(per_side)]` on a static: each side keeps its own instance (a memo of values
//!   only that side's code makes and reads, laid out by that side's std). JIT code reaching an
//!   unmarked provided static is refused; a type not the same bytes on both sides needs an op
//!   or its own per-side instance.
//! - `run`, beside a fn, impl or type mark: the ops allocate in the program's heap ([`run_op`]), so the
//!   program's end takes what they made. A crate whose objects are the program's (a script VM)
//!   says it on every attribute. Without it an op allocates in the host's heap ([`host_op`]).
//!
//! In a normal build the attribute leaves the item as it is. With the crate's `host-shims`
//! feature (only the `makepad` loader build enables it) it also emits, next to the item, an extern "C"
//! wrapper and one [`Entry`] in the `relax_api` link section; [`entries`] reads them all back. The
//! loader is not built for the web (its `platforms`), so the feature is never on for wasm32.
//! Nothing generated is checked in.

pub use relax_api_macro::relax_api;
/// Export const-verified `Option<X>` layouts for concrete payload types in unmarked items.
/// `option_layouts! { u32, MyType }` emits the same entries as the attribute's Option scan,
/// under the using crate's `host-shims` feature. Lifetimes are instantiated as `'static`.
/// Its typed [`OptionListed`] markers are emitted on every target, without that feature.
pub use relax_api_macro::option_layouts;

/// One Option the crate lists or a marked item spells, read by the Option-entry guard
/// on every target (`check --boundary options`). This marker has no runtime cost.
pub struct OptionListed<T>(pub ::core::marker::PhantomData<T>);

static APP_CALL: std::sync::OnceLock<fn(unsafe extern "C" fn(*mut u8), *mut u8) -> bool> = std::sync::OnceLock::new();

/// The host's way of calling code it compiled (`app_call`), once per process.
pub fn set_app_call(call: fn(unsafe extern "C" fn(*mut u8), *mut u8) -> bool) {
    let _ = APP_CALL.set(call);
}

/// How native code calls a function pointer JIT code gave it: a callback, a waker, a router. A host
/// can end the app's run while the call is in there, or before it starts. The call then comes back
/// with false, as if the callback had returned at once: every frame of the caller goes on, whatever
/// it holds is released as usual. Forget the pointer then, it will never work again. Without such a
/// host this is a plain call.
pub fn app_call<F: FnOnce()>(f: F) -> bool {
    let Some(call) = APP_CALL.get() else {
        f();
        return true;
    };
    unsafe extern "C" fn thunk<F: FnOnce()>(p: *mut u8) {
        if let Some(f) = (*(p as *mut Option<F>)).take() {
            f()
        }
    }
    let mut f = Some(f);
    call(thunk::<F>, &mut f as *mut Option<F> as *mut u8)
}

/// An op of a crate whose objects are the program's: what it allocates goes with the program's heap
/// (`relax_api_phase::run`). Only generated wrappers call this.
#[cfg(feature = "host-shims")]
#[inline(always)]
pub fn run_op() -> impl Sized {
    relax_api_phase::run()
}
#[cfg(not(feature = "host-shims"))]
#[inline(always)]
pub fn run_op() {}

/// An op of a crate that holds what outlives a program (the window, the GPU device, the CPU budget):
/// the process's heap, and its own end frees what a program held. Only generated wrappers call this.
#[cfg(feature = "host-shims")]
#[inline(always)]
pub fn host_op() -> impl Sized {
    relax_api_phase::kept()
}
#[cfg(not(feature = "host-shims"))]
#[inline(always)]
pub fn host_op() {}

/// The program's environment, for a provided crate's code. Under the loader a provided crate's
/// bound ops run rustc's code in the loader's process, where `std::env` is the host's environment;
/// the program's own (a package's run-env, the program's writes) is the loader's to answer. Read a
/// setting that only tunes or reports the program's own run (a log, a trace, a tuning knob) here;
/// one that decides what the process loads, runs or opens, a choice of the person's invocation, or
/// a variable a C library reads too stays a `std::env` read.
pub mod env {
    use std::ffi::{OsStr, OsString};

    static PROGRAM_READ: std::sync::OnceLock<fn(&OsStr) -> Option<OsString>> = std::sync::OnceLock::new();

    /// The loader's read of the program's environment (relax-base progenv `program_var_os`), once per
    /// process. Without one, and in JIT-compiled code, the program's environment is `std::env`.
    pub fn set_program_read(read: fn(&OsStr) -> Option<OsString>) {
        let _ = PROGRAM_READ.set(read);
    }

    /// `std::env::var_os` of the program's environment.
    pub fn var_os<K: AsRef<OsStr>>(key: K) -> Option<OsString> {
        match PROGRAM_READ.get() {
            Some(read) => read(key.as_ref()),
            None => std::env::var_os(key),
        }
    }

    /// `std::env::var` of the program's environment.
    pub fn var<K: AsRef<OsStr>>(key: K) -> Result<String, std::env::VarError> {
        match var_os(key) {
            Some(v) => v.into_string().map_err(std::env::VarError::NotUnicode),
            None => Err(std::env::VarError::NotPresent),
        }
    }

    /// `std::env::current_exe` of the program. Under `makepad run`, a provided crate's
    /// `std::env::current_exe` names the loader's own file, but the program has an executable of
    /// its own: Relax's link of the loader, named as cargo names the program's binary (relax-compiler
    /// selfexe.rs), the first word of the run's RELAX_SELF_EXEC, as Relax std's `current_exe` reads
    /// it. Its file stem is the name a native build's executable has. Otherwise, the process's.
    pub fn current_exe() -> std::io::Result<std::path::PathBuf> {
        let run = std::env::var_os("RELAX_SELF_EXEC");
        // (the run's command line, its words apart by 0x1f, an ASCII byte to split the encoding at)
        let first = run.as_ref().and_then(|line| line.as_encoded_bytes().split(|&b| b == 0x1f).next()).filter(|w| !w.is_empty());
        match first {
            Some(exe) => Ok(std::path::PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(exe.to_vec()) })),
            None => std::env::current_exe(),
        }
    }
}

/// The locks a provided crate's process-lifetime data uses. On macOS std's lock allocates a block
/// at its first use, on whatever thread and in whatever phase that is: in an op, that block would
/// be the run's and go with it, and the lock with it. These keep their state inline.
pub mod sync {
    #[cfg(target_vendor = "apple")]
    pub use relax_api_phase::sync::{Condvar, Mutex, MutexGuard};
    #[cfg(not(target_vendor = "apple"))]
    pub use std::sync::{Condvar, Mutex, MutexGuard};
}

/// For a provided crate's statics: what `f` allocates is the process's, shared by every program,
/// not the calling program's. A static of a crate in `run_op` fills its blocks in here, since the
/// program whose op filled it can end while the static goes on.
#[inline(always)]
pub fn process_heap<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(feature = "host-shims")]
    let _kept = relax_api_phase::kept();
    f()
}

/// A generated op wrapper: arguments moved out of `args[i]`, the result written to `ret`;
/// 0 = returned, 1 = panicked (the text: [`last_panic`]).
pub type ShimFn = unsafe extern "C" fn(*const *mut u8, *mut u8) -> u32;
pub type DropFn = unsafe extern "C" fn(*mut u8);

pub const OP: u32 = 1;
pub const HANDLE: u32 = 2;
pub const PLAIN: u32 = 3;
/// a static each side keeps its own instance of (`#[relax_api(per_side)]`)
pub const PER_SIDE: u32 = 4;
/// A static whose one instance is the loader's: JIT code reads and writes it at the loader's address.
pub const SHARED: u32 = 5;
/// A const-verified `Option<X>` layout; `Entry::enum_layout` holds [`option_layout`]'s six words.
pub const OPTION: u32 = 6;

/// `[size, align, payload_off, tag_bytes, none_tag, some_tag]` for `Option<X>`.
/// When rustc niche-encodes it (`size_of::<Option<X>>() == size_of::<X>()`),
/// `tag_bytes` is zero and no layout agreement is stated.
///
/// Generated host entries evaluate this in a const: reading rustc's tags proves the
/// stated width is not too large, and matching images with poisoned padding proves
/// it is not too small or at the wrong offset. A changed layout is a compile error.
/// Always defined: a crate's own `host-shims` feature gates the entries that call it, and such
/// a crate may be built without relax-api's feature (a check of the crate alone).
pub const fn option_layout<X>() -> [u64; 6] {
    // Even a non-const caller must have rustc validate the raw reads and images.
    const {
        use core::mem::{ManuallyDrop, MaybeUninit, align_of, size_of};
        let size = size_of::<Option<X>>();
        let align = align_of::<Option<X>>();
        if size == size_of::<X>() {
            [size as u64, align as u64, 0, 0, 0, 0]
        } else {
            // The tagged class admitted at the provided boundary has no payload niche:
            // zero inhabits X. ManuallyDrop keeps even owning payloads from running drop.
            let none = ManuallyDrop::new(None::<X>);
            let some = ManuallyDrop::new(Some(unsafe { MaybeUninit::<X>::zeroed().assume_init() }));
            let none_ptr = (&none as *const ManuallyDrop<Option<X>>).cast::<u8>();
            let some_ptr = (&some as *const ManuallyDrop<Option<X>>).cast::<u8>();
            let payload_off = match unsafe { &*some_ptr.cast::<Option<X>>() } {
                Some(payload) => unsafe { (payload as *const X).cast::<u8>().offset_from(some_ptr) as usize },
                None => panic!("Some became None"),
            };
            // Integer::for_align on the JIT's hosts: integer alignment equals its size.
            let w = match align_of::<X>() {
                1 | 2 | 4 | 8 | 16 => align_of::<X>(),
                _ => 1,
            };
            assert!(w <= payload_off);
            assert!(size == payload_off + size_of::<X>());
            let mut none_tag = 0u128;
            let mut some_tag = 0u128;
            let mut i = 0;
            while i < w {
                let shift = if cfg!(target_endian = "little") { i * 8 } else { (w - 1 - i) * 8 };
                none_tag |= (unsafe { *none_ptr.add(i) } as u128) << shift;
                some_tag |= (unsafe { *some_ptr.add(i) } as u128) << shift;
                i += 1;
            }
            assert!(none_tag <= u64::MAX as u128 && some_tag <= u64::MAX as u128);
            let mut none_image = MaybeUninit::<Option<X>>::uninit();
            let mut some_image = MaybeUninit::<Option<X>>::uninit();
            let none_bytes = none_image.as_mut_ptr().cast::<u8>();
            let some_bytes = some_image.as_mut_ptr().cast::<u8>();
            unsafe {
                none_bytes.write_bytes(0xff, size);
                some_bytes.write_bytes(0xff, size);
                some_bytes.add(payload_off).write_bytes(0, size_of::<X>());
                i = 0;
                while i < w {
                    *none_bytes.add(i) = *none_ptr.add(i);
                    *some_bytes.add(i) = *some_ptr.add(i);
                    i += 1;
                }
                assert!(matches!(&*none_image.as_ptr(), None));
                assert!(matches!(&*some_image.as_ptr(), Some(_)));
            }
            [size as u64, align as u64, payload_off as u64, w as u64, none_tag as u64, some_tag as u64]
        }
    }
}

/// One marked item or an Option spelling it uses, laid down in the `relax_api` link section.
#[repr(C)]
pub struct Entry {
    pub kind: u32,
    /// `module_path!()` where the item is marked (its crate is the first segment)
    pub module: &'static str,
    /// OP: `core::any::type_name` of the fn item (`<f32 as krate::traits::ScriptNew>::script_new`,
    /// `krate::heap::ScriptHeap::value`); HANDLE/PLAIN: the type's path; a handle's trait ops:
    /// `<path as Clone>::clone`, `<path as PartialEq>::eq`, `<path as Hash>::hash_into`,
    /// `<path as Fmt>::fmt_into`; OPTION: `core::any::type_name::<Option<X>>()`
    pub key: fn() -> &'static str,
    /// OP: `core::any::type_name` of the op's fn pointer type (`fn(&krate::heap::ScriptHeap) -> u32`):
    /// Relax compares it with its own reading of the same source; "" for a handle's trait ops
    /// OPTION: "option_layouts" for an explicit list, "" for a marked item's inferred entry.
    pub sig: fn() -> &'static str,
    pub shim: Option<ShimFn>,
    /// OP: per parameter, then the result: [size, align, pointee size, pointee align] as rustc
    /// lays them out (pointee 0: no sized referent)
    pub slots: &'static [[u64; 4]],
    /// HANDLE/PLAIN: the type's size and align
    pub size: u64,
    pub align: u64,
    /// PLAIN / HANDLE struct: its fields' offsets in declaration order
    pub offsets: &'static [u64],
    /// PLAIN / HANDLE primitive-repr enum: [tag_bytes, variant_count, then per variant in
    /// declaration order: tag_value, field_count, field_offsets..]. Signed tags are cast
    /// through their repr type and i64 to u64. Empty for structs and enums without a
    /// primitive repr; independent of the struct-only `offsets` table.
    /// OPTION: [`option_layout`]'s six words, evaluated in a const. Its other layout,
    /// signature and operation fields are zero, empty or None.
    pub enum_layout: &'static [u64],
    /// HANDLE: rustc's drop_in_place, when the type needs drop
    pub drop: Option<DropFn>,
    /// SHARED: the static's address, taken in its own module (`&raw const`); `size`/`align`
    /// are then rustc's `size_of`/`align_of` of its declared type.
    pub addr: Option<fn() -> usize>,
}

/// An owned buffer in flight across the boundary: pointer, capacity and length (in elements).
/// Both sides allocate with the system allocator, so the buffer moves as is, but each side keeps
/// its own `Vec`/`String` layout: an op never returns a `Vec` or `String`, it writes these
/// words (or returns them) and a plain wrapper, which the caller's side compiles, rebuilds the
/// value with its own `from_raw_parts`. Elements must be plain data (`#[relax_api(plain)]`, or
/// primitives) with an alignment of at most 16.
pub type Words = [usize; 3];

/// Hands a `Vec`'s buffer over (nothing is freed, the receiver owns it now).
pub fn vec_to_words<T>(v: Vec<T>) -> Words {
    debug_assert!(core::mem::align_of::<T>() <= 16);
    let mut v = core::mem::ManuallyDrop::new(v);
    [v.as_mut_ptr() as usize, v.capacity(), v.len()]
}

/// Takes the buffer back as a `Vec`.
///
/// # Safety
/// `w` came from [`vec_to_words`] of a `Vec<T>` for this `T` (or is empty), taken once.
pub unsafe fn vec_from_words<T>(w: Words) -> Vec<T> {
    if w[1] == 0 { return Vec::new(); }
    Vec::from_raw_parts(w[0] as *mut T, w[2], w[1])
}

pub fn string_to_words(s: String) -> Words {
    vec_to_words(s.into_bytes())
}

/// # Safety
/// `w` came from [`string_to_words`] (valid UTF-8), taken once.
pub unsafe fn string_from_words(w: Words) -> String {
    String::from_utf8_unchecked(vec_from_words::<u8>(w))
}

thread_local! { static LAST_PANIC: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) }; }

/// The payload of a panic of the caller's side (Relax-compiled code) on its way through rustc's
/// frames, which run their cleanups as for any panic: the caller's own payload stays on its
/// side. A wrapper that catches it returns 2, and the caller's side goes on with its panic.
/// Dropped anywhere else (a catch_unwind of rustc's ended the panic), it calls `end`, which
/// ends the panic on the caller's side.
#[doc(hidden)]
pub struct CallerPanic {
    pub end: Option<extern "C" fn()>,
}

impl Drop for CallerPanic {
    fn drop(&mut self) {
        if let Some(end) = self.end.take() {
            end();
        }
    }
}

/// A wrapper's caught panic: its text is kept for [`last_panic`]; returns 1. A panic of the
/// caller's side ([`CallerPanic`]) returns 2: it is the caller's to go on with.
#[doc(hidden)]
pub fn caught(p: Box<dyn std::any::Any + Send>) -> u32 {
    let p = match p.downcast::<CallerPanic>() {
        Ok(mut c) => {
            c.end = None;
            return 2;
        }
        Err(p) => p,
    };
    let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".to_string());
    LAST_PANIC.with(|l| *l.borrow_mut() = msg);
    1
}

/// The text of the last panic a wrapper caught on this thread: (ptr, len), valid until the
/// thread's next wrapper call.
pub unsafe extern "C" fn last_panic(ptr: *mut *const u8, len: *mut usize) {
    LAST_PANIC.with(|l| {
        let l = l.borrow();
        *ptr = l.as_ptr();
        *len = l.len();
    });
}

/// Feeds rustc's `Hash` of a handle into Relax's hasher (a handle's `hash` op).
#[doc(hidden)]
pub struct RelaxHasher(pub *mut u8, pub unsafe extern "C-unwind" fn(*mut u8, *const u8, usize));
impl core::hash::Hasher for RelaxHasher {
    fn write(&mut self, b: &[u8]) {
        unsafe { (self.1)(self.0, b.as_ptr(), b.len()) }
    }
    fn finish(&self) -> u64 {
        0
    }
}

/// Every [`Entry`] linked into this binary (empty where the platform has no reader); feature
/// `host-shims`, like the provided crates' (only the `makepad` loader reads the table).
#[cfg(feature = "host-shims")]
pub fn entries() -> &'static [Entry] {
    section::entries()
}

#[cfg(all(feature = "host-shims", target_vendor = "apple"))]
mod section {
    use super::Entry;
    extern "C" {
        #[link_name = "\x01section$start$__DATA$__relax_api"]
        static START: u8;
        #[link_name = "\x01section$end$__DATA$__relax_api"]
        static STOP: u8;
    }
    // one entry so the section (and its start/end symbols) always exists
    #[used]
    #[link_section = "__DATA,__relax_api"]
    static ANCHOR: Entry = super::anchor();
    pub fn entries() -> &'static [Entry] {
        let _ = &ANCHOR;
        unsafe { super::between(core::ptr::addr_of!(START) as *const Entry, core::ptr::addr_of!(STOP) as *const Entry) }
    }
}

#[cfg(all(feature = "host-shims", not(target_vendor = "apple"), not(windows), not(target_arch = "wasm32")))]
mod section {
    use super::Entry;
    extern "C" {
        #[link_name = "__start_relax_api"]
        static START: u8;
        #[link_name = "__stop_relax_api"]
        static STOP: u8;
    }
    #[used]
    #[link_section = "relax_api"]
    static ANCHOR: Entry = super::anchor();
    pub fn entries() -> &'static [Entry] {
        let _ = &ANCHOR;
        unsafe { super::between(core::ptr::addr_of!(START) as *const Entry, core::ptr::addr_of!(STOP) as *const Entry) }
    }
}

#[cfg(all(feature = "host-shims", windows))]
mod section {
    use super::Entry;
    // the linker sorts `.relaxapi$a` < `$m` (the entries) < `$z` and merges them
    #[used]
    #[link_section = ".relaxapi$a"]
    static START: [Entry; 0] = [];
    #[used]
    #[link_section = ".relaxapi$m"]
    static ANCHOR: Entry = super::anchor();
    #[used]
    #[link_section = ".relaxapi$z"]
    static STOP: [Entry; 0] = [];
    pub fn entries() -> &'static [Entry] {
        let _ = &ANCHOR;
        unsafe { super::between(START.as_ptr(), STOP.as_ptr()) }
    }
}

#[cfg(all(feature = "host-shims", target_arch = "wasm32"))]
compile_error!("relax-api's host-shims is the `makepad` loader's, which wasm32 does not run: no table is laid down or read there");

#[allow(dead_code)]
const fn anchor() -> Entry {
    fn none() -> &'static str {
        ""
    }
    Entry { kind: 0, module: "", key: none, sig: none, shim: None, slots: &[], size: 0, align: 0, offsets: &[], enum_layout: &[], drop: None, addr: None }
}

#[allow(dead_code)]
fn platform() -> &'static str {
    if cfg!(target_vendor = "apple") {
        "apple"
    } else if cfg!(windows) {
        "windows"
    } else {
        "elf"
    }
}

#[allow(dead_code)]
unsafe fn between(start: *const Entry, stop: *const Entry) -> &'static [Entry] {
    let bytes = stop as usize - start as usize;
    let size = core::mem::size_of::<Entry>();
    if bytes % size != 0 || (start as usize) % core::mem::align_of::<Entry>() != 0 {
        panic!(
            "relax_api table on {} is not whole Entrys: {} bytes (Entry is {} bytes), start {:#x}",
            platform(),
            bytes,
            size,
            start as usize
        );
    }
    let n = bytes / size;
    core::slice::from_raw_parts(start, n)
}
