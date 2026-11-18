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
//!
//! `makepad test` runs it too (relax issue 231, item 7), over the registry crates the tests see
//! (their path dependencies have the strict test-deps check, check.rs start_test_deps): a body of
//! one that Relax cannot compile is a Relax gap, said once when found, never the run's exit code,
//! and stored (a Failed verdict) so a warm run checks nothing. A run no present will come to (a
//! test run, a headless program) loads the skipped files itself, one crate at a time: the
//! completion thread waits for a first present. A crate's check stores its progress file by file
//! (`Files`): a run that ends before a crate's verdict leaves the next run only the files not in.

use crate::jit::{self, FrontShared, Unit};
use crate::program::{DefId, DefKind, Program};
use crate::typeck::{Body, Res};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// body owners checked, owners with errors, crates whose verdict was stored, cpu ns, wall ns
/// from its start to its end (0 while it runs), files whose progress was stored, owners not checked
/// for their files' stored progress
pub static STATS: [AtomicU64; 7] = [const { AtomicU64::new(0) }; 7];

/// The check is under way (started and not ended): it fills the type side for every body owner of
/// the dependency crates, so the settle rebuild waits for its end. Rebuilt before it, the devhub
/// panel's compiler arena grew back from 0.55 to 1.05 GB committed (352 MB live, against 81 MB when
/// the rebuild came after it): signatures and declarations of 63k checked owners.
static RUNNING: AtomicBool = AtomicBool::new(false);

pub fn running() -> bool {
    RUNNING.load(Ordering::Acquire)
}

/// What a run's background check covers.
pub struct Scope {
    /// the program's crate (checked before main, not here)
    pub root: u32,
    /// where `makepad run`'s reachability walk starts
    pub main: Option<DefId>,
    /// a test run's subjects (empty: `makepad run`): only the registry crates they see are checked
    /// here, and the walk starts at their test fns
    pub test_subjects: Vec<u32>,
    /// per program crate: `` `name` version `` of a registry or git package, whose findings are Relax
    /// gaps (cargo builds it)
    pub registry: Vec<Option<String>>,
}

/// Starts the background check of unit `up`.
pub fn start(up: *mut Unit, scope: Scope) {
    if std::env::var("RELAX_BG_TYPECK").is_ok_and(|v| v == "0") { return; }
    let upn = up as usize;
    let _k = crate::zone_alloc::kept();
    RUNNING.store(true, Ordering::Release);
    // (typeck recurses deeply on big bodies: the check workers' stack)
    let spawned = crate::run_threads::spawn_sized("run-typeck-bg", 256 << 20, move || {
        let _z = crate::zone_alloc::compiler_thread();
        // ended however it ends (done, the run stopping, a panic)
        struct Ended;
        impl Drop for Ended {
            fn drop(&mut self) { RUNNING.store(false, Ordering::Release); }
        }
        let _ended = Ended;
        run(upn as *mut Unit, &scope);
    });
    if spawned.is_err() { RUNNING.store(false, Ordering::Release); }
}

/// The run stops, or quits: a crate's verdict is stored only once all of it is checked, and the
/// next launch checks what this one left
fn stopping() -> bool {
    crate::preempt::is_stopped() || crate::owner::exit_tid() != 0 || crate::jit::QUIT_SEEN.load(Ordering::Relaxed)
}

/// The crates this check covers -- not the app's (checked before main), not a proc-macro crate,
/// not the sysroot, in a test run only the registry crates the tests see, and no verdict stored
/// for it -- with their verdicts' cache keys; and the stored findings of the crates it skips.
fn todo_crates(u: &Unit, scope: &Scope, skipped: &[u128]) -> (Vec<bool>, Vec<Option<u128>>, Vec<(u32, usize)>) {
    use crate::borrowck::lazy::StoredVerdict;
    use crate::vfs::canonical as canon;
    let std_dir = format!("{}/", canon(&crate::std_dir()).trim_end_matches('/'));
    let prog = &u.prog;
    let mut findings = Vec::new();
    let (todo, keys) = (0..prog.crates.len() as u32).map(|k| {
        let c = &prog.crates[k as usize];
        if c.proc_macro || k == scope.root || crate::borrowck::event::app_crate(prog, k)
            || matches!(prog.syms.str(c.name), "core" | "alloc" | "std" | "proc_macro")
            || canon(c.root_path()).starts_with(&std_dir) {
            return (false, None);
        }
        if !scope.test_subjects.is_empty()
            && !(scope.registry.get(k as usize).is_some_and(|r| r.is_some()) && scope.test_subjects.iter().any(|&s| prog.crate_sees(s, k))) {
            return (false, None);
        }
        let Some(c) = u.cache.as_ref() else { return (true, None) };
        let c = unsafe { &mut *c.get() };
        match c.crate_check_key(k, skipped[k as usize], 0) {
            Some(key) => match c.borrowck_stored(key) {
                Some(StoredVerdict::Clean) => (false, None),
                Some(StoredVerdict::Failed(errors)) => {
                    findings.push((k, errors.len()));
                    (false, None)
                }
                _ => (true, Some(key)),
            },
            None => (true, None),
        }
    }).unzip();
    (todo, keys, findings)
}

enum Checked {
    /// clean (or no body owner, or one of a file whose progress is stored): the fns its body names
    Done(Vec<DefId>),
    /// it has errors, or the checker failed on it
    Failed,
    /// it waits on a file the lazy selection skipped: checked again once that is loaded
    Later,
    /// the run is ending
    Stop,
}

/// Where a body owner of a crate checked here stands in this run.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Owner {
    Clean,
    Failed,
    /// not checked yet (or it waits on a skipped file)
    Left,
}

/// A crate's check file by file (relax issue 231, item 7): a file whose every body owner this run
/// checked clean stores its progress once (cache.rs file_check_key), so a run that ends before its
/// crate's verdict (a crate of 10k owners is 2.5 s of checking, and a test run does not wait for
/// it) leaves the next one only the files not in yet. The crate's verdict still waits for all of it.
#[derive(Default)]
struct Files {
    /// per file of the crates checked here (the ones loaded so far): its entry's key, and whether
    /// the entry is stored (by an earlier run, or by this one)
    keys: HashMap<u32, (u128, bool)>,
    /// per file with an entry to come: its owners not checked clean yet
    left: HashMap<u32, usize>,
    /// files with an owner that has errors: no entry this run
    bad: HashSet<u32>,
}

impl Files {
    /// Whether the owners of `file` are still to check: not when its entry is stored.
    fn todo(&self, file: u32) -> bool {
        !self.keys.get(&file).is_some_and(|k| k.1)
    }

    /// A pass over the crates' owners (each with its file and where it stands): what each file has
    /// left. The keys of the files it finds complete (every owner checked clean before this pass).
    fn pass(&mut self, owners: impl Iterator<Item = (u32, Owner)>) -> Vec<u128> {
        self.left.clear();
        for (file, at) in owners {
            if !self.todo(file) || !self.keys.contains_key(&file) {
                continue;
            }
            let left = self.left.entry(file).or_insert(0);
            match at {
                Owner::Clean => {}
                Owner::Failed => { self.bad.insert(file); }
                Owner::Left => *left += 1,
            }
        }
        let complete: Vec<u32> = self.left.iter().filter(|(_, n)| **n == 0).map(|(f, _)| *f).collect();
        complete.into_iter().filter_map(|f| self.complete(f)).collect()
    }

    /// An owner of `file` checked: the file's key when that completes it clean (its entry to store).
    fn checked(&mut self, file: u32, clean: bool) -> Option<u128> {
        if !clean {
            self.bad.insert(file);
            return None;
        }
        let left = self.left.get_mut(&file)?;
        *left = left.saturating_sub(1);
        if *left == 0 { self.complete(file) } else { None }
    }

    /// Its entry, once: not for a file with errors, nor one stored already.
    fn complete(&mut self, file: u32) -> Option<u128> {
        if self.bad.contains(&file) {
            return None;
        }
        let k = self.keys.get_mut(&file)?;
        if k.1 {
            return None;
        }
        k.1 = true;
        Some(k.0)
    }
}

/// The progress keys of the files of `crates` (crate, its check key) not in `known` yet, and
/// whether each one's entry is stored (on the owner: the cache is the owner's).
fn file_keys(u: &Unit, crates: &[(u32, u128)], known: &HashSet<u32>) -> Vec<(u32, u128, bool)> {
    use crate::borrowck::lazy::StoredVerdict;
    let Some(c) = u.cache.as_ref() else { return Vec::new() };
    let c = unsafe { &mut *c.get() };
    let prog = &u.prog;
    let crates: HashMap<u32, u128> = crates.iter().copied().collect();
    let mut out = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    for m in prog.mods.iter() {
        let Some(&key) = crates.get(&m.krate) else { continue };
        if known.contains(&m.file) || !seen.insert(m.file) {
            continue;
        }
        let Some(path) = prog.file_paths.get(m.file as usize) else { continue };
        let fk = c.file_check_key(key, path);
        out.push((m.file, fk, matches!(c.borrowck_stored(fk), Some(StoredVerdict::Clean))));
    }
    out
}

/// No present will come to this run (a test run, a headless program): the completion thread,
/// which waits for one, loads no skipped file, so this check loads the ones it needs -- unless the
/// run loads none in the background (jit::background_loads): then, as in a windowed run, the check
/// waits for the program or an edit to load them.
fn no_present_coming() -> bool {
    jit::past_startup_headless() && !jit::FIRST_PRESENT.load(Ordering::Acquire) && jit::background_loads()
}

fn run(up: *mut Unit, scope: &Scope) {
    while !jit::past_startup() {
        if stopping() { return; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    makepad_cpu_budget::set_thread_class(makepad_cpu_budget::BACKGROUND);
    let (t0, w0) = (relax_session::clock::cpu_ns(), jit::wall_ns());
    let f = unsafe { (*up).front.clone() };
    // the crates' skipped files read here, the keys made on the owner (the cache is the owner's)
    let n = { let r = jit::front_read("typeck bg crates"); r.of(&f).0.crates.len() as u32 };
    let skipped_text: Vec<u128> = (0..n).map(crate::check::skipped_text_hash).collect();
    let upn = up as usize;
    let (todo, keys, stored_findings) = crate::owner::call(crate::owner::Class::Publish, || todo_crates(unsafe { &*(upn as *const Unit) }, scope, &skipped_text));
    *STORED_FINDINGS.lock().unwrap_or_else(|p| p.into_inner()) = stored_findings;
    if !todo.contains(&true) { return finish(t0, w0); }
    // the files' progress: the keys of the files loaded so far (more as skipped files load)
    let crate_keys: Vec<(u32, u128)> = keys.iter().enumerate().filter_map(|(k, key)| key.filter(|_| todo[k]).map(|key| (k as u32, key))).collect();
    let mut files = Files::default();
    let refresh = |files: &mut Files| {
        let known: HashSet<u32> = files.keys.keys().copied().collect();
        let ck = &crate_keys;
        let new = crate::owner::call(crate::owner::Class::Publish, || file_keys(unsafe { &*(upn as *const Unit) }, ck, &known));
        files.keys.extend(new.into_iter().map(|(file, key, stored)| (file, (key, stored))));
    };
    refresh(&mut files);
    // owners checked clean, and with errors (a file of one gets no entry)
    let mut clean: HashSet<DefId> = HashSet::new();
    let mut failed_owners: HashSet<DefId> = HashSet::new();
    let mut seen: HashSet<DefId> = HashSet::new();
    // per crate with errors: their text (stored with its verdict); `ice`: a checker panic's crate
    // (a Relax bug: no verdict stored, checked again next run)
    let mut failed: HashMap<u32, Vec<String>> = HashMap::new();
    let mut ice: HashSet<u32> = HashSet::new();
    let mut stored: HashSet<u32> = HashSet::new();
    // what main (a test run: its test fns) reaches, depth first (named_fns' order)
    let mut stack: Vec<DefId> = if scope.test_subjects.is_empty() {
        scope.main.into_iter().collect()
    } else {
        let r = jit::front_read("typeck bg tests");
        crate::harness::test_fns(r.of(&f).0, &scope.test_subjects)
    };
    while let Some(d) = stack.pop() {
        if !seen.insert(d) { continue; }
        match check(&f, d, &todo, &files, scope, &mut failed, &mut ice) {
            Checked::Done(named) => {
                clean.insert(d);
                stack.extend(named.into_iter().rev().filter(|d| !seen.contains(d)));
            }
            Checked::Failed => { failed_owners.insert(d); }
            Checked::Later => { seen.remove(&d); }
            Checked::Stop => return,
        }
    }
    // every other owner, smallest crate first (a short run completes whole crates); the files the
    // lazy selection skipped are loaded meanwhile (by the completion thread, or here when no
    // present will come): their owners are checked as they appear
    let todo_list: Vec<u32> = (0..todo.len() as u32).filter(|&k| todo[k as usize]).collect();
    // crates given up on: they wait on files no load brings (no verdict stored)
    let mut given_up: HashSet<u32> = HashSet::new();
    // a file's progress goes to the cache with the borrow checker's verdicts (not once a live
    // edit's versions are in the tables: the bodies checked were not all the boot's)
    let store_file = |key: u128| {
        if crate::reload::program_edits() == 0 {
            crate::borrowck::lazy::push_verdict_key(key, crate::borrowck::lazy::StoredVerdict::Clean);
            STATS[5].fetch_add(1, Ordering::Relaxed);
        }
    };
    loop {
        refresh(&mut files);
        let (owners, skipped) = {
            let r = jit::front_read("typeck bg owners");
            let (prog, _) = r.of(&f);
            let all: Vec<Vec<DefId>> = todo_list.iter().map(|&k| crate::check::body_owners(prog, &[k])).collect();
            let at = |d: &DefId| if clean.contains(d) { Owner::Clean } else if failed_owners.contains(d) { Owner::Failed } else { Owner::Left };
            for key in files.pass(all.iter().flatten().map(|d| (prog.def(*d).file, at(d)))) {
                store_file(key);
            }
            let mut per_crate: Vec<Vec<(DefId, u32)>> = all.into_iter().map(|o| o.into_iter().filter(|d| !seen.contains(d)).map(|d| (d, prog.def(d).file)).collect()).collect();
            per_crate.sort_by_key(|o| o.len());
            let owners: Vec<(DefId, u32)> = per_crate.into_iter().flatten().collect();
            // the skipped files of the crates checked here (another crate's do not hold this check up)
            let skipped: HashSet<u32> = prog.skipped_files.keys().map(|&m| prog.mods[m as usize].krate).filter(|&k| todo.get(k as usize).copied().unwrap_or(false)).collect();
            (owners, skipped)
        };
        let mut pending: HashSet<u32> = HashSet::new();
        for (d, file) in owners {
            seen.insert(d);
            match check(&f, d, &todo, &files, scope, &mut failed, &mut ice) {
                Checked::Done(_) => {
                    clean.insert(d);
                    if let Some(key) = files.checked(file, true) { store_file(key); }
                }
                Checked::Failed => {
                    failed_owners.insert(d);
                    files.checked(file, false);
                }
                Checked::Later => {
                    seen.remove(&d);
                    let r = jit::front_read("typeck bg later");
                    pending.insert(r.of(&f).0.def(d).krate);
                }
                Checked::Stop => return,
            }
        }
        // a crate whose every body is loaded and checked clean: its verdict goes to the cache with
        // the borrow checker's (the run's next cache write). Not once a live edit's versions are in
        // the tables (a refused or undone one's too): the bodies checked were not all the boot's,
        // which the keys name.
        // A crate with errors stores them (a Failed verdict: a warm run checks it no more and says
        // nothing early; a call of the body reports it, jit.rs install_error).
        let boot = crate::reload::program_edits() == 0;
        for &k in &todo_list {
            if boot && !ice.contains(&k) && !given_up.contains(&k) && !skipped.contains(&k) && !pending.contains(&k) && stored.insert(k) {
                let verdict = match failed.get(&k) {
                    Some(errors) => crate::borrowck::lazy::StoredVerdict::Failed(errors.clone()),
                    None => crate::borrowck::lazy::StoredVerdict::Clean,
                };
                if let Some(key) = keys[k as usize] { crate::borrowck::lazy::push_verdict_key(key, verdict); }
                STATS[2].fetch_add(1, Ordering::Relaxed);
            }
        }
        if skipped.is_empty() && pending.is_empty() { break; }
        if no_present_coming() {
            // load what the check waits on, one crate at a time, behind the program's compiles: a
            // crate's own skipped files, then (a body that names an item of a skipped file of
            // another crate) the skipped files of the crates it depends on
            let next = {
                let r = jit::front_read("typeck bg load");
                let (prog, _) = r.of(&f);
                let all: HashSet<u32> = prog.skipped_files.keys().map(|&m| prog.mods[m as usize].krate).collect();
                skipped.iter().copied().min().or_else(|| {
                    let mut wanted: Vec<u32> = pending.iter().flat_map(|&k| crate::check::dependency_closure(prog, k)).filter(|k| all.contains(k)).collect();
                    wanted.sort_unstable();
                    wanted.first().copied()
                })
            };
            match next {
                Some(k) => {
                    while !jit::tier1_workers_ready() {
                        if stopping() { return; }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    if stopping() { return; }
                    unsafe { jit::load_skipped_of(up, &[k]) };
                }
                // no file left to load for them: they wait on nothing that will come
                None => {
                    given_up.extend(pending.iter().copied());
                    break;
                }
            }
            continue;
        }
        // files remain skipped: wait for the completion thread to load more
        loop {
            if stopping() { return; }
            std::thread::sleep(std::time::Duration::from_millis(200));
            let r = jit::front_read("typeck bg skipped");
            let (prog, _) = r.of(&f);
            let now: HashSet<u32> = prog.skipped_files.keys().map(|&m| prog.mods[m as usize].krate).filter(|&k| todo.get(k as usize).copied().unwrap_or(false)).collect();
            if now != skipped || no_present_coming() { break; }
        }
    }
    finish(t0, w0)
}

/// The crates a warm run skipped for their stored Failed verdicts, with their error counts
/// (`--verbose`'s done line).
static STORED_FINDINGS: relax_session::sync::Mutex<Vec<(u32, usize)>> = relax_session::sync::Mutex::new(Vec::new());

fn finish(t0: u64, w0: u64) {
    STATS[3].store(relax_session::clock::cpu_ns() - t0, Ordering::Relaxed);
    STATS[4].store((jit::wall_ns() - w0).max(1), Ordering::Relaxed);
    if crate::run::VERBOSE.load(Ordering::Relaxed) {
        let findings = STORED_FINDINGS.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let stored = if findings.is_empty() { String::new() } else {
            format!(", stored findings of {} crate{} ({} errors) not checked again", findings.len(), if findings.len() == 1 { "" } else { "s" }, findings.iter().map(|f| f.1).sum::<usize>())
        };
        let (files, skipped) = (STATS[5].load(Ordering::Relaxed), STATS[6].load(Ordering::Relaxed));
        let files = if files == 0 && skipped == 0 { String::new() } else {
            format!(", {} files' progress stored, {} owners not checked for their files' stored progress", files, skipped)
        };
        eprintln!("relax: background type check done: {} body owners checked, {} with errors, {} crates' verdicts stored{}{}, cpu {:.0} ms over {:.1} s",
            STATS[0].load(Ordering::Relaxed), STATS[1].load(Ordering::Relaxed), STATS[2].load(Ordering::Relaxed), stored, files,
            STATS[3].load(Ordering::Relaxed) as f64 / 1e6, STATS[4].load(Ordering::Relaxed) as f64 / 1e9);
    }
}

/// Type-checks body owner `d` of a crate this check covers and reports its errors; a body of any
/// other crate is only read when a build checked it already (the walk goes on through it).
fn check(f: &FrontShared, d: DefId, todo: &[bool], files: &Files, scope: &Scope, failed: &mut HashMap<u32, Vec<String>>, ice: &mut HashSet<u32>) -> Checked {
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
        // an earlier run checked every owner of its file clean (the file's progress entry)
        if !files.todo(prog.def(d).file) {
            STATS[6].fetch_add(1, Ordering::Relaxed);
            return Checked::Done(Vec::new());
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
            Err(p) => {
                // the checker failed on a body: a makepad run bug, reported like a build's
                // internal error, and the crate's verdict is not stored
                crate::hot_failure::checker_panic(&prog.def_path(d), &jit::payload_text(&*p));
                let e = format!("internal compiler error while checking {} -- Relax bug, report it{}", prog.def_path(d), jit::take_ice_location());
                STATS[1].fetch_add(1, Ordering::Relaxed);
                ice.insert(krate);
                report(&[e], None);
                return Checked::Failed;
            }
        };
        STATS[0].fetch_add(1, Ordering::Relaxed);
        if body.errors.is_empty() { return Checked::Done(named_fns(prog, &body)); }
        // a name in a file the lazy selection skipped: checked again once it is loaded
        if !body.deferred_misses.is_empty() && !prog.skipped_files.is_empty() { return Checked::Later; }
        STATS[1].fetch_add(1, Ordering::Relaxed);
        failed.entry(krate).or_default().extend(body.errors.iter().cloned());
        report(&body.errors, scope.registry.get(krate as usize).and_then(|r| r.as_deref()));
        return Checked::Failed;
    }
}

/// A body's errors: compile errors of code that has not run (it may never run), each once in the
/// process (a call of the body may have reported it already, run.rs first_report). A registry
/// crate's (`registry`: its `` `name` version ``) are a Relax gap: cargo builds that crate, so the
/// run says so and they are warnings, never an error the run's exit counts.
fn report(errors: &[String], registry: Option<&str>) {
    let new: Vec<&String> = errors.iter().filter(|e| crate::run::first_report(0, e)).collect();
    if new.is_empty() { return; }
    if let Some(label) = registry {
        eprintln!("relax: {label}: a body Relax cannot compile (if cargo builds it, a Relax gap: report it)");
    }
    for e in new {
        if crate::run_state::observed() {
            let mut d = crate::diag::from_legacy(e, "typeck");
            if registry.is_some() { d.severity = crate::diag::Severity::Warning; }
            crate::run_state::diagnostic(crate::run::app_domain(), d);
        }
        crate::run::report(if registry.is_some() { "warning" } else { "error" }, e);
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

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&STATS, [const { AtomicU64::new(0) }; 7]);
    reset(&RUNNING, AtomicBool::new(false));
    reset(&STORED_FINDINGS, relax_session::sync::Mutex::new(Vec::new()));
}

#[cfg(test)]
mod tests {
    use super::{Files, Owner};

    /// A file's progress (relax issue 231, item 7): stored once its every owner is checked clean,
    /// never for a file with errors, never twice; a stored file's owners are not checked again; a
    /// file whose owners were all checked before a pass completes at that pass; and the key names
    /// both the crate's key and the file.
    #[test]
    fn a_files_progress_is_stored_once_when_its_every_owner_is_clean() {
        let (a, b, c) = (1u32, 2u32, 3u32);
        let key = |k: u128, path: &str| crate::cache::file_check_key(k, path);
        let mut files = Files::default();
        files.keys.insert(a, (key(7, "src/a.rs"), false));
        files.keys.insert(b, (key(7, "src/b.rs"), false));
        files.keys.insert(c, (key(7, "src/c.rs"), true));
        assert!(files.todo(a) && files.todo(b), "a file with no stored progress is checked");
        assert!(!files.todo(c), "a file whose progress is stored is not checked again");
        // a: two owners left; b: one checked clean before the pass (the reachability walk), one left
        let none = files.pass([(a, Owner::Left), (a, Owner::Left), (b, Owner::Clean), (b, Owner::Left), (c, Owner::Left)].into_iter());
        assert!(none.is_empty(), "no file is complete before its owners are checked: {none:?}");
        assert_eq!(files.checked(a, true), None, "a file with an owner left is not complete");
        assert_eq!(files.checked(a, true), Some(key(7, "src/a.rs")), "a file's last clean owner completes it");
        assert_eq!(files.checked(a, true), None, "a file's progress is stored once");
        assert_eq!(files.checked(b, false), None, "an owner with errors stores nothing");
        assert_eq!(files.pass([(b, Owner::Failed), (b, Owner::Clean)].into_iter()), Vec::<u128>::new(), "a file with errors never completes");
        // a file whose owners were all checked clean before the pass comes back from it
        let d = 4u32;
        files.keys.insert(d, (key(7, "src/d.rs"), false));
        assert_eq!(files.pass([(d, Owner::Clean), (d, Owner::Clean)].into_iter()), vec![key(7, "src/d.rs")], "a file complete before the pass completes at it");
        assert!(!files.todo(d), "a completed file is not checked again in this run either");
        assert_ne!(key(7, "src/a.rs"), key(8, "src/a.rs"), "another crate key (an edit, another Relax build) is another entry");
        assert_ne!(key(7, "src/a.rs"), key(7, "src/b.rs"), "each file has its own entry");
    }
}
