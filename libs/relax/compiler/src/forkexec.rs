//! A program's fork for an exec: Relax std's Command with `pre_exec` closures (std's fork path:
//! PR_SET_PDEATHSIG, setsid, ..). POSIX allows only async-signal-safe work between fork and exec
//! in a multithreaded process: the child has only the forking thread, and a lock another thread
//! held at the fork stays held for good. So that child never enters the compiler or the tier-up
//! machinery:
//! - before the fork, Relax std has the code the child will run built (`__relax_precompile`,
//!   jit.rs: its half of the spawn and each closure, with everything they call);
//! - `__relax_fork_exec` forks as the loader's `fork` does and marks the child (`exec_child`):
//!   a lazy stub reached there anyway ends the child at once with the function's name
//!   (`child_reached_unbuilt`), a count stub just runs on;
//! - every fork for an exec is recorded in a page the children share, from the fork until the
//!   parent learnt the exec happened (`__relax_fork_exec_done`), so `makepad inspect why-hung`
//!   names a child that has not reached exec (in_flight).
//! (2026-10-24: makepad-app-aihub's nvidia-smi probe under `makepad run` on Linux left a forked
//! child in a futex wait for 8+ minutes, holding the hub's listening socket.)

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// This process is a forked child on its way to exec (set in the child only).
static EXEC_CHILD: AtomicBool = AtomicBool::new(false);
/// MAKEPAD_RUN_VERBOSE: a child that reached unbuilt code says which on its stderr
static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn exec_child() -> bool {
    EXEC_CHILD.load(Ordering::Relaxed)
}

const ENTRIES: usize = 64;
/// entry word 0 while the fork is being made (no pid yet)
const CLAIMED: u64 = u64::MAX;
/// entry word 3: what the child did last (low 32 bits; the slot it reached in the high 32)
pub const STATE_FORKING: u64 = 1;
pub const STATE_CHILD: u64 = 2;
pub const STATE_UNBUILT: u64 = 3;

/// One fork for an exec: [pid (0 free), forking thread's OS id, wall ns at the fork, state].
type Entry = [AtomicU64; 4];

/// The shared page (MAP_SHARED: the parent reads what its children write). None: no mapping.
fn table() -> Option<&'static [Entry; ENTRIES]> {
    static T: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    extern "C" {
        fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const MAP_SHARED_ANON: i32 = 0x01 | 0x20;
    #[cfg(target_os = "macos")]
    const MAP_SHARED_ANON: i32 = 0x01 | 0x1000;
    let a = *T.get_or_init(|| {
        let p = unsafe { mmap(std::ptr::null_mut(), std::mem::size_of::<[Entry; ENTRIES]>(), 3, MAP_SHARED_ANON, -1, 0) };
        if p as isize == -1 { 0 } else { p as usize }
    });
    (a != 0).then(|| unsafe { &*(a as *const [Entry; ENTRIES]) })
}

thread_local! {
    /// the entry of the fork this thread is making (the child's copy names its own)
    static ENTRY: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}

/// `__relax_fork_exec`: the fork of Relax std's fork_exec. As the loader's `fork` (jit.rs
/// relax_fork: compile jobs and the owner parked, the child's own copy of the code memory), and
/// the child is marked and recorded.
pub(crate) extern "C" fn relax_fork_exec() -> i32 {
    static ENV: std::sync::Once = std::sync::Once::new();
    ENV.call_once(|| VERBOSE.store(std::env::var_os("MAKEPAD_RUN_VERBOSE").is_some_and(|v| v != "0"), Ordering::Relaxed));
    let t = table();
    let k = t.and_then(|t| t.iter().position(|e| e[0].compare_exchange(0, CLAIMED, Ordering::SeqCst, Ordering::SeqCst).is_ok()));
    if let (Some(t), Some(k)) = (t, k) {
        t[k][1].store(crate::os::thread::current(), Ordering::Relaxed);
        t[k][2].store(crate::clock::wall_ns(), Ordering::Relaxed);
        t[k][3].store(STATE_FORKING, Ordering::Release);
    }
    ENTRY.with(|e| e.set(k.unwrap_or(usize::MAX)));
    crate::jit::fork_prepare();
    let pid = crate::os::thread::fork();
    let err = crate::os::proc::last_error();
    if pid == 0 {
        EXEC_CHILD.store(true, Ordering::SeqCst);
        crate::jit::fork_child();
        if let (Some(t), Some(k)) = (t, k) {
            t[k][3].store(STATE_CHILD, Ordering::Release);
        }
    } else {
        crate::jit::fork_parent();
        if let (Some(t), Some(k)) = (t, k) {
            t[k][0].store(if pid > 0 { pid as u64 } else { 0 }, Ordering::Release);
        }
    }
    crate::os::proc::set_last_error(err);
    pid
}

/// `__relax_fork_exec_done(pid)`: the parent learnt that child `pid` exec'd or ended. 1: the
/// child ended at code that was not built before the fork (a function reached only through a
/// vtable or a pointer made at run time): that code is built now, and std forks again (the
/// child's effects ended with it); 0: done.
pub(crate) extern "C" fn relax_fork_exec_done(pid: i32) -> u32 {
    let _keep = crate::os::proc::KeepLastError::new();
    let Some(t) = table() else { return 0 };
    let mut unbuilt = None;
    for e in t.iter().filter(|e| pid > 0 && e[0].load(Ordering::Acquire) == pid as u64) {
        let st = e[3].load(Ordering::Acquire);
        if st & 0xffff_ffff == STATE_UNBUILT {
            unbuilt = Some((st >> 32) as u32);
        }
        e[0].store(0, Ordering::Release);
    }
    match unbuilt {
        Some(slot) => {
            crate::jit::precompile_from(slot);
            1
        }
        None => 0,
    }
}

/// A lazy stub reached in a forked child before its exec (jit.rs rt_compile): the code was not
/// built before the fork. Compiling here could wait for good on a lock another thread held at
/// the fork, so the child ends at once (exit 127), with the function's name in its entry: the
/// parent builds it and std forks again (`relax_fork_exec_done`). Async-signal-safe: atomics,
/// _exit; the name goes to stderr only with MAKEPAD_RUN_VERBOSE (read before the fork).
pub(crate) fn child_reached_unbuilt(slot: u32) -> ! {
    let k = ENTRY.try_with(|e| e.get()).unwrap_or(usize::MAX);
    if let Some(t) = table().filter(|_| k < ENTRIES) {
        t[k][3].store(STATE_UNBUILT | (slot as u64) << 32, Ordering::Release);
    }
    extern "C" {
        fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    }
    let put = |b: &[u8]| unsafe {
        write(2, b.as_ptr(), b.len());
    };
    if !VERBOSE.load(Ordering::Relaxed) {
        crate::os::thread::exit_now(127)
    }
    put(b"makepad run: a forked child reached `");
    crate::jit::with_slot_name(slot, &mut |n| put(&n[..n.len().min(300)]));
    put(b"` before its exec, and it was not built before the fork: the child of a spawn never compiles (it has only the forking thread; locks other threads held stay held) -- it ends (exit 127), and the spawn builds it and forks again\n");
    crate::os::thread::exit_now(127)
}

/// A fork for an exec that has not been reported done: (pid, 0 while forking), forking thread's
/// OS id, ns since the fork, state, slot (STATE_UNBUILT).
pub struct InFlight {
    pub pid: u64,
    pub thread: u64,
    pub age_ns: u64,
    pub state: u64,
    pub slot: u32,
}

pub fn in_flight() -> Vec<InFlight> {
    let Some(t) = table() else { return Vec::new() };
    let now = crate::clock::wall_ns();
    t.iter()
        .filter_map(|e| {
            let pid = e[0].load(Ordering::Acquire);
            (pid != 0).then(|| {
                let st = e[3].load(Ordering::Acquire);
                InFlight { pid: if pid == CLAIMED { 0 } else { pid }, thread: e[1].load(Ordering::Relaxed), age_ns: now.saturating_sub(e[2].load(Ordering::Relaxed)), state: st & 0xffff_ffff, slot: (st >> 32) as u32 }
            })
        })
        .collect()
}
