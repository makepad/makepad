//! A child owns the process-wide exception handlers and the compile hook. A native handler
//! resumes the test exception whenever the program's handler must skip it.

use crate::jit::{FnKey, Unit};
use crate::os::win32;
use crate::owner::{self, Class};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

const CHILD: &str = "RELAX_PROGEXC_CHILD";
const CODE: u32 = 0xE0D0_0001;
static NATIVE_RESUMES: AtomicUsize = AtomicUsize::new(0);

extern "system" fn native_resume(ep: *mut win32::ExceptionPointers) -> i32 {
    if unsafe { (*(*ep).record).code } == CODE {
        NATIVE_RESUMES.fetch_add(1, SeqCst);
        win32::EXCEPTION_CONTINUE_EXECUTION
    } else {
        win32::EXCEPTION_CONTINUE_SEARCH
    }
}

fn raise() {
    unsafe { win32::RaiseException(CODE, 0, 0, std::ptr::null()) };
}

const FIXTURE: &str = r#"
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

const CODE: u32 = 0xE0D0_0001;
static COUNT: AtomicU64 = AtomicU64::new(0);

#[link(name = "kernel32")]
extern "system" {
    fn AddVectoredExceptionHandler(first: u32, h: extern "system" fn(*const *const u32) -> i32) -> *mut u8;
    pub fn RemoveVectoredExceptionHandler(handle: *mut u8) -> u32;
    fn RaiseException(code: u32, flags: u32, n_args: u32, args: *const usize);
}

extern "system" fn handler(ep: *const *const u32) -> i32 {
    // EXCEPTION_POINTERS begins with the record pointer. Its first DWORD is the code.
    if unsafe { **ep } == CODE {
        COUNT.fetch_add(1, SeqCst);
        -1
    } else {
        0
    }
}

pub fn install(arg: u64) {
    unsafe { *(arg as *mut u64) = AddVectoredExceptionHandler(1, handler) as u64 };
}

pub fn raise(_arg: u64) {
    unsafe { RaiseException(CODE, 0, 0, std::ptr::null()) };
}

#[inline(never)]
fn target_in_compile_body(arg: u64) {
    report(arg);
}

pub fn target_in_compile(arg: u64) {
    target_in_compile_body(arg);
}

pub fn report(arg: u64) {
    unsafe { *(arg as *mut u64) = COUNT.load(SeqCst) };
}
"#;

fn child_with(test: &str, hook: &str) -> std::process::Output {
    let _slot = crate::test_child::slot();
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &format!("progexc_tests::{test}"), "--nocapture", "--test-threads=1"])
        .env(CHILD, test)
        .env("RELAX_TEST_SIGNAL_IN_COMPILE", hook)
        .output()
        .expect("the exception test child must run")
}

fn setup_with(src: &str, name: &str) -> (Box<Unit>, u32) {
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-progexc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lib.rs");
    std::fs::write(&path, src).unwrap();
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), name, false).unwrap();
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    (Unit::new(prog, tcx, core), krate)
}

fn call(u: &mut Unit, krate: u32, name: &str, arg: u64) {
    let up = &mut *u as *mut Unit;
    let def = (0..u.prog.defs.len()).map(|i| crate::program::DefId(i as u32))
        .find(|&d| u.prog.def(d).krate == krate && u.prog.name(d) == name)
        .unwrap_or_else(|| panic!("the fixture must define {name}"));
    let entry = owner::call(Class::Program, || { let f = u.fn_id(FnKey::Inst(def, vec![])); u.entry(f) });
    unsafe { crate::jit::call_jit(up, entry, arg) }.unwrap_or_else(|p| panic!("fixture {name} must return normally: {}", p.message));
}

#[test]
fn program_exception_handlers_run_only_on_program_threads_outside_compiles() {
    const TEST: &str = "program_exception_handlers_run_only_on_program_threads_outside_compiles";
    if std::env::var(CHILD).as_deref() != Ok(TEST) {
        let output = child_with(TEST, "target_in_compile_body,0xE0D00001");
        assert!(output.status.success(), "the exception test child must succeed: {:?}\n{}", output.status, String::from_utf8_lossy(&output.stderr));
        return;
    }
    let (mut u, krate) = setup_with(FIXTURE, "progexc_fixture");
    let native = unsafe { win32::AddVectoredExceptionHandler(0, native_resume) };
    assert!(!native.is_null(), "the native resume handler must be registered");
    let _program = crate::os::program_thread();
    let mut out = [0u64];
    let arg = out.as_mut_ptr() as u64;
    call(&mut u, krate, "install", arg);
    assert_ne!(out[0], 0, "the program must receive a registration handle");
    call(&mut u, krate, "raise", arg);
    call(&mut u, krate, "report", arg);
    assert_eq!(out[0], 1, "a program raise runs the program's handler");
    assert_eq!(NATIVE_RESUMES.load(SeqCst), 0, "the program resumes its own raise before the native handler");

    call(&mut u, krate, "target_in_compile", arg);
    assert_eq!(out[0], 1, "a raise inside a compile never runs the program's handler");
    assert_eq!(NATIVE_RESUMES.load(SeqCst), 1, "the native handler resumes the raise inside the compile");

    std::thread::spawn(raise).join().expect("the host thread must resume");
    call(&mut u, krate, "report", arg);
    assert_eq!(out[0], 1, "a host thread never runs the program's handler");
    assert_eq!(NATIVE_RESUMES.load(SeqCst), 2, "the native handler resumes the host thread");

    crate::os::progexc::reset_statics();
    raise();
    call(&mut u, krate, "report", arg);
    assert_eq!(out[0], 1, "an ended run's handler never runs");
    assert_eq!(NATIVE_RESUMES.load(SeqCst), 3, "the native handler resumes the raise after reset");
    assert_ne!(unsafe { win32::RemoveVectoredExceptionHandler(native) }, 0, "the native resume handler must be removed");
}
