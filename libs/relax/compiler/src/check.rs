//! `makepad check`: the app check, like `cargo check`: every function of the package's workspace
//! crates (every loaded crate whose sources are in the workspace, std excepted) is type- and
//! borrow-checked (generic ones polymorphically, as rustc checks generic bodies once); nothing
//! is compiled or run. Errors print as one line each; the exit code is 0 when there are none.
//!
//! `makepad selftest`: a test of Relax itself on a package -- the same checks, then every
//! non-generic function is compiled to machine code by the run's own per-function compile
//! (`jit::compile_slot`), and so is every instance, closure and glue those reach (rustc's
//! monomorphization collector for `build-pass`), on `MAKEPAD_RUN_JOBS` threads. Every failure prints
//! as one line (`file:line:col: error|internal: ..`); `internal` marks a makepad run bug (a compiler
//! panic, an error without a user-facing code, or an error inside std or a dependency rustc
//! builds). `--root-only` and the watch session's deep check (`collect_deep`) cover the root
//! crate (and what it reaches) only.

macro_rules! eprintln {
    ($($arg:tt)*) => { if !crate::diag::enabled() { stderr_line!($($arg)*); } };
}

use relax_types::next_solver::{Goal, ParamEnvId};
use crate::jit::{self, FnKey, Unit};
use crate::program::DefKind;

/// MAKEPAD_RUN_CHECK_TIMES: one line per serial step of a check that took 50 ms or more.
struct StepTimes { on: bool, t: std::time::Instant, label: &'static str }
impl StepTimes {
    fn new(label: &'static str) -> Self {
        StepTimes { on: std::env::var_os("MAKEPAD_RUN_CHECK_TIMES").is_some(), t: std::time::Instant::now(), label }
    }
    fn mark(&mut self, step: &str) {
        let dt = self.t.elapsed();
        if self.on && dt.as_millis() >= 50 { stderr_line!("makepad check: {} {} {:.2} s", self.label, step, dt.as_secs_f64()); }
        self.t = std::time::Instant::now();
    }
}

fn check_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_CHECK_TRACE").is_some())
}

fn borrowck_unchecked() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_BORROWCK_UNCHECKED").is_some())
}

/// Errors of earlier phases (`file:line:col: ..` lines) that belong to the root crate's files.
pub fn own_errors(prog: &crate::program::Program, root: u32, errs: &[String], phase: &'static str) -> Vec<String> {
    let mut paths: Vec<&str> = Vec::new();
    for (i, p) in prog.file_paths.iter().enumerate() {
        if prog.file_crate.get(i).copied() == Some(root) {
            paths.push(p.as_str());
        }
    }
    errs.iter().filter(|e| paths.iter().any(|p| e.starts_with(p))).map(|e| {
        if crate::diag::enabled() { crate::diag::record(e.clone(),crate::diag::from_legacy(e,phase)) }
        else { e.clone() }
    }).collect()
}

/// The errors of `errs` (`file:line:col: ..` lines) in no file of the `checked` crates, counted by
/// the crate whose file they name, in the order those crates first come (`?`: a line that names
/// no file of the program).
pub fn foreign_errors<'e>(prog: &crate::program::Program, checked: &[u32], errs: impl Iterator<Item = &'e String>) -> Vec<(String, usize)> {
    let files: Vec<(&str, u32)> = prog.file_paths.iter().enumerate().filter_map(|(i, p)| prog.file_crate.get(i).map(|&c| (p.as_str(), c))).collect();
    let mut out: Vec<(String, usize)> = Vec::new();
    for e in errs {
        if files.iter().any(|(p, c)| checked.contains(c) && e.starts_with(p)) { continue; }
        let name = files.iter().find(|(p, _)| e.starts_with(p)).map_or("?", |&(_, c)| prog.syms.str(prog.crates[c as usize].name));
        match out.iter_mut().find(|(n, _)| n == name) {
            Some(x) => x.1 += 1,
            None => out.push((name.to_string(), 1)),
        }
    }
    out
}

/// The crates a one-shot `makepad check` covers: the root and every loaded crate whose root file
/// is in the workspace, except std (MAKEPAD_RUN_STD may live inside it) and proc-macro crates (their
/// code runs in the macro host, never in the program).
pub fn workspace_crates(prog: &crate::program::Program, root: u32) -> Vec<u32> {
    use crate::vfs::canonical as canon;
    let ws = format!("{}/", canon(&crate::run::workspace_dir()).trim_end_matches('/'));
    let std_dir = format!("{}/", canon(&crate::std_dir()).trim_end_matches('/'));
    let mut out = Vec::new();
    for (k, c) in prog.crates.iter().enumerate() {
        let path = canon(c.root_path());
        let inside = path.starts_with(&ws) && !path.starts_with(&std_dir);
        if k as u32 == root || (inside && !c.proc_macro && !matches!(prog.syms.str(c.name), "core" | "alloc" | "std" | "proc_macro")) {
            out.push(k as u32);
        }
    }
    out
}

/// The crates `makepad check` checks: the application's own -- the root and the crates of the
/// app's project tree that are not the framework (the one app-crate rule the check server and
/// Gandalf use, reload.rs gandalf_app_roots), as `cargo check` re-checks only what changed.
pub fn app_crates(prog: &crate::program::Program, root: u32) -> Vec<u32> {
    (0..prog.crates.len() as u32).filter(|&k| k == root || crate::borrowck::event::app_crate(prog, k)).collect()
}

/// One selection for declaration validation, diagnostics and body checks.
pub fn checked_crates(prog: &crate::program::Program, root: u32, root_only: bool, workspace: bool, also: &[u32]) -> Vec<u32> {
    let mut crates = if root_only { vec![root] } else if workspace { workspace_crates(prog, root) } else { app_crates(prog, root) };
    for &k in also {
        if !crates.contains(&k) { crates.push(k); }
    }
    crates
}

/// The body owners of `crates` (fns with bodies, consts and statics with initializers), in
/// definition order.
pub(crate) fn body_owners(prog: &crate::program::Program, crates: &[u32]) -> Vec<crate::program::DefId> {
    let mut own = vec![false; prog.crates.len()];
    for &k in crates { own[k as usize] = true; }
    (0..prog.defs.len()).filter(|&i| own[prog.defs[i].krate as usize]).map(|i| crate::program::DefId(i as u32))
        .filter_map(|d| crate::borrowck::body::CheckedBody::item(prog, d)).map(|b| b.owner).collect()
}

/// The files of each crate the boot's file selection skipped (paths, sorted), as the boot left
/// them -- with a crate's fingerprint, which covers the files the boot loaded, every file of it
/// (cache.rs crate_check_key). Taken before main, before the post-boot loader starts (run.rs).
pub(crate) static BOOT_SKIPPED: std::sync::OnceLock<Vec<Vec<String>>> = std::sync::OnceLock::new();

pub(crate) fn boot_skipped(prog: &crate::program::Program) -> Vec<Vec<String>> {
    let mut out = vec![Vec::new(); prog.crates.len()];
    for (m, f) in &prog.skipped_files {
        out[prog.mods[*m as usize].krate as usize].push(f.0.clone());
    }
    for v in &mut out { v.sort_unstable(); }
    out
}

/// The text of crate `c`'s files the boot skipped (BOOT_SKIPPED: path and content; 0: none).
/// The selection read them already: they are in the OS's file cache.
pub(crate) fn skipped_text_hash(c: u32) -> u128 {
    let Some(paths) = BOOT_SKIPPED.get().and_then(|v| v.get(c as usize)).filter(|v| !v.is_empty()) else { return 0 };
    let mut h = crate::sip::Sip128::new();
    for p in paths {
        h.str(p);
        // (an unreadable file: its absence is the input, its load reports it)
        h.u128(crate::vfs::read(p).map_or(0, |src| crate::crate_image::file_hash(p, &src)));
    }
    h.finish128()
}

/// The test dependency check's own text of each crate of `crates` (cache.rs dep_check_key): the
/// crate as rustc is invoked on it and the text of the files the boot loaded (as its fingerprint
/// hashes them, without its dependencies'; skipped_text_hash has the others).
fn own_text_hashes(prog: &crate::program::Program, crates: &[u32]) -> Vec<u128> {
    let mut per: Vec<Option<crate::sip::Sip128>> = vec![None; prog.crates.len()];
    for &c in crates {
        let ci = &prog.crates[c as usize];
        let mut h = crate::crate_image::fingerprint_head(prog.syms.str(ci.name), ci.root_path(), ci.edition, ci.proc_macro, &ci.cfg.set);
        h.str("own-text");
        h.u128(ci.context.fingerprint());
        per[c as usize] = Some(h);
    }
    for f in 0..prog.boot_files.min(prog.files.len()) {
        if prog.front_macro_fragments.contains(&f) { continue; }
        let Some(h) = prog.file_crate.get(f).and_then(|&c| per.get_mut(c as usize)).and_then(Option::as_mut) else { continue };
        let pf = &prog.files[f];
        let path = prog.file_paths.get(f).map_or("", |p| p.as_str());
        h.u128(crate::crate_image::file_hash(path, &pf.src[..(pf.src_len as usize).min(pf.src.len())]));
    }
    crates.iter().map(|&c| per[c as usize].as_ref().map_or(0, |h| h.finish128())).collect()
}

/// The crates crate `c` depends on, directly or not (itself not).
pub(crate) fn dependency_closure(prog: &crate::program::Program, c: u32) -> Vec<u32> {
    let mut seen = vec![false; prog.crates.len()];
    let mut stack = vec![c];
    let mut out = Vec::new();
    while let Some(k) = stack.pop() {
        for &(_, d) in &prog.crates[k as usize].deps {
            if d != c && !std::mem::replace(&mut seen[d as usize], true) {
                out.push(d);
                stack.push(d);
            }
        }
    }
    out.sort_unstable();
    out
}

/// `makepad run`'s type check of every body of `crates` (the app's own crates whose clean verdict
/// is not stored: run.rs, before `main`), on MAKEPAD_RUN_JOBS threads: the checked bodies stay in
/// the typeck query for the builds that follow. A body that fails while files the lazy selection
/// skipped remain is checked again with every file loaded (its error may be a name in one).
/// Returns the errors, rustc's order, follow-ons dropped; a clean crate's verdict is stored.
pub fn run_check_crates(up: *mut Unit, crates: &[u32]) -> Vec<String> {
    match prepare_check(up, crates) {
        Some(p) => check_prepared(up, p, None, CheckMode::Typeck).strings(),
        None => Vec::new(),
    }
}

/// What an app check checks: the crates without a stored clean verdict, their verdict keys and
/// body owners (their skipped files loaded).
pub struct AppCheck {
    t0: u64,
    todo: Vec<u32>,
    keys: Vec<(u32, u128)>,
    owners: Vec<crate::program::DefId>,
}

/// The app check's lookups and loads (the cache is read here: before main, on the boot thread).
/// None: every crate's clean verdict is stored.
pub fn prepare_check(up: *mut Unit, crates: &[u32]) -> Option<AppCheck> {
    let u = unsafe { &*up };
    let t0 = jit::wall_ns();
    let mut keys = Vec::new();
    let mut todo = Vec::new();
    for &k in crates {
        let skipped = skipped_text_hash(k);
        let key = u.cache.as_ref().and_then(|c| unsafe { &mut *c.get() }.crate_check_key(k, skipped, 0));
        let stored = key.is_some_and(|key| u.cache.as_ref().is_some_and(|c| matches!(unsafe { &*c.get() }.borrowck_stored(key), Some(crate::borrowck::lazy::StoredVerdict::Clean))));
        if !stored {
            todo.push(k);
            keys.extend(key.map(|key| (k, key)));
        }
    }
    RUN_CHECK[0].store(todo.len() as u64, std::sync::atomic::Ordering::Relaxed);
    let t1 = jit::wall_ns();
    RUN_CHECK[3].store(t1 - t0, std::sync::atomic::Ordering::Relaxed);
    if todo.is_empty() { RUN_CHECK[2].store(t1 - t0, std::sync::atomic::Ordering::Relaxed); return None; }
    unsafe { jit::load_skipped_of(up, &todo) };
    RUN_CHECK[4].store(jit::wall_ns() - t1, std::sync::atomic::Ordering::Relaxed);
    let u = unsafe { &*up };
    let owners = body_owners(&u.prog, &todo);
    RUN_CHECK[1].store(owners.len() as u64, std::sync::atomic::Ordering::Relaxed);
    Some(AppCheck { t0, todo, keys, owners })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CheckMode { Typeck, Errors }

struct PreparedErrors {
    errors: Vec<(u32, String)>,
    cpu_ns: u64,
}
impl PreparedErrors {
    fn strings(self) -> Vec<String> { self.errors.into_iter().map(|(_, e)| e).collect() }
}

/// One check of check_prepared -- a body owner's errors, or an impl's soundness -- owning what it
/// reads: a scoped thread runs it, or beside a running program a job of the JIT's queue.
struct BodyCheck {
    upn: usize,
    front: std::sync::Arc<jit::FrontShared>,
    strict: bool,
    beside: bool,
    cpu: std::sync::atomic::AtomicU64,
}
impl BodyCheck {
    fn check(&self, d: crate::program::DefId, impl_check: bool) -> Option<Vec<String>> {
        struct Cpu<'a>(&'a std::sync::atomic::AtomicU64, u64);
        impl Drop for Cpu<'_> {
            fn drop(&mut self) { self.0.fetch_add(relax_session::clock::cpu_ns() - self.1, std::sync::atomic::Ordering::Relaxed); }
        }
        let ice = |prog: &crate::program::Program, d: crate::program::DefId| -> Option<Vec<String>> {
            let def = prog.def(d);
            let pos = prog.files[def.file as usize].ast.item(def.item).lo;
            Some(vec![prog.record_error(def.file, pos, "typeck", format!("internal compiler error while checking {} -- makepad run bug, report it{}", prog.def_path(d), jit::take_ice_location()))])
        };
        let retry = |p: &(dyn std::any::Any + Send)| crate::preempt::is_preempted(p) || p.is::<crate::preempt::NeedsLoad>() || p.is::<crate::preempt::OwnerBusy>();
        let (strict, beside) = (self.strict, self.beside);
        let _cpu = strict.then(|| Cpu(&self.cpu, relax_session::clock::cpu_ns()));
        let _quiet = strict.then(crate::lint::Suppress::new);
        let uu = unsafe { &*(self.upn as *const Unit) };
        let mut tries = 0;
        loop {
            if crate::run_threads::run_ending() { return None; }
            let r = beside.then(|| jit::front_read("app check"));
            let (prog, tcx) = r.as_ref().map_or((&uu.prog, &uu.tcx), |r| r.of(&self.front));
            if !strict && tries >= 100 { return if impl_check { None } else { ice(prog, d) }; }
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _p = beside.then(crate::preempt::Preemptible::on);
                if impl_check { return crate::unsafety::impl_errors(prog, tcx, d); }
                if !strict { return Some(tcx.typeck(prog, d).errors.to_vec()); }
                let owner = crate::borrowck::body::CheckedBody::item(prog, d).expect("prepared body owner");
                let (errors, events) = body_errors(prog, tcx, &prog.session.options, &owner, true, false);
                let mut errors: Vec<String> = errors.into_iter().map(|(_, e)| e).collect();
                errors.extend(events.iter().map(|ev| ev.error.render()));
                Some(errors)
            })) {
                Ok(e) => return e,
                Err(p) if strict && p.is::<crate::preempt::NeedsLoad>() => {
                    if prog.skipped_files.is_empty() { return ice(prog, d); }
                    drop(r);
                    unsafe { jit::load_all_skipped(self.upn as *mut Unit) };
                }
                Err(p) if retry(&*p) => {
                    tries += usize::from(!beside || p.is::<crate::preempt::NeedsLoad>());
                    drop(r);
                    if beside {
                        // (a queued check's CPU permit goes back while the writer runs)
                        let _wait = crate::pool::suspend();
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    } else {
                        std::thread::yield_now();
                    }
                }
                Err(_) => return if impl_check && !strict { None } else { ice(prog, d) },
            }
        }
    }
}

/// Checks prepared bodies. Beside a running program they are jobs of the JIT's queue (`queued_map`):
/// each body holds a front read, yields to writers and retries after them. Test dependencies use
/// the full error pipeline.
fn check_prepared(up: *mut Unit, p: AppCheck, beside: Option<&crate::options::RuntimeOptions>, mode: CheckMode) -> PreparedErrors {
    let AppCheck { t0, todo, keys, owners } = p;
    let strict = mode == CheckMode::Errors;
    let front = unsafe { (*up).front.clone() };
    let ctx = std::sync::Arc::new(BodyCheck {
        upn: up as usize, front: front.clone(), strict, beside: beside.is_some(), cpu: std::sync::atomic::AtomicU64::new(0),
    });
    let check_all = |items: Vec<crate::program::DefId>, impl_check: bool| -> Vec<Option<Vec<String>>> {
        if beside.is_some() { queued_map(&ctx, items, impl_check) } else { par_map(&items, jobs(), |&d| ctx.check(d, impl_check)) }
    };
    // Impl soundness once per crate, in parallel before its bodies. Dependencies are checked
    // regardless of the app-only scope used by the ordinary run's checker.
    let (epoch, admitted, impls) = {
        let r = beside.map(|_| jit::front_read("app check impls"));
        let u = unsafe { &*up };
        let (prog, tcx) = r.as_ref().map_or((&u.prog, &u.tcx), |r| r.of(&front));
        let epoch = tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst);
        let admitted: Vec<u32> = todo.iter().copied().filter(|&k| strict || crate::borrowck::event::admit(prog, k)).collect();
        let impls = if (!strict && crate::lint::enabled()) || admitted.is_empty() { Vec::new() } else { crate::unsafety::crate_impls(prog, tcx, &admitted) };
        (epoch, admitted, impls)
    };
    let mut failed = Vec::new();
    if !impls.is_empty() {
        let parts = check_all(impls.iter().map(|&(_, d)| d).collect(), true);
        for &k in &admitted {
            let mine: Vec<&Option<Vec<String>>> = impls.iter().zip(&parts).filter(|((c, _), _)| *c == k).map(|(_, p)| p).collect();
            if mine.iter().all(|p| p.is_some()) {
                crate::unsafety::store_crate_impl_errors(k, epoch, mine.into_iter().flat_map(|p| p.iter().flatten().cloned()).collect());
            }
        }
        if strict {
            failed.extend(impls.iter().zip(parts).filter_map(|(&(_, d), e)| e.filter(|e| !e.is_empty()).map(|e| (d, e))));
        }
    }
    let mut bodies: Vec<_> = owners.iter().zip(check_all(owners.clone(), false))
        .filter_map(|(&d, e)| e.filter(|e| !e.is_empty()).map(|e| (d, e))).collect();
    // A run's end stops the check: what it did not reach has no verdict, and no error.
    let run_ended = || PreparedErrors { errors: Vec::new(), cpu_ns: 0 };
    if crate::run_threads::run_ending() { return run_ended(); }
    let skipped_left = {
        let r = beside.map(|_| jit::front_read("app check skipped"));
        !r.as_ref().map_or(unsafe { &(*up).prog }, |r| r.of(&front).0).skipped_files.is_empty()
    };
    if (!bodies.is_empty() || !failed.is_empty()) && skipped_left {
        unsafe { jit::load_all_skipped(up) };
        let again: Vec<_> = bodies.iter().map(|f| f.0).collect();
        bodies = again.iter().zip(check_all(again.clone(), false))
            .filter_map(|(&d, e)| e.filter(|e| !e.is_empty()).map(|e| (d, e))).collect();
        let again: Vec<_> = failed.iter().map(|f| f.0).collect();
        failed = again.iter().zip(check_all(again.clone(), true))
            .filter_map(|(&d, e)| e.filter(|e| !e.is_empty()).map(|e| (d, e))).collect();
    }
    failed.extend(bodies);
    if crate::run_threads::run_ending() { return run_ended(); }
    let mut errors: Vec<(u32, String)> = {
        let r = beside.map(|_| jit::front_read("app check verdicts"));
        let prog = r.as_ref().map_or(unsafe { &(*up).prog }, |r| r.of(&front).0);
        failed.into_iter().flat_map(|(d, e)| { let k = prog.def(d).krate; e.into_iter().map(move |e| (k, e)) }).collect()
    };
    let clean = keys.into_iter().filter(|(k, _)| !errors.iter().any(|(c, _)| c == k)).map(|(_, key)| key);
    if beside.is_some() {
        for key in clean { crate::borrowck::lazy::push_verdict_key(key, crate::borrowck::lazy::StoredVerdict::Clean); }
    } else if let Some(c) = unsafe { (*up).cache.as_ref() } {
        let c = unsafe { &mut *c.get() };
        for key in clean { c.record_borrowck(key, crate::borrowck::lazy::StoredVerdict::Clean); }
    }
    if !strict { RUN_CHECK[2].store(jit::wall_ns() - t0, std::sync::atomic::Ordering::Relaxed); }
    errors.sort_by_cached_key(|(_, e)| error_position(e));
    errors.dedup();
    if errors.iter().any(|(_, e)| !is_follow_on(e)) { errors.retain(|(_, e)| !is_follow_on(e)); }
    PreparedErrors { errors, cpu_ns: ctx.cpu.load(std::sync::atomic::Ordering::Relaxed) }
}

/// A batch of a check beside the running program (the app's own crates, a test's dependencies) on
/// the JIT's queue (jit_queue `Job::Check`): no threads of its own. Its jobs run in the Background
/// class behind every build of the queue, on what the budget gives Background beside the program,
/// the JIT and the app's pools (it yields to their waiters between items), and go first in the
/// waiter's class once something waits for them (`waited`: the app's first window frame, the run's
/// exit). Each job takes the next item until its slice is used, then queues itself again: its
/// worker goes back to the queue's order every few milliseconds. (On threads of its own a test's
/// dependency check took every core beside a cold Fab start, first frame 27.5 s; on two of them
/// the start drew at 14.8 s, but the check ended 66 s after the tests.)
pub(crate) struct CheckBatch {
    ctx: std::sync::Arc<BodyCheck>,
    items: Vec<crate::program::DefId>,
    impl_check: bool,
    next: std::sync::atomic::AtomicUsize,
    /// items whose result is not in `out` yet
    left: std::sync::atomic::AtomicUsize,
    out: relax_session::sync::Mutex<Vec<(usize, Option<Vec<String>>)>>,
    done: (relax_session::sync::Mutex<()>, relax_session::sync::Condvar),
}

/// Something waits for the check beside the program (`waited`).
static CHECK_WAITED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// A job's slice: past it, the job gives its worker back to the queue after the item it is on.
const CHECK_SLICE: std::time::Duration = std::time::Duration::from_millis(20);

/// Something waits for the check beside the program: its jobs run first in the Interactive class.
pub(crate) fn check_waited() -> bool {
    CHECK_WAITED.load(std::sync::atomic::Ordering::Acquire)
}

/// The app's first window frame or the run's exit waits for the check beside the program.
fn waited() {
    if !CHECK_WAITED.swap(true, std::sync::atomic::Ordering::AcqRel) { jit::promote_checks(); }
}

/// `items` checked by jobs of the JIT's queue, results in order.
fn queued_map(ctx: &std::sync::Arc<BodyCheck>, items: Vec<crate::program::DefId>, impl_check: bool) -> Vec<Option<Vec<String>>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let n = items.len();
    if n == 0 { return Vec::new(); }
    let batch = std::sync::Arc::new(CheckBatch {
        ctx: ctx.clone(), items, impl_check, next: AtomicUsize::new(0), left: AtomicUsize::new(n),
        out: relax_session::sync::Mutex::new(Vec::with_capacity(n)), done: (relax_session::sync::Mutex::new(()), relax_session::sync::Condvar::new()),
    });
    // as many jobs as the budget admits at most: its classes decide how many of them run
    jit::queue_check(&batch, n.min(makepad_cpu_budget::limit() as usize));
    {
        let mut g = batch.done.0.lock().unwrap_or_else(|e| e.into_inner());
        while batch.left.load(Ordering::Acquire) != 0 { g = batch.done.1.wait(g).unwrap_or_else(|e| e.into_inner()); }
    }
    let mut out = std::mem::take(&mut *batch.out.lock().unwrap_or_else(|e| e.into_inner()));
    out.sort_unstable_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, r)| r).collect()
}

/// One job of a check batch (on a worker of the JIT's queue): items until its slice is
/// used, then the job queues itself again while items are left.
pub(crate) fn run_check_slice(batch: std::sync::Arc<CheckBatch>) {
    use std::sync::atomic::Ordering;
    let start = std::time::Instant::now();
    loop {
        let i = batch.next.fetch_add(1, Ordering::Relaxed);
        let Some(&d) = batch.items.get(i) else { return };
        // (a check's own panics are its ICE verdict; this is one outside them, its bookkeeping)
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| batch.ctx.check(d, batch.impl_check)))
            .unwrap_or_else(|_| Some(vec!["internal compiler error in the check beside the program -- makepad run bug, report it".to_string()]));
        batch.out.lock().unwrap_or_else(|e| e.into_inner()).push((i, r));
        if batch.left.fetch_sub(1, Ordering::AcqRel) == 1 {
            let _g = batch.done.0.lock().unwrap_or_else(|e| e.into_inner());
            batch.done.1.notify_all();
            return;
        }
        if start.elapsed() >= CHECK_SLICE {
            if batch.next.load(Ordering::Relaxed) < batch.items.len() { jit::queue_check(&batch, 1); }
            return;
        }
        crate::pool::checkpoint(0, "check beside the program");
    }
}

struct TestDepsResult {
    errors: Vec<(u32, String)>,
    names: Vec<(u32, String)>,
    selected: usize,
    stored: usize,
    owners: usize,
    cpu_ns: u64,
    wall_ns: u64,
}
struct TestDeps {
    worker: relax_session::sync::Mutex<Option<std::thread::JoinHandle<TestDepsResult>>>,
    failed: std::sync::OnceLock<bool>,
}
static TEST_DEPS: std::sync::OnceLock<TestDeps> = std::sync::OnceLock::new();

/// Selection, verdict lookup and loading all run beside the boot, on the check's thread; the
/// tests wait for its verdict (run.rs: a dependency rustc rejects runs no test, as cargo builds it).
pub fn start_test_deps(up: *mut Unit, select: impl FnOnce(&crate::program::Program) -> Vec<u32> + Send + 'static, mut names: Vec<(u32, String)>, runtime: &crate::options::RuntimeOptions) {
    crate::borrowck::lazy::disable_gandalf();
    let upn = up as usize;
    let runtime = crate::zone_alloc::kept_clone(runtime);
    let _k = crate::zone_alloc::kept();
    let worker = crate::run_threads::spawn_sized("test-deps-check", 256 << 20, move || {
        let _z = crate::zone_alloc::compiler_thread();
        makepad_cpu_budget::set_thread_class(makepad_cpu_budget::BACKGROUND);
        let (t0, cpu0) = (jit::wall_ns(), relax_session::clock::cpu_ns());
        let up = upn as *mut Unit;
        let front = unsafe { (*up).front.clone() };
        // per crate: its own text (the boot's files) and its dependency closure
        let (crates, own, closures) = {
            let r = jit::front_read("test dependency crates");
            let prog = r.of(&front).0;
            let crates = select(prog);
            let own = own_text_hashes(prog, &crates);
            let closures: Vec<Vec<u32>> = crates.iter().map(|&k| dependency_closure(prog, k)).collect();
            (crates, own, closures)
        };
        names.retain(|(k, _)| crates.contains(k));
        names.sort_unstable_by_key(|(k, _)| *k);
        names.dedup_by_key(|(k, _)| *k);
        // the text of the files the boot skipped, of every crate a key names (read here, off the owner)
        let mut skipped = std::collections::HashMap::new();
        for &k in crates.iter().chain(closures.iter().flatten()) { skipped.entry(k).or_insert_with(|| skipped_text_hash(k)); }
        let (todo, keys) = crate::owner::call(crate::owner::Class::Publish, || {
            let u = unsafe { &*up };
            let mut todo = Vec::new();
            let mut keys = Vec::new();
            for ((&k, &own), closure) in crates.iter().zip(&own).zip(&closures) {
                let key = u.cache.as_ref().and_then(|c| unsafe { &mut *c.get() }.dep_check_key(k, own, skipped[&k], closure.iter().map(|d| (*d, skipped[d]))));
                let stored = key.is_some_and(|key| u.cache.as_ref().is_some_and(|c| matches!(unsafe { &*c.get() }.borrowck_stored(key), Some(crate::borrowck::lazy::StoredVerdict::Clean))));
                if !stored {
                    todo.push(k);
                    keys.extend(key.map(|key| (k, key)));
                }
            }
            (todo, keys)
        });
        let stored = crates.len() - todo.len();
        let (count, checked) = if todo.is_empty() {
            (0, PreparedErrors { errors: Vec::new(), cpu_ns: 0 })
        } else {
            unsafe { jit::load_skipped_of(up, &todo) };
            let owners = { let r = jit::front_read("test dependency owners"); body_owners(r.of(&front).0, &todo) };
            (owners.len(), check_prepared(up, AppCheck { t0, todo, keys, owners }, Some(&runtime), CheckMode::Errors))
        };
        TestDepsResult {
            errors: crate::zone_alloc::kept_clone(&checked.errors), names,
            selected: crates.len(), stored, owners: count,
            cpu_ns: checked.cpu_ns + relax_session::clock::cpu_ns() - cpu0, wall_ns: jit::wall_ns() - t0,
        }
    }).expect("test dependency check thread");
    assert!(TEST_DEPS.set(TestDeps { worker: relax_session::sync::Mutex::new(Some(worker)), failed: std::sync::OnceLock::new() }).is_ok(), "test dependency check already started");
}

/// Every test exit joins the same check and reports its errors once, before the cache write.
pub fn finish_test_deps(code: i32) -> i32 {
    // An exit inside a unit call cannot wait for work that needs that owner.
    if crate::run::exit_caller_holds_owner() { return code; }
    let Some(check) = TEST_DEPS.get() else { return code };
    let failed = check.failed.get_or_init(|| {
        let t0 = jit::wall_ns();
        waited();
        let worker = check.worker.lock().unwrap_or_else(|e| e.into_inner()).take().expect("test dependency check worker");
        let result = worker.join();
        let waited = jit::wall_ns() - t0;
        let report = |e: &str| {
            if crate::run_state::observed() {
                let diagnostic = if e.lines().any(|line| line.trim().starts_with("--> ")) {
                    let (file, line, col, _) = error_position(e);
                    crate::diag::Diagnostic::new("borrowck", file, line, col, e.lines().next().unwrap_or(e))
                } else { crate::diag::from_legacy(e, "typeck") };
                crate::run_state::diagnostic(crate::run::app_domain(), diagnostic);
            }
            crate::run::report("error", e);
            eprintln!("{}", e);
        };
        let Ok(r) = result else {
            report("internal compiler error while checking test dependencies -- makepad run bug, report it");
            return true;
        };
        for (_, e) in &r.errors { report(e); }
        for (k, name) in &r.names {
            let n = r.errors.iter().filter(|(c, _)| c == k).count();
            if n != 0 {
                eprintln!("error: could not compile `{}` (lib) due to {} previous error{} (`makepad test` checks every path dependency as cargo builds it)", name, n, if n == 1 { "" } else { "s" });
            }
        }
        if crate::run::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("makepad-run: test dependency check: {} crates selected, {} stored clean, {} body owners, {} errors, cpu {:.0} ms, wall {:.0} ms, end waited {:.0} ms",
                r.selected, r.stored, r.owners, r.errors.len(), r.cpu_ns as f64 / 1e6, r.wall_ns as f64 / 1e6, waited as f64 / 1e6);
        }
        !r.errors.is_empty()
    });
    if *failed { 101 } else { code }
}

/// The app check beside the program (start_app_check): 0 none, 1 running, 2 done clean.
static BESIDE: (relax_session::sync::Mutex<u8>, relax_session::sync::Condvar) = (relax_session::sync::Mutex::new(0), relax_session::sync::Condvar::new());

/// makepad run of an app that presents frames: its app check runs beside main instead of before
/// it. Every body is still checked before it first runs (a lazy compile checks its body; a body
/// with errors is a compile error, which ends the run). The first frame waits for the check
/// (wait_app_check, from the platform's first window frame); an error ends the run before it,
/// reported as a build's errors are (`stop`, then exit 101).
pub fn start_app_check(up: *mut Unit, p: AppCheck, runtime: crate::options::RuntimeOptions, stop: Box<dyn Fn(&[String]) + Send>) {
    *BESIDE.0.lock().unwrap_or_else(|e| e.into_inner()) = 1;
    let upn = up as usize;
    crate::run_threads::spawn_sized("run-app-check", 256 << 20, move || {
        let _z = crate::zone_alloc::compiler_thread();
        makepad_cpu_budget::set_thread_class(makepad_cpu_budget::BACKGROUND);
        let errors = check_prepared(upn as *mut Unit, p, Some(&runtime), CheckMode::Typeck).strings();
        if crate::run::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("makepad-run: app check beside main: {} crates, {} body owners, {} errors, {:.0} ms",
                RUN_CHECK[0].load(std::sync::atomic::Ordering::Relaxed), RUN_CHECK[1].load(std::sync::atomic::Ordering::Relaxed), errors.len(), RUN_CHECK[2].load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6);
        }
        if !errors.is_empty() {
            stop(&errors);
            std::process::exit(101);
        }
        *BESIDE.0.lock().unwrap_or_else(|e| e.into_inner()) = 2;
        BESIDE.1.notify_all();
    }).expect("app check thread");
}

/// The first frame's gate (the platform asks before it submits its first window frame): waits
/// for the app check beside main. An error it finds ends the process instead (start_app_check).
pub fn wait_app_check() {
    if *BESIDE.0.lock().unwrap_or_else(|e| e.into_inner()) != 1 { return; }
    waited();
    let t0 = std::time::Instant::now();
    let mut g = BESIDE.0.lock().unwrap_or_else(|e| e.into_inner());
    while *g == 1 {
        g = BESIDE.1.wait(g).unwrap_or_else(|e| e.into_inner());
    }
    if crate::run::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!("makepad-run: the first frame waited {:.0} ms for the app check", t0.elapsed().as_secs_f64() * 1e3);
    }
}

/// The last run_check_crates: crates checked, their body owners, wall ns, of it the stored verdicts'
/// lookup and the skipped files' load.
pub static RUN_CHECK: [std::sync::atomic::AtomicU64; 5] = [const { std::sync::atomic::AtomicU64::new(0) }; 5];

/// `f` over `items` on MAKEPAD_RUN_JOBS threads (at most 16), results in `items` order.
/// The check's threads: MAKEPAD_RUN_JOBS, else one per hardware thread (at most 16).
fn jobs() -> usize {
    std::env::var("MAKEPAD_RUN_JOBS").ok().and_then(|v| v.parse::<usize>().ok()).filter(|n| *n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()).min(16))
}

/// How one definition's work in the whole check's item passes ended.
enum DefWork<R> {
    Done(R),
    /// The error to report: a fatal diagnostic, or an internal error saying what the panic said.
    Failed(String),
    /// A constant it evaluates needs a file the check left unloaded (`settled_defs`).
    NeedsLoad,
}

/// One definition's work on a worker of the whole check's item passes. A constant it evaluates may
/// find the compiler's owner held up (owner.rs): that work unwinds with no result and no message
/// and is no failure, so it is asked again, as the body checks do. Only a panic is an internal
/// error, and `ice` gets what it said.
fn def_work<R>(ice: impl Fn(&str) -> String, mut f: impl FnMut() -> R) -> DefWork<R> {
    loop {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut f)) {
            Ok(v) => return DefWork::Done(v),
            Err(p) if p.is::<crate::preempt::NeedsLoad>() => return DefWork::NeedsLoad,
            Err(p) if crate::preempt::is_preempted(&*p) || p.is::<crate::preempt::OwnerBusy>() => std::thread::yield_now(),
            Err(p) => {
                if let Some(fatal) = p.downcast_ref::<relax_types::infer::diagnostics::FatalError>() {
                    return DefWork::Failed(fatal.0.clone());
                }
                let said = p.downcast_ref::<String>().map(String::as_str).or_else(|| p.downcast_ref::<&str>().copied());
                return DefWork::Failed(ice(said.unwrap_or("a panic with no text")));
            }
        }
    }
}

/// A test's stand-in for preempted work: the next body checks of the fn named `.0` unwind with no
/// result, `.1` times.
#[cfg(test)]
static FORCED_BODY_UNWINDS: std::sync::Mutex<(String, u32)> = std::sync::Mutex::new((String::new(), 0));

#[cfg(test)]
pub(crate) fn force_body_unwinds(name: &str, times: u32) {
    *FORCED_BODY_UNWINDS.lock().unwrap() = (name.to_string(), times);
}

/// How many of them are still to come: a test that forced none shows nothing.
#[cfg(test)]
pub(crate) fn forced_body_unwinds_left() -> u32 {
    FORCED_BODY_UNWINDS.lock().unwrap().1
}

#[cfg(test)]
fn forced_body_unwind(prog: &crate::program::Program, d: crate::program::DefId) {
    let mut forced = FORCED_BODY_UNWINDS.lock().unwrap();
    if forced.1 > 0 && prog.def_path(d).ends_with(forced.0.as_str()) {
        forced.1 -= 1;
        drop(forced);
        std::panic::resume_unwind(Box::new(crate::preempt::Preempted));
    }
}

/// The same for evaluations alone (eval_items): the next ones of the item named `.0` unwind with
/// no result, `.1` times; with `.2` they panic instead. Its own count: the item's body check,
/// which runs first, must not take it.
#[cfg(test)]
static FORCED_EVALS: std::sync::Mutex<(String, u32, bool)> = std::sync::Mutex::new((String::new(), 0, false));

#[cfg(test)]
pub(crate) fn force_eval_unwinds(name: &str, times: u32) {
    *FORCED_EVALS.lock().unwrap() = (name.to_string(), times, false);
}

#[cfg(test)]
pub(crate) fn force_eval_panic(name: &str) {
    *FORCED_EVALS.lock().unwrap() = (name.to_string(), 1, true);
}

#[cfg(test)]
pub(crate) fn forced_evals_left() -> u32 {
    FORCED_EVALS.lock().unwrap().1
}

#[cfg(test)]
fn forced_eval(prog: &crate::program::Program, d: crate::program::DefId) {
    let mut forced = FORCED_EVALS.lock().unwrap();
    if forced.1 > 0 && prog.def_path(d).ends_with(forced.0.as_str()) {
        forced.1 -= 1;
        let panics = forced.2;
        drop(forced);
        assert!(!panics, "a forced panic");
        std::panic::resume_unwind(Box::new(crate::preempt::Preempted));
    }
}

/// An item pass's internal error, at the definition and saying what went wrong.
fn def_ice(prog: &crate::program::Program, d: crate::program::DefId, what: &str, said: &str) -> String {
    let def = prog.def(d);
    let pos = prog.files[def.file as usize].ast.item(def.item).lo;
    prog.record_error(def.file, pos, "typeck", format!("internal compiler error while checking {} of {}: {}{} -- makepad run bug, report it", what, prog.def_path(d), said, jit::take_ice_location()))
}

/// `f` on `defs`, on the check's workers, in `defs`' order.
fn def_round<R: Send>(prog: &crate::program::Program, tcx: &crate::tcx::Tcx, defs: &[crate::program::DefId], what: &str, f: &(impl Fn(&crate::program::Program, &crate::tcx::Tcx, crate::program::DefId) -> R + Sync)) -> Vec<DefWork<R>> {
    par_map(defs, jobs(), |&d| def_work(|said| def_ice(prog, d, what, said), || f(prog, tcx, d)))
}

/// A pass's results once no definition waits for a file: nothing else runs here, so the check's
/// skipped files load and the definitions that wanted one are asked again, as the body checks do.
fn settled_defs<R: Send>(u: &mut Unit, defs: &[crate::program::DefId], what: &str, mut work: Vec<DefWork<R>>, f: &(impl Fn(&crate::program::Program, &crate::tcx::Tcx, crate::program::DefId) -> R + Sync)) -> Vec<Result<R, String>> {
    loop {
        let left: Vec<usize> = (0..work.len()).filter(|&i| matches!(work[i], DefWork::NeedsLoad)).collect();
        if left.is_empty() || u.prog.skipped_files.is_empty() { break; }
        unsafe { jit::load_all_skipped(u) };
        let again: Vec<crate::program::DefId> = left.iter().map(|&i| defs[i]).collect();
        for (i, w) in left.into_iter().zip(def_round(&u.prog, &u.tcx, &again, what, f)) { work[i] = w; }
    }
    work.into_iter().zip(defs).map(|(w, &d)| match w {
        DefWork::Done(r) => Ok(r),
        DefWork::Failed(e) => Err(e),
        DefWork::NeedsLoad => Err(def_ice(&u.prog, d, what, "it needs a file to be loaded and none is left")),
    }).collect()
}

/// `settled_defs` for one definition, on the check's own thread: nothing else runs here either.
fn settled_def<R>(up: *mut Unit, d: crate::program::DefId, what: &str, mut f: impl FnMut() -> R) -> Result<R, String> {
    loop {
        let u = unsafe { &*up };
        match def_work(|said| def_ice(&u.prog, d, what, said), &mut f) {
            DefWork::Done(r) => return Ok(r),
            DefWork::Failed(e) => return Err(e),
            DefWork::NeedsLoad if u.prog.skipped_files.is_empty() => return Err(def_ice(&u.prog, d, what, "it needs a file to be loaded and none is left")),
            DefWork::NeedsLoad => unsafe { jit::load_all_skipped(up) },
        }
    }
}

fn par_map<T: Sync, R: Send>(items: &[T], jobs: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out: relax_session::sync::Mutex<Vec<(usize, R)>> = relax_session::sync::Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|sc| {
        for w in 0..jobs.min(items.len().max(1)) {
            let (next, out, f) = (&next, &out, &f);
            std::thread::Builder::new().name(format!("run-check-{}", w)).stack_size(256 << 20).spawn_scoped(sc, move || { let _z = crate::zone_alloc::compiler_thread();
                let mut local = Vec::new();
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(x) = items.get(i) else { break };
                    local.push((i, f(x)));
                }
                out.lock().unwrap().extend(local);
            }).expect("check worker");
        }
    });
    let mut v = out.into_inner().unwrap();
    v.sort_by_key(|(i, _)| *i);
    v.into_iter().map(|(_, r)| r).collect()
}

/// Check the selected crates; `selftest` also compiles every function (see the module doc).
pub fn check_crate(u: &mut Unit, root: u32, crates: &[u32], errors: Vec<String>, selftest: bool) -> i32 {
    check_crate_impl(u, root, crates, errors, true, selftest, None).0
}

/// `check_crate` reusing the verdicts of `reuse`'s unchanged crates (check_reuse.rs): their
/// diagnostics replayed, the per-crate passes run on the other crates only; a passing check
/// stores the others' verdicts.
pub(crate) fn check_crate_reusing(u: &mut Unit, root: u32, crates: &[u32], errors: Vec<String>, reuse: &mut crate::check_reuse::Reuse) -> i32 {
    check_crate_impl(u, root, crates, errors, true, false, Some(reuse)).0
}

/// Whether a check may reuse verdicts: nothing it was asked to do needs every body walked (a
/// per-body trace, the relaxed-Rust report or write-back, the field-view export, the borrow
/// checker's whole-program modes).
pub(crate) fn reuse_allowed(options: &crate::options::Options) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    !check_trace() && !borrowck_unchecked() && !crate::relaxed::STRICT_REPORT.load(Relaxed) && !crate::relaxed::FIX.load(Relaxed)
        && std::env::var_os("MAKEPAD_RUN_FV_EXPORT").is_none() && std::env::var_os("MAKEPAD_RUN_MIR_BORROWCK_ALL").is_none()
        && std::env::var_os("MAKEPAD_RUN_MIR_BUILD_ALL").is_none()
        && options.borrowck_all == crate::options::BorrowckAll::Default && !options.borrowck_instances
}

/// `makepad test`'s check of the crates under test before they run (run.rs check_test_subjects):
/// the app check of `crates`, its errors printed; the number of errors (borrow errors included)
/// and the warnings, in order (the caller prints them). No statistics. `harness`: the root is
/// built with `--test` (libtest's generated main replaces the crate's own, rustc_builtin_macros
/// test_harness.rs), so it has no entry point to check.
pub fn check_subjects(u: &mut Unit, root: u32, crates: &[u32], errors: Vec<String>, harness: bool) -> (usize, Vec<String>) {
    SUBJECTS.store(if harness { 2 } else { 1 }, std::sync::atomic::Ordering::Relaxed);
    let n = check_crate_impl(u, root, crates, errors, true, false, None).1.len();
    SUBJECTS.store(0, std::sync::atomic::Ordering::Relaxed);
    (n, std::mem::take(&mut *SUBJECT_WARNINGS.lock().unwrap_or_else(|e| e.into_inner())))
}

/// The warnings of the last check of `check_subjects`, in order
static SUBJECT_WARNINGS: relax_session::sync::Mutex<Vec<String>> = relax_session::sync::Mutex::new(Vec::new());

/// 0: a check; 1: `makepad test`'s check of a plain program; 2: of a libtest harness (check_subjects)
static SUBJECTS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
fn subjects_mode() -> u8 {
    SUBJECTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// The deep check's diagnostics without printing a second stream into a watch response.
pub fn collect_deep(u: &mut Unit, root: u32) -> Vec<crate::diag::Diagnostic> {
    check_crate_impl(u, root, &[root], Vec::new(), false, true, None).1
}

/// The shared body error pipeline. Dependency checks discard the body and leave lints off. Each
/// error comes with where rustc reports it (check_order.rs): library stability with the attribute
/// checks, the type check's errors, then the MIR checks' (unsafety, patterns), privacy late.
/// MAKEPAD_RUN_CHECK_TIMES: the body checks' CPU by step, summed over the threads (BODY_STEPS).
static BODY_NS: [std::sync::atomic::AtomicU64; 9] = [const { std::sync::atomic::AtomicU64::new(0) }; 9];
const BODY_STEPS: [&str; 9] = ["early lints", "typeck", "stability", "body lints", "unsafety", "privacy", "patterns", "borrowck", "borrow reports"];

fn body_times_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_CHECK_TIMES").is_some())
}

/// MAKEPAD_RUN_CHECK_BODY_TIMES=<file>: one row per checked body, its steps' CPU (ns, BODY_STEPS
/// order), its typed expressions and its path (tab-separated), written at the end of the check.
fn body_rows_path() -> Option<&'static std::path::Path> {
    static P: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    P.get_or_init(|| std::env::var_os("MAKEPAD_RUN_CHECK_BODY_TIMES").filter(|v| !v.is_empty()).map(Into::into)).as_deref()
}
static BODY_ROWS: relax_session::sync::Mutex<Vec<String>> = relax_session::sync::Mutex::new(Vec::new());
/// MAKEPAD_RUN_MEM_REPORT: each body's peak and kept bytes, then its owner.
static BODY_MEM_ROWS: relax_session::sync::Mutex<Vec<(i64, i64, crate::program::DefId)>> = relax_session::sync::Mutex::new(Vec::new());
thread_local! {
    static BODY_LOCAL: std::cell::Cell<[u64; 9]> = const { std::cell::Cell::new([0; 9]) };
}

/// `f`'s thread CPU added to body step `k` (when the times are on).
fn body_step<R>(k: usize, f: impl FnOnce() -> R) -> R {
    if !body_times_on() && body_rows_path().is_none() { return f(); }
    let t = relax_session::clock::cpu_ns();
    let r = f();
    let dt = relax_session::clock::cpu_ns() - t;
    BODY_NS[k].fetch_add(dt, std::sync::atomic::Ordering::Relaxed);
    BODY_LOCAL.with(|c| { let mut v = c.get(); v[k] += dt; c.set(v); });
    r
}

/// The body's row (MAKEPAD_RUN_CHECK_BODY_TIMES), its steps' CPU since the last row.
fn body_row(prog: &crate::program::Program, d: crate::program::DefId, exprs: usize) {
    let v = BODY_LOCAL.with(|c| c.replace([0; 9]));
    if body_rows_path().is_none() { return; }
    let ns: Vec<String> = v.iter().map(|x| x.to_string()).collect();
    let row = format!("{}\t{}\t{}", ns.join("\t"), exprs, prog.def_path(d));
    let _k = crate::zone_alloc::kept();
    BODY_ROWS.lock().unwrap().push(row);
}

fn body_times_report() {
    if let Some(p) = body_rows_path() {
        let rows = std::mem::take(&mut *BODY_ROWS.lock().unwrap());
        let mut text = format!("{}\texprs\tpath\n", BODY_STEPS.join("\t"));
        for r in rows { text.push_str(&r); text.push('\n'); }
        if let Err(e) = std::fs::write(p, text) { stderr_line!("makepad check: body times not written to {}: {}", p.display(), e); }
    }
    if !body_times_on() { return; }
    let parts: Vec<String> = BODY_STEPS.iter().enumerate().map(|(k, n)| format!("{} {:.2}", n, BODY_NS[k].load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e9)).collect();
    stderr_line!("makepad check: body cpu s: {}", parts.join(", "));
}

fn body_mem_report(prog: &crate::program::Program) {
    if !crate::memstat::enabled() { return; }
    let mut rows = std::mem::take(&mut *BODY_MEM_ROWS.lock().unwrap());
    if rows.is_empty() { return; }
    let mb = |bytes: i64| bytes as f64 / 1048576.0;
    let mut sum_peak = 0;
    let mut sum_kept = 0;
    let mut buckets = [(0usize, 0i64); 4];
    for &(peak, kept, _) in &rows {
        sum_peak += peak;
        sum_kept += kept;
        let bucket = if kept < 64 * 1024 { 0 } else if kept < 1024 * 1024 { 1 } else if kept < 16 * 1024 * 1024 { 2 } else { 3 };
        buckets[bucket].0 += 1;
        buckets[bucket].1 += kept;
    }
    rows.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    stderr_line!("makepad-run mem: bodies: {} bodies, kept sum {:.1} MB, peak sum {:.1} MB, largest peak {:.1} MB",
        rows.len(), mb(sum_kept), mb(sum_peak), mb(rows[0].0));
    for &(peak, kept, d) in rows.iter().take(12) {
        stderr_line!("makepad-run mem: bodies: top peak: peak {:.1} MB, kept {:.1} MB, {}", mb(peak), mb(kept), prog.def_path(d));
    }
    rows.sort_unstable_by(|a, b| b.1.cmp(&a.1));
    for &(peak, kept, d) in rows.iter().take(12) {
        stderr_line!("makepad-run mem: bodies: top kept: kept {:.1} MB, peak {:.1} MB, {}", mb(kept), mb(peak), prog.def_path(d));
    }
    stderr_line!("makepad-run mem: bodies: kept buckets: under 64 KB: {} bodies, {:.1} MB; 64 KB-1 MB: {} bodies, {:.1} MB; 1-16 MB: {} bodies, {:.1} MB; 16 MB and over: {} bodies, {:.1} MB",
        buckets[0].0, mb(buckets[0].1), buckets[1].0, mb(buckets[1].1), buckets[2].0, mb(buckets[2].1), buckets[3].0, mb(buckets[3].1));
}

fn body_errors(prog: &crate::program::Program, tcx: &crate::tcx::Tcx, options: &crate::options::Options, owner: &crate::borrowck::body::CheckedBody, generic: bool, lints: bool) -> (Vec<(crate::check_order::Stage, String)>, Vec<crate::borrowck::event::GandalfEvent>) {
    use crate::check_order::{Phase, Stage};
    use crate::memstat::Tag;
    let _quiet = (!lints).then(crate::lint::Suppress::new);
    let d = owner.owner;
    let typeck = Stage::of(Phase::Typeck, 0, d);
    let unsafety_stage = Stage::of(Phase::Mir, crate::check_order::MIR_UNSAFETY, d);
    let patterns = Stage::of(Phase::Mir, crate::check_order::MIR_PATTERNS, d);
    let mem = crate::memstat::enabled().then(|| (crate::memstat::thread_live(), crate::memstat::peak_start()));
    // the syntax-level lints do not wait for the types (rustc runs them before analysis)
    if lints { body_step(0, || crate::memstat::tagged(Tag::BodyLints, || relax_types::lint_early::check_def(prog, tcx, d))); }
    let body = body_step(1, || if lints { tcx.typeck(prog, d) } else { tcx.typeck_unstored(prog, d) });
    // library stability (E0658) of what the body names
    let gates = body_step(2, || crate::memstat::tagged(Tag::BodyLints, || relax_types::feature_gate::check_stability(prog, &body)));
    let inner = || -> (Vec<(Stage, String)>, Vec<crate::borrowck::event::GandalfEvent>) {
        if !body.errors.is_empty() {
            if lints { relax_types::lint_dead::note_failed_body(prog, tcx, &body); }
            return (staged(typeck, body.errors.to_vec()).collect(), Vec::new());
        }
        if lints {
            body_step(3, || crate::memstat::tagged(Tag::BodyLints, || {
                relax_types::lint::check_literals(prog, tcx, &body);
                relax_types::lint_pass::check_body(prog, tcx, &body);
            }));
        }
        let unsafety = body_step(4, || crate::memstat::tagged(Tag::BodyLints, || if lints { relax_types::unsafety::check_body(prog, tcx, &body) }
            else { relax_types::unsafety::run_body_errors(prog, tcx, &body) }));
        if !unsafety.is_empty() {
            return (staged(unsafety_stage, unsafety).collect(), Vec::new());
        }
        // rustc_privacy check_mod_privacy: the types of the body's expressions and patterns
        let privacy = body_step(5, || crate::memstat::tagged(Tag::BodyLints, || relax_types::privacy_types::check_body(prog, tcx, &body)));
        if !privacy.is_empty() {
            return (staged(Stage::new(Phase::Late, crate::check_order::LATE_PRIVACY), privacy).collect(), Vec::new());
        }
        // E0004 / E0005 and the pattern lints (rustc check_match: its errors stop the body
        // before borrowck); an internal failure of the analysis reports nothing, as in a check
        let pats = body_step(6, || crate::memstat::tagged(Tag::Patterns, || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| relax_types::pattern_analysis::check_body(prog, tcx, &body)))));
        // (binding conflicts of `x @ pat` do not end the checks: borrowck runs after them)
        let mut at_patterns = Vec::new();
        match pats {
            Ok(r) if !r.errors.is_empty() => { let mut e = r.errors; e.extend(r.at_patterns); return (staged(patterns, e).collect(), Vec::new()) }
            Ok(r) => at_patterns = r.at_patterns,
            Err(p) if crate::preempt::is_cancelled(&*p) => std::panic::resume_unwind(p),
            Err(_) => {
                if check_trace() { stderr_line!("makepad check: pattern analysis failed in {}", prog.def_path(d)); }
            }
        }
        if lints && !crate::borrowck::enabled(options) {
            return (staged(patterns, at_patterns).collect(), Vec::new());
        }
        let v = body_step(7, || crate::memstat::tagged(Tag::Borrowck, || crate::borrowck::check_body(&crate::tcx::EvalCx::new(prog, tcx, crate::tcx::TypingEnv::fully_monomorphized(prog), &crate::tcx::EvalState::new()), prog, tcx, &body)));
        crate::borrowck::note_verdict(&v);
        if borrowck_unchecked() {
            for x in &v.unchecked { eprintln!("makepad check: borrowck unchecked: {}", x); }
        }
        let args: Vec<_> = if owner.kind == crate::borrowck::body::BodyKind::Function && !generic { Vec::new() } else { body.args.to_vec() };
        let origin = if lints { crate::borrowck::event::Origin::Check } else { crate::borrowck::event::Origin::DeferredCheck };
        let events = body_step(8, || crate::memstat::tagged(Tag::Borrowck, || crate::borrowck::lazy::report_checked(prog, 0, tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst), d, &args, v.errors, origin)));
        (staged(patterns, at_patterns).collect(), events)
    };
    let (mut errs, events) = inner();
    errs.extend(staged(Stage::new(Phase::Attrs, 0), gates));
    if let Some((start, scope)) = mem {
        let peak = crate::memstat::peak_end(scope);
        let kept = crate::memstat::thread_live() - start;
        let _k = crate::zone_alloc::kept();
        BODY_MEM_ROWS.lock().unwrap().push((peak, kept, d));
    }
    body_row(prog, d, body.expr_ty.len());
    (errs, events)
}

/// A live save's lints in a test session (reload.rs test_edit): what the test check's pass over a
/// body (body_errors) reports of the body owners `defs` the save changed or added -- their syntax,
/// type and MIR lints, the borrow check's `unused_mut` -- in the check's order. Not the lints that
/// rest on every body of the crate (dead_code, unused imports): the next test check reports them.
/// A body with type errors reports none (the save is refused).
pub(crate) fn live_lints(prog: &crate::program::Program, tcx: &crate::tcx::Tcx, defs: &[crate::program::DefId]) -> Vec<String> {
    use crate::check_order::{Entry, Out};
    if !relax_types::lint::enabled() { return Vec::new(); }
    let mut roots: Vec<crate::program::DefId> = defs.iter().map(|&d| relax_types::type_queries::typeck_root_def_id(prog, d)).collect();
    roots.sort_unstable_by_key(|d| d.0);
    roots.dedup();
    let ((), reported) = relax_types::lint::capture(|| {
        for &d in &roots {
            if crate::borrowck::body::CheckedBody::item(prog, d).is_none() { continue; }
            // (an internal failure of a lint reports nothing, as in a check)
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                relax_types::lint_early::check_def(prog, tcx, d);
                // (typed again: the save's build typed it with no lints)
                let body = tcx.typeck_unstored(prog, d);
                if !body.errors.is_empty() { return; }
                relax_types::lint::check_literals(prog, tcx, &body);
                relax_types::lint_pass::check_body(prog, tcx, &body);
                let _ = relax_types::unsafety::check_body(prog, tcx, &body);
                let state = crate::tcx::EvalState::new();
                let _ = crate::borrowck::check_body(&crate::tcx::EvalCx::new(prog, tcx, crate::tcx::TypingEnv::fully_monomorphized(prog), &state), prog, tcx, &body);
            }));
        }
    });
    let entries: Vec<Entry> = reported.into_iter().filter(|r| !r.lint.is_empty() && roots.contains(&relax_types::type_queries::typeck_root_def_id(prog, r.def)))
        .map(|r| {
            let stage = crate::check_order::stage_of_lint(r.lint, r.def);
            // (a body's liveness lints keep the order its analysis reports them in, as the check's)
            let seq = if stage.phase == crate::check_order::Phase::Mir && stage.step == crate::check_order::MIR_LIVENESS { r.seq } else { 0 };
            Entry::new(stage, seq, if r.level >= relax_types::lint::Level::Deny { Out::Error(r.line) } else { Out::Warning(r.line) })
        })
        .collect();
    crate::check_order::sort(prog, entries).into_iter().filter_map(|e| match e.out { Out::Warning(s) | Out::Error(s) => Some(s), Out::Borrow(_) => None }).collect()
}

fn check_crate_impl(u: &mut Unit, root: u32, crates: &[u32], front: Vec<String>, emit: bool, compile: bool, mut reuse: Option<&mut crate::check_reuse::Reuse>) -> (i32, Vec<crate::diag::Diagnostic>) {
    use crate::check_order::{Phase, Stage};
    let t_check = std::time::Instant::now();
    // every error with where rustc reports it (check_order.rs); the front end's come first
    let mut errors: Vec<(Stage, String)> = staged(Stage::new(Phase::Resolve, 0), front).collect();
    // a constant that cannot be evaluated is one reported failure, not the end of the check
    jit::COMPILE_ERRORS_FATAL.store(false, std::sync::atomic::Ordering::Relaxed);
    // lints (rustc's deny-by-default ones are errors; warn-level ones are printed)
    relax_types::lint::enable();
    crate::layout::set_root_crate(root);
    let mut own = vec![false; u.prog.crates.len()];
    for &k in crates {
        own[k as usize] = true;
        u.prog.crates[k as usize].validate = true;
    }
    u.prog.own_crates = own.clone();
    // the crates whose verdicts are reused (check_reuse.rs) are left out of every per-crate pass:
    // `fresh` / `fresh_crates` are the others; `own` stays the whole check's (the cross-crate
    // checks, the lint scopes)
    let fresh: Vec<bool> = (0..own.len()).map(|k| own[k] && !reuse.as_ref().is_some_and(|r| r.is_hit(k as u32))).collect();
    let fresh_crates: Vec<u32> = crates.iter().copied().filter(|&k| fresh[k as usize]).collect();
    let (own_all, own) = (own, &fresh);
    let t_rsim0 = std::time::Instant::now();
    // the imports that never bind in rustc (each one's error is its report): no unused imports
    let (mut erroneous_imports, strict): (Vec<usize>, Vec<String>) = u.prog.strict_import_errors(&own).into_iter().unzip();
    errors.extend(staged(Stage::new(Phase::Resolve, 1), strict));
    errors.extend(staged(Stage::new(Phase::Resolve, 1), relax_types::lint::import_resolution_diags(&mut u.prog, &own)));
    let d_rsim = t_rsim0.elapsed();
    let t_sigs0 = std::time::Instant::now();
    // every checked definition's signature, on MAKEPAD_RUN_JOBS threads (queries over the shared
    // context); errors in definition order
    {
        let check_times = std::env::var_os("MAKEPAD_RUN_CHECK_TIMES").is_some();
        let defs: Vec<crate::program::DefId> = (0..u.prog.defs.len()).filter(|&i| own[u.prog.defs[i].krate as usize]).map(|i| crate::program::DefId(i as u32)).collect();
        let (prog, tcx) = (&u.prog, &u.tcx);
        let signature = |prog: &crate::program::Program, tcx: &crate::tcx::Tcx, d: crate::program::DefId| {
            let t0 = std::time::Instant::now();
            let mut e = tcx.check_def_signature(prog, d);
            let dt_sig = t0.elapsed();
            if prog.def(d).kind == DefKind::Impl { e.extend(relax_types::unsafety::check_impl(prog, tcx, d)); }
            let dt = t0.elapsed();
            if check_times && dt.as_millis() >= 500 {
                stderr_line!("makepad check: slow sig {:.2} s (check_def_sig {:.2} s): {:?} {}", dt.as_secs_f64(), dt_sig.as_secs_f64(), prog.def(d).kind, prog.def_path(d));
            }
            e
        };
        // the item lint passes meanwhile, on a thread of their own: they report lints only (no
        // error of the check's), and the lints are ordered when printed (check_order.rs)
        let first = std::thread::scope(|sc| {
            let lints = std::thread::Builder::new().name("run-check-lints".into()).stack_size(256 << 20).spawn_scoped(sc, || {
                let _z = crate::zone_alloc::compiler_thread();
                let t0 = std::time::Instant::now();
                relax_types::lint_pass::check_items(prog, tcx, own);
                if check_times { stderr_line!("makepad check: lint_pass::check_items {:.2} s (beside the signatures)", t0.elapsed().as_secs_f64()); }
            }).expect("check lints");
            let first = def_round(prog, tcx, &defs, "the signature", &signature);
            if let Err(p) = lints.join() { std::panic::resume_unwind(p); }
            first
        });
        for e in settled_defs(u, &defs, "the signature", first, &signature) {
            errors.extend(staged(Stage::new(Phase::Wf, 0), e.unwrap_or_else(|failed| vec![failed])));
        }
    }
    let d_sigs = t_sigs0.elapsed();
    let t_items0 = std::time::Instant::now();
    let mut st = StepTimes::new("item_checks");
    errors.extend(staged(Stage::new(Phase::Resolve, 0), relax_types::item_checks::duplicate_items(&u.prog, &own))); st.mark("duplicate_items");
    errors.extend(staged(Stage::new(Phase::Resolve, 0), relax_types::item_checks::duplicate_exported_macros(&u.prog, &own))); st.mark("duplicate_exported_macros");
    errors.extend(staged(Stage::new(Phase::Resolve, 3), relax_types::item_checks::super_paths(&u.prog, &own))); st.mark("super_paths");
    errors.extend(staged(Stage::new(Phase::Late, crate::check_order::LATE_PRIVATE_IN_PUBLIC), relax_types::item_checks::impl_trait_path_privacy(&u.prog, &u.tcx, &own))); st.mark("impl_trait_path_privacy");
    errors.extend(staged(Stage::new(Phase::Late, crate::check_order::LATE_PRIVATE_IN_PUBLIC), relax_types::item_checks::private_assoc_types(&u.prog, &u.tcx, &own))); st.mark("private_assoc_types");
    errors.extend(staged(Stage::new(Phase::Early, 1), relax_types::item_checks::gated_syntax(&u.prog, &own))); st.mark("gated_syntax");
    errors.extend(staged(Stage::new(Phase::Attrs, 0), relax_types::item_checks::node_attribute_targets(&u.prog, &own))); st.mark("node_attribute_targets");
    errors.extend(staged(Stage::new(Phase::Early, 0), relax_types::item_checks::param_attributes(&u.prog, &own))); st.mark("param_attributes");
    errors.extend(staged(Stage::new(Phase::Early, 0), relax_types::item_checks::foreign_blocks(&u.prog, &own))); st.mark("foreign_blocks");
    errors.extend(staged(Stage::new(Phase::Attrs, 0), relax_types::item_checks::duplicate_symbols(&u.prog, &own_all))); st.mark("duplicate_symbols");
    // the compiled binaries: `makepad test`'s subjects are each a test binary (cargo builds every
    // target separately); a check's are its bin crates
    let binaries: Vec<u32> = crates.iter().copied().filter(|&k| subjects_mode() != 0 || u.prog.crates[k as usize].is_bin).collect();
    errors.extend(staged(Stage::new(Phase::Attrs, 0), relax_types::item_checks::global_allocators_and_lang_items(&u.prog, &own_all, &binaries))); st.mark("global_allocators_and_lang_items");
    errors.extend(staged(Stage::new(Phase::Early, 0), relax_types::proc_macro_harness::check(&u.prog, &u.tcx, &own))); st.mark("proc_macro_harness");
    let entry = u.prog.crates[root as usize].is_bin && !u.prog.crates[root as usize].harness && subjects_mode() != 2;
    if entry { errors.extend(staged(Stage::new(Phase::Attrs, 0), relax_types::item_checks::entry_point(&u.prog, root))); }
    if entry { errors.extend(staged(Stage::new(Phase::Attrs, 0), relax_types::item_checks::entry_symbol_conflict(&u.prog, &own_all, root))); }
    st.mark("entry");
    errors.extend(staged(Stage::new(Phase::Early, 2), relax_types::lint_levels::check(&u.prog, &own))); st.mark("lint_levels");
    relax_types::lint_dead::reset();
    let privacy = u.prog.import_privacy_errors(&own);
    erroneous_imports.extend(privacy.iter().filter_map(|(i, _)| *i));
    errors.extend(staged(Stage::new(Phase::Resolve, 6), privacy.into_iter().map(|(_, e)| e))); st.mark("import_privacy");
    errors.extend(staged(Stage::new(Phase::Attrs, 0), u.prog.attribute_errors(&own))); st.mark("attribute_errors");
    errors.extend(staged(Stage::new(Phase::Early, 0), u.prog.ast_validation_errors(&own))); st.mark("ast_validation");
    let d_items = t_items0.elapsed();
    let t_wf0 = std::time::Instant::now();
    // item well-formedness (rustc wfcheck): after the signatures, and only when they are sound -- an
    // item with an unresolved or ill-counted type has no well-formedness to speak of (the lints
    // it reports: those between `wf_reports`, check_reuse.rs WF)
    let wf_ran = errors.is_empty();
    let wf_lo = relax_types::lint::reported_len();
    if wf_ran {
        let check_times = std::env::var_os("MAKEPAD_RUN_CHECK_TIMES").is_some();
        let defs: Vec<crate::program::DefId> = (0..u.prog.defs.len()).filter(|&i| own[u.prog.defs[i].krate as usize]).map(|i| crate::program::DefId(i as u32)).collect();
        let (prog, tcx) = (&u.prog, &u.tcx);
        let well_formed = |prog: &crate::program::Program, tcx: &crate::tcx::Tcx, d: crate::program::DefId| {
            let t0 = std::time::Instant::now();
            let v = relax_types::wfcheck::obligations::check_def(prog, tcx, d);
            let dt_wf = t0.elapsed();
            // rustc_privacy check_mod_privacy: the types of the item's signature
            let privacy = relax_types::privacy_types::check_item(prog, tcx, d);
            let dt = t0.elapsed();
            if check_times && dt.as_millis() >= 500 {
                stderr_line!("makepad check: slow wf {:.2} s (check_def {:.2} s): {:?} {}", dt.as_secs_f64(), dt_wf.as_secs_f64(), prog.def(d).kind, prog.def_path(d));
            }
            (v, privacy)
        };
        let first = def_round(prog, tcx, &defs, "the well-formedness", &well_formed);
        for e in settled_defs(u, &defs, "the well-formedness", first, &well_formed) {
            let e = e.unwrap_or_else(|failed| (vec![failed], Vec::new()));
            errors.extend(staged(Stage::new(Phase::Wf, 0), e.0));
            errors.extend(staged(Stage::new(Phase::Late, crate::check_order::LATE_PRIVACY), e.1));
        }
        if errors.is_empty() && subjects_mode() != 2 {
            if let Some(main) = crate::run::find_main(&u.prog, root) {
                if relax_types::wfcheck::obligations::is_entry_main(&u.prog, main) { errors.extend(staged(Stage::new(Phase::Wf, 0), relax_types::wfcheck::obligations::check_main(&u.prog, &u.tcx, main))); }
            }
        }
    }
    let wf_reports = wf_lo..relax_types::lint::reported_len();
    let d_wf = t_wf0.elapsed();
    let options = &u.prog.session.clone().options;
    crate::borrowck::lazy::disable_gandalf();
    if options.borrowck_all != crate::options::BorrowckAll::Default {
        let n = crate::borrowck::check_all(&u.prog, &u.tcx, options.borrowck_all == crate::options::BorrowckAll::Std);
        return (if n == 0 { 0 } else { 1 }, Vec::new());
    }
    // MIR borrowck pipeline harness (docs: relax_types::mir::build::driver_borrowck).
    if let Some(mode) = std::env::var_os("MAKEPAD_RUN_MIR_BORROWCK_ALL") {
        let errors = relax_types::mir::build::driver_borrowck::borrowck_all(&u.prog, &u.tcx, mode == "std");
        return (if errors == 0 { 0 } else { 1 }, Vec::new());
    }
    // MIR construction harness (docs: relax_types::mir::build::driver): build + validate every body.
    if let Some(mode) = std::env::var_os("MAKEPAD_RUN_MIR_BUILD_ALL") {
        let failures = relax_types::mir::build::driver::build_all(&u.prog, &u.tcx, mode == "std");
        return (if failures == 0 { 0 } else { 1 }, Vec::new());
    }
    // (the check's borrow errors are printed with its other diagnostics, in rustc's order)
    let borrow_events = crate::gandalf_out::Collector::start();
    let mut borrow_diagnostics = Vec::new();
    let up: *mut Unit = u;
    let mut fns = Vec::new();
    // the checked crates' definitions only (the program's other bodies are never looked at)
    for owner in (0..u.prog.defs.len()).filter(|&i| own[u.prog.defs[i].krate as usize]).map(|i| crate::program::DefId(i as u32)).filter_map(|d| crate::borrowck::body::CheckedBody::item(&u.prog, d)) {
        let d = u.prog.def(owner.owner);
        // the app check needs no seeds: no signature is asked here (each costs a query; Stage's
        // 9,200 took 3.4 s on one thread), the body's own args are used as they are
        if !compile {
            fns.push((owner, true));
            continue;
        }
        let in_trait = u.prog.def(d.parent).kind == DefKind::Trait;
        let generic = u.tcx.fn_sig(&u.prog, owner.owner).map_or(true, |s| s.n_generics > 0);
        // a fn whose where-clauses on concrete types cannot hold (`where &'a mut (): Clone`) is
        // type-checked but never compiled (rustc: impossible predicates are not codegened)
        // (only selftest compiles: the app check needs no such filter)
        let impossible = if generic || in_trait || !compile { false } else {
            let infcx = relax_types::infer::InferCtxt::new(&u.tcx, crate::tcx::TypingMode::PostAnalysis, crate::tcx::TypingEnv::fully_monomorphized(&u.prog).limit);

            u.tcx.clauses_of(&u.prog, owner.owner).into_iter().any(|predicate| {
                !infcx.evaluate_obligation(&u.prog, Goal { param_env: ParamEnvId::EMPTY, predicate }).may_apply()
            })
        };
        // a provided crate's fns are built by rustc into the loader: JIT code reaches its listed
        // ops through their shims, and an unlisted fn is compiled from source only when a body
        // reaches it -- that body's compile covers it (#[relax_api]); compiling every unlisted fn
        // here would report what never runs on the JIT side
        let provided = u.provided.iter().any(|(k, _)| *k == d.krate);
        fns.push((owner, generic || in_trait || impossible || provided));
    }
    let n_own = fns.len();
    // (every body: `makepad test`'s check prints the subjects' warnings, and those that rest on
    // every body -- dead code, unused imports, expectations -- need each one walked; a check of
    // unchanged subjects is stored whole, run.rs check_test_subjects)
    let t_owners = t_check.elapsed();
    if std::env::var_os("MAKEPAD_RUN_CHECK_TIMES").is_some() {
        stderr_line!("makepad check: owners breakdown: rsim {:.2} s, sigs {:.2} s, item_checks {:.2} s, wfcheck {:.2} s", d_rsim.as_secs_f64(), d_sigs.as_secs_f64(), d_items.as_secs_f64(), d_wf.as_secs_f64());
    }
    // typeck-audit 1.27: anonymous constants outside bodies are body owners of their own (typed
    // and checked on MAKEPAD_RUN_JOBS threads; their results, and the evaluations, in order)
    let anons: Vec<crate::program::DefId> = fresh_crates.iter().flat_map(|&k| crate::borrowck::body::signature_anon_consts(&u.prog, &u.tcx, k)).collect();
    let check_anon = |prog: &crate::program::Program, tcx: &crate::tcx::Tcx, anon: crate::program::DefId| {
        let body = tcx.typeck(prog, anon);
        let errs = body.errors.to_vec();
        let mut diags = Vec::new();
        if errs.is_empty() {
            relax_types::lint::check_literals(prog, tcx, &body);
            relax_types::lint_pass::check_body(prog, tcx, &body);
        }
        // rustc check_consts: an anonymous constant is a const context of its own
        if errs.is_empty() && crate::borrowck::enabled(options) {
            if let Some((_, relax_front::def_collector::NodeId::AnonConst(expr))) = prog.def_id_to_node_id.get(&anon) {
                let state = crate::tcx::EvalState::new();
                let cx = crate::tcx::EvalCx::new(prog, tcx, crate::tcx::TypingEnv::fully_monomorphized(prog), &state);
                let errs = crate::borrowck::check_anon_const(&cx, prog, tcx, &body, *expr);
                if !errs.is_empty() {
                    diags.extend(crate::borrowck::lazy::report_checked(prog, 0, tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst), anon, &body.args, errs, crate::borrowck::event::Origin::Check).iter().map(|e| e.error.to_diagnostic()));
                }
            }
        }
        (errs, diags)
    };
    let first = def_round(&u.prog, &u.tcx, &anons, "the constant", &check_anon);
    for (&anon, r) in anons.iter().zip(settled_defs(u, &anons, "the constant", first, &check_anon)) {
        match r {
            Ok((e, diags)) => {
                let e_ok = e.is_empty() && diags.is_empty();
                borrow_diagnostics.extend(diags);
                // rustc evaluates every array length / const argument written in a signature (used
                // or not): one that fails is reported there (E0080)
                if e_ok && u.tcx.generic_indices_of(&u.prog, anon).types_consts.is_empty() && !matches!(u.prog.def(u.prog.def(anon).parent).kind, DefKind::Trait | DefKind::TypeAlias) {
                    let env = crate::tcx::TypingEnv::fully_monomorphized(&u.prog);
                    let evaluated = settled_def(up, anon, "the constant", || { let _ = u.tcx.const_eval_resolve(&u.prog, env, anon, &[]); });
                    if let Err(failed) = evaluated { errors.push((Stage::of(Phase::Typeck, 0, anon), failed)); }
                }
                errors.extend(staged(Stage::of(Phase::Typeck, 0, anon), e))
            }
            Err(failed) => errors.push((Stage::of(Phase::Typeck, 0, anon), failed)),
        }
    }
    // every body owner type- and borrow-checked on MAKEPAD_RUN_JOBS threads: typeck and the borrow
    // checker are queries over the shared program and type context (as the run's typeck-ahead
    // workers use them); results are merged in owner order
    struct Outcome { errors: Vec<(Stage, String)>, diags: Vec<crate::diag::Diagnostic>, seed: Option<crate::program::DefId>, needs_load: bool, unchecked: bool }
    let jobs = jobs();
    let upn = up as usize;
    let check_one = move |owner: &crate::borrowck::body::CheckedBody, generic: bool| -> Outcome {
        let uu = unsafe { &*(upn as *const Unit) };
        let (prog, tcx) = (&uu.prog, &uu.tcx);
        let d = owner.owner;
        if check_trace() { stderr_line!("makepad check: body {}", prog.def_path(d)); }
        let mut out = Outcome { errors: Vec::new(), diags: Vec::new(), seed: None, needs_load: false, unchecked: false };
        // (work that unwound with no result is asked again in there, until it has an answer)
        let r = def_work(|said| def_ice(prog, d, "the body", said), || {
            #[cfg(test)]
            forced_body_unwind(prog, d);
            let (errors, events) = body_errors(prog, tcx, options, owner, generic, true);
            (errors, events.iter().map(|ev| ev.error.to_diagnostic()).collect())
        });
        match r {
            DefWork::Done((errs, diags)) => {
                if errs.is_empty() && owner.kind == crate::borrowck::body::BodyKind::Function && !generic {
                    out.seed = Some(d);
                }
                out.errors = errs;
                out.diags = diags;
            }
            // a module the check did not load (another crate's skipped file): no answer yet,
            // checked again once every file is loaded
            DefWork::NeedsLoad => { out.needs_load = true; out.unchecked = true; }
            DefWork::Failed(e) => out.errors.push((Stage::of(Phase::Typeck, 0, d), e)),
        }
        // an error may be a name in a file the app check left unloaded: settled with all loaded
        if !out.errors.is_empty() { out.needs_load = true; }
        out
    };
    let t_par = t_check.elapsed();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut outcomes: Vec<(usize, Outcome)> = Vec::new();
    {
        let results = relax_session::sync::Mutex::new(Vec::with_capacity(fns.len()));
        let (fns, next, results, check_one) = (&fns, &next, &results, &check_one);
        std::thread::scope(|sc| {
            for w in 0..jobs {
                std::thread::Builder::new().name(format!("run-check-ty-{}", w)).stack_size(256 << 20).spawn_scoped(sc, move || { let _z = crate::zone_alloc::compiler_thread();
                    let mut local = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some((owner, generic)) = fns.get(i) else { break };
                        local.push((i, check_one(owner, *generic)));
                        // return the allocator's free pages as the check goes (Stage held 4.6 GB)
                        if i % 20_000 == 19_999 { crate::memstat::trim(); }
                    }
                    results.lock().unwrap().extend(local);
                }).expect("check worker");
            }
        });
        outcomes.extend(results.lock().unwrap().drain(..));
    }
    if std::env::var_os("MAKEPAD_RUN_CHECK_TIMES").is_some() { stderr_line!("makepad check: owners {:.2} s, anon consts {:.2} s, bodies {:.2} s ({} jobs)", t_owners.as_secs_f64(), (t_par - t_owners).as_secs_f64(), (t_check.elapsed() - t_par).as_secs_f64(), jobs); }
    body_times_report();
    outcomes.sort_by_key(|(i, _)| *i);
    // owners that failed or met an unloaded module: every skipped file loaded, checked again
    // (only when some other crate's file is still skipped: else the first answer stands)
    let skipped_left = unsafe { !(&*up).prog.skipped_files.is_empty() };
    if skipped_left && outcomes.iter().any(|(_, o)| o.needs_load) {
        let t_skip = std::time::Instant::now();
        unsafe { jit::load_all_skipped(up) };
        if std::env::var_os("MAKEPAD_RUN_CHECK_TIMES").is_some() {
            stderr_line!("makepad check: load_all_skipped {:.2} s", t_skip.elapsed().as_secs_f64());
        }
    }
    let retry_idxs: Vec<usize> = outcomes.iter().enumerate()
        .filter(|(_, (_, o))| skipped_left && o.needs_load)
        .map(|(idx, _)| idx)
        .collect();
    if !retry_idxs.is_empty() {
        let t_retry = std::time::Instant::now();
        let retried = par_map(&retry_idxs, jobs, |&idx| {
            let fi = outcomes[idx].0;
            let (owner, generic) = &fns[fi];
            check_one(owner, *generic)
        });
        for (idx, new_o) in retry_idxs.iter().copied().zip(retried) {
            outcomes[idx].1 = new_o;
        }
        if std::env::var_os("MAKEPAD_RUN_CHECK_TIMES").is_some() {
            stderr_line!("makepad check: retried {} bodies in {:.2} s", retry_idxs.len(), t_retry.elapsed().as_secs_f64());
        }
    }
    let mut seeds = Vec::new();
    // rustc: an item whose body failed its checks (borrowck included) is tainted: it is never evaluated
    let mut tainted: std::collections::HashSet<crate::program::DefId> = std::collections::HashSet::new();
    for (i, mut o) in outcomes {
        // a body with no answer is no clean body
        if o.unchecked {
            let d = fns[i].0.owner;
            let e = def_ice(unsafe { &(*up).prog }, d, "the body", "it needs a file to be loaded and none is left");
            o.errors.push((Stage::of(Phase::Typeck, 0, d), e));
        }
        if !o.errors.is_empty() || !o.diags.is_empty() { tainted.insert(fns[i].0.owner); }
        errors.extend(o.errors);
        borrow_diagnostics.extend(o.diags);
        if let Some(d) = o.seed { seeds.push(d); }
    }
    let checked = seeds.len();
    let mut st = StepTimes::new("after bodies:");
    st.mark("merge");
    // the anonymous constants inside the bodies are body owners of their own (rustc
    // par_body_owners: an array length or const argument in a body is typed by its own typeck,
    // which reports its errors): their type errors, and their late lints (rustc
    // visit_nested_body: a const argument `U::<300>` is a body of its own, its literal typed as
    // the parameter). A repeat count its owner typed was linted with the owner, but its errors as
    // a constant (`[0; n]` with `n` a local) are its own
    for &k in &fresh_crates {
        let uu = unsafe { &*up };
        for (anon, owner, e) in crate::borrowck::body::body_anon_consts(&uu.prog, &uu.tcx, k) {
            let errs = settled_def(up, anon, "the constant", || {
                let outer = uu.tcx.typeck(&uu.prog, owner);
                let owner_typed = outer.has_expr(e) && outer.expr_typed.get(outer.expr_ix(e));
                let body = uu.tcx.typeck(&uu.prog, anon);
                if !owner_typed && body.errors.is_empty() && outer.errors.is_empty() {
                    relax_types::lint::check_literals(&uu.prog, &uu.tcx, &body);
                    relax_types::lint_dead::collect_body(&uu.prog, &uu.tcx, &body);
                }
                body.errors.to_vec()
            }).unwrap_or_else(|failed| vec![failed]);
            errors.extend(staged(Stage::of(Phase::Typeck, 0, anon), errs));
        }
    }
    st.mark("body anon consts");
    relax_types::lint_early::finish(&u.prog, &own); st.mark("lint_early::finish");
    errors.extend(staged(Stage::new(Phase::Typeck, 0), static_self_access(up, &own))); st.mark("static_self_access");
    // the feature gates (E0658 and friends): items and signatures, then the loaded bodies
    for v in relax_types::feature_gate::check_crates(unsafe { &(*up).prog }, unsafe { &(*up).tcx }, &fresh_crates) {
        errors.extend(staged(Stage::new(Phase::Early, 1), v));
    }
    st.mark("feature gates");
    // rustc_hir_analysis check_crate: every static and every non-generic, non-associated const
    // item of the checked crates is evaluated, used or not; one that cannot be evaluated is an
    // E0080 (the unit notes it, reported below with the builds' other notes)
    eval_items(up, &own, &tainted); st.mark("eval_items");
    // rustc_interface passes.rs analysis: an error (lint errors excluded) of the required analyses
    // -- resolution, feature gates, type and borrow checks, constant evaluation of items -- ends the
    // compilation before misc_checking_3, so check_mod_deathness (dead_code, unnameable_types)
    // and the late lint pass never run: a body that failed its check reported nothing it uses as
    // dead, and no late lint is reported. Nor does check_expectations
    // (unfulfilled_lint_expectations). misc_checking_3's own errors (privacy) do not count.
    let analyses_ok = !errors.iter().any(|(s, _)| s.required()) && borrow_diagnostics.is_empty() && !relax_types::lint::has_hard_error()
        && unsafe { (&*up).errors.iter().all(|e| e.starts_with("compile error in ")) };
    relax_types::lint_dead::finish(&u.prog, &u.tcx, &own, root, analyses_ok, &erroneous_imports); st.mark("lint_dead::finish");
    // rustc_monomorphize: a program's `main` and what it reaches, instantiated -- the constants
    // generic code evaluates there (post-monomorphization E0080) -- in the check of a build only
    // (Options::post_mono): cargo check runs no collector, its errors appear at the build
    let mono = options.post_mono && !compile && errors.is_empty() && borrow_diagnostics.is_empty() && unsafe { (&*up).errors.is_empty() };
    // what the unit records from here on is the instances' when nothing was recorded before:
    // rustc's collector runs after the analysis, its errors are none of the required analyses'
    let mono_only = mono && !unsafe { (&*up).query_errors.has_errors() };
    if mono { mono_check(up, &own_all, root); }
    let unit_stage = |e: &str| if mono_only { Stage::new(Phase::Post, 0) } else { unit_stage(e) };
    st.mark("mono_check");
    let t_front = t_check.elapsed();
    // the type/borrow check's freed memory goes back before the compile starts (Stage: 4.6 GB
    // of malloc free pages held after it)
    if compile { unsafe {
        let _job = jit::compile_job();
        let mut d = crate::owner::call(crate::owner::Class::Program, || jit::settle_drop(up));
        d.release();
    } crate::memstat::trim(); }
    if crate::memstat::enabled() {
        crate::memstat::print("check: type/borrow checked", &crate::memreport::unit_sizes(unsafe { &*up }));
        body_mem_report(unsafe { &(*up).prog });
    }
    for (_, e) in errors.iter_mut() {
        if is_internal(unsafe { &*up }, &own_all, e) && !e.contains(": internal:") && !e.starts_with("internal: ") {
            *e = label_internal(e);
        }
    }
    // every seed, and every instance, closure and glue slot the compiled code reaches (generic
    // callees, drop glue, vtable methods, std), compiled to machine code by the run's per-fn
    // compile until no new slots appear
    let failures = if compile { unsafe { compile_all(up, &seeds) } } else { Vec::new() };
    let n_failed = failures.len();
    // what the builds reported besides their own failures: constants that could not be
    // evaluated (CTFE panics), recorded instead of ending the check
    for e in unsafe { (&(*up).errors).iter().filter(|e| !e.starts_with("compile error in ")).cloned().collect::<Vec<_>>() } {
        let e = if is_internal(unsafe { &*up }, &own_all, &e) { label_internal(&e) } else { e };
        errors.push((unit_stage(&e), e));
    }
    let mut compile_errors = Vec::new();
    for (id, e) in failures {
        let name = unsafe { (&(*up).fns)[id as usize].name.clone() };
        report(up, id, &name, &e, "compile", &own_all, &mut compile_errors);
    }
    errors.extend(staged(Stage::new(Phase::Post, 0), compile_errors));
    // MAKEPAD_RUN_BORROWCK_INSTANCES=1: every reached instance is borrow-checked too (the background
    // checker's view: concrete type arguments, all crates)
    if crate::borrowck::enabled(options) && options.borrowck_instances {
        for id in 0..unsafe { (&(*up).fns).len() } {
            let key = unsafe { (&(*up).fns)[id].key.clone() };
            let FnKey::Inst(d, args) = key else { continue };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                let uu = &mut *up;
                let state = crate::tcx::EvalState::new();
                let cx = crate::tcx::EvalCx::new(&uu.prog, &uu.tcx, crate::tcx::TypingEnv::fully_monomorphized(&uu.prog), &state);
                let body = crate::typeck::instantiate_body(&cx, &uu.prog, &uu.tcx, d, &args);
                if !body.errors.is_empty() {
                    return;
                }
                let v = crate::borrowck::check_body(&crate::tcx::EvalCx::new(&uu.prog, &uu.tcx, crate::tcx::TypingEnv::fully_monomorphized(&uu.prog), &crate::tcx::EvalState::new()), &uu.prog, &uu.tcx, &body);
                crate::borrowck::note_verdict(&v);
                borrow_diagnostics.extend(crate::borrowck::lazy::report_checked(&uu.prog, 0, uu.tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst), d, &body.args, v.errors, crate::borrowck::event::Origin::Check).iter().map(|e| e.error.to_diagnostic()));
            }));
            let _ = r;
        }
    }
    unsafe {
        // a delayed bug no error explains (rustc: an ICE at session end) is one internal failure
        // of the check, never the end of it
        // rustc DiagCtxt::flush_delayed: delayed bugs are ICEs only when no error was emitted
        let explained = !errors.is_empty() || !borrow_diagnostics.is_empty() || !(*up).errors.is_empty() || (*up).query_errors.has_errors();
        for bug in (*up).query_errors.take_delayed() {
            if !explained { errors.push((Stage::new(Phase::Post, 0), format!("internal: delayed bug at the end of the check: {}", bug))); }
        }
        if !errors.is_empty() { (*up).query_errors.note_reported_error(); }
        (*up).flush_query_errors(true);
        errors.extend((*up).errors.iter().map(|e| (unit_stage(e), e.clone())));
    }
    let t_compile = t_check.elapsed() - t_front;
    if crate::memstat::enabled() {
        crate::memstat::print("check", &crate::memreport::unit_sizes(unsafe { &*up }));
    }
    let quiet = subjects_mode() != 0;
    if compile {
        eprintln!("makepad check: {} crates checked, {} fns compiled from {} seeds, {} failed builds; type/borrow check {:.1} s, compile {:.1} s", crates.len(), unsafe { (&(*up).fns).len() }, seeds.len(), n_failed, t_front.as_secs_f64(), t_compile.as_secs_f64());
    } else if !quiet {
        if let Some(r) = reuse.as_deref().filter(|r| r.hits() > 0) {
            eprintln!("makepad check: {} of {} crates unchanged since a passing check: their verdicts reused", r.hits(), r.crates());
        }
        eprintln!("makepad check: {} crates type- and borrow-checked in {:.1} s", fresh_crates.len(), t_front.as_secs_f64());
    }
    // MAKEPAD_RUN_SOLVER_WORK_TRACE: the solver's work counts and goal histograms of the check
    crate::solver_work::report();
    if crate::solver_work::enabled() { relax_types::next_solver::search_graph::goal_hist_report(); }
    let mut st = StepTimes::new("end:");
    if analyses_ok { relax_types::lint_levels::finish(&u.prog); }
    st.mark("lint_levels::finish");
    use crate::check_order::{Entry, Out};
    let reported = relax_types::lint::take_reported();
    let borrow_events = borrow_events.finish();
    // rustc's required analyses (lint errors aside) failed: it stops before misc_checking_3, so no
    // late lint is reported -- counted again here, with what the queries and the builds recorded
    // since the dead code pass ran
    let late_ok = analyses_ok && !errors.iter().any(|(s, _)| s.required()) && !reported.iter().any(|r| r.lint.is_empty())
        && !borrow_events.iter().any(|ev| ev.origin != crate::borrowck::event::Origin::Gate);
    let mut entries: Vec<Entry> = Vec::with_capacity(errors.len() + reported.len());
    // (check_reuse.rs: whether only a check whose analyses passed, or whose well-formedness checks
    // ran, reports each)
    let mut flags: Vec<u8> = Vec::with_capacity(reported.len());
    for (i, r) in reported.into_iter().enumerate() {
        let late = relax_types::lint::is_late(r.lint);
        if !late_ok && late { continue; }
        let stage = if r.lint.is_empty() { Stage::new(Phase::Attrs, 0) } else { crate::check_order::stage_of_lint(r.lint, r.def) };
        // (a body's liveness lints keep the order its analysis reports them in: rustc's)
        let seq = if stage.phase == Phase::Mir && stage.step == crate::check_order::MIR_LIVENESS { r.seq } else { 0 };
        entries.push(Entry::new(stage, seq, if r.level >= relax_types::lint::Level::Deny { Out::Error(r.line) } else { Out::Warning(r.line) }));
        flags.push(if late { crate::check_reuse::LATE } else { 0 } | if wf_reports.contains(&i) { crate::check_reuse::WF } else { 0 });
    }
    // a follow-on failure (a body or signature that could not be checked because of an error
    // already reported) is no error of its own: rustc reports only the cause
    let causes = errors.iter().any(|(_, e)| !is_follow_on(e)) || entries.iter().any(|e| matches!(e.out, Out::Error(_)));
    entries.extend(errors.into_iter().filter(|(_, e)| !causes || !is_follow_on(e)).map(|(stage, e)| Entry::new(stage, 0, Out::Error(e))));
    entries.extend(borrow_events.into_iter().map(|ev| Entry::new(Stage::of(Phase::Mir, crate::check_order::MIR_BORROWCK, ev.error.owner), 0, Out::Borrow(Box::new(ev)))));
    // verdict reuse: a check with no error stores its crates' diagnostics; the unchanged crates'
    // stored ones join this check's, as this check would have reported them
    if let Some(r) = reuse.as_deref_mut() {
        if late_ok && entries.iter().all(|e| matches!(e.out, Out::Warning(_))) { r.record(unsafe { &(*up).prog }, &entries, &flags); }
        entries.extend(r.replayed(late_ok, wf_ran));
    }
    let entries = crate::check_order::sort(unsafe { &(*up).prog }, entries);
    crate::check_order::print(&entries, emit, !quiet, crate::diag::enabled());
    // (`makepad test`'s: run.rs prints them, and a passing check's are stored with its verdict)
    if quiet { *SUBJECT_WARNINGS.lock().unwrap_or_else(|e| e.into_inner()) = entries.iter().filter_map(|e| match &e.out { Out::Warning(s) => Some(s.clone()), _ => None }).collect(); }
    let errors: Vec<String> = entries.iter().filter_map(|e| match &e.out { Out::Error(s) => Some(s.clone()), _ => None }).collect();
    let borrow_count = entries.iter().filter(|e| matches!(e.out, Out::Borrow(_))).count();
    st.mark("diagnostics out");
    crate::borrowck::finish_check(unsafe { &(*up).prog });
    st.mark("borrowck finish");
    if !quiet {
    eprintln!("makepad check: {} body owners of the checked crates, {} compiled in all, {} errors", n_own, unsafe { (*up).stats.compiled }, errors.len() + borrow_count);
    }
    if !quiet {
        let st = unsafe { &(*up).stats };
        eprintln!(
            "makepad check: cpu typeck {} ms, lower {} ms, opt {} ms, codegen {} ms; {} RIR insts, {} KB code",
            st.typeck_ns / 1_000_000,
            st.lower_ns / 1_000_000,
            st.opt_ns / 1_000_000,
            st.codegen_ns / 1_000_000,
            st.rir_insts,
            st.code_bytes / 1024
        );
        eprintln!("makepad check: constant initializers: {} evaluated to data, {} compiled and run; evaluation cpu {:.1} ms", st.ctfe_data, st.ctfe_code, st.ctfe_ns as f64 / 1e6);
    }
    unsafe { crate::jit::backend_bench(up) };
    if std::env::var("MAKEPAD_RUN_PASS_TIMES").is_ok() || std::env::var("MAKEPAD_RUN_OPT_HIST").is_ok() {
        let mut passes = String::new();
        for i in 0..relax_backend::stats::PASS_NAMES.len() {
            passes.push_str(&format!("{} {:.0}  ", relax_backend::stats::PASS_NAMES[i], relax_backend::stats::PASS_NS[i].load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6));
        }
        eprintln!("makepad check: passes (cpu ms): {}", passes);
        eprintln!("makepad check: opt ms by size (<16,<64,<256,<1k,<4k,more) {:?} fns {:?}", relax_codegen::opt::HIST[..6].iter().map(|x| x.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000).collect::<Vec<_>>(), relax_codegen::opt::HIST[6..].iter().map(|x| x.load(std::sync::atomic::Ordering::Relaxed)).collect::<Vec<_>>());
    }
    let _ = checked;
    crate::relaxed::finish_check();
    let mut diagnostics: Vec<_> = errors.iter().map(|e| crate::diag::from_legacy(e, "typeck")).collect();
    diagnostics.extend(borrow_diagnostics);
    (i32::from(!errors.is_empty() || borrow_count != 0), diagnostics)
}

/// Compiles every seed fn, then every slot the compiled code creates (callees, closures, glue,
/// vtable methods), each through the run's own per-fn compile (`jit::compile_slot`: front end
/// on the owner thread, optimisation and code generation off it), on MAKEPAD_RUN_JOBS threads until
/// no slot is left. Returns every failed build: (slot, error).
unsafe fn compile_all(up: *mut Unit, seeds: &[crate::program::DefId]) -> Vec<(u32, String)> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    *jit::CHECK_FAILURES.lock().unwrap_or_else(|p| p.into_inner()) = Some(Vec::new());
    // as a run: the front end on the owner thread, optimisation and code generation off it
    // (and the run's speculative callee builds)
    jit::enable_pipelining(&(*up).prog.session.clone().options);
    // (slots made before the seeds -- the root's main, constants' readers -- are compiled too)
    crate::owner::call(crate::owner::Class::Program, || {
        for &d in seeds {
            (&mut *up).fn_id(FnKey::Inst(d, Vec::new()));
        }
    });
    // every in-flight build holds its front end's and back end's state: at most 8 by default
    let jobs = std::env::var("MAKEPAD_RUN_JOBS").ok().and_then(|v| v.parse::<usize>().ok()).filter(|n| *n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()).min(8));
    // the run's reclamation (jit::settle_drop: RIR, checked bodies, templates of compiled
    // slots) once the process grew SETTLE_GROWTH since the last one (looked at every 2,000
    // compiled slots), so a whole-program check keeps the working set of a run, not every body
    // it ever built (Stage went past 18 GB without it). Dropped RIR is rebuilt for inlining:
    // settling every 20,000 slots made Scope's compile 4x slower.
    const SETTLE_GROWTH: u64 = 3 << 30;
    let compiled = AtomicUsize::new(0);
    let settled_at = AtomicUsize::new(0);
    let settled_rss = std::sync::atomic::AtomicU64::new(crate::memstat::resident() as u64);
    let next = AtomicUsize::new(0);
    let active = AtomicUsize::new(0);
    let panics: relax_session::sync::Mutex<Vec<(u32, String)>> = relax_session::sync::Mutex::new(Vec::new());
    let upn = up as usize;
    std::thread::scope(|sc| {
        for w in 0..jobs {
            let (next, active, panics, compiled, settled_at, settled_rss) = (&next, &active, &panics, &compiled, &settled_at, &settled_rss);
            std::thread::Builder::new().name(format!("run-check-{}", w)).stack_size(256 << 20).spawn_scoped(sc, move || { let _z = crate::zone_alloc::compiler_thread();
                let up = upn as *mut Unit;
                loop {
                    // the next slot not compiled yet; done when none is left and no worker is
                    // compiling (a compile in flight may still make new slots)
                    active.fetch_add(1, Ordering::SeqCst);
                    let pick = {
                        let _job = jit::compile_job();
                        crate::owner::call(crate::owner::Class::Program, || {
                            let n = (&(*up).fns).len();
                            let mut pick = None;
                            loop {
                                let i = next.load(Ordering::SeqCst);
                                if i >= n { break; }
                                if next.compare_exchange(i, i + 1, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                                    let f = &(&(*up).fns)[i];
                                    if !f.compiled {
                                        pick = Some(i as u32);
                                        break;
                                    }
                                }
                            }
                            pick
                        })
                    };
                    let Some(id) = pick else {
                        active.fetch_sub(1, Ordering::SeqCst);
                        if active.load(Ordering::SeqCst) == 0 && next.load(Ordering::SeqCst) >= crate::owner::call(crate::owner::Class::Program, || (&(*up).fns).len()) {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(2));
                        continue;
                    };
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| jit::compile_slot(up, id)));
                    if let Err(p) = r {
                        let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into());
                        panics.lock().unwrap().push((id, format!("internal compiler error: {}{}", msg, jit::take_ice_location())));
                    }
                    active.fetch_sub(1, Ordering::SeqCst);
                    let n = compiled.fetch_add(1, Ordering::Relaxed) + 1;
                    let at = settled_at.load(Ordering::Relaxed);
                    if n >= at + 2_000 && settled_at.compare_exchange(at, n, Ordering::AcqRel, Ordering::Relaxed).is_ok()
                        && crate::memstat::resident() as u64 > settled_rss.load(Ordering::Relaxed) + SETTLE_GROWTH {
                        let _job = jit::compile_job();
                        let mut d = crate::owner::call(crate::owner::Class::Program, || jit::settle_drop(up));
                        d.release();
                        crate::memstat::trim();
                        settled_rss.store(crate::memstat::resident() as u64, Ordering::Relaxed);
                    }
                }
            }).expect("check worker");
        }
    });
    let mut out = jit::CHECK_FAILURES.lock().unwrap_or_else(|p| p.into_inner()).take().unwrap_or_default();
    out.extend(panics.into_inner().unwrap());
    out
}

/// Is a failure a makepad run bug rather than an error rustc would report? A compiler panic, an
/// internal error, an error without an error code, or one located outside the checked crates
/// (std or a dependency: rustc builds those).
fn is_internal(u: &Unit, own: &[bool], e: &str) -> bool {
    let first = e.lines().next().unwrap_or("");
    if first.contains("internal compiler error") || first.contains("internal:") || first.contains("makepad run bug") {
        return true;
    }
    // `path:line:col: E0123: ..`
    let mut it = first.splitn(4, ':');
    let (Some(path), Some(l), Some(c), Some(rest)) = (it.next(), it.next(), it.next(), it.next()) else { return true };
    if l.trim().parse::<u32>().is_err() || c.trim().parse::<u32>().is_err() {
        return true;
    }
    let rest = rest.trim_start();
    // (an error rustc words without a code: Program::record_uncoded_error)
    let coded = rest.starts_with(relax_front::diagnostics::UNCODED_PREFIX) || (rest.len() > 5 && rest.starts_with('E') && rest[1..5].bytes().all(|b| b.is_ascii_digit()));
    if !coded {
        return true;
    }
    match u.prog.file_paths.iter().position(|p| p == path) {
        Some(i) => !u.prog.file_crate.get(i).is_some_and(|k| own.get(*k as usize).copied().unwrap_or(false)),
        None => true,
    }
}

/// `path:line:col: internal: msg` for a makepad run bug (the error keeps its own text otherwise).
fn label_internal(e: &str) -> String {
    let mut it = e.splitn(4, ':');
    if let (Some(p), Some(l), Some(c), Some(rest)) = (it.next(), it.next(), it.next(), it.next()) {
        if l.trim().parse::<u32>().is_ok() && c.trim().parse::<u32>().is_ok() {
            return format!("{}:{}:{}: internal:{}", p, l, c, rest);
        }
    }
    format!("internal: {}", e)
}

/// One failed build as an error line: its location (the error's own, else the fn's), the
/// message's first line, the instance it failed for, and `internal` for a makepad run bug.
fn report(up: *mut Unit, id: u32, name: &str, e: &str, phase: &'static str, own: &[bool], errors: &mut Vec<String>) {
    let u = unsafe { &*up };
    let mut first = e.lines().next().unwrap_or("").to_string();
    // an error without a location takes its fn's
    let located = {
        let mut it = first.splitn(4, ':');
        matches!((it.next(), it.next(), it.next()), (Some(_), Some(l), Some(c)) if l.trim().parse::<u32>().is_ok() && c.trim().parse::<u32>().is_ok())
    };
    if !located {
        if let Some((file, pos)) = function_origin(u, id) {
            let path = &u.prog.file_paths[file as usize];
            let (l, c) = crate::lexer::line_col(&u.prog.files[file as usize].src, pos);
            first = format!("{}:{}:{}: {}", path, l, c, first);
        }
    }
    let internal = is_internal(u, own, &first);
    // the type check of the fn reported it already (a non-generic body checked before compiling)
    let shown = if internal { label_internal(&first) } else { first.clone() };
    if errors.iter().any(|x| *x == shown) {
        return;
    }
    let mut n = name.to_string();
    if let crate::jit::FnKey::Inst(_, args) = &u.fns[id as usize].key {
        if !args.is_empty() && !n.contains('<') {
            let a: Vec<String> = args.iter().map(|t| crate::typeck::ty_to_string(false, &u.prog, &u.tcx, *t)).collect();
            n = format!("{}<{}>", n, a.join(", "));
        }
    }
    let line = format!("{} [in {}]", shown, n);
    if crate::diag::enabled() {
        let mut d = crate::diag::recorded(e).or_else(|| crate::diag::recorded(&first)).unwrap_or_else(|| crate::diag::from_legacy(&first, phase));
        d.notes.push(format!("in {}", n));
        if internal { d.notes.push("internal: a makepad run bug, not an error in the program".to_string()); }
        crate::diag::record(line.clone(), d);
    }
    errors.push(line);
}

fn function_origin(u: &Unit, mut id: u32) -> Option<(u32,u32)> {
    for _ in 0..64 {
        match &u.fns[id as usize].key {
            FnKey::Inst(d,_) => { let d = u.prog.def(*d); return Some((d.file,u.prog.files[d.file as usize].ast.item(d.item).lo)); }
            FnKey::Closure(f,e,_) => return Some((*f,u.prog.files[*f as usize].ast.expr(crate::ast::ExprId(*e)).lo)),
            FnKey::Sandbox(parent) | FnKey::Plain(parent) => id = *parent,
            FnKey::Shim(shim) => { let d = u.prog.def(shim.kind.def_id()); return Some((d.file, u.prog.files[d.file as usize].ast.item(d.item).lo)); }
            FnKey::Glue(_,t) => {
                if let crate::types::TyKind::Adt(d,_, _) = u.tcx.tys.kind(*t) {
                    let d = u.prog.def(*d); return Some((d.file,u.prog.files[d.file as usize].ast.item(d.item).lo));
                }
                return None;
            }
        }
    }
    None
}

/// (path, line, col, text) of a legacy line or a rendered borrow error (no location: first).
fn error_position(e: &str) -> (String, u32, u32, String) {
    let first = e.lines().find_map(|line| line.trim().strip_prefix("--> ")).unwrap_or_else(|| e.lines().next().unwrap_or(""));
    for (i, _) in first.match_indices(':') {
        let mut it = first[i + 1..].splitn(3, ':');
        if let (Some(l), Some(c)) = (it.next(), it.next()) {
            if let (Ok(l), Ok(c)) = (l.trim().parse::<u32>(), c.trim().parse::<u32>()) {
                return (first[..i].to_string(), l, c, e.to_string());
            }
        }
    }
    (String::new(), 0, 0, e.to_string())
}

/// The check's errors with where rustc reports them: the stage of the check that found them,
/// unless the code says where rustc's resolver or privacy checks report it.
fn staged(stage: crate::check_order::Stage, errors: impl IntoIterator<Item = String>) -> impl Iterator<Item = (crate::check_order::Stage, String)> {
    errors.into_iter().map(move |e| (crate::check_order::stage_for(stage, &e), e))
}

/// Where rustc reports what the unit recorded: the queries' errors (coherence, signatures,
/// constants that could not be evaluated -- rustc_hir_analysis check_crate evaluates them with the
/// body owners' type checks) in the analysis, where their code puts them; an internal error after it.
fn unit_stage(e: &str) -> crate::check_order::Stage {
    use crate::check_order::{Phase, Stage};
    let internal = e.starts_with("internal") || e.contains(": internal:") || e.contains("internal compiler error");
    crate::check_order::stage_for(Stage::new(if internal { Phase::Post } else { Phase::Typeck }, 0), e)
}

/// A check that could not run because of an error reported elsewhere (typeck's cascade notes).
fn is_follow_on(e: &str) -> bool {
    let first = e.lines().next().unwrap_or("");
    first.contains("type checking failed due to previous errors")
        || first.contains("signature has unresolved types -- fix the signature errors first")
        || first.contains("has unresolved types in its signature -- fix its signature errors first")
}

/// Evaluates the checked crates' statics and non-generic const items (rustc evaluates them in
/// `cargo check`, used or not). A body with type errors is skipped: those errors are reported.
fn eval_items(up: *mut Unit, own: &[bool], tainted: &std::collections::HashSet<crate::program::DefId>) {
    let u = unsafe { &mut *up };
    let items: Vec<(crate::program::DefId, bool)> = (0..u.prog.defs.len())
        .filter(|&i| own[u.prog.defs[i].krate as usize])
        .map(|i| crate::program::DefId(i as u32))
        .filter_map(|d| match u.prog.def(d).kind {
            DefKind::Static => Some((d, true)),
            DefKind::Const if u.tcx.generic_indices_of(&u.prog, d).types_consts.is_empty() => Some((d, false)),
            _ => None,
        })
        .filter(|(d, _)| relax_types::const_eval::const_body(&u.prog, *d).is_some())
        .collect();
    // the check-mode interpreter (relax_types const_eval interp: rustc's CTFE checks of each
    // initializer) reads only the program and the type context: its verdicts on the items not yet
    // evaluated are computed on the pool here, and the evaluations below take them
    // (check_initializer) instead of interpreting on the owner thread
    let t_ahead = std::time::Instant::now();
    let mut n_ahead = 0;
    if u.prog.session.options.const_fuel {
        let ahead: Vec<crate::program::DefId> = items.iter().map(|&(d, _)| d)
            .filter(|&d| u.prog.crates[u.prog.def(d).krate as usize].validate && !tainted.contains(&d))
            .filter(|&d| if u.prog.def(d).kind == DefKind::Static { !u.statics.contains_key(&d) } else { u.consts.get(&(d, Vec::new())).is_none() })
            .filter(|&d| u.tcx.typeck(&u.prog, d).errors.is_empty())
            .collect();
        n_ahead = ahead.len();
        let (prog, tcx) = (&u.prog, &u.tcx);
        let outs = par_map(&ahead, jobs(), |&d| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| relax_types::const_eval::interp::check_item(prog, tcx, d))).ok());
        for (d, o) in ahead.into_iter().zip(outs) {
            if let Some(o) = o { u.interp_ahead.insert(d, o); }
        }
    }
    let t_ahead = t_ahead.elapsed();
    let _job = jit::compile_job();
    // MAKEPAD_RUN_CHECK_TIMES: where the time goes (typeck lookups, owner-call queueing, statics, consts)
    let times = std::env::var_os("MAKEPAD_RUN_CHECK_TIMES").is_some();
    let (mut t_tyck, mut t_queue, mut t_static, mut t_const, mut n_static, mut n_const) = (0u128, 0u128, 0u128, 0u128, 0usize, 0usize);
    let n_items = items.len();
    for (d, is_static) in items {
        let t0 = std::time::Instant::now();
        let skip = tainted.contains(&d) || !u.tcx.typeck(&u.prog, d).errors.is_empty();
        t_tyck += t0.elapsed().as_nanos();
        if skip { continue; }
        let before = unsafe { (&*up).errors.len() };
        let t1 = std::time::Instant::now();
        let inner = std::sync::atomic::AtomicU64::new(0);
        // (work cancelled on its way is asked again: const_addr's stack pops as it unwinds, and a
        // static's initializer that unwound runs again into its storage (PendingInit). A panic
        // is the check's internal error, never a clean item)
        let evaluated = settled_def(up, d, if is_static { "the static" } else { "the constant" }, || {
            #[cfg(test)]
            forced_eval(unsafe { &(*up).prog }, d);
            crate::owner::call(crate::owner::Class::Program, || unsafe {
                let t2 = std::time::Instant::now();
                let u = &mut *up;
                if is_static { u.static_addr(d); } else { u.const_addr(d, Vec::new()); }
                inner.store(t2.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
            })
        });
        let inner = inner.load(std::sync::atomic::Ordering::Relaxed) as u128;
        t_queue += t1.elapsed().as_nanos().saturating_sub(inner);
        if is_static { t_static += inner; n_static += 1; } else { t_const += inner; n_const += 1; }
        // a foreign static the check does not link is not the program's error
        let u = unsafe { &mut *up };
        let mut i = before;
        while i < u.errors.len() {
            if u.errors[i].starts_with("compile error in ") && u.errors[i].contains("not found in the loaded libraries") { u.errors.remove(i); } else { i += 1; }
        }
        if let Err(failed) = evaluated { u.errors.push(failed); }
    }
    // (a verdict no evaluation took is not kept for a later check)
    unsafe { (*up).interp_ahead.clear(); }
    if times {
        stderr_line!("makepad check: eval_items {} items: interpreter ahead {:.2} s ({} items), typeck lookups {:.2} s, owner calls {:.2} s, {} statics {:.2} s, {} consts {:.2} s",
            n_items, t_ahead.as_secs_f64(), n_ahead, t_tyck as f64 / 1e9, t_queue as f64 / 1e9, n_static, t_static as f64 / 1e9, n_const, t_const as f64 / 1e9);
        let ns = |k: usize| crate::jit::INIT_NS[k].load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e9;
        stderr_line!("makepad check: initializers (cpu, whole check): interpreter {:.2} s, to data {:.2} s, compiled and run {:.2} s, eval_init in all {:.2} s, cache records {:.2} s",
            ns(0), ns(1), ns(2), ns(3), ns(4));
    }
}

/// rustc's collect_and_partition_mono_items for a binary, reduced to its constants and its recursion
/// limit: every instance of the checked crates' fns reachable from `main` is lowered (not compiled),
/// which evaluates the constants it names with its type arguments; a failing one is reported
/// (E0080) as rustc's codegen reports it. An instance more than `recursion_limit` instances of the same
/// definition deep (polymorphic recursion that grows its types) is "reached the recursion limit while
/// instantiating" (rustc_monomorphize collector `check_recursion_limit`). Library code is not entered:
/// its constants are not the program's diagnostics. Only the check of a build runs it (Options::post_mono).
fn mono_check(up: *mut Unit, own: &[bool], root: u32) {
    let u = unsafe { &mut *up };
    let main = (0..u.prog.defs.len()).map(|i| crate::program::DefId(i as u32)).find(|&d| {
        let def = u.prog.def(d);
        def.kind == DefKind::Fn && def.krate == root && u.prog.name(d) == "main"
            && def.parent == u.prog.mods[u.prog.crates[root as usize].root_mod as usize].def
    });
    let limit = u.prog.crates[root as usize].recursion_limit as usize;
    let _job = jit::compile_job();
    // (instance, the instance it was reached from): the chain of a reached instance is its DFS stack
    // (collector.rs: a program starts at `main`; a library's roots are all its non-generic fns)
    let roots: Vec<crate::program::DefId> = match main { Some(m) => vec![m], None => (0..u.prog.defs.len()).map(|i| crate::program::DefId(i as u32)).filter(|&d| {
        let def = u.prog.def(d);
        def.kind == DefKind::Fn && def.krate == root && u.prog.def(def.parent).kind != DefKind::Trait
            && u.tcx.fn_sig(&u.prog, d).is_some_and(|s| s.n_generics == 0)
    }).collect() };
    let mut work: Vec<(u32, Option<u32>)> = roots.into_iter().rev().map(|d| (crate::owner::call(crate::owner::Class::Program, || unsafe { (&mut *up).fn_id(FnKey::Inst(d, Vec::new())) }), None)).collect();
    let mut parents: std::collections::HashMap<u32, Option<u32>> = std::collections::HashMap::new();
    let mut deep_seen: std::collections::HashMap<crate::program::DefId, usize> = std::collections::HashMap::new();
    while let Some((id, parent)) = work.pop() {
        if parents.contains_key(&id) { continue; }
        parents.insert(id, parent);
        let (def_of, mine) = {
            let u = unsafe { &*up };
            let def_of = |i: u32| match &u.fns[i as usize].key { FnKey::Inst(d, _) => Some(*d), _ => None };
            (def_of(id), match &u.fns[id as usize].key { FnKey::Inst(d, _) => own[u.prog.def(*d).krate as usize], _ => false })
        };
        // collector.rs check_recursion_limit: the active instances of this definition on the stack
        let mut same_def = 0usize;
        if let Some(d) = def_of {
            let u = unsafe { &*up };
            let mut at = parent;
            while let Some(a) = at {
                if matches!(&u.fns[a as usize].key, FnKey::Inst(x, _) if *x == d) { same_def += 1; }
                at = parents.get(&a).copied().flatten();
            }
            if same_def > limit {
                let def = u.prog.def(d);
                let pos = u.prog.files[def.file as usize].ast.item(def.item).lo;
                let name = u.fns[id as usize].name.clone();
                let msg = u.prog.record_uncoded_error(def.file, pos, "mono", format!("reached the recursion limit while instantiating `{}`", name));
                unsafe { (&mut *up).errors.push(msg); }
                continue;
            }
        }
        let callees = crate::owner::call(crate::owner::Class::Program, || unsafe {
            // collector.rs: a library instance is lowered by rustc too; Relax lowers it only when a type argument
            // is too big to lay out (its body's layout query is then rustc's SizeOverflow, reported where it is
            // evaluated), and follows none of its callees
            let library_overflow = !mine && matches!(&(&*up).fns[id as usize].key, FnKey::Inst(_, args) if args.iter().any(|t| {
                let uu = &*up;
                uu.tcx.tys.depth(*t) <= 6 && std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| uu.size_overflows(*t))).unwrap_or(false)
            }));
            if !mine && !library_overflow { return Vec::new(); }
            // (a fn taking or returning a value of hundreds of MB is not lowered: its frame would
            // be allocated for real)
            // (instances over deeply nested types -- closures wrapped in closures -- cost more to
            // lower than a constants check is worth; one that recurses into itself is followed to the limit)
            if let FnKey::Inst(_, args) = &(&*up).fns[id as usize].key {
                // (deep instances of one definition are followed up to the recursion limit's worth, so a recursion that
                // nests its type argument deeper at every step (issue-22638) still reaches the limit)
                if args.iter().any(|t| (&*up).tcx.tys.depth(*t) > 6) {
                    let n = deep_seen.entry(def_of.unwrap()).or_insert(0usize);
                    *n += 1;
                    if *n > limit + 2 { return Vec::new(); }
                }
            }
            if let FnKey::Inst(d, _) = &(&*up).fns[id as usize].key {
                let uu = &*up;
                if uu.tcx.typeck(&uu.prog, *d).expr_ty.iter().any(|t| uu.tcx.tys.depth(*t) > 8) { return Vec::new(); }
            }
            let huge = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if let FnKey::Inst(d, args) = &(&*up).fns[id as usize].key {
                    let (d, args) = (*d, args.clone());
                    let uu = &mut *up;
                    if let Some(sig) = uu.tcx.fn_sig(&uu.prog, d).cloned() {
                        let env = crate::tcx::TypingEnv::fully_monomorphized(&uu.prog);
                        for t in sig.params.iter().chain(std::iter::once(&sig.ret)) {
                            let t = uu.tcx.tys.subst(*t, &args);
                            let Ok(t) = uu.tcx.try_normalize_erasing_late_bound_regions(&uu.prog, env, t) else { return true };
                            if uu.tcx.is_coroutine(t) { continue; }
                            if uu.layout_of(t).size > (1 << 28) { return true; }
                        }
                    }
                }
                false
            })).unwrap_or(true);
            if huge { return Vec::new(); }
            let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| jit::build_rir(up, id)));
            // (the lowering's own limit on how deep the type arguments of an instance may nest is the same one)
            if let Ok(Err(msg)) = &built {
                if msg.starts_with("reached the recursion limit while instantiating") {
                    let u = &mut *up;
                    if let FnKey::Inst(d, _) = &u.fns[id as usize].key {
                        let def = u.prog.def(*d);
                        let pos = u.prog.files[def.file as usize].ast.item(def.item).lo;
                        let msg = u.prog.record_uncoded_error(def.file, pos, "mono", msg.clone());
                        u.errors.push(msg);
                    }
                }
            }
            // (rustc's SizeOverflow of a type the instance lays out: lowering's error, at its use)
            if let Ok(Err(msg)) = &built {
                if msg.ends_with("are too big for the target architecture") {
                    // (a library instance's error is reported once by its lowering, with no location in the library's source)
                    if !library_overflow { (&mut *up).errors.push(msg.clone()); }
                }
            }
                if !matches!(built, Ok(Ok(()))) || library_overflow { return Vec::new(); }
            let mut out = Vec::new();
            if let Some(f) = &(&*up).fns[id as usize].rir {
                for b in &f.blocks {
                    for i in &b.insts {
                        match i {
                            relax_backend::rir::Inst::Call(relax_backend::rir::Callee::Fn(c) | relax_backend::rir::Callee::CFn(c, _), _, _) => out.push(*c),
                            _ => {}
                        }
                    }
                }
            }
            out
        });
        work.extend(callees.into_iter().map(|c| (c, Some(id))));
    }
}

/// rustc_const_eval interpret/memory.rs: "encountered static that tried to access itself during
/// initialization" -- the initializer of a static reads the static's own value (`static A: u32 = A`).
/// Taking its address (`&A`, `&A.0`, `addr_of!`) is fine. Initializers holding closures are left
/// alone (a closure body runs later).
fn static_self_access(up: *mut Unit, own: &[bool]) -> Vec<String> {
    use crate::ast::ExprKind;
    use crate::typeck::Res;
    let u = unsafe { &*up };
    let mut out = Vec::new();
    for i in 0..u.prog.defs.len() {
        let d = crate::program::DefId(i as u32);
        let def = u.prog.def(d);
        if def.kind != DefKind::Static || !own[def.krate as usize] { continue; }
        if relax_types::const_eval::const_body(&u.prog, d).is_none() { continue; }
        let Ok(body) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| u.tcx.typeck(&u.prog, d))) else { continue };
        if !body.errors.is_empty() { continue; }
        let ast = &u.prog.files[body.file as usize].ast;
        let ids: Vec<crate::ast::ExprId> = (0..body.expr_ty.len()).map(|k| crate::ast::ExprId(if k < body.expr_n1 as usize { body.expr_lo + k as u32 } else { body.expr_lo2 + k as u32 - body.expr_n1 })).collect();
        if ids.iter().any(|e| matches!(ast.expr(*e).kind, ExprKind::Closure(..) | ExprKind::Async(..))) { continue; }
        let mut addr_only = std::collections::HashSet::new();
        for e in &ids {
            if let ExprKind::AddrOf(_, _, x) = ast.expr(*e).kind {
                let mut x = x;
                loop {
                    addr_only.insert(x.0);
                    match ast.expr(x).kind {
                        ExprKind::Paren(y) | ExprKind::Field(y, _) | ExprKind::TupleField(y, _) | ExprKind::Index(y, _) => x = y,
                        _ => break,
                    }
                }
            }
        }
        // a write through a place rooted in another static
        for e in &ids {
            let (ExprKind::Assign(lhs, _) | ExprKind::AssignOp(_, lhs, _)) = ast.expr(*e).kind else { continue };
            let mut x = lhs;
            loop {
                match ast.expr(x).kind {
                    ExprKind::Paren(y) | ExprKind::Field(y, _) | ExprKind::TupleField(y, _) | ExprKind::Index(y, _) | ExprKind::AddrOf(_, _, y) | ExprKind::Unary(crate::ast::UnOp::Deref, y) => x = y,
                    _ => break,
                }
            }
            if matches!(ast.expr(x).kind, ExprKind::Path(_)) {
                if let Some(Res::Def(t)) = body.res.get(&x.0) {
                    if *t != d && u.prog.def(*t).kind == DefKind::Static {
                        out.push(u.prog.record_error(body.file, ast.expr(*e).lo, "typeck", "E0080: modifying a static's initial value from another static's initializer".to_string()));
                    }
                }
            }
        }
        for e in &ids {
            if matches!(ast.expr(*e).kind, ExprKind::Path(_)) && !addr_only.contains(&e.0) && matches!(body.res.get(&e.0), Some(Res::Def(t)) if *t == d) {
                out.push(u.prog.record_error(body.file, ast.expr(*e).lo, "typeck", "E0080: encountered static that tried to access itself during initialization".to_string()));
                break;
            }
        }
    }
    out
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&BOOT_SKIPPED, std::sync::OnceLock::new());
    reset(&CHECK_WAITED, std::sync::atomic::AtomicBool::new(false));
    reset(&BESIDE, (relax_session::sync::Mutex::new(0), relax_session::sync::Condvar::new()));
    reset(&RUN_CHECK, [const { std::sync::atomic::AtomicU64::new(0) }; 5]);
}

#[cfg(test)]
mod def_work_tests {
    use super::{def_work, DefWork};

    fn ice(said: &str) -> String {
        format!("ice: {said}")
    }

    /// (the signature pass reported any unwind as an internal error, these silent ones too)
    #[test]
    fn work_that_found_the_owner_busy_or_was_preempted_is_asked_again() {
        let mut tries = 0;
        let r = def_work(ice, || {
            tries += 1;
            match tries {
                1 => std::panic::resume_unwind(Box::new(crate::preempt::OwnerBusy)),
                2 => std::panic::resume_unwind(Box::new(crate::preempt::Preempted)),
                _ => tries,
            }
        });
        assert!(matches!(r, DefWork::Done(3)));
    }

    #[test]
    fn work_that_needs_a_file_says_so() {
        let r = def_work(ice, || -> u32 { std::panic::resume_unwind(Box::new(crate::preempt::NeedsLoad)) });
        assert!(matches!(r, DefWork::NeedsLoad));
    }

    #[test]
    fn a_fatal_diagnostic_is_the_definitions_error() {
        let r = def_work(ice, || -> u32 { std::panic::resume_unwind(Box::new(relax_types::infer::diagnostics::FatalError("overflow".to_string()))) });
        assert!(matches!(r, DefWork::Failed(e) if e == "overflow"));
    }

    #[test]
    fn a_panic_is_an_internal_error_and_says_what_it_said() {
        let r = def_work(ice, || -> u32 { panic!("the query's table has no {}", "entry") });
        assert!(matches!(r, DefWork::Failed(e) if e == "ice: the query's table has no entry"));
        let r = def_work(ice, || -> u32 { panic!("plain") });
        assert!(matches!(r, DefWork::Failed(e) if e == "ice: plain"));
    }
}
