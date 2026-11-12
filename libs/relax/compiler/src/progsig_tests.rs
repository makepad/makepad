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

const CHILD: &str = "MAKEPAD_RUN_PROGSIG_CHILD";
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
}
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
"#;

fn child(test: &str, hook: &str) -> std::process::Output {
    let _slot = crate::test_child::slot();
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &format!("progsig_tests::{test}"), "--nocapture", "--test-threads=1"])
        .env(CHILD, test)
        .env("MAKEPAD_RUN_TEST_SIGNAL_IN_COMPILE", hook)
        .output()
        .expect("the signal test child must run")
}

fn setup() -> (Box<Unit>, u32) {
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-progsig-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lib.rs");
    std::fs::write(&path, FIXTURE).unwrap();
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), "progsig_fixture", false).unwrap();
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
        assert!(err.contains("fixture: SIGABRT report") || (cfg!(relax) && err.contains("SIGABRT on thread")), "outside SIGABRT must report before exit:\n{err}");
        return;
    }
    assert_eq!(unsafe { setrlimit(4, &[0, 0]) }, 0, "the SIGABRT child must disable core dumps");
    let (mut u, krate) = setup();
    let _program = crate::os::program_thread();
    let mut out = [0u64; 8];
    let arg = out.as_mut_ptr() as u64;
    call(&mut u, krate, "install_abrt", arg);
    call(&mut u, krate, "target_in_compile", arg);
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
