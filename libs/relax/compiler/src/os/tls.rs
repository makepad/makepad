//! The OS key holding each thread's thread-local block (jit.rs rt_tls_block), and the fast
//! path compiled code reads it through (`Inst::TlsAddr`: `Env::tls_key`).
//!
//! - `key_create(dtor)`: one key per process; `dtor(block)` runs at thread exit.
//! - `get(key)` / `set(key, block)`: the calling thread's block.
//! - `jit_offset(key, emit)`: the value of `Env::tls_key` for this OS (`emit` places glue code
//!   and returns its address). Linux x86-64 reads a native runtime's static-TLS cell via fs;
//!   under Relax it then reads this runtime's word in that block. macOS uses the TSD slot
//!   (checked against pthread_getspecific), Android bionic's TLS_SLOT_APP; under Relax on
//!   Android this runtime's word in the block that slot holds.
//! - `static_tls_*`: Linux x86-64's reserve of static TLS for programs' `#[thread_local]` statics.

include!("../rt_abi.rs");
pub use imp::*;

#[cfg(relax)]
extern "C" {
    pub(crate) fn __relax_tls_cell(key: u64) -> u64;
}

/// The words a runtime under Relax handed to its programs' runtimes (jit.rs relax_tls_cell).
/// Each key names a word in the outermost runtime's block. A run's end frees its words for
/// reuse, and a thread that stays clears them before entering the next run (jit.rs fresh_thread_now).
#[cfg(any(relax, test))]
pub struct Words {
    /// (key, in use)
    keys: Vec<(u64, bool)>,
}

#[cfg(any(relax, test))]
impl Words {
    pub const fn new() -> Words {
        Words { keys: Vec::new() }
    }

    /// Takes a free word, asking the runtime above for a fresh one when all are in use.
    /// A zero key means that runtime hands out no word.
    pub fn take(&mut self, fresh: impl FnOnce() -> u64) -> u64 {
        for (key, in_use) in &mut self.keys {
            if !*in_use {
                *in_use = true;
                return *key;
            }
        }
        let key = fresh();
        if key != 0 {
            self.keys.push((key, true));
        }
        key
    }

    /// The programs that took the words are gone. Every word is free for the next run.
    pub fn run_ended(&mut self) {
        for (_, in_use) in &mut self.keys {
            *in_use = false;
        }
    }

    /// Clears every word on the calling thread. Each address is its writable word there, or zero.
    pub fn clear(&self, addr: impl Fn(u64) -> u64) {
        for &(key, _) in &self.keys {
            let word = addr(key) as *mut u64;
            if !word.is_null() {
                unsafe { word.write_volatile(0) };
            }
        }
    }
}

/// This runtime keeps its words across runs so the next run's runtimes can reuse them.
#[cfg(relax)]
pub static NESTED_WORDS: relax_session::sync::Mutex<Words> = relax_session::sync::Mutex::new(Words::new());

#[cfg(unix)]
mod unix {
    extern "C" {
        pub fn pthread_key_create(key: *mut usize, dtor: Option<extern "C" fn(*mut u8)>) -> i32;
        pub fn pthread_getspecific(key: usize) -> *mut u8;
        pub fn pthread_setspecific(key: usize, v: *mut u8) -> i32;
    }

    pub fn key_create(dtor: extern "C" fn(*mut u8)) -> u64 {
        // pthread_key_t: unsigned long on macOS, unsigned int on Linux (zero-extended here)
        let mut key: usize = 0;
        unsafe {
            pthread_key_create(&mut key, Some(dtor));
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let key = key as u32 as usize;
        key as u64
    }

    pub fn get(key: u64) -> *mut u8 {
        unsafe { pthread_getspecific(key as usize) }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    pub use super::unix::{get, key_create};

    #[cfg(not(relax))]
    thread_local! {
        /// this thread's block (static TLS: JIT code reads it at fs:[env.tls_key])
        static TLS_BLOCK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    #[cfg(not(relax))]
    pub fn set(key: u64, b: *mut u8) {
        unsafe {
            super::unix::pthread_setspecific(key as usize, b);
        }
        TLS_BLOCK.with(|c| c.set(b as u64));
    }

    /// This (native) runtime's block cell's offset from the thread pointer (fs:[0] holds the
    /// thread pointer on x86-64 Linux).
    #[cfg(all(not(relax), target_arch = "x86_64"))]
    fn cell_offset() -> i64 {
        let tp: u64;
        unsafe { core::arch::asm!("mov {}, qword ptr fs:[0]", out(reg) tp, options(nostack, readonly, preserves_flags)) };
        TLS_BLOCK.with(|c| c as *const std::cell::Cell<u64> as u64).wrapping_sub(tp) as i64
    }

    /// Linux x86-64: the block pointer lives in a static-TLS cell of this (executable)
    /// runtime, at a fixed offset from the thread pointer that JIT code reads via fs.
    #[cfg(not(relax))]
    pub fn jit_offset(_key: u64, _emit: &dyn Fn(&[u8]) -> u64) -> u64 {
        #[cfg(target_arch = "x86_64")]
        let off = cell_offset();
        #[cfg(not(target_arch = "x86_64"))]
        let off: i64 = compile_error!("the JIT does not build for Linux on this CPU: its thread-local fast path has only the x86-64 (fs) reader on Linux");
        if off < i32::MIN as i64 || off > i32::MAX as i64 {
            // a plain error: nothing this run does moves the cell
            eprintln!("relax: this runtime's thread-local block cell is {off:#x} from the thread pointer, out of the reach of JIT code");
            std::process::exit(2);
        }
        off as u64
    }

    /// The key of word `off` of this runtime's thread-local block, for a runtime it runs under
    /// Relax (jit.rs relax_tls_cell): this runtime's block cell (fs-relative, negative) in the
    /// high half, `off` in the low half (x64::TLS_TWO_LOAD).
    #[cfg(all(not(relax), target_arch = "x86_64"))]
    pub fn cell_key(off: u32) -> u64 {
        ((cell_offset() as i32 as u32 as u64) << 32) | off as u64
    }

    /// This runtime runs under Relax: the key of its word in the outermost runtime's thread-local
    /// block (`__relax_tls_cell`, jit.rs relax_tls_cell), taken once per process: a run's end keeps
    /// it (each run would take one more word).
    #[cfg(relax)]
    static CELL: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

    #[cfg(relax)]
    fn cell() -> u64 {
        *CELL.get_or_init(|| unsafe { super::__relax_tls_cell(0) })
    }

    #[cfg(relax)]
    pub fn set(key: u64, b: *mut u8) {
        unsafe {
            super::unix::pthread_setspecific(key as usize, b);
        }
        let c = cell();
        if c != 0 {
            let w = unsafe { super::__relax_tls_cell(c) };
            if w != 0 {
                unsafe { (w as *mut u64).write_volatile(b as u64) };
            }
        }
    }

    /// Under Relax: the two-load key of this runtime's word in the outermost runtime's block,
    /// checked through the emitted two-load reader (relax_backend x64::tls2_reader).
    #[cfg(relax)]
    pub fn jit_offset(key: u64, emit: &dyn Fn(&[u8]) -> u64) -> u64 {
        let c = cell();
        if c == 0 {
            eprintln!("relax: the runtime that runs this one hands out no thread-local block word (__relax_tls_cell), so JIT code cannot reach this runtime's thread-local block");
            std::process::exit(2);
        }
        let reader = emit(&relax_backend::x64::tls2_reader());
        let read: rt_fn_ty!(fn(u64) -> u64) = unsafe { std::mem::transmute(reader as usize) };
        let old = get(key);
        set(key, 0x5a5a_0000 as *mut u8);
        let seen = read(c);
        set(key, old);
        if seen != 0x5a5a_0000 {
            panic!("thread-local fast path: block word {:#x} (fs:[{:#x}] + {:#x}) reads {:#x}", c, (c >> 32) as i32, c as u32, seen);
        }
        c
    }
}

/// Linux x86-64: the executable's static TLS that JIT programs' `#[thread_local]` statics share
/// (jit.rs Unit::thread_local_place, `Inst::TlsStatic`): `RESERVE` bytes at one offset from the
/// thread pointer on every thread (the executable's own static TLS, as `TLS_BLOCK`), zero when a
/// thread starts (its TLS image is .tbss). Handed out once per process, never freed: a variable
/// keeps its offset for the process. Not when this runtime itself runs under Relax: its
/// thread-locals are the outer program's, which need not be at a fixed offset.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod fixed {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU32, Ordering};

    const RESERVE: usize = 16 << 10;
    /// 64-aligned: the thread pointer is aligned to the executable's TLS alignment, so a variable
    /// aligned at most that within the reserve is aligned in memory
    #[repr(C, align(64))]
    struct Reserve([Cell<u64>; RESERVE / 8]);
    thread_local! {
        static PROGRAM_TLS: Reserve = const { Reserve([const { Cell::new(0) }; RESERVE / 8]) };
    }
    static BASE: std::sync::OnceLock<Option<i32>> = std::sync::OnceLock::new();
    static NEXT: AtomicU32 = AtomicU32::new(0);

    fn reserve() -> *const Cell<u64> {
        PROGRAM_TLS.with(|r| r.0.as_ptr())
    }

    pub fn static_tls_init(emit: &dyn Fn(&[u8]) -> u64) {
        BASE.get_or_init(|| {
            if cfg!(relax) {
                return None;
            }
            let reader = emit(&relax_backend::x64::fs_reader());
            let fs_base: rt_fn_ty!(fn() -> u64) = unsafe { std::mem::transmute(reader as usize) };
            let off = (reserve() as u64).wrapping_sub(fs_base()) as i64;
            (off >= i32::MIN as i64 && off + RESERVE as i64 <= i32::MAX as i64).then_some(off as i32)
        });
    }

    pub fn static_tls_base() -> Option<i32> {
        BASE.get().copied().flatten()
    }

    pub fn static_tls_alloc(size: u32, align: u32) -> Option<i32> {
        let base = static_tls_base()?;
        if align > 64 {
            return None;
        }
        let mut cur = NEXT.load(Ordering::Relaxed);
        loop {
            let at = (cur + align - 1) / align * align;
            if at as usize + size as usize > RESERVE {
                return None;
            }
            match NEXT.compare_exchange(cur, at + size, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => return Some(base + at as i32),
                Err(c) => cur = c,
            }
        }
    }

    /// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
    pub(crate) unsafe fn reset_statics() {
        use relax_session::reset_static as reset;
        // the offsets start over; jit.rs `fresh_thread` gives a thread that stays a clean reserve
        reset(&BASE, std::sync::OnceLock::new());
        reset(&NEXT, AtomicU32::new(0));
    }

    /// The calling thread's words are the ended run's: they go.
    pub fn static_tls_clear() {
        let r = reserve();
        for k in 0..RESERVE / 8 {
            unsafe { (*r.add(k)).set(0) };
        }
    }

    /// Exchanges the calling thread's static TLS words in use with `saved` (empty: zeros), so a
    /// scoped thread-local block (jit.rs domain_callback) starts from fresh values and the
    /// thread's own come back after it.
    pub fn static_tls_swap(saved: &mut Vec<u64>) {
        if static_tls_base().is_none() {
            return;
        }
        let n = (NEXT.load(Ordering::Acquire) as usize + 7) / 8;
        saved.resize(n, 0);
        let r = reserve();
        for (k, w) in saved.iter_mut().enumerate() {
            *w = unsafe { (*r.add(k)).replace(*w) };
        }
    }
}
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
mod fixed {
    pub fn static_tls_init(_emit: &dyn Fn(&[u8]) -> u64) {}
    pub fn static_tls_base() -> Option<i32> {
        None
    }
    pub fn static_tls_alloc(_size: u32, _align: u32) -> Option<i32> {
        None
    }
    pub fn static_tls_swap(_saved: &mut Vec<u64>) {}
    pub fn static_tls_clear() {}
    pub(crate) unsafe fn reset_statics() {}
}
pub use fixed::*;

#[cfg(target_os = "android")]
mod imp {
    pub use super::unix::{get, key_create};

    /// bionic TLS_SLOT_APP: word 2 of the slot array tpidr_el0 points at, "available for use by
    /// apps in API level 29 and later" (bionic libc/platform/bionic/tls_defines.h; the run host's
    /// minSdk is 29, cargo-makepad android HOST_MIN_SDK). The process is ours (one makepad run host), so Relax owns the slot. The pthread key
    /// stays for its thread-exit destructor; JIT code reads the slot (codegen arm64 tls_addr).
    #[cfg(not(relax))]
    const TLS_SLOT_APP: u64 = 2;

    #[cfg(not(relax))]
    fn slot() -> *mut u64 {
        let tp: u64;
        unsafe { core::arch::asm!("mrs {}, tpidr_el0", out(reg) tp, options(nomem, nostack, preserves_flags)); }
        (tp as *mut u64).wrapping_add(TLS_SLOT_APP as usize)
    }

    #[cfg(not(relax))]
    pub fn set(key: u64, block: *mut u8) {
        unsafe {
            super::unix::pthread_setspecific(key as usize, block);
            slot().write_volatile(block as u64);
        }
    }

    /// The slot index JIT code reads (`Env::tls_key`), checked through the emitted reader.
    #[cfg(not(relax))]
    pub fn jit_offset(key: u64, emit: &dyn Fn(&[u8]) -> u64) -> u64 {
        let reader = emit(&relax_backend::arm64::tsd_reader());
        let read: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(reader as usize) };
        let old = get(key);
        set(key, 0x5a5a_0000 as *mut u8);
        let seen = read(TLS_SLOT_APP);
        set(key, old);
        if seen != 0x5a5a_0000 {
            panic!("thread-local fast path: bionic TLS_SLOT_APP reads {:#x}", seen);
        }
        TLS_SLOT_APP
    }

    /// The key of word `off` of this runtime's thread-local block, for a runtime it runs under
    /// Relax (jit.rs relax_tls_cell): TLS_SLOT_APP's byte offset from tpidr_el0 in the high half,
    /// `off` in the low half (arm64::TLS_TWO_LOAD).
    #[cfg(not(relax))]
    pub fn cell_key(off: u32) -> u64 {
        ((TLS_SLOT_APP * 8) << 32) | off as u64
    }

    /// This runtime runs under Relax: the key of its word in the outermost runtime's thread-local
    /// block (`__relax_tls_cell`, jit.rs relax_tls_cell), taken once per process: a run's end keeps
    /// it (each run would take one more word). TLS_SLOT_APP is the outermost runtime's.
    #[cfg(relax)]
    static CELL: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

    #[cfg(relax)]
    fn cell() -> u64 {
        *CELL.get_or_init(|| unsafe { super::__relax_tls_cell(0) })
    }

    #[cfg(relax)]
    pub fn set(key: u64, b: *mut u8) {
        unsafe {
            super::unix::pthread_setspecific(key as usize, b);
        }
        let c = cell();
        if c != 0 {
            let w = unsafe { super::__relax_tls_cell(c) };
            if w != 0 {
                unsafe { (w as *mut u64).write_volatile(b as u64) };
            }
        }
    }

    /// Under Relax: the two-load key of this runtime's word in the outermost runtime's block,
    /// checked through the emitted two-load reader (relax_backend arm64::tls2_reader).
    #[cfg(relax)]
    pub fn jit_offset(key: u64, emit: &dyn Fn(&[u8]) -> u64) -> u64 {
        let c = cell();
        if c == 0 {
            eprintln!("relax: the runtime that runs this one hands out no thread-local block word (__relax_tls_cell), so JIT code cannot reach this runtime's thread-local block");
            std::process::exit(2);
        }
        let reader = emit(&relax_backend::arm64::tls2_reader());
        let read: rt_fn_ty!(fn(u64) -> u64) = unsafe { std::mem::transmute(reader as usize) };
        let old = get(key);
        set(key, 0x5a5a_0000 as *mut u8);
        let seen = read(c);
        set(key, old);
        if seen != 0x5a5a_0000 {
            panic!("thread-local fast path: block word {:#x} (tpidr_el0[{:#x}] + {:#x}) reads {:#x}", c, c >> 32, c as u32, seen);
        }
        c
    }
}

#[cfg(target_vendor = "apple")]
mod imp {
    pub use super::unix::{get, key_create};
    use super::unix::pthread_setspecific;

    pub fn set(key: u64, b: *mut u8) {
        unsafe {
            pthread_setspecific(key as usize, b);
        }
    }

    /// The inline fast path reads the TSD slot directly: check it sees pthread's value.
    pub fn jit_offset(key: u64, emit: &dyn Fn(&[u8]) -> u64) -> u64 {
        #[cfg(target_arch = "aarch64")]
        let reader = emit(&relax_backend::arm64::tsd_reader());
        #[cfg(not(target_arch = "aarch64"))]
        let reader: u64 = compile_error!("the JIT does not build for x86-64 Apple targets: its thread-local fast path has no x86-64 Apple TLS reader (Apple hosts are AArch64)");
        let read: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(reader as usize) };
        let old = get(key);
        set(key, 0x5a5a_0000 as *mut u8);
        let seen = read(key);
        set(key, old);
        if seen != 0x5a5a_0000 {
            panic!("thread-local fast path: TSD slot {} reads {:#x}", key, seen);
        }
        key
    }
}

/// Windows: a TlsAlloc index, any of the 1088; compiled code reads the word TlsGetValue reads
/// with two loads (relax_backend::x64::win_tls_key): the TEB's TlsSlots for indexes below 64,
/// its TlsExpansionSlots array above (a process with GPU drivers loaded has used up the 64 direct
/// slots before Relax creates its key). The OS has no per-key destructor: a host thread-local
/// guard frees the block at thread exit.
#[cfg(windows)]
mod imp {
    use crate::os::win32::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DTOR: AtomicUsize = AtomicUsize::new(0);

    struct Guard(std::cell::Cell<(u64, usize)>);
    impl Drop for Guard {
        fn drop(&mut self) {
            let (key, b) = self.0.get();
            let d = DTOR.load(Ordering::Acquire);
            if b != 0 && d != 0 {
                let dtor: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(d) };
                unsafe { TlsSetValue(key as u32, std::ptr::null_mut()) };
                dtor(b as *mut u8);
            }
        }
    }
    thread_local! {
        static GUARD: Guard = const { Guard(std::cell::Cell::new((0, 0))) };
    }

    pub fn key_create(dtor: extern "C" fn(*mut u8)) -> u64 {
        DTOR.store(dtor as *const () as usize, Ordering::Release);
        let k = unsafe { TlsAlloc() };
        if k == u32::MAX {
            panic!("TlsAlloc failed");
        }
        k as u64
    }

    pub fn get(key: u64) -> *mut u8 {
        unsafe { TlsGetValue(key as u32) }
    }

    pub fn set(key: u64, b: *mut u8) {
        unsafe {
            TlsSetValue(key as u32, b);
        }
        let _ = GUARD.try_with(|g| g.0.set((key, b as usize)));
    }

    pub fn jit_offset(key: u64, emit: &dyn Fn(&[u8]) -> u64) -> u64 {
        let Some(off) = relax_backend::x64::win_tls_key(key) else {
            panic!("thread-local fast path: TlsAlloc index {} is past the TEB's 1088 slots", key);
        };
        let reader = emit(&relax_backend::x64::tls2_reader());
        let read: rt_fn_ty!(fn(u64) -> u64) = unsafe { std::mem::transmute(reader as usize) };
        let old = get(key);
        unsafe { TlsSetValue(key as u32, 0x5a5a_0000 as *mut u8) };
        let seen = read(off);
        unsafe { TlsSetValue(key as u32, old) };
        if seen != 0x5a5a_0000 {
            panic!("thread-local fast path: index {} (gs:[{:#x}] + {:#x}) reads {:#x}", key, off >> 32, off as u32, seen);
        }
        off
    }
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    fixed::reset_statics();
    // run_end.rs ends the unit before reset_all. Its programs are gone, so the next run's
    // runtimes can take their words.
    #[cfg(relax)]
    NESTED_WORDS.lock().unwrap_or_else(|e| e.into_inner()).run_ended();
}

#[cfg(all(test, any(all(target_os = "linux", target_arch = "x86_64"), target_os = "android")))]
mod tests {
    /// relax#263: runtimes nested under Relax at any depth each keep their thread's block pointer
    /// in a word of the outermost runtime's thread-local block. Four words, as four nested
    /// runtimes take them: on each of two threads the block pointers written at the words'
    /// addresses read back through the two-load reader JIT code inlines (Linux x86-64: the block
    /// cell at a negative fs offset, `sar`; Android arm64: bionic's TLS_SLOT_APP), each thread its
    /// own, and lead to a variable of that runtime's block; a thread with no block, then a block
    /// with no word set, reads nulls. Under Relax the same requests go through the forward. A
    /// run's end frees the words a runtime handed out for its next runtimes, and a staying thread
    /// reads them as nulls after clearing.
    #[test]
    fn nested_runtimes_each_reach_their_word() {
        let keys: Vec<u64> = (0..4).map(|_| crate::jit::relax_tls_cell(0)).collect();
        for (i, &key) in keys.iter().enumerate() {
            assert_ne!(key, 0, "runtime {i} needs a nonzero key");
            assert!(!keys[..i].contains(&key), "runtime {i} shares key {key:#x} with an earlier runtime");
            let hi = (key >> 32) as i32;
            #[cfg(target_os = "linux")]
            assert!(hi < 0, "runtime {i}'s outer block cell needs a negative fs offset, got {hi}");
            #[cfg(target_os = "android")]
            assert_eq!(hi, 16, "runtime {i}'s outer block must be bionic's TLS_SLOT_APP (byte 16 of the slot array)");
            assert_eq!(hi, (keys[0] >> 32) as i32, "runtime {i} must use the same outer block cell");
            assert_eq!((key as u32) % 8, 0, "runtime {i}'s word must be 8-byte aligned");
        }

        #[cfg(target_os = "linux")]
        let reader = relax_backend::x64::tls2_reader();
        #[cfg(target_os = "android")]
        let reader = relax_backend::arm64::tls2_reader();
        let code = relax_backend::exec::Code::new(&reader).expect("executable memory");
        let read: rt_fn_ty!(fn(u64) -> u64) = unsafe { std::mem::transmute(code.ptr()) };
        // Each word holds what a nested runtime keeps there: its thread's block (here a buffer per
        // thread and runtime), whose variables its JIT code reads at an offset after the two loads.
        let write_words = |t: u64| -> Vec<Box<[u64; 4]>> {
            let blocks: Vec<Box<[u64; 4]>> = (0..keys.len() as u64).map(|i| Box::new([t, i, t * 100 + i, !0])).collect();
            for (i, &key) in keys.iter().enumerate() {
                let word = crate::jit::relax_tls_cell(key) as *mut u64;
                assert!(!word.is_null(), "thread {t} needs an address for word {i}");
                unsafe { word.write_volatile(blocks[i].as_ptr() as u64) };
            }
            blocks
        };
        let check_words = |t: u64, blocks: &[Box<[u64; 4]>]| {
            for (i, &key) in keys.iter().enumerate() {
                let block = read(key) as *const u64;
                assert_eq!(block, blocks[i].as_ptr(), "thread {t} must read its own block pointer for word {i}");
                assert_eq!(unsafe { block.add(2).read_volatile() }, t * 100 + i as u64, "thread {t} must reach a variable of runtime {i}'s block through its word");
            }
        };
        std::thread::scope(|s| {
            s.spawn(|| {
                let mine = write_words(1);
                // Keep thread 1's words live across thread 2's writes and exit.
                s.spawn(|| {
                    let theirs = write_words(2);
                    check_words(2, &theirs);
                }).join().unwrap();
                check_words(1, &mine);
            });
        });
        std::thread::scope(|s| {
            s.spawn(|| {
                for &key in &keys {
                    assert_eq!(read(key), 0, "a fresh thread must read null for key {key:#x}");
                }
                crate::jit::tls_block();
                for &key in &keys {
                    assert_eq!(read(key), 0, "an unset word in an allocated block must read null for key {key:#x}");
                }
            });
        });

        let mut words = super::Words::new();
        let fresh = || crate::jit::relax_tls_cell(0);
        let a = words.take(fresh);
        let b = words.take(fresh);
        assert_ne!(a, 0, "the first runtime needs a word");
        assert_ne!(b, 0, "the second runtime needs a word");
        assert_ne!(a, b, "two live runtimes must have distinct words");
        std::thread::spawn(move || {
            for (key, value) in [(a, 11), (b, 22)] {
                let word = crate::jit::relax_tls_cell(key) as *mut u64;
                assert!(!word.is_null(), "the staying thread needs an address for key {key:#x}");
                unsafe { word.write_volatile(value) };
            }
            assert_eq!(read(a), 11, "the staying thread must see the first runtime's value");
            assert_eq!(read(b), 22, "the staying thread must see the second runtime's value");

            words.run_ended();
            let first = words.take(fresh);
            let second = words.take(fresh);
            assert!((first, second) == (a, b) || (first, second) == (b, a), "both ended runtimes' words must be reused");
            let third = words.take(fresh);
            assert_ne!(third, 0, "the third runtime needs a word");
            assert!(third != a && third != b, "the third runtime must get a fresh word while both reused words are in use");

            words.clear(|key| crate::jit::relax_tls_cell(key));
            for key in [a, b, third] {
                assert_eq!(read(key), 0, "the staying thread must read null for key {key:#x} after clearing");
            }
        }).join().unwrap();

        let mut words = super::Words::new();
        assert_eq!(words.take(|| 0), 0, "a runtime that hands out no word must return zero");
        words.run_ended();
        assert_eq!(words.take(|| 7), 7, "a zero key must never be kept for reuse");
    }
}
