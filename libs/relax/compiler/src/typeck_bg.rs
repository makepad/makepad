//! `makepad run`'s background type check. Type checking on the startup path is demand-driven (a
//! build checks the bodies it needs, the ahead helpers the ones its callers are about to need);
//! every body is still checked (the user's rule, 2026-11-02). After the first present (a headless
//! program: once main runs) one thread at the compile workers' low priority type-checks every body
//! owner of the dependency crates -- first what main reaches (the fns its checked bodies name,
//! depth first), then every other owner, crate by crate -- yielding while the program's threads
//! compile. An error is reported at once, as a compile error of code that has not run (stderr, the
//! launcher's report, the run's diagnostics), whether that code ever runs or not; the program
//! itself never runs unchecked code (a build checks its body first, jit.rs). A crate whose every
//! body is clean stores its verdict (cache.rs crate_check_key), so a later run checks only what
//! changed, as cargo builds a dependency once. Not checked here: the app's own crates (checked
//! whole before main, run.rs), proc-macro crates (they run in the macro host) and the sysroot
//! (shipped checked, as rustc ships it). The bodies checked here are not kept
//! (Tcx::typeck_unstored): the program's builds keep only the ones they use.

use crate::jit::{self, FrontShared, Unit};
use crate::program::{DefId, DefKind, Program};
use crate::typeck::{Body, Res};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

/// body owners checked, owners with errors, crates whose clean verdict was stored, cpu ns, wall ns
/// from its start to its end (0 while it runs)
pub static STATS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

/// Starts the background check of unit `up` (`root`: the program's crate; `main`: where the
/// reachability walk starts).
pub fn start(up: *mut Unit, root: u32, main: Option<DefId>, runtime: &crate::options::RuntimeOptions) {
    if std::env::var("MAKEPAD_RUN_BG_TYPECK").is_ok_and(|v| v == "0") { return; }
    let upn = up as usize;
    let runtime = crate::zone_alloc::kept_clone(runtime);
    let _k = crate::zone_alloc::kept();
    // (typeck recurses deeply on big bodies: the check workers' stack)
    let _ = std::thread::Builder::new().name("run-typeck-bg".into()).stack_size(256 << 20).spawn(move || {
        let _z = crate::zone_alloc::compiler_thread();
        run(upn as *mut Unit, root, main, &runtime);
    });
}

fn stopping() -> bool {
    crate::preempt::is_stopped() || crate::owner::exit_tid() != 0
}

/// The crates this check covers -- not the app's (checked before main), not a proc-macro crate,
/// not the sysroot, and no clean verdict stored for it -- with their verdicts' cache keys.
fn todo_crates(u: &Unit, root: u32, skipped: &[u128]) -> (Vec<bool>, Vec<Option<u128>>) {
    fn canon(p: &str) -> String {
        std::fs::canonicalize(p).map(|c| c.display().to_string()).unwrap_or_else(|_| p.to_string())
    }
    let std_dir = format!("{}/", canon(&crate::std_dir()).trim_end_matches('/'));
    let prog = &u.prog;
    (0..prog.crates.len() as u32).map(|k| {
        let c = &prog.crates[k as usize];
        if c.proc_macro || k == root || crate::borrowck::event::app_crate(prog, k)
            || matches!(prog.syms.str(c.name), "core" | "alloc" | "std" | "proc_macro")
            || canon(c.root_path()).starts_with(&std_dir) {
            return (false, None);
        }
        let Some(c) = u.cache.as_ref() else { return (true, None) };
        let c = unsafe { &mut *c.get() };
        match c.crate_check_key(k, skipped[k as usize], 0) {
            Some(key) if matches!(c.borrowck_stored(key), Some(crate::borrowck::lazy::StoredVerdict::Clean)) => (false, None),
            key => (true, key),
        }
    }).unzip()
}

enum Checked {
    /// the fns its body names (empty: it has errors, or it is no body owner)
    Done(Vec<DefId>),
    /// it waits on a file the lazy selection skipped: checked again once that is loaded
    Later,
    /// the run is ending
    Stop,
}

fn run(up: *mut Unit, root: u32, main: Option<DefId>, runtime: &crate::options::RuntimeOptions) {
    while !jit::past_startup() {
        if stopping() { return; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    jit::lower_this_thread_priority(runtime);
    let (t0, w0) = (relax_backend::cpu_ns(), jit::wall_ns());
    let f = unsafe { (*up).front.clone() };
    // the crates' skipped files read here, the keys made on the owner (the cache is the owner's)
    let n = { let r = jit::front_read("typeck bg crates"); r.of(&f).0.crates.len() as u32 };
    let skipped_text: Vec<u128> = (0..n).map(crate::check::skipped_text_hash).collect();
    let upn = up as usize;
    let (todo, keys) = crate::owner::call(crate::owner::Class::Publish, || todo_crates(unsafe { &*(upn as *const Unit) }, root, &skipped_text));
    if !todo.contains(&true) { return finish(t0, w0); }
    let mut seen: HashSet<DefId> = HashSet::new();
    let mut failed: HashSet<u32> = HashSet::new();
    let mut stored: HashSet<u32> = HashSet::new();
    // what main reaches, depth first (named_fns' order)
    let mut stack: Vec<DefId> = main.into_iter().collect();
    while let Some(d) = stack.pop() {
        if !seen.insert(d) { continue; }
        match check(&f, d, &todo, &mut failed) {
            Checked::Done(named) => stack.extend(named.into_iter().rev().filter(|d| !seen.contains(d))),
            Checked::Later => { seen.remove(&d); }
            Checked::Stop => return,
        }
    }
    // every other owner; the files the lazy selection skipped are loaded by the completion
    // thread meanwhile (jit.rs start_completion): their owners are checked as they appear
    loop {
        let (owners, skipped) = {
            let r = jit::front_read("typeck bg owners");
            let (prog, _) = r.of(&f);
            let crates: Vec<u32> = (0..prog.crates.len() as u32).filter(|&k| todo.get(k as usize).copied().unwrap_or(false)).collect();
            let owners: Vec<DefId> = crate::check::body_owners(prog, &crates).into_iter().filter(|d| !seen.contains(d)).collect();
            let skipped: HashSet<u32> = prog.skipped_files.keys().map(|&m| prog.mods[m as usize].krate).collect();
            (owners, skipped)
        };
        let mut pending: HashSet<u32> = HashSet::new();
        for d in owners {
            seen.insert(d);
            match check(&f, d, &todo, &mut failed) {
                Checked::Done(_) => {}
                Checked::Later => {
                    seen.remove(&d);
                    let r = jit::front_read("typeck bg later");
                    pending.insert(r.of(&f).0.def(d).krate);
                }
                Checked::Stop => return,
            }
        }
        // a crate whose every body is loaded and checked clean: its verdict goes to the cache with
        // the borrow checker's (the run's next cache write)
        for k in 0..todo.len() as u32 {
            if todo[k as usize] && !failed.contains(&k) && !skipped.contains(&k) && !pending.contains(&k) && stored.insert(k) {
                if let Some(key) = keys[k as usize] { crate::borrowck::lazy::push_verdict_key(key, crate::borrowck::lazy::StoredVerdict::Clean); }
                STATS[2].fetch_add(1, Ordering::Relaxed);
            }
        }
        if skipped.is_empty() && pending.is_empty() { break; }
        // files remain skipped: wait for the completion thread to load more
        loop {
            if stopping() { return; }
            std::thread::sleep(std::time::Duration::from_millis(200));
            let r = jit::front_read("typeck bg skipped");
            let (prog, _) = r.of(&f);
            if prog.skipped_files.is_empty() || prog.skipped_files.keys().map(|&m| prog.mods[m as usize].krate).collect::<HashSet<u32>>() != skipped { break; }
        }
    }
    finish(t0, w0)
}

fn finish(t0: u64, w0: u64) {
    STATS[3].store(relax_backend::cpu_ns() - t0, Ordering::Relaxed);
    STATS[4].store((jit::wall_ns() - w0).max(1), Ordering::Relaxed);
    if crate::run::VERBOSE.load(Ordering::Relaxed) {
        eprintln!("makepad-run: background type check done: {} body owners checked, {} with errors, {} crates' clean verdicts stored, cpu {:.0} ms over {:.1} s",
            STATS[0].load(Ordering::Relaxed), STATS[1].load(Ordering::Relaxed), STATS[2].load(Ordering::Relaxed),
            STATS[3].load(Ordering::Relaxed) as f64 / 1e6, STATS[4].load(Ordering::Relaxed) as f64 / 1e9);
    }
}

/// Type-checks body owner `d` of a crate this check covers and reports its errors; a body of any
/// other crate is only read when a build checked it already (the walk goes on through it).
fn check(f: &FrontShared, d: DefId, todo: &[bool], failed: &mut HashSet<u32>) -> Checked {
    loop {
        if stopping() { return Checked::Stop; }
        // the program's own compiles go first (a burst of the UI thread's lazy compiles)
        while !jit::tier1_workers_ready() {
            if stopping() { return Checked::Stop; }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let r = jit::front_read("typeck bg");
        let (prog, tcx) = r.of(f);
        if d.0 as usize >= prog.defs.len() || crate::borrowck::body::CheckedBody::item(prog, d).is_none() { return Checked::Done(Vec::new()); }
        let krate = prog.def(d).krate;
        if !todo.get(krate as usize).copied().unwrap_or(false) {
            return Checked::Done(tcx.typeck_peek(prog, d).filter(|b| b.errors.is_empty()).map(|b| named_fns(prog, &b)).unwrap_or_default());
        }
        let body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _p = crate::preempt::Preemptible::on();
            crate::preempt::point();
            tcx.typeck_unstored(prog, d)
        }));
        let body = match body {
            Ok(b) => b,
            // a front writer waits (a load, a patch), or the owner was busy with an earlier request:
            // checked again after it
            Err(p) if crate::preempt::is_preempted(&*p) || p.is::<crate::preempt::OwnerBusy>() => {
                drop(r);
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            }
            Err(p) if p.is::<crate::preempt::NeedsLoad>() && !prog.skipped_files.is_empty() => return Checked::Later,
            Err(_) => {
                // the checker failed on a body: a makepad run bug, reported like a build's
                // internal error, and the crate's verdict is not stored
                let e = format!("internal compiler error while checking {} -- makepad run bug, report it{}", prog.def_path(d), jit::take_ice_location());
                STATS[1].fetch_add(1, Ordering::Relaxed);
                failed.insert(krate);
                report(&[e]);
                return Checked::Done(Vec::new());
            }
        };
        STATS[0].fetch_add(1, Ordering::Relaxed);
        if body.errors.is_empty() { return Checked::Done(named_fns(prog, &body)); }
        // a name in a file the lazy selection skipped: checked again once it is loaded
        if !body.deferred_misses.is_empty() && !prog.skipped_files.is_empty() { return Checked::Later; }
        STATS[1].fetch_add(1, Ordering::Relaxed);
        failed.insert(krate);
        report(&body.errors);
        return Checked::Done(Vec::new());
    }
}

/// A body's errors: compile errors of code that has not run (it may never run).
fn report(errors: &[String]) {
    for e in errors {
        if crate::run_state::observed() { crate::run_state::diagnostic(crate::run::app_domain(), crate::diag::from_legacy(e, "typeck")); }
        crate::run::report("error", e);
        eprintln!("{}", e);
    }
}

/// The fns a checked body names: its paths, then its method calls, then its operators.
fn named_fns(prog: &Program, b: &Body) -> Vec<DefId> {
    let mut out: Vec<DefId> = b.res.values().filter_map(|r| match r {
        Res::Def(d) if matches!(prog.def(*d).kind, DefKind::Fn | DefKind::AssocFn) => Some(*d),
        _ => None,
    }).collect();
    out.extend(b.methods.values().chain(b.binops.values()).map(|m| m.def));
    out
}
