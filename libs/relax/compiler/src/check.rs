//! `makepad check`: the app check, like `cargo check`: every function of the package's workspace
//! crates (every loaded crate whose sources are in the workspace, std excepted) is type- and
//! borrow-checked (generic ones polymorphically, as rustc checks generic bodies once); nothing
//! is compiled or run. Errors print as one line each; the exit code is 0 when there are none.
//!
//! `makepad selftest`: a test of Relax itself on a package -- the same checks, then every
//! non-generic function is compiled to machine code by the run's own per-function compile
//! (`jit::compile_slot`), and so is every instance, closure and glue those reach (rustc's
//! monomorphization collector for `build-pass`), on `RELAX_JOBS` threads. Every failure prints
//! as one line (`file:line:col: error|internal: ..`); `internal` marks a makepad run bug (a compiler
//! panic, an error without a user-facing code, or an error inside std or outside any crate).
//! `--root-only` and the watch session's deep check (`collect_deep`) cover the root
//! crate (and what it reaches) only.

macro_rules! eprintln {
    ($($arg:tt)*) => { if !crate::diag::enabled() { stderr_line!($($arg)*); } };
}

use relax_types::next_solver::{Goal, ParamEnvId};
use crate::jit::{self, FnKey, Unit};
use crate::program::{DefId, DefKind, Program};
use crate::tcx::Tcx;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// RELAX_CHECK_TIMES: one line per serial step of a check that took 50 ms or more.
struct StepTimes { on: bool, t: std::time::Instant, label: &'static str }
impl StepTimes {
    fn new(label: &'static str) -> Self {
        StepTimes { on: std::env::var_os("RELAX_CHECK_TIMES").is_some(), t: std::time::Instant::now(), label }
    }
    fn mark(&mut self, step: &str) {
        let dt = self.t.elapsed();
        if self.on && dt.as_millis() >= 50 { stderr_line!("relax check: {} {} {:.2} s", self.label, step, dt.as_secs_f64()); }
        self.t = std::time::Instant::now();
    }
}

fn check_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RELAX_CHECK_TRACE").is_some())
}

fn borrowck_unchecked() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RELAX_BORROWCK_UNCHECKED").is_some())
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

/// Whether a crate named `name` comes from a registry (the cargo home's or the run dir's
/// `registry/`): a published crate, which builds under rustc -- its error is most likely a
/// makepad run divergence, and the check asks for a report (run.rs).
pub fn published_crate(prog: &crate::program::Program, name: &str) -> bool {
    use crate::vfs::canonical as canon;
    let dirs = crate::dirs::registry_dirs().map(|d| format!("{}/", canon(&d.to_string_lossy()).trim_end_matches('/')));
    prog.crates.iter().any(|c| prog.syms.str(c.name) == name && dirs.iter().any(|d| canon(c.root_path()).starts_with(d.as_str())))
}

/// The crates a one-shot `makepad check` covers: the root and every loaded crate whose root file
/// is in the workspace, except std (RELAX_STD may live inside it) and proc-macro crates (their
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

/// The crates of Relax's std the program reads for its target (core, alloc, std and what the std
/// folder provides), proc_macro aside: the proc-macro world is this machine's.
pub fn std_crates(prog: &crate::program::Program) -> Vec<u32> {
    (0..prog.crates.len() as u32)
        .filter(|&k| std_crate(prog, k) && !prog.crates[k as usize].proc_macro && prog.syms.str(prog.crates[k as usize].name) != "proc_macro")
        .collect()
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

/// The audit compares resolved types and names across runs, using indices local to the body.
pub(crate) fn typeck_digest(prog: &crate::program::Program, tcx: &crate::tcx::Tcx, body: &crate::typeck::Body) -> u128 {
    use crate::typeck::{ty_to_string, Res};
    let mut h = crate::sip::Sip128::new();
    for (types, typed) in [(&body.expr_ty, &body.expr_typed), (&body.pat_ty, &body.pat_typed)] {
        for (i, &ty) in types.iter().enumerate() {
            if typed.get(i) {
                h.u8(1);
                h.u64(i as u64);
                h.str(&ty_to_string(true, prog, tcx, ty));
            }
        }
        h.u8(0);
    }
    // Side tables are keyed by ExprId in the file's arena, including lazily parsed bodies.
    let index = |key| body.expr_ix(crate::ast::ExprId(key)) as u64;
    h.u64(body.res.len() as u64);
    for (&key, &res) in &body.res {
        h.u64(index(key));
        match res {
            Res::Def(d) => { h.u8(0); h.str(&prog.def_path(d)); }
            Res::Local(n) => { h.u8(1); h.u32(n); }
            Res::ConstParam(_) => h.u8(2),
            Res::Err => h.u8(3),
        }
    }
    for table in [&body.methods, &body.binops] {
        h.u64(table.len() as u64);
        for (&key, m) in table {
            h.u64(index(key));
            h.str(&prog.def_path(m.def));
            h.u32(m.derefs);
            h.u8(m.autoref);
            let args = tcx.tys.list(m.args);
            h.u64(args.len() as u64);
            for &ty in args { h.str(&ty_to_string(true, prog, tcx, ty)); }
        }
    }
    h.u64(body.errors.len() as u64);
    h.finish128()
}

/// RELAX_CHECK_AUDIT's folder, read once per run (reset_statics: a run in place reads it again).
static AUDIT_DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

pub(crate) fn audit_dir() -> Option<&'static Path> {
    AUDIT_DIR.get_or_init(|| {
        let _keep = crate::zone_alloc::kept();
        std::env::var_os("RELAX_CHECK_AUDIT").map(PathBuf::from)
    }).as_deref()
}

pub(crate) type AuditReads = relax_session::sync::Mutex<Vec<(DefId, DefId)>>;

/// One attempt at a body check. An unwind records its reads before the worker retries.
pub(crate) struct ReadBarrier<'a> {
    owner: DefId,
    reads: &'a AuditReads,
}

impl<'a> ReadBarrier<'a> {
    pub(crate) fn begin(prog: &Program, owner: DefId, reads: &'a AuditReads) -> Self {
        relax_types::type_queries::read_barrier_begin(prog.def(owner).krate);
        Self { owner, reads }
    }
}

impl Drop for ReadBarrier<'_> {
    fn drop(&mut self) {
        let reads = relax_types::type_queries::read_barrier_end();
        if !reads.is_empty() {
            self.reads.lock().unwrap_or_else(|p| p.into_inner()).extend(reads.into_iter().map(|r| (self.owner, r)));
        }
    }
}

#[derive(Default)]
struct CrateAudit {
    hit: Option<u128>,
    // Definition order breaks ties between equal printed paths, regardless of worker order.
    bodies: BTreeMap<DefId, (String, u128)>,
    holes: BTreeMap<(DefId, DefId), (String, String)>,
    incomplete: Vec<String>,
}

/// A printed impl path can name several bodies. Compare all its rows in definition order.
fn differing_body_paths<'a>(old: &'a str, current: &'a str) -> Vec<&'a str> {
    let mut paths = BTreeMap::<&str, [Vec<&str>; 2]>::new();
    for (i, snapshot) in [old, current].into_iter().enumerate() {
        for line in snapshot.lines() {
            let (path, digest) = line.rsplit_once('\t').unwrap_or((line, ""));
            paths.entry(path).or_default()[i].push(digest);
        }
    }
    paths.into_iter().filter_map(|(path, [before, now])| (before != now).then_some(path)).collect()
}

/// Shared by the app and background checks. Only owned strings and digests reach the file I/O.
pub(crate) struct CheckAudit {
    dir: &'static Path,
    pub(crate) reads: std::sync::Arc<AuditReads>,
    crates: HashMap<u32, CrateAudit>,
    files: HashMap<usize, crate::cache::FileDigest>,
    checked: usize,
    audited: usize,
    failed: usize,
    missing: usize,
    hole_reads: usize,
}

impl CheckAudit {
    pub(crate) fn new(dir: &'static Path) -> Self {
        Self {
            dir, reads: Default::default(), crates: HashMap::new(), files: HashMap::new(),
            checked: 0, audited: 0, failed: 0, missing: 0, hole_reads: 0,
        }
    }

    pub(crate) fn hit(&mut self, k: u32, key: u128) {
        self.crates.entry(k).or_default().hit = Some(key);
    }

    pub(crate) fn body(&mut self, prog: &Program, tcx: &crate::tcx::Tcx, d: DefId, body: &crate::typeck::Body) {
        let krate = self.crates.entry(prog.def(d).krate).or_default();
        let path = prog.def_path(d);
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| typeck_digest(prog, tcx, body))) {
            Ok(digest) => { krate.bodies.insert(d, (path, digest)); }
            Err(_) => krate.incomplete.push(format!("could not digest {path}")),
        }
    }

    pub(crate) fn incomplete(&mut self, k: u32, reason: String) {
        self.crates.entry(k).or_default().incomplete.push(reason);
    }

    pub(crate) fn collect_reads(&mut self, prog: &Program) {
        let mut reads = std::mem::take(&mut *self.reads.lock().unwrap_or_else(|p| p.into_inner()));
        reads.sort_unstable();
        reads.dedup();
        for (d, r) in reads {
            let Some(krate) = self.crates.get_mut(&prog.def(d).krate).filter(|c| c.hit.is_some()) else { continue };
            let def = prog.def(r);
            let f = def.file as usize;
            let digest = self.files.entry(f).or_insert_with(|| crate::cache::file_digest(&prog.files[f]));
            if crate::cache::owner_body_hash(&prog.files[f], digest, def.item).is_some() {
                krate.holes.entry((d, r)).or_insert_with(|| (prog.def_path(d), prog.def_path(r)));
            }
        }
    }

    /// Compare the previous interface-key snapshot before replacing either key. A missing
    /// baseline seeds the next replay. Return whether the check may publish a Clean verdict.
    pub(crate) fn verdict<'a>(
        &mut self, k: u32, name: &str, keys: impl Iterator<Item = u128>, errors: impl Iterator<Item = &'a str>,
    ) -> bool {
        use std::fmt::Write;
        let krate = self.crates.remove(&k).unwrap_or_default();
        self.checked += 1;
        let mut failed = false;
        let mut fail = |what: &str| {
            stderr_line!("relax: AUDIT FAIL: crate {name}: {what}");
            failed = true;
        };
        let mut bodies: Vec<_> = krate.bodies.into_values().collect();
        bodies.sort_by(|a, b| a.0.cmp(&b.0));
        let mut text = String::new();
        for (path, digest) in bodies { writeln!(text, "{path}\t{digest:032x}").unwrap(); }
        if let Some(key) = krate.hit {
            self.audited += 1;
            for error in errors { fail(error); }
            self.hole_reads += krate.holes.len();
            for (checked, read) in krate.holes.into_values() { fail(&format!("{checked} read {read}")); }
            match std::fs::read_to_string(self.dir.join(format!("{key:032x}"))) {
                Ok(old) if krate.incomplete.is_empty() && old != text => {
                    for path in differing_body_paths(&old, &text) {
                        fail(&format!("body digest differs: {path}"));
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    self.missing += 1;
                    stderr_line!("relax: AUDIT: crate {name}: no digest stored for {key:032x}");
                }
                Err(e) => fail(&format!("cannot read digest {key:032x}: {e}")),
            }
        }
        if krate.incomplete.is_empty() {
            for key in keys {
                let result = std::fs::create_dir_all(self.dir)
                    .and_then(|()| crate::pack::write_atomic(self.dir, &format!("{key:032x}"), text.as_bytes()));
                if let Err(e) = result { fail(&format!("cannot write digest {key:032x}: {e}")); }
            }
        } else {
            for error in krate.incomplete { fail(&error); }
        }
        self.failed += usize::from(failed);
        !failed
    }

    pub(crate) fn summary(&self, suffix: &str) {
        stderr_line!("relax: check audit{suffix}: {} crates checked, {} by an interface key hit audited, {} failed, {} without a stored digest, {} hole reads",
            self.checked, self.audited, self.failed, self.missing, self.hole_reads);
    }
}

pub(crate) struct ItemErrors {
    errors: Vec<String>,
    /// Session diagnostics can be emitted just once, e.g. when the coherence query finds E0119.
    /// A retry of the definition can read the completed query without emitting them again.
    reported: Vec<String>,
}

impl ItemErrors {
    pub(crate) fn messages(mut self) -> Vec<String> {
        self.errors.append(&mut self.reported);
        self.errors
    }
}

fn with_item_diagnostics(prog: &Program, tcx: &Tcx, check: impl FnOnce() -> Vec<String>) -> ItemErrors {
    let cp = tcx.dcx.checkpoint();
    let errors = check();
    let (reported, warnings) = tcx.dcx.take_since(cp);
    let reported = reported.into_iter().map(|error| {
        let cause = error.cause;
        if cause.body_id == crate::program::NO_DEF { error.message }
        else { prog.record_error(prog.def(cause.body_id).file, cause.span, "typeck", error.message) }
    }).collect();
    for w in warnings { stderr_line!("warning: {}", w.message); }
    ItemErrors { errors, reported }
}

pub(crate) fn def_item_errors(prog: &Program, tcx: &Tcx, d: DefId) -> ItemErrors {
    with_item_diagnostics(prog, tcx, || {
        let mut errors = tcx.check_def_signature(prog, d);
        if prog.def(d).kind == DefKind::Impl {
            errors.extend(relax_types::unsafety::check_impl(prog, tcx, d));
            // The overlap check is a query: its E0119 went to whichever thread asked it first (a
            // build, the background coherence check). Asked again uncached, this check's own
            // diagnostics hold it, so an incoherent impl never gets a clean item verdict.
            if !tcx.impl_trait_coherent(prog, d) {
                if let Some(&td) = tcx.impl_trait.get(&d) {
                    relax_types::next_solver::specialization_graph::specialization_graph(tcx, prog, prog.def(d).krate, td);
                }
            }
        }
        errors
    })
}

pub(crate) fn def_wf_errors(prog: &Program, tcx: &Tcx, d: DefId) -> ItemErrors {
    with_item_diagnostics(prog, tcx, || relax_types::wfcheck::obligations::check_def(prog, tcx, d))
}

/// The paths of the files of `crates`.
fn crate_paths<'p>(prog: &'p Program, crates: &[u32]) -> std::collections::HashSet<&'p str> {
    prog.file_paths.iter().zip(&prog.file_crate).filter(|(_, k)| crates.contains(k)).map(|(p, _)| p.as_str()).collect()
}

/// The loader's name resolution errors in files belonging to the selected crates.
pub(crate) fn crate_resolve_errors(prog: &Program, crates: &[u32]) -> Vec<String> {
    if prog.errors.is_empty() { return Vec::new(); }
    let paths = crate_paths(prog, crates);
    prog.errors.iter().filter(|e| paths.contains(error_position(e).0.as_str())).cloned().collect()
}

/// The loader's name resolution errors in no file of `crates` (a check's crates: it prints
/// their errors, crate_resolve_errors, with its own).
pub(crate) fn other_resolve_errors<'p>(prog: &'p Program, crates: &[u32]) -> Vec<&'p String> {
    let paths = crate_paths(prog, crates);
    prog.errors.iter().filter(|e| !paths.contains(error_position(e).0.as_str())).collect()
}

/// The item-level errors of `crates` for a failed build. Check well-formedness once all the
/// signatures are clean, then include resolution errors and drop follow-ons as the app check does.
pub(crate) fn item_errors(prog: &Program, tcx: &Tcx, crates: &[u32]) -> Vec<String> {
    let defs = prog.defs.iter().enumerate().filter(|(_, def)| crates.contains(&def.krate))
        .map(|(i, _)| DefId(i as u32));
    let mut errors: Vec<_> = defs.clone().flat_map(|d| def_item_errors(prog, tcx, d).messages()).collect();
    if errors.is_empty() {
        errors.extend(defs.flat_map(|d| def_wf_errors(prog, tcx, d).messages()));
    }
    errors.extend(crate_resolve_errors(prog, crates));
    errors.sort_by_cached_key(|e| error_position(e));
    errors.dedup();
    if errors.iter().any(|e| !is_follow_on(e)) { errors.retain(|e| !is_follow_on(e)); }
    errors
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

pub(crate) fn boot_skipped_paths(c: u32) -> &'static [String] {
    BOOT_SKIPPED.get().and_then(|v| v.get(c as usize)).map_or(&[], Vec::as_slice)
}

/// The text of crate `c`'s files the boot skipped (BOOT_SKIPPED: path and content; 0: none).
/// The selection read them already: they are in the OS's file cache.
pub(crate) fn skipped_text_hash(c: u32) -> u128 {
    let paths = boot_skipped_paths(c);
    if paths.is_empty() { return 0; }
    let mut h = crate::sip::Sip128::new();
    for p in paths {
        h.str(&crate::dirs::anchored(p));
        // (an unreadable file: its absence is the input, its load reports it)
        h.u128(crate::vfs::read(p).map_or(0, |src| crate::crate_image::file_hash(p, &src)));
    }
    h.finish128()
}

/// The interface keys' own text of each crate of `crates` (cache.rs dep_check_key, app_check_key): the
/// crate as rustc is invoked on it and the text of the files the boot loaded (as its fingerprint
/// hashes them, without its dependencies'; skipped_text_hash has the others).
fn own_text_hashes(prog: &crate::program::Program, crates: &[u32]) -> Vec<u128> {
    if crates.is_empty() { return Vec::new(); }
    let mut per: Vec<Option<crate::sip::Sip128>> = vec![None; prog.crates.len()];
    for &c in crates {
        let ci = &prog.crates[c as usize];
        let mut h = crate::crate_image::fingerprint_head(prog.syms.str(ci.name), ci.root_path(), ci.edition, ci.proc_macro, &ci.cfg.set);
        h.str("own-text");
        h.u128(ci.context.fingerprint());
        per[c as usize] = Some(h);
    }
    let mut inputs = Vec::new();
    let mut hashes = Vec::new();
    for f in 0..prog.boot_files.min(prog.files.len()) {
        if prog.front_macro_fragments.contains(&f) { continue; }
        let Some(&c) = prog.file_crate.get(f) else { continue };
        let c = c as usize;
        if per.get(c).and_then(Option::as_ref).is_none() { continue; }
        let pf = &prog.files[f];
        let path = prog.file_paths.get(f).map_or("", |p| p.as_str());
        inputs.push((path, &pf.src[..(pf.src_len as usize).min(pf.src.len())]));
        hashes.push((c, 0u128));
    }
    if !inputs.is_empty() {
        let threads = std::thread::available_parallelism().map_or(4, |x| x.get()).clamp(1, 8);
        let chunk = inputs.len().div_ceil(threads);
        crate::pool::scope(|s| {
            for (out, inputs) in hashes.chunks_mut(chunk).zip(inputs.chunks(chunk)) {
                s.spawn(move || {
                    for ((_, hash), &(path, src)) in out.iter_mut().zip(inputs) {
                        *hash = crate::crate_image::file_hash(path, src);
                    }
                });
            }
        });
    }
    for (c, hash) in hashes {
        per[c].as_mut().unwrap().u128(hash);
    }
    per.into_iter().map(|h| h.map_or(0, |h| h.finish128())).collect()
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

/// What the body and item interface keys hash beside the interfaces: own text for the body
/// lookups (by crate index), each selected crate's dependency closure, each closure crate's check extra
/// (by crate index; 0 for a crate no closure holds) and the text of the skipped files of every
/// crate named (the crates and their closures).
pub(crate) struct IfaceInputs {
    pub own: Vec<u128>,
    pub closures: Vec<Vec<u32>>,
    pub extras: Vec<u128>,
    pub skipped: std::collections::HashMap<u32, u128>,
}

impl IfaceInputs {
    /// The dependency terms of the key for the crate at position `i` in the selection.
    pub(crate) fn deps(&self, i: usize) -> impl Iterator<Item = (u32, u128, u128)> + '_ {
        self.closures[i].iter().map(move |d| (*d, self.skipped[d], self.extras[*d as usize]))
    }
}

pub(crate) fn iface_inputs(prog: &crate::program::Program, crates: &[u32]) -> IfaceInputs {
    iface_inputs_with_skipped(prog, crates, crates, &[])
}

/// Share closure inputs across both lookups, hashing whole crate text only for body misses.
/// The background check also supplies the skipped text hashes it read for its fingerprint keys.
pub(crate) fn iface_inputs_with_skipped(prog: &crate::program::Program, crates: &[u32], body_crates: &[u32], skipped_text: &[u128]) -> IfaceInputs {
    let own = own_text_hashes(prog, body_crates);
    let closures: Vec<Vec<u32>> = crates.iter().map(|&k| dependency_closure(prog, k)).collect();
    let mut want = vec![false; prog.crates.len()];
    for &d in closures.iter().flatten() { want[d as usize] = true; }
    let extras = crate::cache::crate_check_extras(prog, &want);
    let mut skipped = std::collections::HashMap::new();
    for &k in crates.iter().chain(closures.iter().flatten()) {
        skipped.entry(k).or_insert_with(|| skipped_text.get(k as usize).copied().unwrap_or_else(|| skipped_text_hash(k)));
    }
    IfaceInputs { own, closures, extras, skipped }
}

/// The two keys for a crate whose item check is pending. Only a Clean verdict skips the work.
#[derive(Default)]
pub(crate) struct ItemKeys {
    pub(crate) fp: Option<u128>,
    iface: Option<u128>,
    files_at_lookup: usize,
}

impl ItemKeys {
    pub(crate) fn missed(cache: &mut crate::cache::Cache, k: u32, skipped: u128) -> Option<Self> {
        let fp = cache.crate_check_key(k, skipped, crate::cache::ITEMS_KEY_MODE);
        let clean = matches!(fp.and_then(|key| cache.borrowck_stored(key)), Some(crate::borrowck::lazy::StoredVerdict::Clean));
        (!clean).then_some(Self { fp, ..Self::default() })
    }

    pub(crate) fn iface_hit(&mut self, cache: &mut crate::cache::Cache, u: &Unit, k: u32, inputs: &IfaceInputs, i: usize) -> bool {
        self.files_at_lookup = u.prog.files.len();
        self.iface = cache.own_items(u, k).and_then(|own| cache.items_check_key(k, own, inputs.skipped[&k], inputs.deps(i)));
        if self.iface.is_none() { self.fp = None; }
        matches!(self.iface.and_then(|key| cache.borrowck_stored(key)), Some(crate::borrowck::lazy::StoredVerdict::Clean))
    }

    pub(crate) fn keys(self, prog: &Program, k: u32) -> impl Iterator<Item = u128> {
        // Loading a skipped module can discover a child file absent from the boot's skipped
        // list. The check saw its definitions, but neither key covered its text at lookup.
        let skipped = boot_skipped_paths(k);
        let covered = self.iface.is_some() && prog.file_crate.get(self.files_at_lookup..).is_some_and(|added| {
            added.iter().enumerate().all(|(i, &c)| c != k || prog.file_paths.get(self.files_at_lookup + i)
                .is_some_and(|path| skipped.binary_search(path).is_ok()))
        });
        self.fp.into_iter().chain(self.iface).filter(move |_| covered)
    }
}

/// Check bodies before main on RELAX_JOBS threads, keeping their types for later builds.
/// A run also checks the app's items. The target std check uses only the body pass.
pub fn run_check_crates(up: *mut Unit, crates: &[u32], check_items: bool) -> Vec<String> {
    match prepare_check(up, crates, check_items) {
        Some(p) => check_prepared(up, p, None, CheckMode::Typeck).strings(),
        None => Vec::new(),
    }
}

/// The uncached body and item checks, with their verdict keys. The boot thread loads the skipped
/// files of every crate selected for either check before collecting its owners.
pub struct AppCheck {
    t0: u64,
    todo: Vec<u32>,
    keys: Vec<(u32, u128)>,
    owners: Vec<crate::program::DefId>,
    audited: Vec<(u32, u128)>,
    items: HashMap<u32, ItemKeys>,
    /// The crates of the run's item check (empty without one): the crate-level checks' scope
    scope: Vec<u32>,
}

/// The app check's lookups and loads (the cache is read here: before main, on the boot thread).
/// Returns None when both selections are clean. Fingerprint hits need no interface inputs
/// or file digests. On a miss both lookups share the dependency closure and skipped text.
///
/// `RELAX_CHECK_AUDIT=<folder>` rechecks Clean body interface-key hits in the app and background
/// checks. Each must still check clean, request no type check of a dependency body omitted
/// from its interface, and match the stored per-body digest of resolved types, paths and
/// methods. Snapshots live under both verdict keys in the folder. A missing snapshot seeds
/// the next comparison.
pub fn prepare_check(up: *mut Unit, crates: &[u32], check_items: bool) -> Option<AppCheck> {
    use crate::borrowck::lazy::StoredVerdict;
    use std::sync::atomic::Ordering::Relaxed;
    let u = unsafe { &*up };
    let t0 = jit::wall_ns();
    RUN_CHECK[7].store(0, Relaxed);
    let mut keys = Vec::new();
    let mut todo = Vec::new();
    let mut items = HashMap::new();
    let mut audited = Vec::new();
    let (mut iface_hits, mut iface_ns) = (0, 0);
    if let Some(c) = &u.cache {
        let cache = unsafe { &mut *c.get() };
        let mut misses = Vec::new();
        for &k in crates {
            let skipped = skipped_text_hash(k);
            let fp_key = cache.crate_check_key(k, skipped, 0);
            let body_missed = !matches!(fp_key.and_then(|key| cache.borrowck_stored(key)), Some(StoredVerdict::Clean));
            if check_items {
                if let Some(keys) = ItemKeys::missed(cache, k, skipped) { items.insert(k, keys); }
            }
            if body_missed || items.contains_key(&k) {
                misses.push((k, fp_key, skipped, body_missed));
            }
        }
        if !misses.is_empty() {
            let t = jit::wall_ns();
            let missed_crates: Vec<u32> = misses.iter().map(|m| m.0).collect();
            let body_crates: Vec<_> = misses.iter().filter(|m| m.3).map(|m| m.0).collect();
            let inputs = iface_inputs_with_skipped(&u.prog, &missed_crates, &body_crates, &[]);
            for (i, (k, fp_key, skipped, body_missed)) in misses.into_iter().enumerate() {
                // A skipped file changed between the two reads. Check the crate without caching it.
                if inputs.skipped[&k] != skipped {
                    if body_missed { todo.push(k); }
                    if let Some(keys) = items.get_mut(&k) { *keys = ItemKeys::default(); }
                    continue;
                }
                if let Some(keys) = items.get_mut(&k) {
                    if keys.iface_hit(cache, u, k, &inputs, i) {
                        if let Some(fp) = keys.fp { cache.record_borrowck(fp, StoredVerdict::Clean); }
                        items.remove(&k);
                    }
                }
                if !body_missed { continue; }
                let iface_key = cache.app_check_key(k, inputs.own[k as usize], skipped, inputs.deps(i));
                if let Some(key) = iface_key.filter(|&key| matches!(cache.borrowck_stored(key), Some(StoredVerdict::Clean))) {
                    iface_hits += 1;
                    if audit_dir().is_none() {
                        if let Some(key) = fp_key { cache.record_borrowck(key, StoredVerdict::Clean); }
                        continue;
                    }
                    audited.push((k, key));
                }
                todo.push(k);
                keys.extend(fp_key.map(|key| (k, key)));
                keys.extend(iface_key.map(|key| (k, key)));
            }
            iface_ns = jit::wall_ns() - t;
        }
    } else {
        todo.extend_from_slice(crates);
        if check_items { items.extend(crates.iter().map(|&k| (k, ItemKeys::default()))); }
    }
    let mut load = todo.clone();
    load.extend(items.keys().filter(|k| !todo.contains(k)).copied());
    RUN_CHECK[0].store(load.len() as u64, Relaxed);
    RUN_CHECK[5].store(iface_hits, Relaxed);
    RUN_CHECK[6].store(iface_ns, Relaxed);
    let t1 = jit::wall_ns();
    RUN_CHECK[3].store(t1 - t0, Relaxed);
    if load.is_empty() {
        RUN_CHECK[2].store(t1 - t0, Relaxed);
        if let Some(dir) = audit_dir() { CheckAudit::new(dir).summary(""); }
        return None;
    }
    unsafe { jit::load_skipped_of(up, &load) };
    RUN_CHECK[4].store(jit::wall_ns() - t1, Relaxed);
    let u = unsafe { &*up };
    let owners = if todo.is_empty() { Vec::new() } else { body_owners(&u.prog, &todo) };
    RUN_CHECK[1].store(owners.len() as u64, Relaxed);
    Some(AppCheck { t0, todo, keys, owners, audited, items, scope: if check_items { crates.to_vec() } else { Vec::new() } })
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum CheckKind { Body, Impl, Items, Wf }

impl CheckKind {
    fn is_item(self) -> bool { matches!(self, Self::Items | Self::Wf) }
}

/// A definition's check on a scoped thread or, beside a running program, the JIT's queue.
struct BodyCheck {
    upn: usize,
    front: std::sync::Arc<jit::FrontShared>,
    strict: bool,
    beside: bool,
    cpu: std::sync::atomic::AtomicU64,
    audit_reads: Option<std::sync::Arc<AuditReads>>,
    item_diagnostics: relax_session::sync::Mutex<Vec<(u32, String)>>,
}
impl BodyCheck {
    fn check(&self, d: crate::program::DefId, kind: CheckKind) -> Option<Vec<String>> {
        struct Cpu<'a>(&'a std::sync::atomic::AtomicU64, u64);
        impl Drop for Cpu<'_> {
            fn drop(&mut self) { self.0.fetch_add(relax_session::clock::cpu_ns() - self.1, std::sync::atomic::Ordering::Relaxed); }
        }
        let ice = |prog: &crate::program::Program, d: crate::program::DefId| -> Option<Vec<String>> {
            let def = prog.def(d);
            let pos = prog.files[def.file as usize].ast.item(def.item).lo;
            Some(vec![prog.record_error(def.file, pos, "typeck", format!("internal compiler error while checking {} -- Relax bug, report it{}", prog.def_path(d), jit::take_ice_location()))])
        };
        let retry = |p: &(dyn std::any::Any + Send)| crate::preempt::is_preempted(p) || p.is::<crate::preempt::NeedsLoad>() || p.is::<crate::preempt::OwnerBusy>();
        let (strict, beside) = (self.strict, self.beside);
        // (the app check beside main: its own counter, for the line at the first frame)
        let _cpu = if strict { Some(Cpu(&self.cpu, relax_session::clock::cpu_ns())) } else if beside { Some(Cpu(&APP_CHECK_CPU, relax_session::clock::cpu_ns())) } else { None };
        let _quiet = strict.then(crate::lint::Suppress::new);
        let uu = unsafe { &*(self.upn as *const Unit) };
        let mut tries = 0;
        loop {
            if crate::run_threads::run_ending() { return None; }
            let r = beside.then(|| jit::front_read("app check"));
            let (prog, tcx) = r.as_ref().map_or((&uu.prog, &uu.tcx), |r| r.of(&self.front));
            if !strict && tries >= 100 {
                if kind.is_item() {
                    let path = prog.def_path(d);
                    drop(r);
                    crate::typeck_bg::report_checker_panic(&path, "checker retries exhausted");
                    return None;
                }
                return if kind == CheckKind::Impl { None } else { ice(prog, d) };
            }
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _p = beside.then(crate::preempt::Preemptible::on);
                match kind {
                    CheckKind::Impl => return crate::unsafety::impl_errors(prog, tcx, d),
                    CheckKind::Items | CheckKind::Wf => {
                        crate::preempt::point();
                        let result = if kind == CheckKind::Items { def_item_errors(prog, tcx, d) } else { def_wf_errors(prog, tcx, d) };
                        if !result.reported.is_empty() {
                            self.item_diagnostics.lock().unwrap_or_else(|e| e.into_inner())
                                .extend(result.reported.into_iter().map(|e| (prog.def(d).krate, e)));
                        }
                        return Some(result.errors);
                    }
                    CheckKind::Body => {}
                }
                let _reads = self.audit_reads.as_deref().map(|reads| ReadBarrier::begin(prog, d, reads));
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
                    // a constant it asked for needs a skipped module, which the owner does not
                    // load for a request made under a front read (jit::load_deferred): loaded
                    // now that the read has ended
                    if beside && p.is::<crate::preempt::NeedsLoad>() { unsafe { jit::load_wanted(self.upn as *mut Unit) }; }
                    if beside {
                        // (a queued check's CPU permit goes back while the writer runs)
                        let _wait = crate::pool::suspend();
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    } else {
                        std::thread::yield_now();
                    }
                }
                Err(p) if kind.is_item() => {
                    if let Some(fatal) = p.downcast_ref::<relax_types::infer::diagnostics::FatalError>() {
                        return Some(vec![fatal.0.clone()]);
                    }
                    let path = prog.def_path(d);
                    drop(r);
                    crate::typeck_bg::report_checker_panic(&path, &jit::payload_text(&*p));
                    return None;
                }
                Err(_) => return if kind == CheckKind::Impl && !strict { None } else { ice(prog, d) },
            }
        }
    }
}

/// Checks the prepared bodies and, for the app, its definitions. Beside a running program each
/// job holds a front read, yields to writers and retries. Test dependencies use the body error pipeline.
fn check_prepared(up: *mut Unit, p: AppCheck, beside: Option<&crate::options::RuntimeOptions>, mode: CheckMode) -> PreparedErrors {
    let AppCheck { t0, todo, mut keys, owners, audited, mut items, scope } = p;
    let strict = mode == CheckMode::Errors;
    let front = unsafe { (*up).front.clone() };
    let mut audit = audit_dir().map(CheckAudit::new);
    let ctx = std::sync::Arc::new(BodyCheck {
        upn: up as usize, front: front.clone(), strict, beside: beside.is_some(), cpu: std::sync::atomic::AtomicU64::new(0),
        audit_reads: audit.as_ref().map(|a| a.reads.clone()),
        item_diagnostics: relax_session::sync::Mutex::new(Vec::new()),
    });
    let check_all = |defs: Vec<DefId>, kind: CheckKind| -> Vec<Option<Vec<String>>> {
        if defs.is_empty() { return Vec::new(); }
        if beside.is_some() { queued_map(&ctx, defs, kind) } else { par_map(&defs, jobs(), |&d| ctx.check(d, kind)) }
    };
    // A run's end stops the check: what it did not reach has no verdict, and no error.
    let run_ended = || PreparedErrors { errors: Vec::new(), cpu_ns: 0 };
    let skipped_left = || {
        let r = beside.map(|_| jit::front_read("app check skipped"));
        !r.as_ref().map_or(unsafe { &(*up).prog }, |r| r.of(&front).0).skipped_files.is_empty()
    };
    // the crates of the run's item check that are checked here (by their items or their bodies)
    let checked: Vec<bool> = if scope.is_empty() { Vec::new() } else {
        let r = beside.map(|_| jit::front_read("app check crates"));
        let prog = r.as_ref().map_or(unsafe { &(*up).prog }, |r| r.of(&front).0);
        let mut checked = vec![false; prog.crates.len()];
        for &k in items.keys().chain(&todo).filter(|k| scope.contains(k)) { checked[k as usize] = true; }
        checked
    };
    let mut item_errors = Vec::new();
    if mode == CheckMode::Typeck && !items.is_empty() {
        let mut defs: Vec<(u32, DefId)> = {
            let r = beside.map(|_| jit::front_read("app check item defs"));
            let prog = r.as_ref().map_or(unsafe { &(*up).prog }, |r| r.of(&front).0);
            prog.defs.iter().enumerate().filter(|(_, def)| items.contains_key(&def.krate))
                .map(|(d, def)| (def.krate, DefId(d as u32))).collect()
        };
        RUN_CHECK[7].store(defs.len() as u64, std::sync::atomic::Ordering::Relaxed);
        for kind in [CheckKind::Items, CheckKind::Wf] {
            let mut parts = check_all(defs.iter().map(|&(_, d)| d).collect(), kind);
            if crate::run_threads::run_ending() { return run_ended(); }
            let again: Vec<_> = parts.iter().enumerate().filter(|(_, p)| p.as_ref().is_some_and(|e| !e.is_empty())).map(|(i, _)| i).collect();
            if !again.is_empty() && skipped_left() {
                unsafe { jit::load_all_skipped(up) };
                let retried = check_all(again.iter().map(|&i| defs[i].1).collect(), kind);
                for (i, part) in again.into_iter().zip(retried) { parts[i] = part; }
            }
            for (&(k, _), part) in defs.iter().zip(parts) {
                match part {
                    Some(e) if e.is_empty() => {}
                    Some(e) => { items.remove(&k); item_errors.extend(e.into_iter().map(|e| (k, e))); }
                    None => { items.remove(&k); keys.retain(|(c, _)| *c != k); }
                }
            }
            for (k, e) in std::mem::take(&mut *ctx.item_diagnostics.lock().unwrap_or_else(|e| e.into_inner())) {
                items.remove(&k);
                item_errors.push((k, e));
            }
            if kind == CheckKind::Items { defs.retain(|(k, _)| items.contains_key(k)); }
        }
        if crate::run_threads::run_ending() { return run_ended(); }
    }
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
        let parts = check_all(impls.iter().map(|&(_, d)| d).collect(), CheckKind::Impl);
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
    let mut bodies: Vec<_> = owners.iter().zip(check_all(owners.clone(), CheckKind::Body))
        .filter_map(|(&d, e)| e.filter(|e| !e.is_empty()).map(|e| (d, e))).collect();
    if crate::run_threads::run_ending() { return run_ended(); }
    if (!bodies.is_empty() || !failed.is_empty()) && skipped_left() {
        unsafe { jit::load_all_skipped(up) };
        let again: Vec<_> = bodies.iter().map(|f| f.0).collect();
        bodies = again.iter().zip(check_all(again.clone(), CheckKind::Body))
            .filter_map(|(&d, e)| e.filter(|e| !e.is_empty()).map(|e| (d, e))).collect();
        let again: Vec<_> = failed.iter().map(|f| f.0).collect();
        failed = again.iter().zip(check_all(again.clone(), CheckKind::Impl))
            .filter_map(|(&d, e)| e.filter(|e| !e.is_empty()).map(|e| (d, e))).collect();
    }
    failed.extend(bodies);
    if crate::run_threads::run_ending() { return run_ended(); }
    // The crate-level checks (crate_level_errors, a check's own): no def's check makes them, and
    // what they find, rustc refuses. The crates whose items or bodies are checked here: an edit
    // to a body can bring what they look for (an attribute, a nested item). After the bodies: a
    // run parses a file's skipped bodies on their first need, and the gated syntax in them is
    // known once they are parsed.
    if mode == CheckMode::Typeck && !checked.is_empty() {
        let mut st = StepTimes::new("app check crate level");
        let root = crate::layout::root_crate();
        let refused = {
            let r = beside.map(|_| jit::front_read("app check crate level"));
            let u = unsafe { &*up };
            let (prog, tcx) = r.as_ref().map_or((&u.prog, &u.tcx), |r| r.of(&front));
            let mut all = vec![false; prog.crates.len()];
            for &k in &scope { all[k as usize] = true; }
            let binaries: Vec<u32> = scope.iter().copied().filter(|&k| prog.crates[k as usize].is_bin).collect();
            let entry = prog.crates.get(root as usize).is_some_and(|c| c.is_bin && !c.harness && all[root as usize]);
            // the imports rustc never binds (a check's are before its signatures: check_crate_impl)
            let strict = prog.strict_import_errors(&checked).into_iter().map(|(_, e)| e);
            strict.chain(crate_level_errors(prog, tcx, &checked, &all, &binaries, entry.then_some(root), &mut st).0.into_iter().map(|(_, e)| e)).collect::<Vec<String>>()
        };
        if !refused.is_empty() {
            // the crates' errors together: none of the crates checked here gets a clean verdict
            items.retain(|&k, _| !checked[k as usize]);
            keys.retain(|&(k, _)| !checked[k as usize]);
            item_errors.extend(refused.into_iter().map(|e| (root, e)));
        }
        if crate::run_threads::run_ending() { return run_ended(); }
    }
    let mut errors: Vec<(u32, String)> = {
        let r = beside.map(|_| jit::front_read("app check verdicts"));
        let prog = r.as_ref().map_or(unsafe { &(*up).prog }, |r| r.of(&front).0);
        failed.into_iter().flat_map(|(d, e)| { let k = prog.def(d).krate; e.into_iter().map(move |e| (k, e)) }).collect()
    };
    if let Some(audit) = audit.as_mut() {
        for (k, key) in audited { audit.hit(k, key); }
        let names: Vec<String> = {
            let r = beside.map(|_| jit::front_read("app check audit"));
            let u = unsafe { &*up };
            let (prog, tcx) = r.as_ref().map_or((&u.prog, &u.tcx), |r| r.of(&front));
            for d in owners {
                let body = tcx.typeck_peek(prog, d).or_else(|| {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let _reads = ReadBarrier::begin(prog, d, &audit.reads);
                        tcx.typeck(prog, d)
                    })).ok()
                });
                match body {
                    Some(body) => audit.body(prog, tcx, d, &body),
                    None => audit.incomplete(prog.def(d).krate, format!("could not check {} for its digest", prog.def_path(d))),
                }
            }
            audit.collect_reads(prog);
            todo.iter().map(|&k| prog.syms.str(prog.crates[k as usize].name).to_owned()).collect()
        };
        for (&k, name) in todo.iter().zip(names) {
            let crate_keys = keys.iter().filter(|(c, _)| *c == k).map(|&(_, key)| key);
            let crate_errors = errors.iter().filter(|(c, _)| *c == k).map(|(_, e)| e.as_str());
            if !audit.verdict(k, &name, crate_keys, crate_errors) { keys.retain(|(c, _)| *c != k); }
        }
        audit.summary("");
    }
    keys.retain(|(k, _)| !errors.iter().any(|(c, _)| c == k));
    errors.extend(item_errors);
    if !items.is_empty() && crate::reload::program_edits() == 0 {
        let r = beside.map(|_| jit::front_read("app item verdicts"));
        let prog = r.as_ref().map_or(unsafe { &(*up).prog }, |r| r.of(&front).0);
        keys.extend(items.into_iter().flat_map(|(k, item)| item.keys(prog, k).map(move |key| (k, key))));
    }
    let clean = keys.into_iter().map(|(_, key)| key);
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
/// the JIT's queue (jit_queue `Job::Check`): no threads of its own. Its jobs run last in Visible
/// during a UI app's boot check, otherwise last in Background. They yield to higher-priority
/// waiters between items. Once a test or the run's exit waits, they go
/// first in the waiter's class (`waited`). Each job takes the next item until its slice is used, then queues itself again: its
/// worker goes back to the queue's order every few milliseconds. (On threads of its own a test's
/// dependency check took every core beside a large app's cold start, first frame 27.5 s; on two of them
/// the start drew at 14.8 s, but the check ended 66 s after the tests.)
pub(crate) struct CheckBatch {
    ctx: std::sync::Arc<BodyCheck>,
    items: Vec<crate::program::DefId>,
    kind: CheckKind,
    next: std::sync::atomic::AtomicUsize,
    /// items whose result is not in `out` yet
    left: std::sync::atomic::AtomicUsize,
    /// its jobs queued or running
    jobs: std::sync::atomic::AtomicUsize,
    out: relax_session::sync::Mutex<Vec<(usize, Option<Vec<String>>)>>,
    done: (relax_session::sync::Mutex<()>, relax_session::sync::Condvar),
}

/// The boot check started for a program expected to present a frame.
static CHECK_EARLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Something waits for the check beside the program (`waited`).
static CHECK_WAITED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// CPU time (ns) of the app check's jobs for its verdict line.
static APP_CHECK_CPU: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// The check jobs beside the program: running now, most at once while something waited, slices
/// the OS raised to the waiter's class, raises refused, and the slices' total queue time (ns)
/// while something waited (a slice queued before the wait began counts from the wait's start).
static CHECK_SLICES: [std::sync::atomic::AtomicU64; 5] = [const { std::sync::atomic::AtomicU64::new(0) }; 5];
/// A job's slice: past it, the job gives its worker back to the queue after the item it is on.
const CHECK_SLICE: std::time::Duration = std::time::Duration::from_millis(20);

pub(crate) fn check_early() -> bool {
    CHECK_EARLY.load(std::sync::atomic::Ordering::Acquire) && !jit::past_startup_headless()
}

/// Something waits for the check beside the program: its jobs run first in the Interactive class.
pub(crate) fn check_waited() -> bool {
    CHECK_WAITED.load(std::sync::atomic::Ordering::Acquire)
}

/// When something began to wait for the check (wall ns; 0: nothing has).
static CHECK_WAITED_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A slice queued at `since` (wall ns) starts now: count only the time during the wait.
pub(crate) fn slice_queued(since: u64) {
    let waited_at = CHECK_WAITED_AT.load(std::sync::atomic::Ordering::Acquire);
    if waited_at == 0 { return; }
    let ns = crate::clock::wall_ns().saturating_sub(since.max(waited_at));
    CHECK_SLICES[4].fetch_add(ns, std::sync::atomic::Ordering::Relaxed);
}

/// How many jobs a check batch runs at once: the budget's limit, and every core while something
/// waits for the check (its jobs are Interactive then, whose room is the cores).
fn check_width() -> usize {
    let width = if check_waited() { makepad_cpu_budget::cores() } else { makepad_cpu_budget::limit() };
    width.max(1) as usize
}

/// A test or the run's exit waits for the check beside the program.
fn waited() {
    if !CHECK_WAITED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        CHECK_WAITED_AT.store(crate::clock::wall_ns(), std::sync::atomic::Ordering::Release);
        jit::promote_checks();
    }
}

/// An empty boot batch for the isolated queue test, with the same promotion as a verdict wait.
#[cfg(test)]
pub(crate) fn with_early_batch_for_test(front: std::sync::Arc<jit::FrontShared>, test: impl FnOnce(std::sync::Arc<CheckBatch>, fn())) {
    use std::sync::{Arc, atomic::Ordering::Relaxed};
    let batch = Arc::new(CheckBatch {
        ctx: Arc::new(BodyCheck {
            upn: 0, front, strict: false, beside: true, cpu: Default::default(),
            audit_reads: None, item_diagnostics: Default::default(),
        }),
        items: Vec::new(), kind: CheckKind::Body, next: Default::default(), left: Default::default(),
        jobs: Default::default(), out: Default::default(), done: Default::default(),
    });
    let old_early = CHECK_EARLY.swap(true, Relaxed);
    let old_waited = CHECK_WAITED.swap(false, Relaxed);
    let old_at = CHECK_WAITED_AT.load(Relaxed);
    test(batch, waited);
    CHECK_WAITED_AT.store(old_at, Relaxed);
    CHECK_WAITED.store(old_waited, Relaxed);
    CHECK_EARLY.store(old_early, Relaxed);
}

/// `items` checked by jobs of the JIT's queue, results in order.
fn queued_map(ctx: &std::sync::Arc<BodyCheck>, items: Vec<crate::program::DefId>, kind: CheckKind) -> Vec<Option<Vec<String>>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let n = items.len();
    if n == 0 { return Vec::new(); }
    // as many jobs as the budget admits at most: its classes decide how many of them run
    let jobs = n.min(check_width());
    let batch = std::sync::Arc::new(CheckBatch {
        ctx: ctx.clone(), items, kind, next: AtomicUsize::new(0), left: AtomicUsize::new(n), jobs: AtomicUsize::new(jobs),
        out: relax_session::sync::Mutex::new(Vec::with_capacity(n)), done: (relax_session::sync::Mutex::new(()), relax_session::sync::Condvar::new()),
    });
    jit::queue_check(&batch, jobs);
    if !ctx.strict { coordinator().active(false); }
    {
        let mut g = batch.done.0.lock().unwrap_or_else(|e| e.into_inner());
        while batch.left.load(Ordering::Acquire) != 0 { g = batch.done.1.wait(g).unwrap_or_else(|e| e.into_inner()); }
    }
    if !ctx.strict { coordinator().active(true); }
    let mut out = std::mem::take(&mut *batch.out.lock().unwrap_or_else(|e| e.into_inner()));
    out.sort_unstable_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, r)| r).collect()
}

/// One job of a check batch (on a worker of the JIT's queue): items until its slice is used, then
/// the job gives its place to as many as the batch's width allows now (`check_width`) while items
/// are left.
pub(crate) fn run_check_slice(batch: std::sync::Arc<CheckBatch>) {
    use std::sync::atomic::Ordering;
    let _span = crate::inspect::timeline::span(match batch.kind {
        CheckKind::Items => "check items",
        CheckKind::Wf => "check wf",
        CheckKind::Impl => "check impl",
        CheckKind::Body => "check body",
    });
    let start = std::time::Instant::now();
    struct Running;
    impl Drop for Running {
        fn drop(&mut self) { CHECK_SLICES[0].fetch_sub(1, Ordering::Relaxed); }
    }
    let now = CHECK_SLICES[0].fetch_add(1, Ordering::Relaxed) + 1;
    let _running = Running;
    // something waits for the check (a test or the run's exit): the queue admits its jobs
    // in the Interactive class (promote_checks), and the worker runs it at that class at the OS
    // too. A Linux thread without CAP_SYS_NICE that ran Background work cannot be raised: counted.
    if check_waited() {
        CHECK_SLICES[1].fetch_max(now, Ordering::Relaxed);
        let raised = makepad_cpu_budget::set_thread_class(makepad_cpu_budget::INTERACTIVE);
        CHECK_SLICES[if raised { 2 } else { 3 }].fetch_add(1, Ordering::Relaxed);
    }
    loop {
        let i = batch.next.fetch_add(1, Ordering::Relaxed);
        let Some(&d) = batch.items.get(i) else {
            batch.jobs.fetch_sub(1, Ordering::AcqRel);
            return;
        };
        // (a check's own panics are its ICE verdict; this is one outside them, its bookkeeping)
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| batch.ctx.check(d, batch.kind)))
            .unwrap_or_else(|p| {
                if batch.kind.is_item() {
                    crate::typeck_bg::report_checker_panic(&format!("definition {}", d.0), &jit::payload_text(&*p));
                    None
                } else { Some(vec!["internal compiler error in the check beside the program -- Relax bug, report it".to_string()]) }
            });
        batch.out.lock().unwrap_or_else(|e| e.into_inner()).push((i, r));
        if batch.left.fetch_sub(1, Ordering::AcqRel) == 1 {
            batch.jobs.fetch_sub(1, Ordering::AcqRel);
            let _g = batch.done.0.lock().unwrap_or_else(|e| e.into_inner());
            batch.done.1.notify_all();
            return;
        }
        if start.elapsed() >= CHECK_SLICE {
            // this job leaves, and the batch's jobs are made up to its width now: every core once
            // something waits for the check (a wait that began after the batch did), at least one
            // while items are unclaimed
            let unclaimed = batch.items.len().saturating_sub(batch.next.load(Ordering::Relaxed));
            let wanted = unclaimed.min(check_width());
            let mut add = 0;
            let _ = batch.jobs.try_update(Ordering::AcqRel, Ordering::Acquire, |jobs| {
                let others = jobs - 1;
                add = wanted.saturating_sub(others);
                Some(others + add)
            });
            if add > 0 { jit::queue_check(&batch, add); }
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
    errors: std::sync::OnceLock<Vec<String>>,
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
        let (crates, inputs) = {
            let r = jit::front_read("test dependency crates");
            let prog = r.of(&front).0;
            let crates = select(prog);
            let inputs = iface_inputs(prog, &crates);
            (crates, inputs)
        };
        names.retain(|(k, _)| crates.contains(k));
        names.sort_unstable_by_key(|(k, _)| *k);
        names.dedup_by_key(|(k, _)| *k);
        let (todo, keys) = crate::owner::call(crate::owner::Class::Publish, || {
            let u = unsafe { &*up };
            let mut todo = Vec::new();
            let mut keys = Vec::new();
            for (i, &k) in crates.iter().enumerate() {
                let key = u.cache.as_ref().and_then(|c| unsafe { &mut *c.get() }.dep_check_key(k, inputs.own[k as usize], inputs.skipped[&k], inputs.deps(i)));
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
            (owners.len(), check_prepared(up, AppCheck { t0, todo, keys, owners, audited: Vec::new(), items: HashMap::new(), scope: Vec::new() }, Some(&runtime), CheckMode::Errors))
        };
        TestDepsResult {
            errors: crate::zone_alloc::kept_clone(&checked.errors), names,
            selected: crates.len(), stored, owners: count,
            cpu_ns: checked.cpu_ns + relax_session::clock::cpu_ns() - cpu0, wall_ns: jit::wall_ns() - t0,
        }
    }).expect("test dependency check thread");
    assert!(TEST_DEPS.set(TestDeps { worker: relax_session::sync::Mutex::new(Some(worker)), errors: std::sync::OnceLock::new() }).is_ok(), "test dependency check already started");
}

/// Every test exit joins the same check and reports its errors once, before the cache write.
pub fn finish_test_deps(code: i32) -> i32 {
    if test_dep_errors().is_empty() { code } else { 101 }
}

pub(crate) fn test_dep_errors() -> &'static [String] {
    // An exit inside a unit call cannot wait for work that needs that owner.
    if crate::run::exit_caller_holds_owner() { return &[]; }
    let Some(check) = TEST_DEPS.get() else { return &[] };
    check.errors.get_or_init(|| {
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
            let error = "internal compiler error while checking test dependencies -- Relax bug, report it";
            report(error);
            return vec![error.into()];
        };
        for (_, e) in &r.errors { report(e); }
        for (k, name) in &r.names {
            let n = r.errors.iter().filter(|(c, _)| c == k).count();
            if n != 0 {
                eprintln!("error: could not compile `{}` (lib) due to {} previous error{} (`{tool} test` checks every path dependency as cargo builds it)", name, n, if n == 1 { "" } else { "s" }, tool = crate::tool_name());
            }
        }
        if crate::run::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("relax: test dependency check: {} crates selected, {} stored clean, {} body owners, {} errors, cpu {:.0} ms, wall {:.0} ms, end waited {:.0} ms",
                r.selected, r.stored, r.owners, r.errors.len(), r.cpu_ns as f64 / 1e6, r.wall_ns as f64 / 1e6, waited as f64 / 1e6);
        }
        r.errors.into_iter().map(|(_, e)| e).collect()
    })
}

/// At process exit, let the app check finish before choosing the exit code. As for test
/// dependencies, an exit inside an owner request must go on with its own code: the check
/// can need that owner to finish.
pub(crate) fn finish_app_check(code: i32) -> i32 {
    if crate::run::exit_caller_holds_owner() {
        if app_check_running() {
            eprintln!("relax: exiting with code {code} while the app check runs: the exiting thread holds the owner");
        }
        return code;
    }
    if wait_app_check() { 101 } else { code }
}

/// The app check beside the program (start_app_check): 0 none, 1 running, 2 done clean, 3 found
/// errors.
static BESIDE: (relax_session::sync::Mutex<u8>, relax_session::sync::Condvar) = (relax_session::sync::Mutex::new(0), relax_session::sync::Condvar::new());
static APP_CHECK_THREAD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[derive(Clone, Copy)]
struct Coordinator { tid: u64, final_cpu: Option<u64>, busy: u64, since: Option<u64> }
const NEW_COORDINATOR: Coordinator = Coordinator { tid: 0, final_cpu: Some(0), busy: 0, since: None };
static COORDINATOR: relax_session::sync::Mutex<Coordinator> = relax_session::sync::Mutex::new(NEW_COORDINATOR);
fn coordinator() -> relax_session::sync::MutexGuard<'static, Coordinator> { COORDINATOR.lock().unwrap_or_else(|e| e.into_inner()) }
impl Coordinator {
    fn wall(&self) -> u64 { self.busy + self.since.map_or(0, |t| jit::wall_ns() - t) }
    fn active(&mut self, on: bool) { self.busy = self.wall(); self.since = on.then(jit::wall_ns); }
    fn cpu(&self) -> Option<u64> {
        if self.tid == 0 { return self.final_cpu; }
        #[cfg(unix)]
        if self.tid == crate::inspect::threads::self_os_id() { return Some(relax_session::clock::cpu_ns()); }
        let mut ns = None;
        crate::inspect::threads::each_thread(|id, _, cpu| if id == self.tid { ns = cpu() });
        ns
    }
}
fn app_check_done(verdict: u8) {
    let clock = *coordinator();
    let cpu = clock.cpu();
    { let mut c = coordinator(); c.active(false); c.final_cpu = cpu; c.tid = 0; }
    *BESIDE.0.lock().unwrap_or_else(|e| e.into_inner()) = verdict;
    BESIDE.1.notify_all();
}

/// makepad run of an app that presents frames: its app check runs beside main instead of before
/// it. Every body is still checked before it first runs (a lazy compile checks its body; a body
/// with errors is a compile error, which ends the run). The app presents as soon as it can.
/// The check reports its errors as a build does (`stop`). A live session's hosted run ends in place, as a
/// restart's end does, and its host waits with no run for the next save or relaunch
/// (run_end::check_failed); any other run exits 101 (the program's exit: os::proc::exit).
pub fn start_app_check(up: *mut Unit, p: AppCheck, runtime: crate::options::RuntimeOptions, stop: Box<dyn Fn(&[String]) + Send>) {
    *BESIDE.0.lock().unwrap_or_else(|e| e.into_inner()) = 1;
    let upn = up as usize;
    crate::run_threads::spawn_sized("run-app-check", 256 << 20, move || {
        use std::sync::atomic::Ordering::Relaxed;
        struct CheckThread;
        impl Drop for CheckThread {
            fn drop(&mut self) { APP_CHECK_THREAD.store(0, Relaxed); }
        }
        let _thread = CheckThread;
        {
            let _state = BESIDE.0.lock().unwrap_or_else(|e| e.into_inner());
            let mut c = coordinator();
            c.tid = crate::inspect::threads::self_os_id();
            c.active(true);
            APP_CHECK_THREAD.store(crate::waits::me(), Relaxed);
            // An exit already waiting can now name the check thread.
            BESIDE.1.notify_all();
        }
        let _z = crate::zone_alloc::compiler_thread();
        makepad_cpu_budget::set_thread_class(makepad_cpu_budget::BACKGROUND);
        let errors = {
            let _span = crate::inspect::timeline::span("app check");
            struct EarlyCheck;
            impl Drop for EarlyCheck {
                fn drop(&mut self) { CHECK_EARLY.store(false, std::sync::atomic::Ordering::Release); }
            }
            let _early = EarlyCheck;
            CHECK_EARLY.store(!jit::past_startup_headless(), std::sync::atomic::Ordering::Release);
            check_prepared(upn as *mut Unit, p, Some(&runtime), CheckMode::Typeck)
        }.strings();
        let n = |i: usize| RUN_CHECK[i].load(Relaxed);
        let ms = |ns: u64| ns as f64 / 1e6;
        let present = jit::FIRST_PRESENT_NS.load(std::sync::atomic::Ordering::Acquire);
        let when = if present == 0 {
            "before the first present".to_string()
        } else {
            format!("{:.0} ms after the first present", ms(jit::wall_ns().saturating_sub(present)))
        };
        let verdict = if errors.is_empty() { "clean".to_string() } else { format!("{} errors", errors.len()) };
        eprintln!("relax: app check: {} crates, {} bodies, {} item defs in {:.0} ms ({:.0} ms cpu); verdict: {verdict}, {when}",
            n(0), n(1), n(7), ms(n(2)), ms(APP_CHECK_CPU.load(Relaxed)));
        if !errors.is_empty() {
            stop(&errors);
            // a live session: the process never ends for a compile error. The run's end is the
            // restart's (the program's threads, the run's, the unit, the statics), from its own
            // thread, never from under the program's threads here.
            if crate::run::live_session() && crate::run::hosted_run() {
                // the app crates' folders: the held host watches them for the fix's save, which can
                // be in any of them, not only in the package's own folder
                let dirs = crate::run::app_dirs();
                if crate::run_end::check_failed(&errors, &dirs).is_ok() {
                    app_check_done(3);
                    return;
                }
            }
            // as the program's own exit ends (os::proc::exit): without the C library's exit
            // handlers, which a native library registered (a GPU driver's teardown, from the
            // window's load) and which run while the program's threads go on in that library
            app_check_done(3);
            crate::os::proc::exit(101);
        }
        app_check_done(2);
    }).expect("app check thread");
}

/// The app check beside main is running (start_app_check).
pub(crate) fn app_check_running() -> bool {
    *BESIDE.0.lock().unwrap_or_else(|e| e.into_inner()) == 1
}

/// Wait for the verdict at the run's end. True if the check reported errors.
fn wait_app_check() -> bool {
    use std::sync::atomic::Ordering::{Relaxed, Release};
    let state = *BESIDE.0.lock().unwrap_or_else(|e| e.into_inner());
    if state != 1 { return state == 3; }

    let (t0, cpu0) = (std::time::Instant::now(), APP_CHECK_CPU.load(Relaxed));
    let clock = *coordinator();
    let (coord_wall, coord_cpu) = (clock.wall(), clock.cpu());
    waited();
    let _ui = crate::preempt::UiWaits::new();
    let _held = jit::MainHeld::new();
    let mut state = BESIDE.0.lock().unwrap_or_else(|e| e.into_inner());
    while *state == 1 {
        let holder = APP_CHECK_THREAD.load(Relaxed);
        let _wait = crate::waits::begin(crate::waits::Kind::Check, 0, holder, "the run's end");
        state = BESIDE.1.wait_while(state, |s| *s == 1 && APP_CHECK_THREAD.load(Relaxed) == holder)
            .unwrap_or_else(|e| e.into_inner());
    }
    CHECK_WAITED_AT.store(0, Release);
    CHECK_WAITED.store(false, Release);
    let failed = *state == 3;
    drop(state);
    // what the exit waited and what the check got meanwhile: on a loaded Linux box a waited
    // check's slices on a worker that ran Background work cannot be raised to the waiter's class
    // (no CAP_SYS_NICE): counted here
    let ms = |ns: u64| ns as f64 / 1e6;
    let c = *coordinator();
    let cpu = c.final_cpu.zip(coord_cpu).map_or_else(|| "-".into(), |(end, start)| format!("{:.0}", ms(end.saturating_sub(start))));
    let busy = ms(c.wall().saturating_sub(coord_wall));
    eprintln!("relax: the run's end waited {:.0} ms for the app check; its checks {:.0} ms cpu meanwhile (at most {} jobs at once, \
        CPU limit {}; {} slices raised to the waiter's class at the OS, {} refused; its slices waited {:.0} ms for a worker; run-app-check {cpu} ms cpu, {busy:.0} ms busy wall)",
        t0.elapsed().as_secs_f64() * 1e3, ms(APP_CHECK_CPU.load(Relaxed).saturating_sub(cpu0)), CHECK_SLICES[1].load(Relaxed),
        makepad_cpu_budget::limit(), CHECK_SLICES[2].load(Relaxed), CHECK_SLICES[3].load(Relaxed), ms(CHECK_SLICES[4].load(Relaxed)));
    failed
}

/// The last run_check_crates: crates checked, their body owners, wall ns, of it the stored verdicts'
/// lookup and the skipped files' load, body verdicts found by their dependencies' interfaces,
/// wall ns making and looking up interface keys, and unique defs checked by the item passes.
pub static RUN_CHECK: [std::sync::atomic::AtomicU64; 8] = [const { std::sync::atomic::AtomicU64::new(0) }; 8];

/// `f` over `items` on RELAX_JOBS threads (at most 16), results in `items` order.
/// The check's threads: RELAX_JOBS, else one per hardware thread (at most 16).
fn jobs() -> usize {
    std::env::var("RELAX_JOBS").ok().and_then(|v| v.parse::<usize>().ok()).filter(|n| *n > 0)
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
    prog.record_error(def.file, pos, "typeck", format!("internal compiler error while checking {} of {}: {}{} -- Relax bug, report it", what, prog.def_path(d), said, jit::take_ice_location()))
}

/// What a borrowck verdict says beyond its errors: an internal error for every body a failure left
/// unchecked (never taken as checked: rustc's delayed bug), and a note on stderr for every body
/// holding a construct the MIR builder does not express (a known gap, not refused).
/// The verdict of an instance whose body has no normal form in the borrow check's typing mode
/// (typeck::borrowck_body) while it has one revealed: the check could not run, an internal error.
pub(crate) fn unnormalized_verdict(prog: &crate::program::Program, d: crate::program::DefId) -> crate::borrowck::Verdict {
    crate::borrowck::Verdict { errors: Vec::new(), unchecked: vec![format!("{}: its instance has no normal form in the borrow check's typing mode", prog.def_path(d))],
        unsupported: Vec::new(), fv_only: Vec::new(), fv_records: Vec::new(), relaxed_records: Vec::new(), bodies: 0 }
}

pub(crate) fn verdict_gaps(prog: &crate::program::Program, d: crate::program::DefId, v: &crate::borrowck::Verdict) -> Vec<String> {
    let errors = v.unchecked.iter().map(|x| def_ice(prog, d, "the borrows", x)).collect();
    for x in &v.unsupported { stderr_line!("relax check: note: not borrow-checked: {}", x); }
    errors
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
/// Returns the exit code and the errors in other crates, counted by crate.
pub fn check_crate(u: &mut Unit, root: u32, crates: &[u32], errors: Vec<String>, selftest: bool) -> (i32, Vec<(String, usize)>) {
    let (code, _, foreign) = check_crate_impl(u, root, crates, errors, true, selftest, None);
    (code, foreign)
}

/// `check_crate` reusing the verdicts of `reuse`'s unchanged crates (check_reuse.rs): their
/// diagnostics replayed, the per-crate passes run on the other crates only; a passing check
/// stores the others' verdicts.
pub(crate) fn check_crate_reusing(u: &mut Unit, root: u32, crates: &[u32], errors: Vec<String>, reuse: &mut crate::check_reuse::Reuse) -> (i32, Vec<(String, usize)>) {
    let (code, _, foreign) = check_crate_impl(u, root, crates, errors, true, false, Some(reuse));
    (code, foreign)
}

/// Whether a check may reuse verdicts: nothing it was asked to do needs every body walked (a
/// per-body trace, the relaxed-Rust report or write-back, the field-view export, the borrow
/// checker's whole-program modes).
pub(crate) fn reuse_allowed(options: &crate::options::Options) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    !check_trace() && !borrowck_unchecked() && !crate::relaxed::STRICT_REPORT.load(Relaxed) && !crate::relaxed::FIX.load(Relaxed)
        && std::env::var_os("RELAX_FV_EXPORT").is_none() && std::env::var_os("RELAX_MIR_BORROWCK_ALL").is_none()
        && std::env::var_os("RELAX_MIR_BUILD_ALL").is_none()
        && options.borrowck_all == crate::options::BorrowckAll::Default && !options.borrowck_instances
}

/// `makepad test`'s check of the crates under test before they run (run.rs check_test_subjects):
/// the app check of `crates`, its errors printed; the diagnostics (borrow errors included)
/// and the warnings, in order (the caller prints them). No statistics. `harness`: the root is
/// built with `--test` (libtest's generated main replaces the crate's own, rustc_builtin_macros
/// test_harness.rs), so it has no entry point to check.
pub fn check_subjects(u: &mut Unit, root: u32, crates: &[u32], errors: Vec<String>, harness: bool) -> (Vec<crate::diag::Diagnostic>, Vec<String>) {
    SUBJECTS.store(if harness { 2 } else { 1 }, std::sync::atomic::Ordering::Relaxed);
    let diagnostics = check_crate_impl(u, root, crates, errors, true, false, None).1;
    SUBJECTS.store(0, std::sync::atomic::Ordering::Relaxed);
    (diagnostics, std::mem::take(&mut *SUBJECT_WARNINGS.lock().unwrap_or_else(|e| e.into_inner())))
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
/// RELAX_CHECK_TIMES: the body checks' CPU by step, summed over the threads (BODY_STEPS).
static BODY_NS: [std::sync::atomic::AtomicU64; 9] = [const { std::sync::atomic::AtomicU64::new(0) }; 9];
const BODY_STEPS: [&str; 9] = ["early lints", "typeck", "stability", "body lints", "unsafety", "privacy", "patterns", "borrowck", "borrow reports"];

fn body_times_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RELAX_CHECK_TIMES").is_some())
}

/// RELAX_CHECK_BODY_TIMES=<file>: one row per checked body, its steps' CPU (ns, BODY_STEPS
/// order), its typed expressions and its path (tab-separated), written at the end of the check.
fn body_rows_path() -> Option<&'static std::path::Path> {
    static P: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    P.get_or_init(|| std::env::var_os("RELAX_CHECK_BODY_TIMES").filter(|v| !v.is_empty()).map(Into::into)).as_deref()
}
static BODY_ROWS: relax_session::sync::Mutex<Vec<String>> = relax_session::sync::Mutex::new(Vec::new());
/// RELAX_MEM_REPORT: each body's peak and kept bytes, then its owner.
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

/// The body's row (RELAX_CHECK_BODY_TIMES), its steps' CPU since the last row.
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
        if let Err(e) = std::fs::write(p, text) { stderr_line!("relax check: body times not written to {}: {}", p.display(), e); }
    }
    if !body_times_on() { return; }
    let parts: Vec<String> = BODY_STEPS.iter().enumerate().map(|(k, n)| format!("{} {:.2}", n, BODY_NS[k].load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e9)).collect();
    stderr_line!("relax check: body cpu s: {}", parts.join(", "));
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
    stderr_line!("relax mem: bodies: {} bodies, kept sum {:.1} MB, peak sum {:.1} MB, largest peak {:.1} MB",
        rows.len(), mb(sum_kept), mb(sum_peak), mb(rows[0].0));
    for &(peak, kept, d) in rows.iter().take(12) {
        stderr_line!("relax mem: bodies: top peak: peak {:.1} MB, kept {:.1} MB, {}", mb(peak), mb(kept), prog.def_path(d));
    }
    rows.sort_unstable_by(|a, b| b.1.cmp(&a.1));
    for &(peak, kept, d) in rows.iter().take(12) {
        stderr_line!("relax mem: bodies: top kept: kept {:.1} MB, peak {:.1} MB, {}", mb(kept), mb(peak), prog.def_path(d));
    }
    stderr_line!("relax mem: bodies: kept buckets: under 64 KB: {} bodies, {:.1} MB; 64 KB-1 MB: {} bodies, {:.1} MB; 1-16 MB: {} bodies, {:.1} MB; 16 MB and over: {} bodies, {:.1} MB",
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
        // before borrowck); an internal failure of the analysis is an internal error of the body
        let pats = body_step(6, || crate::memstat::tagged(Tag::Patterns, || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| relax_types::pattern_analysis::check_body(prog, tcx, &body)))));
        // (binding conflicts of `x @ pat` do not end the checks: borrowck runs after them)
        let at_patterns = match pats {
            Ok(r) if !r.errors.is_empty() => { let mut e = r.errors; e.extend(r.at_patterns); return (staged(patterns, e).collect(), Vec::new()) }
            Ok(r) => r.at_patterns,
            Err(p) if crate::preempt::is_cancelled(&*p) => std::panic::resume_unwind(p),
            Err(p) => {
                return (vec![(patterns, def_ice(prog, d, "the patterns", p.downcast_ref::<String>().map(String::as_str).or_else(|| p.downcast_ref::<&str>().copied()).unwrap_or("a panic")))], Vec::new());
            }
        };
        if lints && !crate::borrowck::enabled(options) {
            return (staged(patterns, at_patterns).collect(), Vec::new());
        }
        let v = body_step(7, || crate::memstat::tagged(Tag::Borrowck, || crate::borrowck::check_body(&crate::tcx::EvalCx::new(prog, tcx, crate::tcx::TypingEnv::fully_monomorphized(prog), &crate::tcx::EvalState::new()), prog, tcx, &body)));
        crate::borrowck::note_verdict(&v);
        // a body the borrow check could not finish is never taken as checked (rustc: a delayed bug)
        let borrowck = Stage::of(Phase::Mir, crate::check_order::MIR_BORROWCK, d);
        let mut internal: Vec<(Stage, String)> = verdict_gaps(prog, d, &v).into_iter().map(|e| (borrowck, e)).collect();
        let args: Vec<_> = if owner.kind == crate::borrowck::body::BodyKind::Function && !generic { Vec::new() } else { body.args.to_vec() };
        let origin = if lints { crate::borrowck::event::Origin::Check } else { crate::borrowck::event::Origin::DeferredCheck };
        let events = body_step(8, || crate::memstat::tagged(Tag::Borrowck, || crate::borrowck::lazy::report_checked(prog, 0, tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst), d, &args, v.errors, origin)));
        internal.extend(staged(patterns, at_patterns));
        (internal, events)
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
///
/// With them, the borrow check's errors of those bodies, by body owner (`LiveCheck::borrows`): the
/// save runs (as a lazy build does), and the test session's verdict is red while one is left
/// (harness_live `note_borrows`). Before, they were checked and dropped: a save rustc rejects went
/// green under `--live` (E0502 in a sim crate, 11-18).
pub(crate) struct LiveCheck {
    pub lints: Vec<String>,
    /// every body owner the save's bodies have (typeck roots): what `borrows` speaks for
    pub owners: Vec<u32>,
    /// (owner, rendered error): each body's borrow check errors, and the bodies it could not check
    pub borrows: Vec<(u32, String)>,
}

pub(crate) fn live_lints(prog: &crate::program::Program, tcx: &crate::tcx::Tcx, defs: &[crate::program::DefId]) -> LiveCheck {
    use crate::check_order::{Entry, Out};
    let lints = relax_types::lint::enabled();
    let mut roots: Vec<crate::program::DefId> = defs.iter().map(|&d| relax_types::type_queries::typeck_root_def_id(prog, d)).collect();
    roots.sort_unstable_by_key(|d| d.0);
    roots.dedup();
    let mut borrows: Vec<(u32, String)> = Vec::new();
    let ((), reported) = relax_types::lint::capture(|| {
        for &d in &roots {
            if crate::borrowck::body::CheckedBody::item(prog, d).is_none() { continue; }
            // (an internal failure of a lint reports nothing, as in a check; one of the borrow
            // check leaves its body unchecked, which is no pass)
            let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if lints { relax_types::lint_early::check_def(prog, tcx, d); }
                // (typed again: the save's build typed it with no lints)
                let body = tcx.typeck_unstored(prog, d);
                if !body.errors.is_empty() { return Vec::new(); }
                if lints {
                    relax_types::lint::check_literals(prog, tcx, &body);
                    relax_types::lint_pass::check_body(prog, tcx, &body);
                    let _ = relax_types::unsafety::check_body(prog, tcx, &body);
                }
                let state = crate::tcx::EvalState::new();
                let v = crate::borrowck::check_body(&crate::tcx::EvalCx::new(prog, tcx, crate::tcx::TypingEnv::fully_monomorphized(prog), &state), prog, tcx, &body);
                let mut out: Vec<String> = v.errors.iter().map(|e| e.render()).collect();
                out.extend(v.unchecked.iter().map(|x| format!("error: internal compiler error while checking the borrows of {}: {} -- Relax bug, report it", prog.def_path(d), x)));
                out
            }));
            match checked {
                Ok(errors) => borrows.extend(errors.into_iter().map(|e| (d.0, e))),
                Err(_) => borrows.push((d.0, format!("error: internal compiler error while checking the borrows of {} -- Relax bug, report it", prog.def_path(d)))),
            }
        }
    });
    let owners: Vec<u32> = roots.iter().map(|d| d.0).collect();
    if !lints { return LiveCheck { lints: Vec::new(), owners, borrows }; }
    let entries: Vec<Entry> = reported.into_iter().filter(|r| !r.lint.is_empty() && roots.contains(&relax_types::type_queries::typeck_root_def_id(prog, r.def)))
        .map(|r| {
            let stage = crate::check_order::stage_of_lint(r.lint, r.def);
            // (a body's liveness lints keep the order its analysis reports them in, as the check's)
            let seq = if crate::check_order::keeps_report_order(stage, r.lint) { r.seq } else { 0 };
            Entry::new(stage, seq, if r.level >= relax_types::lint::Level::Deny { Out::Error(r.line) } else { Out::Warning(r.line) })
        })
        .collect();
    let lints = crate::check_order::sort(prog, entries).into_iter().filter_map(|e| match e.out { Out::Warning(s) | Out::Error(s) => Some(s), Out::Borrow(_) => None }).collect();
    LiveCheck { lints, owners, borrows }
}

/// The crate-level checks of the crates `own`: rustc's resolve, early, attribute and AST
/// validation passes, which no definition's check makes. `all` holds the whole check's crates, for
/// the checks across crates (the symbols, the global allocators, the entry symbol); `entry` is the
/// root crate when it is a program's entry. Each error with where rustc reports it
/// (check_order.rs), and the erroneous imports (each counts as used). A check's
/// (check_crate_impl) and a run's app check (check_prepared): what rustc refuses here, a run
/// refuses too.
fn crate_level_errors(prog: &Program, tcx: &Tcx, own: &[bool], all: &[bool], binaries: &[u32], entry: Option<u32>, st: &mut StepTimes) -> (Vec<(crate::check_order::Stage, String)>, Vec<usize>) {
    use crate::check_order::{Phase, Stage, LATE_PRIVATE_IN_PUBLIC};
    use relax_types::item_checks as ic;
    let mut errors = Vec::new();
    let mut add = |stage: Stage, found: Vec<String>, step: &str| {
        errors.extend(staged(stage, found));
        st.mark(step);
    };
    add(Stage::new(Phase::Resolve, 0), ic::duplicate_items(prog, own), "duplicate_items");
    add(Stage::new(Phase::Resolve, 0), ic::duplicate_exported_macros(prog, own), "duplicate_exported_macros");
    add(Stage::new(Phase::Resolve, 3), ic::super_paths(prog, own), "super_paths");
    add(Stage::new(Phase::Late, LATE_PRIVATE_IN_PUBLIC), ic::impl_trait_path_privacy(prog, tcx, own), "impl_trait_path_privacy");
    add(Stage::new(Phase::Late, LATE_PRIVATE_IN_PUBLIC), ic::private_assoc_types(prog, tcx, own), "private_assoc_types");
    add(Stage::new(Phase::Early, 1), ic::gated_syntax(prog, own), "gated_syntax");
    add(Stage::new(Phase::Attrs, 0), ic::node_attribute_targets(prog, own), "node_attribute_targets");
    add(Stage::new(Phase::Early, 0), ic::param_attributes(prog, own), "param_attributes");
    add(Stage::new(Phase::Early, 0), ic::foreign_blocks(prog, own), "foreign_blocks");
    add(Stage::new(Phase::Attrs, 0), ic::duplicate_symbols(prog, all), "duplicate_symbols");
    add(Stage::new(Phase::Attrs, 0), ic::global_allocators_and_lang_items(prog, all, binaries), "global_allocators_and_lang_items");
    add(Stage::new(Phase::Early, 0), relax_types::proc_macro_harness::check(prog, tcx, own), "proc_macro_harness");
    if let Some(root) = entry {
        add(Stage::new(Phase::Attrs, 0), ic::entry_point(prog, root), "entry_point");
        add(Stage::new(Phase::Attrs, 0), ic::entry_symbol_conflict(prog, all, root), "entry_symbol_conflict");
    }
    let privacy = prog.import_privacy_errors(own);
    let refused_imports = privacy.iter().filter_map(|(_, i, _)| *i).collect();
    add(Stage::new(Phase::Resolve, 6), privacy.into_iter().map(|(_, _, e)| e).collect(), "import_privacy");
    add(Stage::new(Phase::Attrs, 0), prog.attribute_errors(own), "attribute_errors");
    add(Stage::new(Phase::Early, 0), prog.ast_validation_errors(own), "ast_validation");
    (errors, refused_imports)
}

fn check_crate_impl(u: &mut Unit, root: u32, crates: &[u32], front: Vec<String>, emit: bool, compile: bool, mut reuse: Option<&mut crate::check_reuse::Reuse>) -> (i32, Vec<crate::diag::Diagnostic>, Vec<(String, usize)>) {
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
    // every checked definition's signature, on RELAX_JOBS threads (queries over the shared
    // context); errors in definition order
    {
        let check_times = std::env::var_os("RELAX_CHECK_TIMES").is_some();
        let defs: Vec<crate::program::DefId> = (0..u.prog.defs.len()).filter(|&i| own[u.prog.defs[i].krate as usize]).map(|i| crate::program::DefId(i as u32)).collect();
        let (prog, tcx) = (&u.prog, &u.tcx);
        let signature = |prog: &crate::program::Program, tcx: &crate::tcx::Tcx, d: crate::program::DefId| {
            let t0 = std::time::Instant::now();
            let mut e = tcx.check_def_signature(prog, d);
            let dt_sig = t0.elapsed();
            if prog.def(d).kind == DefKind::Impl { e.extend(relax_types::unsafety::check_impl(prog, tcx, d)); }
            let dt = t0.elapsed();
            if check_times && dt.as_millis() >= 500 {
                stderr_line!("relax check: slow sig {:.2} s (check_def_sig {:.2} s): {:?} {}", dt.as_secs_f64(), dt_sig.as_secs_f64(), prog.def(d).kind, prog.def_path(d));
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
                if check_times { stderr_line!("relax check: lint_pass::check_items {:.2} s (beside the signatures)", t0.elapsed().as_secs_f64()); }
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
    // the compiled binaries: `makepad test`'s subjects are each a test binary (cargo builds every
    // target separately); a check's are its bin crates
    let binaries: Vec<u32> = crates.iter().copied().filter(|&k| subjects_mode() != 0 || u.prog.crates[k as usize].is_bin).collect();
    let entry = u.prog.crates[root as usize].is_bin && !u.prog.crates[root as usize].harness && subjects_mode() != 2;
    let (crate_level, refused_imports) = crate_level_errors(&u.prog, &u.tcx, own, &own_all, &binaries, entry.then_some(root), &mut st);
    errors.extend(crate_level);
    erroneous_imports.extend(refused_imports);
    errors.extend(staged(Stage::new(Phase::Early, 2), relax_types::lint_levels::check(&u.prog, &own))); st.mark("lint_levels");
    relax_types::lint_dead::reset();
    let d_items = t_items0.elapsed();
    let t_wf0 = std::time::Instant::now();
    // item well-formedness (rustc wfcheck): after the signatures, and only when they are sound -- an
    // item with an unresolved or ill-counted type has no well-formedness to speak of (the lints
    // it reports: those between `wf_reports`, check_reuse.rs WF)
    let wf_ran = errors.is_empty();
    let wf_lo = relax_types::lint::reported_len();
    if wf_ran {
        let check_times = std::env::var_os("RELAX_CHECK_TIMES").is_some();
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
                stderr_line!("relax check: slow wf {:.2} s (check_def {:.2} s): {:?} {}", dt.as_secs_f64(), dt_wf.as_secs_f64(), prog.def(d).kind, prog.def_path(d));
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
        return (if n == 0 { 0 } else { 1 }, Vec::new(), Vec::new());
    }
    // MIR borrowck pipeline harness (docs: relax_types::mir::build::driver_borrowck).
    if let Some(mode) = std::env::var_os("RELAX_MIR_BORROWCK_ALL") {
        let errors = relax_types::mir::build::driver_borrowck::borrowck_all(&u.prog, &u.tcx, mode == "std");
        return (if errors == 0 { 0 } else { 1 }, Vec::new(), Vec::new());
    }
    // MIR construction harness (docs: relax_types::mir::build::driver): build + validate every body.
    if let Some(mode) = std::env::var_os("RELAX_MIR_BUILD_ALL") {
        let failures = relax_types::mir::build::driver::build_all(&u.prog, &u.tcx, mode == "std");
        return (if failures == 0 { 0 } else { 1 }, Vec::new(), Vec::new());
    }
    // (the check's borrow errors are printed with its other diagnostics, in rustc's order)
    let borrow_events = crate::gandalf_out::Collector::start();
    let mut borrow_diagnostics = Vec::new();
    let up: *mut Unit = u;
    let mut fns = Vec::new();
    // the checked crates' definitions only (the program's other bodies are never looked at)
    for owner in (0..u.prog.defs.len()).filter(|&i| own[u.prog.defs[i].krate as usize]).map(|i| crate::program::DefId(i as u32)).filter_map(|d| crate::borrowck::body::CheckedBody::item(&u.prog, d)) {
        let d = u.prog.def(owner.owner);
        // the app check needs no seeds: no signature is asked here (each costs a query; a large app's
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
    if std::env::var_os("RELAX_CHECK_TIMES").is_some() {
        stderr_line!("relax check: owners breakdown: rsim {:.2} s, sigs {:.2} s, item_checks {:.2} s, wfcheck {:.2} s", d_rsim.as_secs_f64(), d_sigs.as_secs_f64(), d_items.as_secs_f64(), d_wf.as_secs_f64());
    }
    // typeck-audit 1.27: anonymous constants outside bodies are body owners of their own (typed
    // and checked on RELAX_JOBS threads; their results, and the evaluations, in order)
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
    // every body owner type- and borrow-checked on RELAX_JOBS threads: typeck and the borrow
    // checker are queries over the shared program and type context (as the run's typeck-ahead
    // workers use them); results are merged in owner order
    struct Outcome { errors: Vec<(Stage, String)>, diags: Vec<crate::diag::Diagnostic>, seed: Option<crate::program::DefId>, needs_load: bool, unchecked: bool }
    // body verdicts (check_bodies.rs): a fresh crate's body whose stored verdict stands is
    // replayed, not checked (Verify mode compares them instead); an app check's, with the store
    let (mut bodies, replay) = match reuse.as_deref_mut().filter(|_| !compile) {
        Some(r) => { let (b, f) = crate::check_bodies::Bodies::open(r, &u.prog, &u.tcx, &fresh_crates, &fns); (Some(b), f) }
        None => (None, Vec::new()),
    };
    // (the owners replayed as the body round starts: a second round may check some after all)
    let replayed_set: std::collections::HashSet<crate::program::DefId> = bodies.as_ref().map_or_else(Default::default, |b| b.replayed().clone());
    let replayed = &replayed_set;
    // by crate: has bodies replayed, whose stored uses its import reports wait on (lint_imports::Judge)
    let mut replayed_crates = vec![false; u.prog.crates.len()];
    for &d in replayed { replayed_crates[u.prog.def(d).krate as usize] = true; }
    let jobs = jobs();
    let upn = up as usize;
    // `replay`: an owner with a stored verdict standing is replayed (not in a second round)
    let check_one = move |owner: &crate::borrowck::body::CheckedBody, generic: bool, replay: bool| -> Outcome {
        let uu = unsafe { &*(upn as *const Unit) };
        let (prog, tcx) = (&uu.prog, &uu.tcx);
        let d = owner.owner;
        if check_trace() { stderr_line!("relax check: body {}", prog.def_path(d)); }
        let mut out = Outcome { errors: Vec::new(), diags: Vec::new(), seed: None, needs_load: false, unchecked: false };
        // (work that unwound with no result is asked again in there, until it has an answer)
        let r = def_work(|said| def_ice(prog, d, "the body", said), || {
            // a stored verdict stands: the body is parsed and its macros expanded as its check
            // would (the late inputs and the expansion records the crate passes read), not checked
            if replay && replayed.contains(&d) {
                relax_types::body_facts::with_owner(prog, d, || { let def = prog.def(d); let _ = prog.fn_body(def.file, def.item); });
                return (Vec::new(), Vec::new());
            }
            #[cfg(test)]
            forced_body_unwind(prog, d);
            let (errors, events) = relax_types::body_facts::with_owner(prog, d, || body_errors(prog, tcx, options, owner, generic, true));
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
    // the cache files' pages the load touched (the front's arrays: sources, tokens, the ASTs of
    // every crate) leave the check's resident set: they are read-only file pages, and the ones a
    // body reads again come back from the page cache (a minor fault)
    crate::pack::release_resident();
    relax_types::body_facts::begin(&fresh);
    relax_types::body_facts::open();
    for (d, f) in replay { relax_types::body_facts::replay(&u.prog, d, f); }
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
                        local.push((i, check_one(owner, *generic, true)));
                        // return the allocator's free pages as the check goes (a large app held 4.6 GB),
                        // and the cache mappings' resident pages (pack::release_resident)
                        if i % 20_000 == 19_999 { crate::memstat::trim(); crate::pack::release_resident(); }
                    }
                    results.lock().unwrap().extend(local);
                }).expect("check worker");
            }
        });
        outcomes.extend(results.lock().unwrap().drain(..));
    }
    if std::env::var_os("RELAX_CHECK_TIMES").is_some() { stderr_line!("relax check: owners {:.2} s, anon consts {:.2} s, bodies {:.2} s ({} jobs)", t_owners.as_secs_f64(), (t_par - t_owners).as_secs_f64(), (t_check.elapsed() - t_par).as_secs_f64(), jobs); }
    body_times_report();
    outcomes.sort_by_key(|(i, _)| *i);
    // owners that failed or met an unloaded module: every skipped file loaded, checked again
    // (only when some other crate's file is still skipped: else the first answer stands)
    let skipped_left = unsafe { !(&*up).prog.skipped_files.is_empty() };
    if skipped_left && outcomes.iter().any(|(_, o)| o.needs_load) {
        let t_skip = std::time::Instant::now();
        unsafe { jit::load_all_skipped(up) };
        if std::env::var_os("RELAX_CHECK_TIMES").is_some() {
            stderr_line!("relax check: load_all_skipped {:.2} s", t_skip.elapsed().as_secs_f64());
        }
    }
    let retry_idxs: Vec<usize> = outcomes.iter().enumerate()
        .filter(|(_, (_, o))| skipped_left && o.needs_load)
        .map(|(idx, _)| idx)
        .collect();
    // the owners whose verdicts are not stored: a check that failed, met a skipped file or was
    // asked again
    let mut unclean: std::collections::HashSet<crate::program::DefId> = retry_idxs.iter().map(|&idx| fns[outcomes[idx].0].0.owner).collect();
    if !retry_idxs.is_empty() {
        let t_retry = std::time::Instant::now();
        let retried = par_map(&retry_idxs, jobs, |&idx| {
            let fi = outcomes[idx].0;
            let (owner, generic) = &fns[fi];
            check_one(owner, *generic, true)
        });
        for (idx, new_o) in retry_idxs.iter().copied().zip(retried) {
            outcomes[idx].1 = new_o;
        }
        if std::env::var_os("RELAX_CHECK_TIMES").is_some() {
            stderr_line!("relax check: retried {} bodies in {:.2} s", retry_idxs.len(), t_retry.elapsed().as_secs_f64());
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
        if o.unchecked || !o.errors.is_empty() || !o.diags.is_empty() { unclean.insert(fns[i].0.owner); }
        errors.extend(o.errors);
        borrow_diagnostics.extend(o.diags);
        if let Some(d) = o.seed { seeds.push(d); }
    }
    let checked = seeds.len();
    if let Some(b) = bodies.as_ref().filter(|_| subjects_mode() == 0) {
        eprintln!("relax check: {} of {} bodies replayed from stored verdicts", b.stats.replayed, n_own);
    }
    let mut st = StepTimes::new("after bodies:");
    st.mark("merge");
    // the anonymous constants inside the bodies are body owners of their own (rustc
    // par_body_owners: an array length or const argument in a body is typed by its own typeck,
    // which reports its errors): their type errors, and their late lints (rustc
    // visit_nested_body: a const argument `U::<300>` is a body of its own, its literal typed as
    // the parameter). A repeat count its owner typed was linted with the owner, but its errors as
    // a constant (`[0; n]` with `n` a local) are its own
    // (of the owners `of` takes; a replayed body's constants' facts are in its stored ones)
    let body_anons = |of: &dyn Fn(crate::program::DefId) -> bool, errors: &mut Vec<(Stage, String)>, unclean: &mut std::collections::HashSet<crate::program::DefId>| {
        for &k in &fresh_crates {
            let uu = unsafe { &*up };
            for (anon, owner, e) in crate::borrowck::body::body_anon_consts(&uu.prog, &uu.tcx, k) {
                if !of(owner) { continue; }
                let errs = settled_def(up, anon, "the constant", || relax_types::body_facts::with_owner(&uu.prog, owner, || {
                    let outer = uu.tcx.typeck(&uu.prog, owner);
                    let owner_typed = outer.has_expr(e) && outer.expr_typed.get(outer.expr_ix(e));
                    let body = uu.tcx.typeck(&uu.prog, anon);
                    if !owner_typed && body.errors.is_empty() && outer.errors.is_empty() {
                        relax_types::lint::check_literals(&uu.prog, &uu.tcx, &body);
                        relax_types::lint_dead::collect_body(&uu.prog, &uu.tcx, &body);
                    }
                    body.errors.to_vec()
                })).unwrap_or_else(|failed| vec![failed]);
                if !errs.is_empty() { unclean.insert(owner); }
                errors.extend(staged(Stage::of(Phase::Typeck, 0, anon), errs));
            }
        }
    };
    body_anons(&|owner| !replayed.contains(&owner), &mut errors, &mut unclean);
    relax_types::body_facts::close();
    let facts = relax_types::body_facts::take();
    // (the guard at the end: no replayed body was type-checked)
    #[cfg(test)]
    let mut replayed_guard: Vec<crate::program::DefId> = replayed.iter().copied().collect();
    if let Some(b) = bodies.as_mut() {
        errors.extend(b.collect(&u.prog, &facts, &unclean).into_iter().map(|e| (Stage::new(Phase::Post, 0), e)));
        if b.verifying() && subjects_mode() == 0 {
            eprintln!("relax check: {} stored body verdicts verified, {} differ", b.stats.verified, b.stats.differ);
        }
    }
    #[cfg(test)]
    if crate::check_bodies::FACT_CODEC_CHECK.with(|f| f.get()) {
        crate::check_bodies::assert_round_trip(&u.prog, &u.tcx, &own_all, &facts);
    }
    relax_types::body_facts::publish(&u.prog, facts);
    #[cfg(test)]
    assert_eq!(relax_types::body_facts::escaped(), 0, "BodyFacts escape guard: body-phase sink writes without an owner");
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
    let deferred = relax_types::lint_dead::finish(&u.prog, &u.tcx, &own, root, analyses_ok, &erroneous_imports, &replayed_crates); st.mark("lint_dead::finish");
    // a crate with replayed bodies held its import reports back: one the recorded uses decide
    // would say something (lint_imports::Judge), and a stored verdict that lacks a use would make
    // it false. Its replayed bodies are checked now, in a second round, their facts published, and
    // its imports judged again on them
    if !deferred.is_empty() {
        let t_again = std::time::Instant::now();
        let krates: std::collections::HashSet<u32> = deferred.iter().map(|d| d.krate).collect();
        let again: Vec<usize> = (0..fns.len()).filter(|&i| replayed.contains(&fns[i].0.owner) && krates.contains(&u.prog.def(fns[i].0.owner).krate)).collect();
        let again_owners: Vec<crate::program::DefId> = again.iter().map(|&i| fns[i].0.owner).collect();
        let again_set: std::collections::HashSet<crate::program::DefId> = again_owners.iter().copied().collect();
        relax_types::body_facts::open();
        let mut outs = par_map(&again, jobs, |&i| check_one(&fns[i].0, fns[i].1, false));
        // (an owner that met an unloaded module: every skipped file loaded, checked again)
        if outs.iter().any(|o| o.needs_load) && unsafe { !(&*up).prog.skipped_files.is_empty() } {
            unsafe { jit::load_all_skipped(up) };
            let redo: Vec<usize> = (0..outs.len()).filter(|&j| outs[j].needs_load).collect();
            let redone = par_map(&redo, jobs, |&j| check_one(&fns[again[j]].0, fns[again[j]].1, false));
            for (j, o) in redo.into_iter().zip(redone) { outs[j] = o; }
        }
        let mut again_unclean: std::collections::HashSet<crate::program::DefId> = std::collections::HashSet::new();
        for (j, mut o) in outs.into_iter().enumerate() {
            let d = again_owners[j];
            if o.unchecked { o.errors.push((Stage::of(Phase::Typeck, 0, d), def_ice(unsafe { &(*up).prog }, d, "the body", "it needs a file to be loaded and none is left"))); }
            if o.unchecked || !o.errors.is_empty() || !o.diags.is_empty() { again_unclean.insert(d); }
            errors.extend(o.errors);
            borrow_diagnostics.extend(o.diags);
        }
        body_anons(&|owner| again_set.contains(&owner), &mut errors, &mut again_unclean);
        relax_types::body_facts::close();
        let facts = relax_types::body_facts::take();
        let differ = match (bodies.as_mut(), reuse.as_deref()) {
            (Some(b), Some(r)) => b.rechecked(&u.prog, &u.tcx, r.idents(), &facts, &again_unclean, &again_owners),
            _ => Vec::new(),
        };
        let candidates: Vec<String> = deferred.iter().flat_map(|d| d.candidates.iter().cloned()).collect();
        let cleared = relax_types::lint_dead::finish_imports(&u.prog, &u.tcx, deferred, &erroneous_imports, facts);
        #[cfg(test)]
        replayed_guard.retain(|d| !again_set.contains(d));
        if subjects_mode() == 0 {
            eprintln!("relax check: unused-import candidates in {} crate(s) with replayed bodies: {} replayed bodies checked in {:.2} s", krates.len(), again.len(), t_again.elapsed().as_secs_f64());
            for c in &candidates { eprintln!("relax check:   candidate: {c}"); }
            eprintln!("relax check: unused-import candidates cleared by a recheck: {}", cleared.len());
            for c in &cleared { eprintln!("relax check:   cleared: {c}"); }
            for w in &differ { eprintln!("relax check:   a stored verdict differs from its recheck: {w}"); }
        }
        st.mark("recheck of replayed bodies");
    }
    // item text a body computed first (body_facts.rs item_text): each is a cell of a fresh crate
    // the owners phase did not compute, so its uses could depend on which crates are fresh
    if bodies.is_some() && subjects_mode() == 0 {
        let (n, kinds) = relax_types::body_facts::items_in_bodies();
        eprintln!("relax check: items resolved in a body: {n}{}", if kinds.is_empty() { String::new() } else { format!(" ({kinds})") });
    }
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
    // the type/borrow check's freed memory goes back before the compile starts (a large app: 4.6 GB
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
        if is_internal(unsafe { &*up }, e) && !e.contains(": internal:") && !e.starts_with("internal: ") {
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
        let e = if is_internal(unsafe { &*up }, &e) { label_internal(&e) } else { e };
        errors.push((unit_stage(&e), e));
    }
    let mut compile_errors = Vec::new();
    for (id, e) in failures {
        let name = unsafe { (&(*up).fns)[id as usize].name.clone() };
        report(up, id, &name, &e, "compile", &mut compile_errors);
    }
    errors.extend(staged(Stage::new(Phase::Post, 0), compile_errors));
    // RELAX_BORROWCK_INSTANCES=1: every reached instance is borrow-checked too (the background
    // checker's view: concrete type arguments, all crates)
    if crate::borrowck::enabled(options) && options.borrowck_instances {
        let mut borrow_errors = Vec::new();
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
                // the check reads the body as it types it (opaques kept), not the revealed instance
                let v = match crate::typeck::borrowck_body(&uu.prog, &uu.tcx, d, &args) {
                    Ok(checked) => crate::borrowck::check_body(&crate::tcx::EvalCx::new(&uu.prog, &uu.tcx, crate::tcx::TypingEnv::fully_monomorphized(&uu.prog), &crate::tcx::EvalState::new()), &uu.prog, &uu.tcx, &checked),
                    Err(_) => unnormalized_verdict(&uu.prog, d),
                };
                crate::borrowck::note_verdict(&v);
                borrow_errors.extend(verdict_gaps(&uu.prog, d, &v));
                borrow_diagnostics.extend(crate::borrowck::lazy::report_checked(&uu.prog, 0, uu.tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst), d, &body.args, v.errors, crate::borrowck::event::Origin::Check).iter().map(|e| e.error.to_diagnostic()));
            }));
            let _ = r;
        }
        for e in borrow_errors { errors.push((unit_stage(&e), e)); }
    }
    let bodies_blocked;
    unsafe {
        // a delayed bug no error explains (rustc: an ICE at session end) is one internal failure
        // of the check, never the end of it
        // rustc DiagCtxt::flush_delayed: delayed bugs are ICEs only when no error was emitted
        let explained = !errors.is_empty() || !borrow_diagnostics.is_empty() || !(*up).errors.is_empty() || (*up).query_errors.has_errors();
        let delayed = (*up).query_errors.take_delayed();
        // no body verdict is stored by a check that met a failure no body is charged with: a
        // constant's evaluation, a query's error, a delayed bug, a site accepted only in relaxed Rust
        bodies_blocked = !delayed.is_empty() || !(*up).errors.is_empty() || (*up).query_errors.has_errors()
            || relax_types::borrowck::relaxed_pending() || crate::relaxed::pending();
        for bug in delayed {
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
        eprintln!("relax check: {} crates checked, {} fns compiled from {} seeds, {} failed builds; type/borrow check {:.1} s, compile {:.1} s", crates.len(), unsafe { (&(*up).fns).len() }, seeds.len(), n_failed, t_front.as_secs_f64(), t_compile.as_secs_f64());
    } else if !quiet {
        if let Some(r) = reuse.as_deref().filter(|r| r.hits() > 0) {
            eprintln!("relax check: {} of {} crates unchanged since a passing check: their verdicts reused", r.hits(), r.crates());
        }
        eprintln!("relax check: {} crates type- and borrow-checked in {:.1} s", fresh_crates.len(), t_front.as_secs_f64());
    }
    // RELAX_SOLVER_WORK_TRACE: the solver's work counts and goal histograms of the check
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
        let seq = if crate::check_order::keeps_report_order(stage, r.lint) { r.seq } else { 0 };
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
        if let Some(b) = bodies.take() {
            let stats = b.store(r, unsafe { &(*up).prog }, unsafe { &(*up).tcx }, bodies_blocked);
            if std::env::var_os("RELAX_CHECK_TIMES").is_some() {
                eprintln!("relax check: body verdicts: {} owners, {} replayed, {} stored anew; keys and decoding {:.1} ms, encoding {:.1} ms",
                    stats.owners, stats.replayed, stats.stored, stats.open.as_secs_f64() * 1e3, stats.store.as_secs_f64() * 1e3);
            }
        }
        if late_ok && entries.iter().all(|e| matches!(e.out, Out::Warning(_))) { r.record(unsafe { &(*up).prog }, &entries, &flags); }
        r.flush_bodies(options.verbose);
        entries.extend(r.replayed(late_ok, wf_ran));
    }
    #[cfg(test)]
    for &d in &replayed_guard {
        let uu = unsafe { &*up };
        assert!(uu.tcx.typeck_peek(&uu.prog, d).is_none(), "a body replayed from its stored verdict was type-checked: {}", uu.prog.def_path(d));
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
    eprintln!("relax check: {} body owners of the checked crates, {} compiled in all, {} errors", n_own, unsafe { (*up).stats.compiled }, errors.len() + borrow_count);
    // the borrow check's fn-signature binder scopes by how they closed (binder_scope.rs)
    let [eliminated, later, test, aside] = relax_types::borrowck::type_check::scope_closes();
    if eliminated + later + test + aside > 0 {
        eprintln!("relax check: binder scopes: {eliminated} eliminated, {later} kept for a later placeholder, {test} kept for a type test; {aside} nested components set aside");
    }
    }
    if !quiet {
        let st = unsafe { &(*up).stats };
        eprintln!(
            "relax check: cpu typeck {} ms, lower {} ms, opt {} ms, codegen {} ms; {} RIR insts, {} KB code",
            st.typeck_ns / 1_000_000,
            st.lower_ns / 1_000_000,
            st.opt_ns / 1_000_000,
            st.codegen_ns / 1_000_000,
            st.rir_insts,
            st.code_bytes / 1024
        );
        let (twins, bytes, ms) = crate::ctfe_twin::census(unsafe { &*up });
        eprintln!("relax check: constant initializers: {} evaluated to data, {} compiled and run ({} constant-evaluation twins, {:.1} KB, built in {:.1} ms); evaluation cpu {:.1} ms", st.ctfe_data, st.ctfe_code, twins, bytes as f64 / 1024.0, ms, st.ctfe_ns as f64 / 1e6);
    }
    unsafe { crate::jit::backend_bench(up) };
    if std::env::var("RELAX_PASS_TIMES").is_ok() || std::env::var("RELAX_OPT_HIST").is_ok() {
        let mut passes = String::new();
        for i in 0..relax_backend::stats::PASS_NAMES.len() {
            passes.push_str(&format!("{} {:.0}  ", relax_backend::stats::PASS_NAMES[i], relax_backend::stats::PASS_NS[i].load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6));
        }
        eprintln!("relax check: passes (cpu ms): {}", passes);
        eprintln!("relax check: opt ms by size (<16,<64,<256,<1k,<4k,more) {:?} fns {:?}", relax_codegen::opt::HIST[..6].iter().map(|x| x.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000).collect::<Vec<_>>(), relax_codegen::opt::HIST[6..].iter().map(|x| x.load(std::sync::atomic::Ordering::Relaxed)).collect::<Vec<_>>());
    }
    let _ = checked;
    crate::relaxed::finish_check();
    let mut diagnostics: Vec<_> = errors.iter().map(|e| crate::diag::from_legacy(e, "typeck")).collect();
    diagnostics.extend(borrow_diagnostics);
    let foreign = foreign_errors(&u.prog, crates, errors.iter().filter(|e| !e.contains(": internal:") && !e.starts_with("internal: ")))
        .into_iter().filter(|(name, _)| name != "?").collect();
    (i32::from(!errors.is_empty() || borrow_count != 0), diagnostics, foreign)
}

/// Compiles every seed fn, then every slot the compiled code creates (callees, closures, glue,
/// vtable methods), each through the run's own per-fn compile (`jit::compile_slot`: front end
/// on the owner thread, optimisation and code generation off it), on RELAX_JOBS threads until
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
    let jobs = std::env::var("RELAX_JOBS").ok().and_then(|v| v.parse::<usize>().ok()).filter(|n| *n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()).min(8));
    // the run's reclamation (jit::settle_drop: RIR, checked bodies, templates of compiled
    // slots) once the process grew SETTLE_GROWTH since the last one (looked at every 2,000
    // compiled slots), so a whole-program check keeps the working set of a run, not every body
    // it ever built (a large app went past 18 GB without it). Dropped RIR is rebuilt for inlining:
    // settling every 20,000 slots made a large app's compile 4x slower.
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

pub(crate) fn std_crate(prog: &crate::program::Program, k: u32) -> bool {
    use crate::vfs::canonical as canon;
    let c = &prog.crates[k as usize];
    if matches!(prog.syms.str(c.name), "core" | "alloc" | "std" | "proc_macro") { return true; }
    let std_dir = format!("{}/", canon(&crate::std_dir()).trim_end_matches('/'));
    canon(c.root_path()).starts_with(&std_dir)
}

/// Is a failure a makepad run bug rather than an error rustc would report? A compiler panic, an
/// internal error, an error without an error code, or one located in std or outside any crate.
/// An error in any other crate belongs to the program, including a dependency cargo builds.
fn is_internal(u: &Unit, e: &str) -> bool {
    let first = e.lines().next().unwrap_or("");
    if first.contains("internal compiler error") || first.contains("internal:") || first.contains("Relax bug") {
        return true;
    }
    // `path:line:col: E0123: ..`
    let Some((path, _, _, Some(rest))) = place_of(first) else { return true };
    let rest = rest.trim_start();
    // (an error rustc words without a code: Program::record_uncoded_error)
    let coded = rest.starts_with(relax_front::diagnostics::UNCODED_PREFIX) || (rest.len() > 5 && rest.starts_with('E') && rest[1..5].bytes().all(|b| b.is_ascii_digit()));
    if !coded {
        return true;
    }
    match u.prog.file_paths.iter().position(|p| p == path) {
        Some(i) => u.prog.file_crate.get(i).map_or(true, |&k| std_crate(&u.prog, k)),
        None => true,
    }
}

/// The path, line, column and the text after them (None: nothing follows the column) of an
/// error whose first line starts `path:line:col`. The path ends at the first colon that a line
/// and a column follow, so a Windows drive's own colon (`C:\ws\lib.rs:1:19: E0425: ..`) is
/// the path's: split at every colon, each located error of a check on Windows was `internal`.
pub(crate) fn place_of(e: &str) -> Option<(&str, &str, &str, Option<&str>)> {
    let first = e.lines().next().unwrap_or("");
    for (i, _) in first.match_indices(':') {
        let mut it = e[i + 1..].splitn(3, ':');
        if let (Some(l), Some(c)) = (it.next(), it.next()) {
            if l.trim().parse::<u32>().is_ok() && c.trim().parse::<u32>().is_ok() {
                return Some((&e[..i], l, c, it.next()));
            }
        }
    }
    None
}

/// `path:line:col: internal: msg` for a makepad run bug (the error keeps its own text otherwise).
pub(crate) fn label_internal(e: &str) -> String {
    if let Some((p, l, c, Some(rest))) = place_of(e) {
        return format!("{}:{}:{}: internal:{}", p, l, c, rest);
    }
    format!("internal: {}", e)
}

/// One failed build as an error line: its location (the error's own, else the fn's), the
/// message's first line, the instance it failed for, and `internal` for a makepad run bug.
fn report(up: *mut Unit, id: u32, name: &str, e: &str, phase: &'static str, errors: &mut Vec<String>) {
    let u = unsafe { &*up };
    let mut first = e.lines().next().unwrap_or("").to_string();
    // an error without a location takes its fn's
    let located = place_of(&first).is_some();
    if !located {
        if let Some((file, pos)) = function_origin(u, id) {
            let path = &u.prog.file_paths[file as usize];
            let (l, c) = crate::lexer::line_col(&u.prog.files[file as usize].src, pos);
            first = format!("{}:{}:{}: {}", path, l, c, first);
        }
    }
    let internal = is_internal(u, &first);
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
        if internal { d.notes.push("internal: a Relax bug, not an error in the program".to_string()); }
        crate::diag::record(line.clone(), d);
    }
    errors.push(line);
}

fn function_origin(u: &Unit, mut id: u32) -> Option<(u32,u32)> {
    for _ in 0..64 {
        match &u.fns[id as usize].key {
            FnKey::Inst(d,_) => { let d = u.prog.def(*d); return Some((d.file,u.prog.files[d.file as usize].ast.item(d.item).lo)); }
            FnKey::Closure(f,e,_) => return Some((*f,u.prog.files[*f as usize].ast.expr(crate::ast::ExprId(*e)).lo)),
            FnKey::Sandbox(parent) | FnKey::Plain(parent) | FnKey::Ctfe(parent) => id = *parent,
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
pub(crate) fn error_position(e: &str) -> (String, u32, u32, String) {
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
pub(crate) fn is_follow_on(e: &str) -> bool {
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
    // RELAX_CHECK_TIMES: where the time goes (typeck lookups, owner-call queueing, statics, consts)
    let times = std::env::var_os("RELAX_CHECK_TIMES").is_some();
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
                if is_static {
                    if !u.static_is_shared(d) { u.static_addr(d); }
                } else { u.const_addr(d, Vec::new()); }
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
        stderr_line!("relax check: eval_items {} items: interpreter ahead {:.2} s ({} items), typeck lookups {:.2} s, owner calls {:.2} s, {} statics {:.2} s, {} consts {:.2} s",
            n_items, t_ahead.as_secs_f64(), n_ahead, t_tyck as f64 / 1e9, t_queue as f64 / 1e9, n_static, t_static as f64 / 1e9, n_const, t_const as f64 / 1e9);
        let ns = |k: usize| crate::jit::INIT_NS[k].load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e9;
        stderr_line!("relax check: initializers (cpu, whole check): interpreter {:.2} s, to data {:.2} s, compiled and run {:.2} s, eval_init in all {:.2} s, cache records {:.2} s",
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
    reset(&AUDIT_DIR, std::sync::OnceLock::new());
    reset(&CHECK_EARLY, std::sync::atomic::AtomicBool::new(false));
    reset(&CHECK_WAITED, std::sync::atomic::AtomicBool::new(false));
    reset(&CHECK_WAITED_AT, std::sync::atomic::AtomicU64::new(0));
    reset(&BESIDE, (relax_session::sync::Mutex::new(0), relax_session::sync::Condvar::new()));
    reset(&APP_CHECK_THREAD, std::sync::atomic::AtomicU64::new(0));
    reset(&COORDINATOR, relax_session::sync::Mutex::new(NEW_COORDINATOR));
    reset(&RUN_CHECK, [const { std::sync::atomic::AtomicU64::new(0) }; 8]);
    reset(&APP_CHECK_CPU, std::sync::atomic::AtomicU64::new(0));
    reset(&CHECK_SLICES, [const { std::sync::atomic::AtomicU64::new(0) }; 5]);
}

#[cfg(test)]
mod def_work_tests {
    use super::{def_work, DefWork};

    #[test]
    fn finish_app_check_waits_for_the_verdict_unless_the_exit_holds_the_owner() {
        use super::{app_check_done, check_waited, finish_app_check, APP_CHECK_THREAD, BESIDE};
        use std::sync::atomic::Ordering::Relaxed;
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        // Isolate the run's globals and capture what the owner-held exit actually reported.
        const NAME: &str = "check::def_work_tests::finish_app_check_waits_for_the_verdict_unless_the_exit_holds_the_owner";
        if std::env::var("RELAX_TEST_CHILD").as_deref() != Ok(NAME) {
            let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--test-threads=1", "--nocapture"]).env("RELAX_TEST_CHILD", NAME));
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success() && stdout.contains("1 passed"), "{stdout}\n{stderr}");
            let exits: Vec<_> = stderr.lines().filter(|l| l.contains("the exiting thread holds the owner")).collect();
            assert_eq!(exits, ["relax: exiting with code 7 while the app check runs: the exiting thread holds the owner"]);
            return;
        }

        let holder = crate::waits::me();
        APP_CHECK_THREAD.store(holder, Relaxed);
        *BESIDE.0.lock().unwrap() = 1;
        assert!(crate::owner::queued_note().contains("\"check\":1"));
        assert!(!check_waited(), "observing the check must not promote it");

        let (owner_tx, owner_rx) = mpsc::channel();
        let owner = std::thread::spawn(move || crate::owner::call(crate::owner::Class::Program, || {
            assert!(crate::run::exit_caller_holds_owner());
            owner_tx.send(finish_app_check(7)).unwrap();
        }));
        let own_code = owner_rx.recv_timeout(Duration::from_secs(5));

        let (tx, rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let id = crate::waits::me();
            tx.send(id).unwrap();
            let code = finish_app_check(0);
            (code, crate::waits::of(id).is_none())
        });
        let id = rx.recv().unwrap();
        let start = Instant::now();
        let registered = loop {
            if crate::waits::of(id).is_some_and(|w| w.kind == crate::waits::Kind::Check && w.holder == holder && w.label == "the run's end")
                && check_waited() && crate::preempt::ui_waits() {
                break true;
            }
            if start.elapsed() >= Duration::from_millis(500) { break false; }
            std::thread::sleep(Duration::from_millis(1));
        };
        let width = super::check_width();
        app_check_done(3);
        let (error_code, cleared) = waiter.join().unwrap();
        owner.join().unwrap();
        assert_eq!(own_code.unwrap(), 7, "the owner-held exit keeps its code before the failing verdict");
        assert!(registered, "the parked exit must name the check thread and promote the check");
        assert_eq!(width, makepad_cpu_budget::cores().max(1) as usize, "a waited check's batches take every core");
        assert_eq!(error_code, 101, "the non-owner exit takes the failing verdict");
        assert!(cleared && !check_waited() && !crate::preempt::ui_waits(), "the row and promotion must clear");
        assert!(crate::owner::queued_note().contains("\"check\":0"));
        app_check_done(2);
        assert_eq!(finish_app_check(8), 8, "a clean verdict preserves the exit code");
        crate::owner::call(crate::owner::Class::Program, || assert_eq!(finish_app_check(9), 9));

        // A slice queued before the wait began counts from the wait's start.
        let counted0 = super::CHECK_SLICES[4].load(Relaxed);
        let old_at = super::CHECK_WAITED_AT.swap(crate::clock::wall_ns(), Relaxed);
        super::slice_queued(crate::clock::wall_ns().saturating_sub(5_000_000_000));
        let counted = super::CHECK_SLICES[4].load(Relaxed) - counted0;
        super::CHECK_WAITED_AT.store(old_at, Relaxed);
        assert!(counted < 1_000_000_000, "a slice's queue time before the wait began was counted: {counted} ns");
    }

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
