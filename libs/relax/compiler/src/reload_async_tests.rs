//! Live edits of async code: after a patch every new call runs the new code -- an edited async
//! fn awaited from an async block or an async fn of a file the edit does not touch, an edited
//! async block, and an edit that also retypes a field of a struct the coroutine holds.
//! (`live_app` and `in_child` are the live-session harness of reload_items_tests too.)
use std::sync::Arc;

/// Runs each case of test `test` (its path in this crate) in a child process (process-global
/// runtime state); in the child, `body(case)`.
pub(crate) fn in_child(test: &str, cases: &[&str], body: impl FnOnce(&str)) {
    const CHILD: &str = "MAKEPAD_RUN_RELOAD_LIVE_CASE";
    let Ok(case) = std::env::var(CHILD) else {
        for case in cases {
            let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", test, "--nocapture", "--test-threads=1"])
                .env(CHILD, case).env("MAKEPAD_RUN_GANDALF_ALL", "0"));
            assert!(output.status.success(), "{case}: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
        return;
    };
    body(&case)
}

/// The unit of the running `live_app` (a test that looks into it).
pub(crate) static LIVE_UNIT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A live session (M10 layouts, an app-scope crate) over crate `app` of `files` (paths under
/// src/ with their text; lib.rs first), then `run(call, edit)`: `call(name)` runs the crate's
/// fn `name(out: &mut u64)` (one of `names`, found at load) and returns `*out`; `edit(file,
/// subs)` applies text substitutions to that file live and returns the outcome.
pub(crate) fn live_app(tag: &str, files: &[(&str, &str)], names: &[&str], run: impl FnOnce(&dyn Fn(&str) -> u64, &mut dyn FnMut(&str, &[(&str, &str)]) -> crate::reload::Outcome)) {
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    crate::layout::LIVE_SESSION.store(true, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-reload-{}-{}", tag, std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let path = |f: &str| dir.join("app/src").join(f).to_string_lossy().into_owned();
    for (f, text) in files {
        std::fs::write(path(f), text).unwrap();
    }
    let lib = path("lib.rs");
    let mut session = crate::Session::default();
    session.options.live = true;
    let session = Arc::new(session);
    let (prog, core, app_crate) = crate::load_with_std(&session, &lib, "live_app", false).unwrap();
    crate::borrowck::event::set_app_scope(vec![lib.clone()]);
    let entries_of: Vec<_> = names.iter().map(|name| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == *name).then_some(id)
    }).unwrap_or_else(|| panic!("no fn `{name}`"))).collect();
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    LIVE_UNIT.store(up as usize, std::sync::atomic::Ordering::Relaxed);
    let entries: Vec<u64> = crate::owner::call(crate::owner::Class::Program, || {
        entries_of.iter().map(|&d| { let id = u.fn_id(crate::jit::FnKey::Inst(d, vec![])); u.entry(id) }).collect()
    });
    let call = |name: &str| -> u64 {
        let entry = entries[names.iter().position(|n| *n == name).unwrap()];
        let mut x = 0u64;
        unsafe { crate::jit::call_jit(up, entry, &mut x as *mut u64 as u64) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        x
    };
    let mut texts: Vec<(String, String)> = files.iter().map(|(f, t)| (f.to_string(), t.to_string())).collect();
    let mut edit = |file: &str, subs: &[(&str, &str)]| {
        let text = &mut texts.iter_mut().find(|(f, _)| f == file).unwrap().1;
        for (from, to) in subs {
            assert!(text.contains(from), "{file}: {from}");
            *text = text.replace(from, to);
        }
        let p = path(file);
        let index = unsafe { (*up).prog.current_source_files(|q| q == p)[&p] };
        unsafe { crate::reload::apply(up, vec![crate::reload::Edit { file: index, path: p, src: text.clone().into_bytes() }]) }
    };
    run(&call, &mut edit);
    // Like settle_tests: process exit owns the runtime worker/executable-memory teardown.
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

const LIB: &str = r#"mod driver;
pub use driver::*;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

pub struct Trace { pub from: Option<u64>, pub n: u64 }

pub struct Yield(pub bool);
impl Future for Yield {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.0 { Poll::Ready(()) } else { self.0 = true; Poll::Pending }
    }
}

pub async fn run(out: &mut u64) {
    let t = Trace { from: Some(1), n: 0 };
    Yield(false).await;
    *out = t.from.unwrap_or(0) + t.n + 100;
}

pub fn drive_block(out: &mut u64) {
    driver::block_on(Box::pin(async move {
        Yield(false).await;
        *out = 7;
    }));
}
"#;

const DRIVER: &str = r#"use std::future::Future;
use std::pin::{pin, Pin};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker { noop_raw() }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    RawWaker::new(std::ptr::null(), &VT)
}

pub fn block_on<F: Future>(f: F) -> F::Output {
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut f = pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

pub fn drive(out: &mut u64) {
    let job: Pin<Box<dyn Future<Output = ()> + '_>> = Box::pin(async move { crate::run(out).await; });
    block_on(job);
}

async fn outer(out: &mut u64) {
    crate::run(out).await;
    *out += 1000;
}

pub fn drive_outer(out: &mut u64) {
    block_on(outer(out));
}

static HELD: std::sync::Mutex<Option<Pin<Box<dyn Future<Output = u64> + Send>>>> = std::sync::Mutex::new(None);

async fn held() -> u64 {
    let mut v = 0u64;
    crate::run(&mut v).await;
    v + 1000
}

/// A future suspended inside `run`, kept for `resume`.
pub fn hold(out: &mut u64) {
    let mut f: Pin<Box<dyn Future<Output = u64> + Send>> = Box::pin(held());
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    *out = if f.as_mut().poll(&mut Context::from_waker(&waker)).is_pending() { 1 } else { 0 };
    *HELD.lock().unwrap() = Some(f);
}

pub fn resume(out: &mut u64) {
    let f = HELD.lock().unwrap().take().unwrap();
    *out = block_on(f);
}
"#;

/// (the "restart" case's) an async fn of a trait impl awaiting `run`
const JOB: &str = r#"
pub trait Job {
    async fn go(&self, out: &mut u64);
}
pub struct J;
impl Job for J {
    async fn go(&self, out: &mut u64) {
        crate::run(out).await;
    }
}

pub fn drive_job(out: &mut u64) {
    block_on(J.go(out));
}
"#;

/// "patch": the edits that apply live; "restart": an edit whose dependents cannot take it live.
#[test]
fn edited_async_code_runs_on_the_next_call() {
    in_child("reload_async_tests::edited_async_code_runs_on_the_next_call", &["patch", "restart"], |case| {
        let driver = if case == "restart" { format!("{DRIVER}{JOB}") } else { DRIVER.to_string() };
        let names: &[&str] = if case == "restart" { &["drive", "drive_job"] } else { &["drive", "drive_outer", "drive_block", "hold", "resume"] };
        live_app(&format!("async-{case}"), &[("lib.rs", LIB), ("driver.rs", &driver)], names, |call, edit| {
            assert_eq!(call("drive"), 101);
            if case == "restart" {
                // `J::go` (an async fn of a trait impl) awaits `run`: its future holds `run`'s, so
                // it is replaced with it, and a trait impl's fn is not replaced live yet -- the
                // edit restarts (before, the impl's coroutine kept calling the replaced `run` and
                // the patch "applied")
                assert_eq!(call("drive_job"), 101);
                let outcome = edit("lib.rs", &[("t.n + 100;", "t.n + 200;")]);
                assert_eq!(outcome.rung, crate::reload::Rung::Restart, "{:?}", outcome.lines);
                assert_eq!((call("drive"), call("drive_job")), (101, 101), "nothing of the edit installed");
                return;
            }
            let mut patch = |subs: &[(&str, &str)]| {
                let outcome = edit("lib.rs", subs);
                assert_eq!(outcome.rung, crate::reload::Rung::Patch, "{subs:?}: {:?}", outcome.lines);
            };
            assert_eq!((call("drive_outer"), call("drive_block"), call("hold")), (1101, 7, 1), "(hold: a future suspended inside run)");
            // the edited async fn's new body runs when it is awaited from an async block (behind a
            // boxed dyn future) and from an async fn, both in a file the edit does not touch; the
            // suspended future finishes on the code it started in (its code before the await changed)
            patch(&[("Trace { from: Some(1), n: 0 };", "Trace { from: Some(1), n: 4 };"), ("t.n + 100;", "t.n + 200;")]);
            assert_eq!(call("drive"), 205, "an async block of an untouched file awaits the edited async fn");
            assert_eq!(call("drive_outer"), 1205, "an async fn of an untouched file awaits the edited async fn");
            assert_eq!(call("resume"), 1101, "the suspended future finishes on its old code, all of it");
            // an async block edited in its fn
            patch(&[("*out = 7;", "*out = 8;")]);
            assert_eq!(call("drive_block"), 8, "the edited async block runs");
            // a field of a struct the coroutine holds across its await retyped, with the async fn's body
            patch(&[
                ("pub from: Option<u64>,", "pub from: Option<(u64, u64)>,"),
                ("Trace { from: Some(1), n: 4 };", "Trace { from: Some((1, 2)), n: 0 };"),
                ("t.from.unwrap_or(0) + t.n + 200;", "t.from.map_or(0, |(a, b)| a + b) + t.n + 300;"),
            ]);
            assert_eq!(call("drive"), 303, "the async fn's body over the retyped field");
            assert_eq!(call("drive_outer"), 1303, "the same through the async fn awaiting it");
        });
    });
}

const OUTPUT_LIB: &str = r#"use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

pub struct Yield(pub bool);
impl Future for Yield {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.0 { Poll::Ready(()) } else { self.0 = true; Poll::Pending }
    }
}

pub async fn val() -> u64 {
    Yield(false).await;
    5
}

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker { noop_raw() }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    RawWaker::new(std::ptr::null(), &VT)
}

static HELD: std::sync::Mutex<Option<Pin<Box<dyn Future<Output = u64> + Send>>>> = std::sync::Mutex::new(None);

pub fn hold(out: &mut u64) {
    let mut f: Pin<Box<dyn Future<Output = u64> + Send>> = Box::pin(async { val().await as u64 });
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    *out = f.as_mut().poll(&mut Context::from_waker(&waker)).is_pending() as u64;
    *HELD.lock().unwrap() = Some(f);
}

pub fn resume(out: &mut u64) {
    let mut f = HELD.lock().unwrap().take().unwrap();
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut Context::from_waker(&waker)) {
            *out = v;
            return;
        }
    }
}
"#;

/// A suspended future is resumed in new code only when its output and captures keep their types:
/// the same frame size and the same code up to the last await are not enough (an old caller
/// still reads the old output).
#[test]
fn suspended_future_keeps_its_code_when_its_output_type_changes() {
    in_child("reload_async_tests::suspended_future_keeps_its_code_when_its_output_type_changes", &["output"], |_| {
        live_app("async-output", &[("lib.rs", OUTPUT_LIB)], &["hold", "resume"], |call, edit| {
            assert_eq!(call("hold"), 1, "a future suspended inside val");
            let outcome = edit("lib.rs", &[("pub async fn val() -> u64 {", "pub async fn val() -> u32 {"), ("    5\n}", "    6\n}")]);
            assert!(!outcome.lines.iter().any(|l| l.contains("resume in the new code")), "{:?}", outcome.lines);
            if outcome.rung == crate::reload::Rung::Patch {
                assert_eq!(call("resume"), 5, "the suspended future finishes on its old code: {:?}", outcome.lines);
            }
        });
    });
}
