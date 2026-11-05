//! A run's end in place: what a restart in the same process does before the next run starts. The
//! program's threads end (`domain::end_run`), what compiles for it stops, the run's own threads go
//! (`run_threads`) with what they hold in thread-locals, the unit goes with its code and data, and
//! every static a run wrote is back to what a fresh process has (`reset_statics` of each module).
//! What stays is the process: its main loop, the window, the GPU device, the host's threads.

use crate::jit::Unit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long the run's own threads get to see the end. A compile between two of its points is what
/// takes the time.
const THREADS_GRACE: Duration = Duration::from_secs(10);
/// How long the program's threads get.
const PROGRAM_GRACE: Duration = Duration::from_secs(5);

pub struct Ended {
    pub program: crate::domain::RunEnd,
    /// The run's own threads that did not end in time, by name.
    pub stayed: Vec<String>,
    /// ms: the code's end and the program's threads; the cache's write; the run's threads and the
    /// compile workers; the unit; the owner thread and the statics.
    pub ms: [f64; 5],
}

/// A restart is on its way: from here to the next run's start.
static RESTARTING: AtomicBool = AtomicBool::new(false);

pub fn restarting() -> bool {
    RESTARTING.load(Ordering::Acquire)
}

/// What the run before this one left for it (reload.rs next_run_env: the recovery record, the
/// state, the edit's save time), for a run started in place: the restart hands it over here. The
/// process's environment is never written for it: the host's threads and the C code they run read
/// the environment meanwhile, and a write under a read is undefined.
static HANDED: std::sync::Mutex<Option<Vec<(&'static str, Option<std::ffi::OsString>)>>> = std::sync::Mutex::new(None);

/// The input `name` of next_run_env as this run has it: from the run before it in this process
/// (`restart`), or for the process's first run from the environment its starter gave it.
pub fn run_input(name: &str) -> Option<std::ffi::OsString> {
    match &*HANDED.lock().unwrap_or_else(|e| e.into_inner()) {
        Some(handed) => handed.iter().find(|(n, _)| *n == name).and_then(|(_, v)| v.clone()),
        None => std::env::var_os(name),
    }
}

/// This run was started in place, by a restart in this process (`run_input` reads its hand-over).
pub fn started_in_place() -> bool {
    HANDED.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// # Safety
/// `up` is the running unit; nothing of the run is called after this.
pub unsafe fn end(up: *mut Unit, grace: Duration) -> Ended {
    let t = Instant::now();
    let mut ms = [0.0; 5];
    let mut mark = {
        let mut last = t;
        move || {
            let now = Instant::now();
            let d = now.duration_since(last).as_secs_f64() * 1e3;
            last = now;
            d
        }
    };
    crate::run_threads::run_ends();
    crate::inspect::events::run_ends();
    // (a compile job running the program's initializer loses its code next: no error of the program)
    crate::jit::COMPILE_ERRORS_FATAL.store(false, Ordering::Relaxed);
    let program = crate::domain::end_run(up, grace);
    crate::run::host_end_app();
    if let Some((native, _)) = NATIVE_END.get() {
        native();
    }
    ms[0] = mark();
    crate::run::summary("restart");
    crate::owner::end_exit();
    ms[1] = mark();
    crate::run::end_console();
    crate::jit::stop_compiling();
    let stayed = crate::run_threads::end_all(THREADS_GRACE);
    let workers_stopped = crate::jit::stop_queue_workers(THREADS_GRACE);
    if !workers_stopped {
        eprintln!("makepad-run: a run's end: compile workers were still in a job after {THREADS_GRACE:?}");
    }
    crate::pool::retire_idle();
    makepad_cpu_budget::end_run();
    ms[2] = mark();
    // from here the process's own threads are not at the run's state
    let gate = crate::run_threads::reset_gate();
    crate::pmhost::free_world();
    let up = crate::run::take_run_unit();
    if !up.is_null() {
        let no_thread_in_it = stayed.is_empty() && workers_stopped;
        if !no_thread_in_it {
            eprintln!("makepad-run: a run's end: its unit stays in memory, threads that did not end may read it: {stayed:?}");
        }
        crate::jit::end_unit(up, no_thread_in_it);
    }
    ms[3] = mark();
    crate::owner::stop();
    reset_all();
    drop(gate);
    ms[4] = mark();
    Ended { program, stayed, ms }
}

static NATIVE_END: std::sync::OnceLock<(fn(), fn(std::ops::Range<usize>))> = std::sync::OnceLock::new();

/// What the crates linked into this binary hold of a run goes in `native`, called when a run's
/// threads have ended (`hal::end_run`, `script::end_run`). What they hold of one domain's heap goes
/// in `domain`, called before that heap goes, also while other programs go on.
pub fn set_native_end(native: fn(), domain: fn(std::ops::Range<usize>)) {
    let _ = NATIVE_END.set((native, domain));
}

/// A domain's heap is about to go.
pub(crate) fn heap_goes(heap: std::ops::Range<usize>) {
    if let Some((_, domain)) = NATIVE_END.get() {
        domain(heap);
    }
}

/// Every static of a run, in the crates under this one and here, as a fresh process has it.
unsafe fn reset_all() {
    crate::jit::thread_locals_end();
    crate::reset_statics();
    relax_types::reset_statics();
    relax_front::reset_statics();
    relax_codegen::reset_statics();
    relax_backend::reset_statics();
    relax_base::reset_statics();
    relax_session::reset_statics();
    crate::cache::reset_stores();
    crate::run_threads::begin_run();
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    leftovers::report();
    crate::zone_alloc::give_back();
}

/// The run ends and the next one starts in this process, with `state` as the app left it. From any
/// thread: this returns when the restart is on its way, and a thread of the run goes on until its
/// run's end meets it (`leave`, or the code it returns to). An error: nothing was started.
pub fn restart(why: String, state: Option<&[u8]>) -> Result<(), String> {
    if RESTARTING.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let _k = crate::zone_alloc::kept();
    let next = match crate::reload::next_run_env(state) {
        Ok(next) => next,
        Err(e) => {
            RESTARTING.store(false, Ordering::Release);
            return Err(e);
        }
    };
    let spawned = std::thread::Builder::new().name("run-restart".into()).spawn(move || {
        eprintln!("makepad live: restarting ({why})");
        let t = Instant::now();
        crate::reload::restart_begins();
        let up = crate::run::running_unit();
        let ended = unsafe { end(up, PROGRAM_GRACE) };
        eprintln!("makepad-run: {} ; run's threads stayed: {:?}; {:.0} ms in all: end {:.0}, cache {:.0}, threads {:.0}, unit {:.0}, statics {:.0}", ended.program.line(), ended.stayed, t.elapsed().as_secs_f64() * 1e3, ended.ms[0], ended.ms[1], ended.ms[2], ended.ms[3], ended.ms[4]);
        // what the next run reads (run_input)
        *HANDED.lock().unwrap_or_else(|e| e.into_inner()) = Some(next);
        RESTARTING.store(false, Ordering::Release);
        crate::run::host_next_run();
    });
    if let Err(e) = spawned {
        eprintln!("makepad-run: the restart's thread did not start: {e}");
        std::process::abort();
    }
    Ok(())
}

/// A thread of the program in a host call, the restart it waits for asked: it leaves the run when
/// the run's end comes, from here (the code of the call it is in may be gone by then).
pub(crate) fn leave() -> ! {
    while !crate::domain::current_is_ending() {
        std::thread::sleep(Duration::from_millis(1));
    }
    unsafe { crate::jit::leave_ended_run() }
}

/// `MAKEPAD_RUN_LEFTOVERS=<file>` (Linux): what a run's end left in the process's statics. Their bytes
/// when `cmd_run` starts are the process's own; after each run's end every symbol whose bytes differ
/// goes in the file. tools/run/fixture/restart_in_place.py holds them against the statics a process
/// keeps on purpose: anything else is the next run seeing the last one.
#[cfg(target_os = "linux")]
pub mod leftovers {
    use std::sync::Mutex;
    extern "C" {
        static __data_start: u8;
        static _end: u8;
    }
    static START: Mutex<Option<Vec<u8>>> = Mutex::new(None);
    fn range() -> (usize, usize) {
        (std::ptr::addr_of!(__data_start) as usize, std::ptr::addr_of!(_end) as usize)
    }
    /// The process's statics as they start.
    pub fn mark() {
        if std::env::var_os("MAKEPAD_RUN_LEFTOVERS").is_none() {
            return;
        }
        ON.store(true, std::sync::atomic::Ordering::SeqCst);
        let (lo, hi) = range();
        let bytes = unsafe { std::slice::from_raw_parts(lo as *const u8, hi - lo) }.to_vec();
        *START.lock().unwrap_or_else(|e| e.into_inner()) = Some(bytes);
        *relax_session::RESETS.lock().unwrap_or_else(|e| e.into_inner()) = Some(Vec::new());
    }
    /// `mark` ran. (A heap goes on a thread that is ending: this reads no environment there.)
    static ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static GONE: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
    /// A domain's heap went: from here its addresses stay unmapped and no later heap gets them, so
    /// whatever still reads it faults. Its range for `report`: no static may point there.
    pub(crate) fn heap_gone(base: usize, size: usize) {
        if !ON.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        extern "C" {
            fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
        }
        // PROT_NONE; MAP_PRIVATE | MAP_FIXED | MAP_ANONYMOUS | MAP_NORESERVE
        unsafe { mmap(base as *mut u8, size, 0, 0x02 | 0x10 | 0x20 | 0x4000, -1, 0) };
        GONE.lock().unwrap_or_else(|e| e.into_inner()).push((base, size));
    }
    /// (value, size, name offset) of each object symbol of this binary.
    fn symbols(elf: &[u8]) -> Vec<(u64, u64, String)> {
        let u16at = |o: usize| u16::from_le_bytes(elf[o..o + 2].try_into().unwrap()) as usize;
        let u32at = |o: usize| u32::from_le_bytes(elf[o..o + 4].try_into().unwrap()) as usize;
        let u64at = |o: usize| u64::from_le_bytes(elf[o..o + 8].try_into().unwrap());
        let (shoff, shentsize, shnum) = (u64at(0x28) as usize, u16at(0x3a), u16at(0x3c));
        let mut out = Vec::new();
        for i in 0..shnum {
            let sh = shoff + i * shentsize;
            if u32at(sh + 4) != 2 {
                continue;
            }
            let (off, size, link) = (u64at(sh + 0x18) as usize, u64at(sh + 0x20) as usize, u32at(sh + 0x28));
            let strs = u64at(shoff + link * shentsize + 0x18) as usize;
            for k in 0..size / 24 {
                let e = off + k * 24;
                if elf[e + 4] & 0xf != 1 {
                    continue;
                }
                let n = strs + u32at(e);
                let len = elf[n..].iter().position(|&b| b == 0).unwrap_or(0);
                out.push((u64at(e + 8), u64at(e + 16), String::from_utf8_lossy(&elf[n..n + len]).into_owned()));
            }
        }
        out
    }
    /// The symbols that differ from `mark` and were not restored, in the file: `name size`.
    pub fn report() {
        let Some(path) = std::env::var_os("MAKEPAD_RUN_LEFTOVERS") else { return };
        let start = START.lock().unwrap_or_else(|e| e.into_inner());
        let Some(start) = start.as_ref() else { return };
        let Ok(elf) = std::fs::read("/proc/self/exe") else { return };
        // the load address: an object symbol's value against where the binary's mapped
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap_or_default();
        let exe = std::fs::read_link("/proc/self/exe").unwrap_or_default();
        let base = maps.lines().find(|l| l.ends_with(exe.to_str().unwrap_or("?"))).and_then(|l| u64::from_str_radix(l.split('-').next()?, 16).ok()).unwrap_or(0);
        let (lo, hi) = range();
        let restored = relax_session::RESETS.lock().unwrap_or_else(|e| e.into_inner()).replace(Vec::new()).unwrap_or_default();
        let gone = GONE.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut lines = String::new();
        for (value, size, name) in symbols(&elf) {
            let at = (base + value) as usize;
            if size == 0 || at < lo || at + size as usize > hi {
                continue;
            }
            let (now, then) = unsafe { (std::slice::from_raw_parts(at as *const u8, size as usize), &start[at - lo..at - lo + size as usize]) };
            if now != then && !restored.iter().any(|&(a, n)| at < a + n && a < at + size as usize) {
                lines.push_str(&format!("{name} {size}\n"));
            }
            // (a restored static's padding holds whatever the stack held; these only name addresses)
            let restored_here = restored.iter().any(|&(a, n)| at < a + n && a < at + size as usize);
            if !gone.is_empty() && !restored_here && !name.contains("5waits5TABLE") && !name.contains("6domain3LOW") {
                let words = unsafe { std::slice::from_raw_parts(at as *const u8, size as usize) }.chunks_exact(8);
                // Not a pointer into a heap: its first address as the end of the mapping below it
                // (a spare stack's top), or a word whose low half is zero (a lock's state and flag
                // over the high bytes of a stack address its padding holds; heaps reserve 64 GB, so
                // such a word lands in one)
                let into = |w: usize| w & 0xffff_ffff != 0 && gone.iter().any(|&(b, n)| w > b && w - b < n);
                if words.map(|w| u64::from_le_bytes(w.try_into().unwrap()) as usize).any(into) {
                    lines.push_str(&format!("-> a heap that went: {name} {size}\n"));
                }
            }
        }
        lines.push_str("--\n");
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = f.write_all(lines.as_bytes());
        }
    }
}

/// `MAKEPAD_RUN_LEFTOVERS` on macOS: a heap that went stays unmapped, and no static may point there
/// (the check above, without the symbols' before and after). What it finds goes to stderr.
#[cfg(target_vendor = "apple")]
pub mod leftovers {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    static ON: AtomicBool = AtomicBool::new(false);
    static GONE: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
    pub fn mark() {
        ON.store(std::env::var_os("MAKEPAD_RUN_LEFTOVERS").is_some(), Ordering::SeqCst);
    }
    pub(crate) fn heap_gone(base: usize, size: usize) {
        if !ON.load(Ordering::SeqCst) {
            return;
        }
        extern "C" {
            fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
        }
        // PROT_NONE; MAP_PRIVATE | MAP_FIXED | MAP_ANON
        unsafe { mmap(base as *mut u8, size, 0, 0x02 | 0x10 | 0x1000, -1, 0) };
        GONE.lock().unwrap_or_else(|e| e.into_inner()).push((base, size));
    }
    pub fn report() {
        if !ON.load(Ordering::SeqCst) {
            return;
        }
        extern "C" {
            fn _dyld_get_image_header(i: u32) -> *const u8;
            fn _dyld_get_image_vmaddr_slide(i: u32) -> isize;
        }
        let gone = GONE.lock().unwrap_or_else(|e| e.into_inner()).clone();
        unsafe {
            let header = _dyld_get_image_header(0);
            let slide = _dyld_get_image_vmaddr_slide(0);
            let ncmds = *(header.add(16) as *const u32);
            let mut c = header.add(32);
            for _ in 0..ncmds {
                let (cmd, size) = (*(c as *const u32), *(c.add(4) as *const u32) as usize);
                // LC_SEGMENT_64: its name, then vmaddr and vmsize
                if cmd == 0x19 && std::slice::from_raw_parts(c.add(8), 6) == b"__DATA" {
                    let (addr, len) = (*(c.add(24) as *const u64), *(c.add(32) as *const u64));
                    let lo = (addr as isize + slide) as usize;
                    for at in (lo..lo + len as usize).step_by(8) {
                        let w = *(at as *const usize);
                        // (heaps sit low here, so two small u32s read as an address: pointers are aligned)
                        if w & 7 == 0 && gone.iter().any(|&(b, n)| w.wrapping_sub(b) < n) {
                            // the unslid address: `nm` names it
                            eprintln!("makepad-run: -> a heap that went: {:#x} -> {w:#x}", (at as isize - slide) as usize);
                        }
                    }
                }
                c = c.add(size);
            }
        }
    }
}
