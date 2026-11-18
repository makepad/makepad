//! Program signals wait for a compile, run on program threads, and keep their dispositions and
//! queued values. Default fatal signals still end a compile, and bypassing the trampoline reports
//! re-entry. Each test uses a child because signal dispositions and compiler state are process-wide,
//! and some cases must terminate the process.

use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::process::ExitStatusExt;

use crate::jit::{FnKey, Unit};
use crate::owner::{self, Class};

const CHILD: &str = "RELAX_PROGSIG_CHILD";
#[cfg(target_os = "linux")]
const SIGUSR2: i32 = 12;
#[cfg(target_vendor = "apple")]
const SIGUSR2: i32 = 31;

extern "C" {
    fn pipe(fds: *mut i32) -> i32;
    fn pthread_self() -> usize;
    fn pthread_kill(thread: usize, sig: i32) -> i32;
    fn setrlimit(resource: i32, limit: *const [u64; 2]) -> i32;
}

const FIXTURE: &str = r#"
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering::SeqCst};
use std::sync::Mutex;
use std::ptr::{null, null_mut};

#[cfg(target_os = "linux")]
#[repr(C)]
struct SigAction {
    handler: usize,
    mask: [u64; 16],
    flags: i32,
    pad: i32,
    restorer: usize,
}
#[cfg(target_vendor = "apple")]
#[repr(C)]
struct SigAction {
    handler: usize,
    mask: u32,
    flags: i32,
}
extern "C" {
    fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
    fn signal(sig: i32, h: usize) -> usize;
    fn pthread_self() -> usize;
    fn raise(sig: i32) -> i32;
    fn write(fd: i32, b: *const u8, n: usize) -> isize;
    pub fn setrlimit(r: i32, l: *const [u64; 2]) -> i32;
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
    fn sigaltstack(ss: *const StackT, old: *mut StackT) -> i32;
    fn memset(p: *mut u8, c: i32, n: usize) -> *mut u8;
}
#[cfg(target_os = "linux")]
#[repr(C)]
struct StackT {
    sp: *mut u8,
    flags: i32,
    pad: i32,
    size: usize,
}
#[cfg(target_vendor = "apple")]
#[repr(C)]
struct StackT {
    sp: *mut u8,
    size: usize,
    flags: i32,
}
const SIGSEGV: i32 = 11;
#[cfg(target_os = "linux")]
const SA_ONSTACK: i32 = 0x0800_0000;
#[cfg(target_vendor = "apple")]
const SA_ONSTACK: i32 = 0x1;
#[cfg(target_os = "linux")]
const MAP_PRIVATE_ANON: i32 = 0x22;
#[cfg(target_vendor = "apple")]
const MAP_PRIVATE_ANON: i32 = 0x1002;
/// a small stack's size and its guard's, and the protected page's (any page size)
const SMALL: usize = 1 << 16;
#[cfg(target_os = "linux")]
const SIGUSR1: i32 = 10;
#[cfg(target_vendor = "apple")]
const SIGUSR1: i32 = 30;
#[cfg(target_os = "linux")]
const SIGUSR2: i32 = 12;
#[cfg(target_vendor = "apple")]
const SIGUSR2: i32 = 31;
const SIGABRT: i32 = 6;
const SIGWINCH: i32 = 28;
#[cfg(target_os = "linux")]
const SA_SIGINFO: i32 = 4;
#[cfg(target_vendor = "apple")]
const SA_SIGINFO: i32 = 0x40;
#[cfg(target_os = "linux")]
const SA_RESTART: i32 = 0x1000_0000;
#[cfg(target_vendor = "apple")]
const SA_RESTART: i32 = 0x2;
#[cfg(target_os = "linux")]
const SA_NODEFER: i32 = 0x4000_0000;
#[cfg(target_vendor = "apple")]
const SA_NODEFER: i32 = 0x10;
#[cfg(target_os = "linux")]
const SA_RESETHAND: i32 = 0x8000_0000u32 as i32;
#[cfg(target_vendor = "apple")]
const SA_RESETHAND: i32 = 0x4;

static SEQ: AtomicU64 = AtomicU64::new(0);
static RUNS: AtomicU64 = AtomicU64::new(0);
static H_THREAD: AtomicU64 = AtomicU64::new(0);
static H_ORDER: AtomicU64 = AtomicU64::new(0);
static T_ORDER: AtomicU64 = AtomicU64::new(0);
static MAIN: AtomicU64 = AtomicU64::new(0);
static IDLE: AtomicU64 = AtomicU64::new(0);
static PIPE_FD: AtomicI32 = AtomicI32::new(-1);
static STOP: AtomicBool = AtomicBool::new(false);
static IDLE_JOIN: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
static NVALUES: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "linux")]
static VALUES: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn action(handler: usize, flags: i32) -> SigAction {
    let mut a: SigAction = unsafe { std::mem::zeroed() };
    a.handler = handler;
    a.flags = flags;
    a
}

#[inline(never)]
fn first_in_handler(x: u64) -> u64 { x.wrapping_mul(3).wrapping_add(1) }

extern "C" fn on_usr2(sig: i32, _info: *mut u8, _uc: *mut u8) {
    RUNS.fetch_add(1, SeqCst);
    H_THREAD.store(unsafe { pthread_self() } as u64, SeqCst);
    H_ORDER.store(SEQ.fetch_add(1, SeqCst), SeqCst);
    let byte = first_in_handler(sig as u64) as u8;
    let fd = PIPE_FD.load(SeqCst);
    if fd >= 0 { unsafe { write(fd, &byte, 1); } }
}

pub fn install(out: &mut [u64; 8]) {
    MAIN.store(unsafe { pthread_self() } as u64, SeqCst);
    PIPE_FD.store(out[0] as i32, SeqCst);
    let a = action(on_usr2 as *const () as usize, SA_SIGINFO | SA_RESTART);
    assert_eq!(unsafe { sigaction(SIGUSR2, &a, null_mut()) }, 0, "SIGUSR2 installation must succeed");
}

pub fn start_idle(_out: &mut [u64; 8]) {
    let (ready, wait) = std::sync::mpsc::channel();
    let h = std::thread::spawn(move || {
        IDLE.store(unsafe { pthread_self() } as u64, SeqCst);
        ready.send(()).expect("the idle thread must publish its identity");
        while !STOP.load(SeqCst) { std::thread::park(); }
    });
    *IDLE_JOIN.lock().unwrap() = Some(h);
    wait.recv().expect("the idle thread must be ready before the compile");
}

pub fn stop_idle(_out: &mut [u64; 8]) {
    STOP.store(true, SeqCst);
    let h = IDLE_JOIN.lock().unwrap().take().expect("the idle thread must have started");
    h.thread().unpark();
    h.join().expect("the idle thread must finish normally");
}

pub fn target_in_compile(_out: &mut [u64; 8]) { T_ORDER.store(SEQ.fetch_add(1, SeqCst), SeqCst); }

pub fn report(out: &mut [u64; 8]) {
    *out = [RUNS.load(SeqCst), H_THREAD.load(SeqCst), H_ORDER.load(SeqCst), T_ORDER.load(SeqCst),
        MAIN.load(SeqCst), IDLE.load(SeqCst), NVALUES.load(SeqCst), 0];
}

extern "C" fn other(_sig: i32) {}

pub fn observables(out: &mut [u64; 8]) {
    *out = [0; 8];
    let usr2 = on_usr2 as *const () as usize;
    let other = other as *const () as usize;
    let flags = SA_SIGINFO | SA_RESTART | SA_NODEFER;
    let mut a = action(other, flags);
    #[cfg(target_os = "linux")]
    { a.mask[0] = 1u64 << (SIGWINCH - 1); }
    #[cfg(target_vendor = "apple")]
    { a.mask = 1u32 << (SIGWINCH - 1); }
    let mut old0 = action(0, 0);
    assert_eq!(unsafe { sigaction(SIGUSR2, &a, &mut old0) }, 0, "replacing the program action must succeed");
    out[0] = (old0.handler == usr2) as u64;
    let mut got = action(0, 0);
    assert_eq!(unsafe { sigaction(SIGUSR2, null(), &mut got) }, 0, "querying the replacement must succeed");
    out[1] = (got.handler == other) as u64;
    #[cfg(target_os = "linux")]
    { out[2] = (got.mask[0] & (1u64 << (SIGWINCH - 1)) != 0) as u64; }
    #[cfg(target_vendor = "apple")]
    { out[2] = (got.mask & (1u32 << (SIGWINCH - 1)) != 0) as u64; }
    out[3] = (got.flags & (flags | SA_RESETHAND) == flags) as u64;
    assert_eq!(unsafe { sigaction(SIGUSR2, &old0, null_mut()) }, 0, "restoring the old action must succeed");
    assert_eq!(unsafe { sigaction(SIGUSR2, null(), &mut got) }, 0, "querying the restored action must succeed");
    out[4] = (got.handler == usr2) as u64;
    out[5] = (unsafe { signal(SIGUSR2, other) } == usr2) as u64;
    out[6] = (unsafe { signal(SIGUSR2, usr2) } == other) as u64;
}

pub fn install_resethand(_out: &mut [u64; 8]) {
    let a = action(on_usr2 as *const () as usize, SA_SIGINFO | SA_RESETHAND);
    assert_eq!(unsafe { sigaction(SIGUSR2, &a, null_mut()) }, 0, "SA_RESETHAND installation must succeed");
}

pub fn query_handler(out: &mut [u64; 8]) {
    let mut got = action(0, 0);
    assert_eq!(unsafe { sigaction(SIGUSR2, null(), &mut got) }, 0, "querying the current handler must succeed");
    out[0] = got.handler as u64;
}

pub fn remove_usr2(_out: &mut [u64; 8]) {
    assert_ne!(unsafe { signal(SIGUSR2, 0) }, usize::MAX, "removing the SIGUSR2 handler must succeed");
}

extern "C" fn on_abrt(_sig: i32) {
    let message = b"fixture: SIGABRT report\n";
    unsafe {
        write(2, message.as_ptr(), message.len());
        signal(SIGABRT, 0);
        raise(SIGABRT);
    }
}

pub fn install_abrt(_out: &mut [u64; 8]) {
    assert_ne!(unsafe { signal(SIGABRT, on_abrt as *const () as usize) }, usize::MAX,
        "SIGABRT installation must succeed");
}

static PROTECTED: AtomicU64 = AtomicU64::new(0);
static SEGV_RUNS: AtomicU64 = AtomicU64::new(0);

#[inline(never)]
fn first_in_segv(x: u64) -> u64 { x.wrapping_mul(5).wrapping_add(1) }

extern "C" fn on_segv(_sig: i32, _info: *mut u8, _uc: *mut u8) {
    SEGV_RUNS.fetch_add(first_in_segv(0), SeqCst);
    unsafe { mprotect(PROTECTED.load(SeqCst) as *mut u8, SMALL, 3); }
}

pub fn install_segv(_out: &mut [u64; 8]) {
    let a = action(on_segv as *const () as usize, SA_SIGINFO | SA_ONSTACK);
    assert_eq!(unsafe { sigaction(SIGSEGV, &a, null_mut()) }, 0, "SIGSEGV installation must succeed");
}

/// A 64 KB signal stack (above a guard) placed above the calling thread's stack, a page it
/// protected, and a write into that page by the C library: the handler runs on the small stack.
pub fn segv_on_small_stack(out: &mut [u64; 8]) {
    let mark = 0u8;
    let here = &mark as *const u8 as usize;
    let mut base = null_mut();
    for shift in 28..40 {
        let hint = (here + (1usize << shift)) & !(SMALL - 1);
        let p = unsafe { mmap(hint as *mut u8, 2 * SMALL, 3, MAP_PRIVATE_ANON, -1, 0) };
        if p as isize == -1 { continue; }
        if p as usize > here { base = p; break; }
        unsafe { munmap(p, 2 * SMALL); }
    }
    assert!(!base.is_null(), "a small stack above the thread's");
    assert_eq!(unsafe { mprotect(base, SMALL, 0) }, 0, "the small stack's guard");
    let mut ss: StackT = unsafe { std::mem::zeroed() };
    ss.sp = unsafe { base.add(SMALL) };
    ss.size = SMALL;
    let mut old: StackT = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { sigaltstack(&ss, &mut old) }, 0, "the small signal stack must be taken");
    let page = unsafe { mmap(null_mut(), SMALL, 0, MAP_PRIVATE_ANON, -1, 0) };
    PROTECTED.store(page as u64, SeqCst);
    unsafe { memset(page, 7, std::hint::black_box(1)); }
    assert_eq!(unsafe { sigaltstack(&old, null_mut()) }, 0, "the thread's signal stack must come back");
    out[0] = SEGV_RUNS.load(SeqCst);
    out[1] = unsafe { *page } as u64;
}

#[cfg(target_os = "linux")]
extern "C" fn on_rt(_sig: i32, info: *mut u8, _uc: *mut u8) {
    let n = NVALUES.load(SeqCst) as usize;
    VALUES[n].store(unsafe { *(info.add(24) as *const u64) }, SeqCst);
    NVALUES.fetch_add(1, SeqCst);
    RUNS.fetch_add(1, SeqCst);
}

#[cfg(target_os = "linux")]
pub fn install_rt(_out: &mut [u64; 8]) {
    let a = action(on_rt as *const () as usize, SA_SIGINFO);
    assert_eq!(unsafe { sigaction(50, &a, null_mut()) }, 0, "realtime signal installation must succeed");
}

#[cfg(target_os = "linux")]
pub fn values(out: &mut [u64; 8]) {
    for i in 0..3 { out[i] = VALUES[i].load(SeqCst); }
    out[3] = NVALUES.load(SeqCst);
}

static USR1_RUNS: AtomicU64 = AtomicU64::new(0);
static WATCH_SIG_RUNS: AtomicU64 = AtomicU64::new(0);
extern "C" fn on_usr1(_sig: i32, _info: *mut u8, _uc: *mut u8) { USR1_RUNS.fetch_add(1, SeqCst); }
extern "C" fn on_watch_sig(_sig: i32, _info: *mut u8, _uc: *mut u8) { WATCH_SIG_RUNS.fetch_add(1, SeqCst); }

/// Handlers on SIGUSR1 and on the host's watchdog signal (out[0]).
pub fn install_usr1(out: &mut [u64; 8]) {
    let a = action(on_usr1 as *const () as usize, SA_SIGINFO | SA_RESTART);
    assert_eq!(unsafe { sigaction(SIGUSR1, &a, null_mut()) }, 0, "SIGUSR1 installation must succeed");
    let a = action(on_watch_sig as *const () as usize, SA_SIGINFO | SA_RESTART);
    assert_eq!(unsafe { sigaction(out[0] as i32, &a, null_mut()) }, 0, "the watchdog signal's installation must succeed");
}

/// A thread of the program calls the host's out[4] and ends; joined.
pub fn a_thread_calls_and_ends(out: &mut [u64; 8]) {
    let f: extern "C" fn() = unsafe { std::mem::transmute(out[4] as usize) };
    std::thread::spawn(move || f()).join().expect("the program's thread must end normally");
}

/// Raises out[1] (0: none); out[2], out[3]: the SIGUSR1 and watchdog-signal handlers' runs.
pub fn raise_and_count(out: &mut [u64; 8]) {
    if out[1] != 0 {
        assert_eq!(unsafe { raise(out[1] as i32) }, 0, "raise must succeed");
    }
    out[2] = USR1_RUNS.load(SeqCst);
    out[3] = WATCH_SIG_RUNS.load(SeqCst);
}
"#;

fn child(test: &str, hook: &str) -> std::process::Output {
    child_with(test, hook, &[])
}

fn child_with(test: &str, hook: &str, env: &[(&str, &str)]) -> std::process::Output {
    let _slot = crate::test_child::slot();
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &format!("progsig_tests::{test}"), "--nocapture", "--test-threads=1"])
        .env(CHILD, test)
        .env("RELAX_TEST_SIGNAL_IN_COMPILE", hook)
        .envs(env.iter().copied())
        .output()
        .expect("the signal test child must run")
}

fn setup() -> (Box<Unit>, u32) {
    setup_with(FIXTURE, "progsig_fixture")
}

/// A unit of fixture crate `name` (source `src`).
fn setup_with(src: &str, name: &str) -> (Box<Unit>, u32) {
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-progsig-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lib.rs");
    std::fs::write(&path, src).unwrap();
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), name, false).unwrap();
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let u = Unit::new(prog, tcx, core);
    crate::os::progsig::start();
    (u, krate)
}

fn call(u: &mut Unit, krate: u32, name: &str, arg: u64) {
    let up = &mut *u as *mut Unit;
    let def = (0..u.prog.defs.len()).map(|i| crate::program::DefId(i as u32))
        .find(|&d| u.prog.def(d).krate == krate && u.prog.name(d) == name)
        .unwrap_or_else(|| panic!("the fixture must define {name}"));
    let entry = owner::call(Class::Program, || { let f = u.fn_id(FnKey::Inst(def, vec![])); u.entry(f) });
    unsafe { crate::jit::call_jit(up, entry, arg) }.unwrap_or_else(|p| panic!("fixture {name} must return normally: {}", p.message));
}

fn pipe_pair() -> (File, File) {
    let mut fds = [-1; 2];
    assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0, "the synchronization pipe must open");
    unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) }
}

fn deflect(u: &mut Unit, krate: u32, delivered: &mut File, runs: u64) {
    let (mut wait, mut release) = pipe_pair();
    let (ready, receive) = std::sync::mpsc::channel();
    let host = std::thread::spawn(move || {
        ready.send(unsafe { pthread_self() }).expect("the host thread must publish its identity");
        wait.read_exact(&mut [0]).expect("the host thread must stay alive until delivery");
    });
    let tid = receive.recv().expect("the host thread must be ready for its signal");
    assert_eq!(unsafe { pthread_kill(tid, SIGUSR2) }, 0, "sending the signal to the host thread must succeed");
    delivered.read_exact(&mut [0]).expect("the deflected handler must acknowledge delivery");
    let mut out = [0u64; 8];
    call(u, krate, "report", out.as_mut_ptr() as u64);
    assert_eq!(out[0], runs, "deflection must run the handler exactly once");
    assert_eq!(out[1], out[4], "deflection must reach the remaining program thread");
    assert_ne!(out[1], tid as u64, "the host thread must never run the program handler");
    assert!(crate::os::progsig::counts(SIGUSR2)[2] >= 1, "the host delivery must be counted as deflected");
    release.write_all(&[1]).expect("the host thread must be released after delivery");
    host.join().expect("the host thread must finish normally");
}

#[test]
fn a_signal_in_a_compile_runs_its_handler_after_it_on_the_same_thread() {
    const TEST: &str = "a_signal_in_a_compile_runs_its_handler_after_it_on_the_same_thread";
    if std::env::var(CHILD).as_deref() != Ok(TEST) {
        let output = child(TEST, &format!("target_in_compile,{SIGUSR2},self"));
        let err = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "the deferred signal child must succeed: {:?}\n{err}", output.status);
        assert!(err.contains("its handler ran 0 times so far"), "the handler must not run inside the compile:\n{err}");
        return;
    }
    let (mut u, krate) = setup();
    let _program = crate::os::program_thread();
    let mut out = [0u64; 8];
    let arg = out.as_mut_ptr() as u64;
    out[0] = u64::MAX;
    call(&mut u, krate, "install", arg);
    call(&mut u, krate, "start_idle", arg);
    call(&mut u, krate, "target_in_compile", arg);
    call(&mut u, krate, "report", arg);
    assert_eq!(out[0], 1, "the deferred signal must run its handler once");
    assert_eq!(out[1], out[4], "the deferred handler must run on the compiling thread");
    assert_ne!(out[1], out[5], "the deferred handler must not move to the idle program thread");
    assert!(out[2] < out[3], "the deferred handler must run before the target body");
    assert_eq!(crate::os::progsig::counts(SIGUSR2)[1], 1, "the compile must defer the signal once");
    call(&mut u, krate, "stop_idle", arg);

    let (mut delivered, writer) = pipe_pair();
    out[0] = writer.as_raw_fd() as u64;
    call(&mut u, krate, "install", arg);
    deflect(&mut u, krate, &mut delivered, 2);

    call(&mut u, krate, "observables", arg);
    assert_eq!(out[0], 1, "sigaction must return the old program handler");
    assert_eq!(out[1], 1, "a query must return the replacement program handler");
    assert_eq!(out[2], 1, "a query must preserve the program's SIGWINCH mask bit");
    assert_eq!(out[3], 1, "a query must preserve the program's action flags");
    assert_eq!(out[4], 1, "restoring the old action must restore the program handler");
    assert_eq!(out[5], 1, "signal must return the previous on_usr2 handler");
    assert_eq!(out[6], 1, "signal must return the previous other handler");

    call(&mut u, krate, "install_resethand", arg);
    deflect(&mut u, krate, &mut delivered, 3);
    call(&mut u, krate, "query_handler", arg);
    assert_eq!(out[0], 0, "SA_RESETHAND must reset only after delivery runs the program handler");

    // the install reads its pipe (none) through the pointer
    out[0] = u64::MAX;
    call(&mut u, krate, "install", out.as_mut_ptr() as u64);
    let bit = 1u64 << (SIGUSR2 - 1);
    crate::os::progsig::applied_mask(|m| m & bit != 0);
    call(&mut u, krate, "remove_usr2", arg);
    crate::os::progsig::applied_mask(|m| m & bit == 0);
}

#[test]
fn a_process_signal_in_a_compile_runs_its_handler_once_on_a_program_thread() {
    const TEST: &str = "a_process_signal_in_a_compile_runs_its_handler_once_on_a_program_thread";
    if std::env::var(CHILD).as_deref() != Ok(TEST) {
        let output = child(TEST, &format!("target_in_compile,{SIGUSR2},process"));
        assert!(output.status.success(), "the process signal child must succeed: {:?}\n{}", output.status, String::from_utf8_lossy(&output.stderr));
        return;
    }
    let (mut u, krate) = setup();
    let _program = crate::os::program_thread();
    let (mut delivered, writer) = pipe_pair();
    let mut out = [0u64; 8];
    let arg = out.as_mut_ptr() as u64;
    out[0] = writer.as_raw_fd() as u64;
    call(&mut u, krate, "install", arg);
    call(&mut u, krate, "start_idle", arg);
    call(&mut u, krate, "target_in_compile", arg);
    // A process-directed delivery can still be travelling through host threads when the call returns.
    delivered.read_exact(&mut [0]).expect("the process signal handler must acknowledge delivery");
    call(&mut u, krate, "stop_idle", arg);
    call(&mut u, krate, "report", arg);
    assert_eq!(out[0], 1, "the process signal must run its handler once");
    assert!(out[1] == out[4] || out[1] == out[5], "the process signal handler must run on a program thread");
}

#[test]
fn a_default_sigterm_in_a_hung_compile_ends_the_process() {
    const TEST: &str = "a_default_sigterm_in_a_hung_compile_ends_the_process";
    if std::env::var(CHILD).as_deref() != Ok(TEST) {
        let output = child(TEST, "target_in_compile,15,process,block");
        assert_eq!(output.status.signal(), Some(15), "default SIGTERM must end a hung compile: {:?}\n{}", output.status, String::from_utf8_lossy(&output.stderr));
        return;
    }
    let (mut u, krate) = setup();
    let _program = crate::os::program_thread();
    let mut out = [0u64; 8];
    call(&mut u, krate, "target_in_compile", out.as_mut_ptr() as u64);
    panic!("the hung compile must not return after default SIGTERM");
}

#[test]
fn an_outside_sigabrt_in_a_compile_reports_and_ends_the_process() {
    const TEST: &str = "an_outside_sigabrt_in_a_compile_reports_and_ends_the_process";
    if std::env::var(CHILD).as_deref() != Ok(TEST) {
        let output = child(TEST, "target_in_compile,6,process");
        let err = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.signal(), Some(6), "outside SIGABRT must end the compile: {:?}\n{err}", output.status);
        assert!(err.contains("SIGABRT on thread"), "outside SIGABRT must have relax's report:\n{err}");
        assert!(err.contains("fixture: SIGABRT report"), "outside SIGABRT must run the program's handler after the report:\n{err}");
        return;
    }
    assert_eq!(unsafe { setrlimit(4, &[0, 0]) }, 0, "the SIGABRT child must disable core dumps");
    let (mut u, krate) = setup();
    let _program = crate::os::program_thread();
    let mut out = [0u64; 8];
    let arg = out.as_mut_ptr() as u64;
    call(&mut u, krate, "install_abrt", arg);
    call(&mut u, krate, "target_in_compile", arg);
    // A host thread that took the process-directed signal hands it back to the process, so it can
    // still be on its way when the call returns: returning now would end the child with status 0.
    // The minute only turns a lost signal into a failure instead of a hung parent.
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(60) {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the outside SIGABRT must end the process once the compile is over");
}

/// One source for the SIGABRT fixture: run natively in this binary (`abrt_native`) and compiled by
/// a unit (`ABRT_FIXTURE`), so both children run the same program.
macro_rules! abrt_fixture {
    ($($t:tt)*) => {
        mod abrt_native { $($t)* }
        const ABRT_FIXTURE: &str = stringify!($($t)*);
    };
}

abrt_fixture! {
    use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
    extern "C" {
        fn signal(sig: i32, h: usize) -> usize;
        fn raise(sig: i32) -> i32;
        fn abort() -> !;
        fn write(fd: i32, buf: *const core::ffi::c_void, n: usize) -> isize;
        fn _exit(code: i32) -> !;
    }
    static RUNS: AtomicU64 = AtomicU64::new(0);
    // a fn of the test's that the ending handler calls first (out[2]; 0: none)
    static HOOK: AtomicU64 = AtomicU64::new(0);
    fn say(m: &[u8]) {
        unsafe { write(2, m.as_ptr().cast(), m.len()) };
    }
    extern "C" fn on_abrt(_sig: i32) {
        RUNS.fetch_add(1, SeqCst);
        say(b"abrt fixture: the handler ran\n");
    }
    extern "C" fn on_abrt_exit(_sig: i32) {
        let hook = HOOK.load(SeqCst);
        if hook != 0 {
            let f: extern "C" fn() = unsafe { core::mem::transmute(hook as usize) };
            f();
        }
        say(b"abrt fixture: the handler ends the process\n");
        unsafe { _exit(3) }
    }
    // out[0]: 1 raise(SIGABRT), 2 abort(), each with a handler that returns; 3 raise(SIGABRT)
    // with one that ends the process; out[1]: the returning handler's runs
    pub fn abrt_case(out: &mut [u64; 8]) {
        HOOK.store(out[2], SeqCst);
        let h = if out[0] == 3 { on_abrt_exit as *const () as usize } else { on_abrt as *const () as usize };
        unsafe { signal(6, h) };
        if out[0] == 2 {
            unsafe { abort() }
        }
        unsafe { raise(6) };
        say(b"abrt fixture: raise returned\n");
        out[1] = RUNS.load(SeqCst);
    }
}

/// A program's SIGABRT handler under the JIT does what it does natively: it runs, and when it
/// returns raise(SIGABRT) goes on and abort() ends the process, relax's report after it; one that
/// ends the process is the only one to speak, and finds the abort's site in the flight recorder.
/// Each case runs natively and as a unit's program; their ends and the fixture's lines must be the
/// same.
#[test]
fn a_program_sigabrt_handler_runs_and_returns_as_it_does_natively() {
    const TEST: &str = "a_program_sigabrt_handler_runs_and_returns_as_it_does_natively";
    const MODE: &str = "RELAX_PROGSIG_ABRT_MODE";
    if std::env::var(CHILD).as_deref() != Ok(TEST) {
        let run = |mode: &str| {
            let output = child_with(TEST, "", &[(MODE, mode)]);
            let err = String::from_utf8_lossy(&output.stderr).into_owned();
            let lines: Vec<String> = err.lines().filter(|l| l.starts_with("abrt fixture:")).map(str::to_string).collect();
            (output.status.code(), output.status.signal(), lines, err)
        };
        for (case, reports) in [("raise", 1), ("abort", 1), ("exit", 0)] {
            let (code, signal, lines, err) = run(&format!("native,{case}"));
            // (the baseline ran the program's handler: else this binary's own run took the signal)
            assert!(lines.iter().any(|l| l.starts_with("abrt fixture: the handler")), "{case}: natively the handler runs:\n{err}");
            let (jit_code, jit_signal, jit_lines, jit_err) = run(&format!("jit,{case}"));
            assert_eq!((jit_code, jit_signal), (code, signal), "{case}: under the JIT the process ends as natively:\n{jit_err}\nnatively:\n{err}");
            assert_eq!(jit_lines, lines, "{case}: under the JIT the program says what it does natively:\n{jit_err}\nnatively:\n{err}");
            let report = |e: &str| e.matches("relax: SIGABRT on thread").count();
            assert_eq!(report(&jit_err), report(&err) + reports, "{case}: {reports} report(s) of the unit's:\n{jit_err}");
            if case == "exit" {
                assert!(jit_err.lines().any(|l| l == "abrt recorder: 1"), "{case}: the abort's site in the flight recorder before the handler:\n{jit_err}");
            }
        }
        return;
    }
    assert_eq!(unsafe { setrlimit(4, &[0, 0]) }, 0, "the SIGABRT child must disable core dumps");
    let mode = std::env::var(MODE).unwrap();
    let (how, case) = mode.split_once(',').unwrap();
    let mut out = [match case { "abort" => 2, "exit" => 3, _ => 1 }, 0, 0, 0, 0, 0, 0, 0];
    if how == "native" {
        abrt_native::abrt_case(&mut out);
    } else {
        // what the ending handler reads first: the unit's flight recorder
        extern "C" fn recorder() {
            eprintln!("abrt recorder: {}", crate::inspect::flightrec::count_events("signal.abort"));
        }
        crate::inspect::flightrec::init();
        out[2] = recorder as *const () as usize as u64;
        let (mut u, krate) = setup_with(ABRT_FIXTURE, "abrt_fixture");
        let _program = crate::os::program_thread();
        crate::os::fault::reassert();
        call(&mut u, krate, "abrt_case", out.as_mut_ptr() as u64);
    }
    eprintln!("abrt fixture: runs {}", out[1]);
    std::process::exit(0);
}

#[test]
fn a_handler_installed_around_the_trampoline_ends_a_compile_loudly() {
    const TEST: &str = "a_handler_installed_around_the_trampoline_ends_a_compile_loudly";
    if std::env::var(CHILD).as_deref() != Ok(TEST) {
        let output = child(TEST, &format!("target_in_compile,{SIGUSR2},raw-self"));
        let err = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(101), "handler re-entry must exit with code 101: {:?}\n{err}", output.status);
        assert!(err.contains("internal compiler error: a lazy call (slot"), "handler re-entry must identify the lazy compile:\n{err}");
        return;
    }
    let (mut u, krate) = setup();
    let _program = crate::os::program_thread();
    // out[0] is the handler's pipe: none, as the process must end before the handler runs
    let mut out = [u64::MAX, 0, 0, 0, 0, 0, 0, 0];
    let arg = out.as_mut_ptr() as u64;
    call(&mut u, krate, "install", arg);
    call(&mut u, krate, "target_in_compile", arg);
}

#[cfg(target_os = "linux")]
#[test]
fn queued_realtime_signals_in_a_compile_keep_their_count_and_values() {
    const TEST: &str = "queued_realtime_signals_in_a_compile_keep_their_count_and_values";
    if std::env::var(CHILD).as_deref() != Ok(TEST) {
        let output = child(TEST, "target_in_compile,50,rt3-self");
        assert!(output.status.success(), "the realtime signal child must succeed: {:?}\n{}", output.status, String::from_utf8_lossy(&output.stderr));
        return;
    }
    let (mut u, krate) = setup();
    let _program = crate::os::program_thread();
    let mut out = [0u64; 8];
    let arg = out.as_mut_ptr() as u64;
    call(&mut u, krate, "install_rt", arg);
    call(&mut u, krate, "target_in_compile", arg);
    call(&mut u, krate, "values", arg);
    assert_eq!(out[3], 3, "all three queued realtime signals must be delivered");
    assert_eq!(&out[..3], &[1, 2, 3], "queued realtime signals must preserve their values and order");
    assert_eq!(crate::os::progsig::counts(50)[0], 3, "all three realtime handler runs must be counted");
}

/// A program's SIGSEGV handler runs on a 64 KB signal stack of the program's, and its first call
/// compiles with 256 KB of stack (the compile hook): the compile moves to a stack segment and
/// the handler fixes the page; the write lands. Before the stack check knew its stack, the
/// compile ran on the signal stack and overflowed it with SIGSEGV blocked (the kernel's kill, no
/// report). And right after the program's `sigaction`, the host's handler stays in the kernel
/// with the program's behind it: the host's policy sees every fault without waiting for the run
/// monitor to put it back.
#[test]
fn a_fault_handler_that_compiles_on_a_small_signal_stack_runs() {
    const TEST: &str = "a_fault_handler_that_compiles_on_a_small_signal_stack_runs";
    if std::env::var(CHILD).as_deref() != Ok(TEST) {
        let output = child_with(TEST, "", &[("RELAX_TEST_COMPILE_STACK", "first_in_segv,256")]);
        assert!(output.status.success(), "the small-stack child must succeed: {:?}\n{}", output.status, String::from_utf8_lossy(&output.stderr));
        return;
    }
    let (mut u, krate) = setup();
    let _program = crate::os::program_thread();
    let mut out = [0u64; 8];
    let arg = out.as_mut_ptr() as u64;
    call(&mut u, krate, "install_segv", arg);
    assert_eq!(crate::os::progsig::kernel_handler(SIGSEGV), crate::os::fault::host_handler(), "the host's handler stays in the kernel");
    assert_eq!(crate::os::fault::chained_handler(SIGSEGV), crate::os::progsig::trampoline_addr(), "the program's handler is chained behind it");
    call(&mut u, krate, "segv_on_small_stack", arg);
    assert_eq!(out[0], 1, "the program's handler ran once");
    assert_eq!(out[1], 7, "the write landed once the handler gave the page back");
}

/// A fault inside a compile is the compiler's: no program handler runs there, and the process
/// ends by the signal with a report naming the signal, the address, the thread and the compile.
/// Which fault runs the program's handler: on its thread outside a compile; on a thread with no
/// program state, outside the host's own code.
#[test]
fn a_fault_inside_a_compile_ends_the_process_with_a_report() {
    const TEST: &str = "a_fault_inside_a_compile_ends_the_process_with_a_report";
    if std::env::var(CHILD).as_deref() != Ok(TEST) {
        use crate::os::progsig::runs_handler;
        assert!(runs_handler(true, 0, || true), "a program thread outside a compile runs it");
        assert!(!runs_handler(true, 1, || false) && !runs_handler(true, 2, || false), "inside a compile it never does");
        assert!(runs_handler(false, 0, || false), "a thread with no state runs it for a fault outside the host's code");
        assert!(!runs_handler(false, 0, || true), "a fault in the host's code with no state is the host's crash");
        crate::os::fault::note_host_text();
        let host = crate::os::fault::host_code as *const () as u64;
        let libc = libc_write as *const () as u64;
        assert!(crate::os::fault::host_code(host), "the host's code is its own");
        // (under the JIT the host's code is JIT code in no image: every pc counts as the host's)
        assert_eq!(crate::os::fault::host_code(libc), cfg!(relax), "the C library's code is not the host's");
        let output = child(TEST, "target_in_compile,11,fault");
        let err = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.signal(), Some(SIGSEGV), "the fault ends the process by its signal: {:?}\n{err}", output.status);
        let line = err.lines().find(|l| l.starts_with("relax: fatal signal 11 on thread ")).unwrap_or_else(|| panic!("no fatal signal report:\n{err}"));
        let at = err.lines().find_map(|l| l.strip_prefix("relax test: a fault at ")).and_then(|l| l.split(' ').next()).unwrap_or_else(|| panic!("no fault address from the hook:\n{err}"));
        assert!(line.contains(&format!(" addr {at}")) && line.contains("inside a compile"), "the report names the address and why: {line}");
        assert!(err.contains("in the compile of slot ") && err.contains("target_in_compile"), "the report names the compile:\n{err}");
        return;
    }
    let (mut u, krate) = setup();
    let _program = crate::os::program_thread();
    let mut out = [0u64; 8];
    let arg = out.as_mut_ptr() as u64;
    call(&mut u, krate, "install_segv", arg);
    call(&mut u, krate, "target_in_compile", arg);
    panic!("the fault inside the compile must end the process");
}

const SIGSEGV: i32 = 11;

extern "C" {
    #[link_name = "write"]
    fn libc_write(fd: i32, buf: *const core::ffi::c_void, n: usize) -> isize;
}

/// A program's SIGUSR1 is the program's: its handler runs, also after the run monitor put the
/// host's handlers back on top. The host's watchdog has a signal of its own, and only its kicks
/// are the host's: the program's own send of that signal runs the program's handler on it.
#[test]
fn a_program_sigusr1_and_its_send_of_the_watchdog_signal_run_its_handlers() {
    const TEST: &str = "a_program_sigusr1_and_its_send_of_the_watchdog_signal_run_its_handlers";
    if std::env::var(CHILD).as_deref() != Ok(TEST) {
        let output = child(TEST, "");
        assert!(output.status.success(), "the SIGUSR1 child must succeed: {:?}\n{}", output.status, String::from_utf8_lossy(&output.stderr));
        return;
    }
    let (mut u, krate) = setup();
    let _program = crate::os::program_thread();
    let watch = crate::os::fault::watchdog_signal();
    assert_ne!(watch, SIGUSR1, "the watchdog's signal is not SIGUSR1");
    let mut out = [0u64; 8];
    out[0] = watch as u64;
    let arg = out.as_mut_ptr() as u64;
    call(&mut u, krate, "install_usr1", arg);
    assert_eq!(crate::os::progsig::kernel_handler(watch), crate::os::fault::host_handler(), "the host's handler stays on the watchdog's signal");
    assert_eq!(crate::os::fault::chained_handler(watch), crate::os::progsig::trampoline_addr(), "the program's handler is chained behind it");
    // the run monitor's tick
    crate::os::fault::reassert();
    out[1] = SIGUSR1 as u64;
    call(&mut u, krate, "raise_and_count", arg);
    assert_eq!((out[2], out[3]), (1, 0), "the program's SIGUSR1 runs its handler once");
    crate::os::thread::kick(crate::os::thread::current());
    out[1] = 0;
    call(&mut u, krate, "raise_and_count", arg);
    assert_eq!((out[2], out[3]), (1, 0), "a kick runs no program handler");
    out[1] = watch as u64;
    call(&mut u, krate, "raise_and_count", arg);
    assert_eq!((out[2], out[3]), (1, 1), "the program's own send of the watchdog's signal runs its handler once");
}

/// `send_at_its_end`'s send: 1 + what pthread_kill returned (0: not sent).
static SENT_AT_ITS_END: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// On a thread of the program: once its program code has returned (a thread-local's destructor),
/// the watchdog's signal is sent at it, as the program's own kick of it would come then.
extern "C" fn send_at_its_end() {
    struct AtItsEnd;
    impl Drop for AtItsEnd {
        fn drop(&mut self) {
            let r = unsafe { pthread_kill(pthread_self(), crate::os::fault::watchdog_signal()) };
            SENT_AT_ITS_END.store(1 + r, std::sync::atomic::Ordering::SeqCst);
        }
    }
    thread_local! {
        static AT_ITS_END: AtItsEnd = const { AtItsEnd };
    }
    AT_ITS_END.with(|_| {});
}

/// The program's own send of the watchdog's signal at one of its threads (a kick of a JIT the
/// program runs) that comes as the thread ends stays on that thread: it ends with the thread. Sent
/// on to the process, it ran the program's handler on a thread it was not aimed at, where that
/// handler found no kick of its own and ended the process.
#[test]
fn a_kick_aimed_at_a_thread_at_its_end_stays_on_that_thread() {
    const TEST: &str = "a_kick_aimed_at_a_thread_at_its_end_stays_on_that_thread";
    if std::env::var(CHILD).as_deref() != Ok(TEST) {
        let output = child(TEST, "");
        assert!(output.status.success(), "the thread-end child must succeed: {:?}\n{}", output.status, String::from_utf8_lossy(&output.stderr));
        return;
    }
    let (mut u, krate) = setup();
    let _program = crate::os::program_thread();
    let watch = crate::os::fault::watchdog_signal();
    let mut out = [0u64; 8];
    out[0] = watch as u64;
    let arg = out.as_mut_ptr() as u64;
    call(&mut u, krate, "install_usr1", arg);
    out[4] = send_at_its_end as *const () as usize as u64;
    call(&mut u, krate, "a_thread_calls_and_ends", arg);
    assert_eq!(SENT_AT_ITS_END.load(std::sync::atomic::Ordering::SeqCst), 1, "the thread must send the signal at itself at its end");
    assert_eq!(crate::os::progsig::counts(watch)[2], 0, "a signal aimed at a thread at its end must not go on to the process");
    out[1] = 0;
    call(&mut u, krate, "raise_and_count", arg);
    assert_eq!(out[3], 0, "the program's handler must not run for it on another thread");
}

#[cfg(target_os = "linux")]
const SIGUSR1: i32 = 10;
#[cfg(target_vendor = "apple")]
const SIGUSR1: i32 = 30;
