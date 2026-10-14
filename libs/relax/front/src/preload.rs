//! Parallel Rust source scans and parsing with ordered macro publication.

use crate::ast::{Attr, TokRange};
use crate::cfg::{CfgSet, Src};
use crate::lexer::{Tok, T};
use crate::macros::{MacroRef, MacroTable, MacroUse};
use crate::parser::ParsedFile;
use crate::fxhash::{HashMap, HashSet};
use std::sync::Arc;
use relax_session::sync::{Condvar, Mutex};

#[path = "lazy.rs"]
mod lazy;
pub use lazy::PendingPlan as DemandPlan;
pub(crate) use lazy::items as rs_items;
pub use lazy::{DemandPart, DemandImage, FileIndex as FileIndex};

pub(crate) fn configured_tokens(src: &[u8], tokens: &[Tok], cfg: &CfgSet) -> Vec<Tok> {
    lazy::demand::active_tokens(src, tokens, cfg)
}

/// One crate of a boot, in load order.
#[derive(Clone)]
pub struct PreCrate {
    pub name: String,
    pub root: String,
    pub edition: u16,
    pub cfg: CfgSet,
    /// the extern prelude: (name the crate uses, the dependency's crate name)
    pub deps: Vec<(String, String)>,
    pub proc_macro: bool,
    /// Indices in this preload, in Program dependency order (including implicit std).
    pub image_deps: Vec<usize>,
    pub image_dep_names: Vec<(String, String)>,
}

/// What load_crate takes for one crate.
pub struct CrateAhead {
    pub name: String,
    /// index of the crate in its preload (parse results are keyed by it and the path)
    pub idx: u32,
    /// `macro_rules!` definitions in prescan order
    pub defs: Vec<MacroRef>,
    /// macro imports under another name ((crate, alias) -> (first path segment, orig))
    pub aliases: Vec<((String, String), (String, String))>,
    pub macro_use: Vec<String>,
    pub mods: Arc<crate::macros::CrateMods>,
    /// every file found, sorted by path (each is a parse job unless there are no workers)
    pub files: Vec<String>,
    /// without workers: (path, src, tokens) of the files read and lexed by the scan
    pub prelexed: Vec<(String, Vec<u8>, Vec<Tok>)>,
    pub image: bool,
    /// restored, or parsed with a validated record (CrateImage::fp_only): its store is current
    pub valid: bool,
    /// A failed scan cannot certify the crate's complete macro/module input set.
    pub complete: bool,
    /// Includes scanned files that collection later skips. Their macros still matter.
    pub inputs: Vec<(String, u128)>,
    /// Module resolution choices, including absent earlier candidates.
    pub probes: Vec<(String, bool)>,
    /// Indexed module files omitted by this load's demand closure.
    pub deferred: Vec<String>,
    /// Only this crate's indexed/reached selection, compared after demand planning.
    pub demand_plan: u128,
}

pub struct Preparsed {
    /// A lowered file parsed in the loader's scope failed; retrying cannot change it.
    pub error: Option<String>,
    pub verify: Option<fn(&ParsedFile, &ParsedFile)>,
    pub parsed: Option<(ParsedFile, MacroUse)>,
    /// cfg strip, identifier list and body block index (Program::prepare_file)
    pub prep: Option<crate::program::FilePrep>,
    pub mod_path: String,
    pub src_len: usize,
}

/// Optional embedder check for complete worker/collection-scope parse equivalence.
pub static VERIFY: std::sync::OnceLock<fn(&ParsedFile, &ParsedFile)> = std::sync::OnceLock::new();

struct Job {
    /// load-order crate index
    krate: u32,
    path: String,
    mod_path: String,
    src: Vec<u8>,
    toks: Vec<Tok>,
    ctx: Arc<CrateCtx>,

}

/// Prepared without the pool lock, installed in crate order under that lock.
struct Publication {
    ahead: CrateAhead,
    jobs: Vec<Job>,
    done: Vec<(String, Preparsed)>,
}

struct CrateCtx {
    context: Arc<crate::source_context::Context>,
    name: String,
    edition: u16,
    cfg: CfgSet,
    table: Arc<MacroTable>,
}

/// (crate index, file, its dir for `mod x;`, its module path)
type ScanKey = (usize, String, String, String);
/// Lowerings before Rust work; unblock dependency crates first, largest file
/// first within each crate. Large leaf crates must not delay the loader's first
/// lowered dependency (and the serial macro work that can overlap later scans).
type ScanPriority = (usize, u64);

struct PoolState {
    stopped: bool,
    plan_pending: bool,
    planning: bool,
    /// Crates whose loader has published the dependency macro imports and own definitions.
    parse_ready: HashSet<u32>,
    images: Vec<Option<crate::crate_image::CrateImage>>,
    pending_images: Vec<Option<crate::crate_image::CrateImage>>,
    /// per crate: its record validated (restored or fingerprint only)
    valid: Vec<bool>,
    scans: std::collections::BTreeMap<ScanPriority, ScanKey>,
    scan_seq: u64,
    seen: HashSet<ScanKey>,
    /// per crate: scans queued or running
    scan_left: Vec<usize>,
    scanned: Vec<HashMap<ScanKey, Option<Scanned>>>,
    /// One publisher owns the table outside the queue lock until its crate commits.
    publishing: bool,
    /// crates whose definitions and parse jobs are out, in load order
    published: usize,
    /// Highest crate allowed to scan/parse (cold write-then-map bounds the queue).
    through: Option<u32>,
    /// the macro table as the next crate to publish starts
    table: MacroTable,
    ahead: HashMap<u32, CrateAhead>,
    /// parse jobs by (crate, larger first, arrival)
    jobs: std::collections::BTreeMap<(u32, std::cmp::Reverse<usize>, u64), Job>,
    job_seq: u64,
    done: HashMap<(u32, String), Preparsed>,
    running: HashSet<(u32, String)>,
    cancelled: HashSet<(u32, String)>,
}

pub struct Pool {
    pub demand_plan: Option<Arc<DemandPlan>>,
    /// A collected prefix can reuse a completed plan for the entire graph.
    plan_offset: usize,
    contexts: Vec<Arc<crate::source_context::Context>>,
    crates: Vec<PreCrate>,
    /// parse on workers (false: the loader parses, from the scan's tokens)
    parse: bool,
    st: Mutex<PoolState>,
    cv: Condvar,
}

impl Drop for Pool {
    fn drop(&mut self) {
        for context in &self.contexts { context.release_prescan(); }
        if let Some(context) = self.contexts.first() { context.report_host_profile(); }
        if trace() {
            let st = self.st.lock().unwrap();
            for (p, x) in st.done.iter() {
                eprintln!("boot-trace-left: {} {}", p.1, x.src_len);
            }
        }
    }
}

enum Work {
    Plan,
    Scan(ScanKey),
    Parse(Job),
}

impl Pool {
    /// Start this crate's parses only once the sequential loader has the macro environment
    /// its dependencies produced (including imports emitted by macro expansions).
    pub fn start_parsing(&self, ci: u32, table: Arc<MacroTable>) {
        let mut st = self.st.lock().unwrap();
        let c = &self.crates[ci as usize];
        let ctx = Arc::new(CrateCtx { context: self.contexts[ci as usize].clone(), name: c.name.clone(), edition: c.edition, cfg: c.cfg.clone(), table });
        for job in st.jobs.values_mut().filter(|j| j.krate == ci) { job.ctx = ctx.clone(); }
        st.parse_ready.insert(ci);
        self.cv.notify_all();
    }

    pub fn context(&self, ci: u32) -> Arc<crate::source_context::Context> { self.contexts[ci as usize].clone() }
    /// The sequential loader's reads go through the pool's snapshot while it loads a crate
    /// (a scope on the loader's stack, for that load only).
    pub fn bind_reads(&self) -> Option<crate::vfs::ReadScope> { self.contexts.first().map(|c| c.bind_source_reads()) }
    /// What the scan found for crate `ci` (its definitions, files), helping with the scan while
    /// it is not complete.
    /// Crate `ci`'s dependencies as the preload saw them (the crate image's identity: extern
    /// crate aliases its sources add later are not part of it).
    pub fn dep_names(&self, ci: u32) -> Vec<(String, String)> {
        self.crates.get(ci as usize).map(|c| c.image_dep_names.clone()).unwrap_or_default()
    }

    pub fn take_ahead(&self, ci: u32) -> CrateAhead {
        let profile = self.crates[ci as usize].cfg.session.runtime.profile;
        let _profile = crate::load_profile::span(profile, "loader-ahead", &self.crates[ci as usize].name);
        let t = std::time::Instant::now();
        let mut st = self.st.lock().unwrap();
        if let Some(through) = &mut st.through { *through = (*through).max(ci.saturating_add(1)); }
        self.cv.notify_all();
        loop {
            if let Some(a) = st.ahead.remove(&ci) {
                WAIT_NS.fetch_add(t.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
                return a;
            }
            // the loader thread helps with scans (never with a parse: those must not wait on it)
            if let Some(key) = Self::next_scan(&st, Some(ci as usize)) {
                let key = st.scans.remove(&key).unwrap();
                drop(st);
                self.scan(key);
                st = self.st.lock().unwrap();
                continue;
            }
            let _wait = crate::load_profile::span(profile, "wait-scan", &self.crates[ci as usize].name);
            st = self.cv.wait(st).unwrap();
        }
    }

    /// The parse of a submitted file; a job not started yet is parsed right here.
    pub fn wait(&self, sess: &std::sync::Arc<crate::Session>, krate: u32, path: &str) -> Option<Preparsed> {
        self.wait_in_scope(sess, krate, path, None)
    }

    pub fn wait_in_scope(&self, sess: &std::sync::Arc<crate::Session>, krate: u32, path: &str, table: Option<Arc<MacroTable>>) -> Option<Preparsed> {
        let _profile = crate::load_profile::span(sess.runtime.profile, "loader-file", path);
        let t = std::time::Instant::now();
        let key = (krate, path.to_string());
        let r = self.wait_inner(sess, &key, table);
        WAIT_NS.fetch_add(t.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        r
    }

    fn wait_inner(&self, sess: &std::sync::Arc<crate::Session>, key: &(u32, String), table: Option<Arc<MacroTable>>) -> Option<Preparsed> {
        let mut st = self.st.lock().unwrap();
        loop {
            if let Some(x) = st.done.remove(key) {
                return Some(x);
            }
            if !st.running.contains(key) {
                let k = st.jobs.iter().find(|(_, j)| j.krate == key.0 && j.path == key.1).map(|(k, _)| *k);
                let Some(k) = k else { return None };
                let mut job = st.jobs.remove(&k).unwrap();
                {
                    if let Some(table) = table {
                        let ctx = &job.ctx;
                        job.ctx = Arc::new(CrateCtx { context: ctx.context.clone(), name: ctx.name.clone(), edition: ctx.edition, cfg: ctx.cfg.clone(), table });
                    }
                }
                self.cv.notify_all();
                drop(st);
                return Some(parse_job(sess, job, true).1);
            }
            let _wait = crate::load_profile::span(sess.runtime.profile, "wait-parse", &key.1);
            st = self.cv.wait(st).unwrap();
        }
    }

    /// Files a crate's modules did not load (cfg'd out in ways the scan cannot see): drop their
    /// jobs and results.
    pub fn cancel(&self, krate: u32, paths: &[String]) {
        let mut st = self.st.lock().unwrap();
        let set: HashSet<&String> = paths.iter().collect();
        st.jobs.retain(|_, j| j.krate != krate || !set.contains(&j.path));
        for p in paths {
            let key = (krate, p.clone());
            st.done.remove(&key);
            if st.running.contains(&key) {
                st.cancelled.insert(key);
            }
        }
        self.cv.notify_all();
    }

    pub fn stop(&self) {
        self.st.lock().unwrap().stopped = true;
        self.cv.notify_all();
    }

    pub fn unbound(&self) {
        self.st.lock().unwrap().through = None;
        self.cv.notify_all();
    }

    fn scan(&self, key: ScanKey) {
        let c = &self.crates[key.0];
        // Rust scans preserve the full macro surface regardless of selection.
        // They can run before the closure; only their parse jobs need its answer.
        let r = scan_file(&key.1, &key.2, &key.3, &c.name, c.edition, &c.cfg, &self.contexts[key.0]);
        let lock = crate::load_profile::span(c.cfg.session.runtime.profile, "scan-publish-lock", &key.1);
        let mut st = self.st.lock().unwrap();
        drop(lock);
        if let Some(s) = &r {
            for (p, d, m) in &s.children {
                let k = (key.0, p.clone(), d.clone(), m.clone());
                if st.seen.insert(k.clone()) {
                    st.scan_seq += 1;
                    let seq = st.scan_seq;
                    st.scans.insert((key.0, seq), k);
                    st.scan_left[key.0] += 1;
                }
            }
        }
        let ci = key.0;
        st.scanned[ci].insert(key, r);
        st.scan_left[ci] -= 1;
        drop(self.publish(st));
        self.cv.notify_all();
    }

    fn next_scan(st: &PoolState, loader: Option<usize>) -> Option<ScanPriority> {
        st.scans.iter().find(|(_, key)| {
            let ci = &key.0;
            st.through.map_or(true, |t| *ci <= t as usize) && loader.map_or(true, |t| *ci <= t)
        }).map(|(priority, _)| *priority)
    }

    // Candidates are source-validated during preload setup. Selection validation
    // belongs to the planning worker, never a dereference on the boot thread.
    fn resolve_images(&self, st: &mut PoolState) {
        let Some(pending) = &self.demand_plan else { return; };
        for ci in 0..self.crates.len() {
            if st.pending_images[ci].is_none() { continue; }
            let plan_ci = ci + self.plan_offset;
            let selection = if let Some(plan) = pending.ready() {
                Some((plan.selection_identity(plan_ci), plan.deferred(plan_ci)))
            } else if pending.eagerly_selected(plan_ci) { Some((0, Vec::new())) }
            else { None };
            let Some((selection, deferred)) = selection else { continue; };
            let c = &self.crates[ci];
            if c.image_deps.iter().any(|&d| st.pending_images[d].is_some()) { continue; }
            let image = st.pending_images[ci].take().unwrap();
            let valid = image.ahead.demand_plan == selection && image.ahead.deferred == deferred
                && c.image_deps.iter().all(|&d| st.valid[d]);
            st.valid[ci] = valid;
            if valid && !image.fp_only {
                st.images[ci] = Some(image);
                st.scan_left[ci] = 0;
            } else {
                let key = (ci, c.root.clone(), crate::program::dir_of(&c.root).to_owned(), c.name.clone());
                st.seen.insert(key.clone());
                st.scan_seq += 1;
                        st.scans.insert((ci, st.scan_seq), key);
            }
        }
    }

    fn selection_ready(&self, st: &PoolState, ci: usize) -> bool {
        let pending = self.demand_plan.as_ref().filter(|p| p.ready().is_none());
        // The eager prefix is retained in full; the last crate also needs the
        // whole-graph certificate. Use the same gate for preparation and commit.
        st.planning == false || pending.is_none_or(|p| ci + 1 != self.crates.len() && p.eagerly_selected(ci + self.plan_offset))
    }

    /// Publish complete prescans in load order. Only queue transfers hold st:
    /// hashing, source replay, macro snapshots and dependency ledgers do not.
    fn publish<'a>(&'a self, mut st: relax_session::sync::MutexGuard<'a, PoolState>) -> relax_session::sync::MutexGuard<'a, PoolState> {
        if st.publishing { return st; }
        st.publishing = true;
        self.resolve_images(&mut st);
        while st.published < self.crates.len() && st.scan_left[st.published] == 0 {
            let ci = st.published;
            let c = &self.crates[ci];
            if !self.selection_ready(&st, ci) { break; }
            let mut table = std::mem::take(&mut st.table);
            let scanned = std::mem::take(&mut st.scanned[ci]);
            let image = st.images[ci].take();
            let valid = st.valid[ci];
            drop(st);
            let t = std::time::Instant::now();
            let publication = self.publish_inner(ci, &mut table, scanned, image, valid);
            PUBLISH_NS.fetch_add(t.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
            st = self.st.lock().unwrap();
            let _commit = crate::load_profile::span(c.cfg.session.runtime.profile, "publish-commit", &c.name);
            let queued: HashSet<_> = st.jobs.range(
                (ci as u32, std::cmp::Reverse(usize::MAX), 0)..=(ci as u32, std::cmp::Reverse(0), u64::MAX)
            ).map(|(_, job)| job.path.clone()).collect();
            let mut redundant = Vec::new();
            for job in publication.jobs {
                let key = (ci as u32, job.path.clone());
                if st.done.contains_key(&key) || st.running.contains(&key)
                    || queued.contains(&job.path) {
                    // A reached parse can finish while publication builds. The
                    // loader still validates it against its actual macro scope.
                    redundant.push(job);
                    continue;
                }
                st.job_seq += 1;
                let priority = (ci as u32, std::cmp::Reverse(job.src.len()), st.job_seq);
                st.jobs.insert(priority, job);
            }
            for (path, file) in publication.done { st.done.insert((ci as u32, path), file); }
            st.table = table;
            st.ahead.insert(ci as u32, publication.ahead);
            st.published += 1;
            self.cv.notify_all();
            drop(_commit);
            drop(st);
            // Retire unused speculative input buffers outside the queue lock too.
            drop(redundant);
            st = self.st.lock().unwrap();
            // Planning may have completed while the table was detached.
            self.resolve_images(&mut st);
        }
        st.publishing = false;
        st
    }

    fn publish_inner(&self, ci: usize, table: &mut MacroTable,
        mut scanned: HashMap<ScanKey, Option<Scanned>>,
        image: Option<crate::crate_image::CrateImage>, valid: bool) -> Publication {
        let c = &self.crates[ci];
        let _profile = crate::load_profile::span(c.cfg.session.runtime.profile, "publish", &c.name);
        if let Some(mut image) = image {
            self.contexts[ci].restore_fingerprint(image.selection_hash);
            image.ahead.idx = ci as u32;
            for (k, v) in &image.ahead.aliases {
                table.add_alias(k.0.clone(), k.1.clone(), v.clone());
            }
            for u in &image.ahead.macro_use {
                let v = table.macro_use.entry(c.name.clone()).or_default();
                if !v.contains(u) {
                    v.push(u.clone());
                }
            }
            if c.name == "core" {
                table.s2_fmt = crate::vfs::exists(std::path::Path::new(&c.root).parent().unwrap_or(std::path::Path::new("")).join("fmt/mod.rs"));
            }
            for d in &image.ahead.defs {
                table.push(d.clone());
            }
            table.add_crate(c.name.clone(), image.ahead.mods.clone());
            return Publication { ahead: image.ahead, jobs: Vec::new(), done: image.files };
        }
        let mut complete = true;
        let mut inputs = Vec::new();
        let mut probes = Vec::new();
        let mut stack = vec![(c.root.clone(), crate::program::dir_of(&c.root).to_string(), c.name.clone())];
        let mut seen: HashMap<String, String> = HashMap::default();
        let mut defs = Vec::new();
        let mut aliases = Vec::new();
        let mut macro_use: Vec<String> = Vec::new();
        let mut cmods = crate::macros::CrateMods { root: c.name.clone(), src: c.root.clone(), ..Default::default() };
        for (e, n) in &c.deps {
            cmods.externs.insert(e.clone(), n.clone());
        }
        let mut lexed: Vec<(String, Vec<u8>, Vec<Tok>)> = Vec::new();
        let replay = crate::load_profile::span(c.cfg.session.runtime.profile, "publish-replay", &c.name);
        while let Some((path, dir, mpath)) = stack.pop() {
            if seen.contains_key(&path) {
                continue;
            }
            seen.insert(path.clone(), mpath.clone());
            let Some(Some(s)) = scanned.remove(&(ci, path.clone(), dir, mpath)) else {
                complete = false;
                continue;
            };
            inputs.push((path.clone(), crate::crate_image::file_hash(&path, &s.src[..s.src_len])));
            probes.extend(s.probes);
            for d in s.defs {
                defs.push(Arc::new(d));
            }
            for (alias, qual, target) in s.aliases {
                table.add_alias(c.name.clone(), alias.clone(), (qual.clone(), target.clone()));
                aliases.push(((c.name.clone(), alias), (qual, target)));
            }
            for m in s.mods {
                cmods.mods.insert(m);
            }
            for u in s.uses {
                if let Some(b) = &u.bind {
                    if u.path.last() != Some(b) {
                        cmods.renames.insert(b.clone());
                    }
                    // `extern crate self as b;` at the root: b names this crate everywhere
                    if u.path.len() == 1 && u.path[0] == "crate" && u.module == c.name {
                        cmods.externs.insert(b.clone(), c.name.clone());
                    }
                }
                cmods.uses.entry(u.module.clone()).or_default().push(u);
            }
            for u in s.macro_use {
                let v = table.macro_use.entry(c.name.clone()).or_default();
                if !v.contains(&u) {
                    v.push(u.clone());
                }
                macro_use.push(u);
            }
            // Stack traversal must publish sibling definitions in source order.
            stack.extend(s.children.into_iter().rev());
            if !self.demand_plan.as_ref().is_some_and(|p| p.skips(&c.root, &path)) {
                lexed.push((path, s.src[..s.src_len].to_vec(), s.toks.into_vec()));
            }
        }
        if c.name == "core" {
            let dir = std::path::Path::new(&c.root).parent().map(|p| p.to_path_buf()).unwrap_or_default();
            let path = dir.join("fmt/mod.rs");
            table.s2_fmt = crate::vfs::exists(&path);
            probes.push((path.to_string_lossy().into_owned(), table.s2_fmt));
        }
        drop(replay);
        let snapshot = crate::load_profile::span(c.cfg.session.runtime.profile, "publish-macro-snapshot", &c.name);
        for d in &defs {
            table.push(d.clone());
        }
        let cmods = Arc::new(cmods);
        table.add_crate(c.name.clone(), cmods.clone());
        let mut files: Vec<String> = seen.keys().cloned().collect();
        files.sort();
        let mut prelexed = Vec::new();
        let mut jobs = Vec::new();
        if self.parse {
            let ctx = Arc::new(CrateCtx { context: self.contexts[ci].clone(), name: c.name.clone(), edition: c.edition, cfg: c.cfg.clone(), table: Arc::new(table.clone()) });
            for (path, src, toks) in lexed {
                let mod_path = seen[&path].clone();
                jobs.push(Job { krate: ci as u32, path, mod_path, src, toks, ctx: ctx.clone() });
            }
        } else {
            prelexed = lexed;
        }
        drop(snapshot);
        let _inputs = crate::load_profile::span(c.cfg.session.runtime.profile, "publish-inputs", &c.name);
        let plan = self.demand_plan.as_ref().and_then(|p| p.ready()).filter(|p| p.counts().1 != 0);
        let deferred = plan.map(|p| p.deferred(ci + self.plan_offset)).unwrap_or_default();
        let demand_plan = plan.map_or(0, |p| p.selection_identity(ci + self.plan_offset));
        // The selection and scan/schema inputs belong to this crate. The
        // whole-plan certificate is recorded separately in the graph.
        inputs.sort(); inputs.dedup();
        probes.sort(); probes.dedup();
        let ahead = CrateAhead { name: c.name.clone(), idx: ci as u32, defs, aliases, macro_use, mods: cmods, files, prelexed, inputs, probes, deferred, demand_plan, image: false, valid, complete };
        Publication { ahead, jobs, done: Vec::new() }
    }

    fn work(&self, sess: &std::sync::Arc<crate::Session>, _worker: usize) {
        let _reads = self.contexts.first().map(|c| c.bind_source_reads());
        loop {
            let w = {
                let mut st = self.st.lock().unwrap();
                loop {
                    if st.stopped { return; }
                    if st.plan_pending {
                        st.plan_pending = false;
                        break Work::Plan;
                    }
                    let scan = Self::next_scan(&st, None);
                    // Syntax, derives and file preparation run on workers. The loader
                    // validates recorded macro lookups in its exact textual environment
                    // before publishing definitions/imports, for both Rust and lowered input.
                    let ready = st.jobs.iter().find(|((ci, _, _), _)| st.parse_ready.contains(ci)
                        && st.through.map_or(true, |t| *ci <= t)).map(|(k, _)| *k);
                    if let Some(key) = ready {
                        let j = st.jobs.remove(&key).unwrap();
                        st.running.insert((j.krate, j.path.clone()));
                        break Work::Parse(j);
                    }
                    if let Some(key) = scan {
                        break Work::Scan(st.scans.remove(&key).unwrap());
                    }
                    if st.published == self.crates.len() && st.jobs.is_empty() {
                        return;
                    }
                    let _wait = crate::load_profile::span(sess.runtime.profile, "worker-wait", "no eligible scan or parse; awaiting scan completion / loader macro scope");
                    st = self.cv.wait(st).unwrap();
                }
            };
            match w {
                Work::Plan => {
                    if let Some(plan) = &self.demand_plan { let _ = std::ops::Deref::deref(&**plan); }
                    let mut st = self.st.lock().unwrap();
                    st.planning = false;
                    drop(self.publish(st));
                    self.cv.notify_all();
                }
                Work::Scan(k) => self.scan(k),
                Work::Parse(job) => {
                    let ((krate, path), x) = parse_job(sess, job, false);
                    let mut st = self.st.lock().unwrap();
                    let key = (krate, path);
                    st.running.remove(&key);
                    if !st.cancelled.remove(&key) {
                        st.done.insert(key, x);
                    }
                    self.cv.notify_all();
                }
            }
        }
    }
}

/// Parses one file against its crate's table snapshot. A speculative failure is
/// retried in the loader's scope; only that scope can certify a parse error.
fn parse_job(_sess: &std::sync::Arc<crate::Session>, job: Job, in_scope: bool) -> ((u32, String), Preparsed) {
    let Job { krate, path, mod_path, src, toks, ctx } = job;
    crate::why::event("parse", &path, 0, "", None);
    let src_len = src.len();
    let t = std::time::Instant::now();
    let bctx = std::rc::Rc::new(crate::builtins::FileCtx::new(&ctx.name, &path, &mod_path, &ctx.cfg));
    let r = ctx.context.parse_input(src, Some(toks), ctx.edition, Some(crate::macros::MacroScope::new(ctx.table.clone())), bctx, crate::parser::lazy_auto());
    PARSE_NS.fetch_add(t.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    let mut error = None;
    let mut parsed = match r {
        Ok((pf, used)) => Some((pf, used.unwrap())),
        Err(e) => {
            if trace() { eprintln!("boot-trace-parse-error: {path} in_scope={in_scope} {e}"); }
            if in_scope { error = Some(e); }
            None
        }
    };
    let prep = parsed.as_mut().map(|(pf, _)| crate::program::prepare_file(pf, &ctx.cfg));
    let verify = ctx.cfg.session.runtime.parse_verify.then(|| VERIFY.get().copied()).flatten();
    ((krate, path), Preparsed { parsed, prep, mod_path, src_len, verify, error })
}

/// MAKEPAD_RUN_BOOT_TRACE=1: boot-path timing and why files were parsed again (stderr `boot-trace`).
pub fn trace() -> bool {
    static T: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *T.get_or_init(|| std::env::var("MAKEPAD_RUN_BOOT_TRACE").is_ok())
}

/// MAKEPAD_RUN_TRACE, read once (parse workers ask per expansion; std::env::var takes the env lock and
/// allocates).
pub fn relax_trace() -> Option<&'static str> {
    static T: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    T.get_or_init(|| std::env::var("MAKEPAD_RUN_TRACE").ok()).as_deref()
}

/// MAKEPAD_RUN_DERIVES=0 (derives are not expanded; measurement baseline), read once.
pub fn derives_off(options: &crate::options::Options) -> bool {
    !options.derives
}

/// Parse-time split for MAKEPAD_RUN_BOOT_TRACE: 0 fn bodies, 1 derives, 2 macro_rules transcription,
/// 3 built-in macros (format family etc.), 4 hosted / native fn-like macros; outermost call per slot only.
pub static PHASE_NS: [std::sync::atomic::AtomicU64; 5] = [const { std::sync::atomic::AtomicU64::new(0) }; 5];

pub fn timed<R>(slot: usize, f: impl FnOnce() -> R) -> R {
    let _reload = (slot != 0).then(|| crate::parse_work::time(match slot { 1 => 2, 2 => 4, _ => 3 }));
    thread_local! {
        static DEPTH: std::cell::Cell<[u8; 5]> = const { std::cell::Cell::new([0; 5]) };
    }
    use crate::memstat::Tag;
    let _mem = crate::memstat::tag([Tag::ParseBody, Tag::Derive, Tag::MacroRules, Tag::Fmt, Tag::MacOther][slot.min(4)]);
    if !trace() {
        return f();
    }
    let mut d = DEPTH.with(|d| d.get());
    if d[slot] > 0 {
        return f();
    }
    d[slot] = 1;
    DEPTH.with(|x| x.set(d));
    let t = std::time::Instant::now();
    let r = f();
    PHASE_NS[slot].fetch_add(t.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    let mut d = DEPTH.with(|d| d.get());
    d[slot] = 0;
    DEPTH.with(|x| x.set(d));
    r
}

/// Per macro name: (calls, ns) for MAKEPAD_RUN_BOOT_TRACE.
pub static BY_NAME: Mutex<Vec<(String, u64, u64)>> = Mutex::new(Vec::new());

pub fn timed_named<R>(slot: usize, name: &str, f: impl FnOnce() -> R) -> R {
    if !trace() {
        return timed(slot, f);
    }
    let t = std::time::Instant::now();
    let r = timed(slot, f);
    let ns = t.elapsed().as_nanos() as u64;
    let mut m = BY_NAME.lock().unwrap();
    match m.iter_mut().find(|e| e.0 == name) {
        Some(e) => {
            e.1 += 1;
            e.2 += ns;
        }
        None => m.push((name.to_string(), 1, ns)),
    }
    r
}

/// Time the sequential loader waited for parse results (boot trace).
pub static WAIT_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Time spent publishing crates under the pool lock (boot trace).
pub static PUBLISH_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Worker CPU time spent parsing (boot trace).
pub static PARSE_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

thread_local! {
    /// a crate loaded while the program runs (a run-time module): parsed on the calling thread,
    /// no parse workers (hotreload lane)
    pub static SINGLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn jobs() -> usize {
    if SINGLE.with(|s| s.get()) {
        return 1;
    }
    std::env::var("MAKEPAD_RUN_JOBS").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or_else(crate::options::default_parse_jobs).clamp(1, 32)
}

/// A scanned file's source bytes: its own, a mapped image's, or the registry's authored bytes
/// shared (a scan of an authored file with no expansion text holds the one copy the registry
/// holds, not a second one).
#[derive(Clone)]
pub(crate) enum ScanBytes {
    Owned(Vec<u8>),
    Mapped(&'static [u8]),
    Shared(Arc<[u8]>),
}
impl std::ops::Deref for ScanBytes {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &[u8] {
        match self { Self::Owned(v) => v, Self::Mapped(s) => s, Self::Shared(a) => a }
    }
}
impl From<Vec<u8>> for ScanBytes {
    fn from(v: Vec<u8>) -> Self { Self::Owned(v) }
}
impl From<&crate::source::SourceBytes> for ScanBytes {
    fn from(src: &crate::source::SourceBytes) -> Self {
        match src {
            crate::source::SourceBytes::Shared(a) => Self::Shared(a.clone()),
            crate::source::SourceBytes::Mapped(m) => Self::Mapped(m),
        }
    }
}

/// A scanned file: source, tokens, its `macro_rules!` definitions in token order and the
/// module files it declares (path, dir for their `mod x;`, module path).
#[derive(Clone)]
struct Scanned {
    src_len: usize,
    probes: Vec<(String, bool)>,
    src: ScanBytes,
    toks: crate::arena::Arena<Tok>,
    defs: Vec<crate::macros::MacroDef>,
    children: Vec<(String, String, String)>,
    /// `use path::{orig as alias}` imports: (alias, first path segment, orig)
    aliases: Vec<(String, String, String)>,
    /// `#[macro_use] extern crate dep;` crates
    macro_use: Vec<String>,
    /// module-level `use` / `extern crate` bindings (macro path resolution)
    uses: Vec<crate::macros::UseItem>,
    /// module paths of the file (its own and inline / declared children)
    mods: Vec<String>,
}

/// The bindings of a `use` tree in tokens `lo..hi` (after `use`, before `;`) under `prefix`.
fn use_tree(text: &dyn Fn(usize) -> String, toks: &[Tok], lo: usize, hi: usize, prefix: &[String], module: &str, is_pub: bool, out: &mut Vec<crate::macros::UseItem>) {
    let mut path: Vec<String> = prefix.to_vec();
    let mut i = lo;
    if i < hi && toks[i].kind == T::PathSep && prefix.is_empty() {
        path.push(String::new());
        i += 1;
    }
    while i < hi {
        match toks[i].kind {
            T::OpenBrace => {
                // `{a, b::c, d as e, f::*}`: top-level comma-separated subtrees
                let mut d = 0i32;
                let mut s = i + 1;
                let mut k = i;
                while k < hi {
                    match toks[k].kind {
                        T::OpenBrace => d += 1,
                        T::CloseBrace => {
                            d -= 1;
                            if d == 0 {
                                if k > s {
                                    use_tree(text, toks, s, k, &path, module, is_pub, out);
                                }
                                break;
                            }
                        }
                        T::Comma if d == 1 => {
                            if k > s {
                                use_tree(text, toks, s, k, &path, module, is_pub, out);
                            }
                            s = k + 1;
                        }
                        _ => {}
                    }
                    k += 1;
                }
                return;
            }
            T::Star => {
                out.push(crate::macros::UseItem { module: module.to_string(), path, bind: None, is_pub });
                return;
            }
            T::PathSep => i += 1,
            _ => {
                let t = text(i);
                if t == "as" {
                    // `x as y` / `x as _`
                    if i + 1 < hi {
                        let b = text(i + 1);
                        if b != "_" && !path.is_empty() {
                            let last = path.last().cloned().unwrap_or_default();
                            let p = if last == "self" { path[..path.len() - 1].to_vec() } else { path.clone() };
                            out.push(crate::macros::UseItem { module: module.to_string(), path: p, bind: Some(b), is_pub });
                        }
                    }
                    return;
                }
                path.push(t);
                i += 1;
            }
        }
    }
    // a plain path: bound under its last segment (`a::{self}` binds `a`)
    if let Some(last) = path.last().cloned() {
        if last == "self" {
            if path.len() >= 2 {
                let b = path[path.len() - 2].clone();
                out.push(crate::macros::UseItem { module: module.to_string(), path: path[..path.len() - 1].to_vec(), bind: Some(b), is_pub });
            }
        } else if !last.is_empty() {
            out.push(crate::macros::UseItem { module: module.to_string(), path, bind: Some(last), is_pub });
        }
    }
}

/// Attributes of a declaration, configured before the prescan publishes it.
struct DeclAttrs {
    active: bool,
    path: Option<String>,
    macro_use: bool,
    exported: bool,
    local_inner: bool,
}

fn decl_attrs(src: &[u8], toks: &[Tok], i: usize, cfg: &CfgSet) -> DeclAttrs {
    let p = Src { src, plain: toks, ext: &[], parts: None };
    let attrs = crate::cfg::attrs_before(p, i, cfg);
    let mut out = DeclAttrs {
        active: crate::cfg::active(p, &attrs, cfg),
        path: None,
        macro_use: false,
        exported: false,
        local_inner: false,
    };
    for attr in attrs {
        let lo = attr.toks.lo as usize;
        let hi = attr.toks.hi as usize;
        if lo >= hi { continue; }
        match crate::cfg::tok_str(p, lo) {
            b"path" if lo + 2 < hi && toks[lo + 1].kind == T::Eq => {
                out.path = Some(String::from_utf8_lossy(crate::cfg::tok_str(p, lo + 2)).trim_matches('"').to_string());
            }
            b"macro_use" => out.macro_use = true,
            b"macro_export" => {
                out.exported = true;
                out.local_inner |= (lo + 1..hi).any(|k| crate::cfg::tok_str(p, k) == b"local_inner_macros");
            }
            _ => {}
        }
    }
    out
}

/// The file's leading inner attributes (`#![..]`) hold their #[cfg]s.
pub fn file_cfg_active(src: &[u8], toks: &[Tok], cfg: &CfgSet) -> bool {
    let mut attrs = Vec::new();
    let mut i = 0;
    while i + 2 < toks.len() && toks[i].kind == T::Pound && toks[i + 1].kind == T::Not && toks[i + 2].kind == T::OpenBracket {
        let mut d = 0i32;
        let mut j = i + 2;
        while j < toks.len() {
            match toks[j].kind {
                T::OpenBracket => d += 1,
                T::CloseBracket => {
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        attrs.push(Attr { inner: true, toks: TokRange { lo: (i + 3) as u32, hi: j as u32 } });
        i = j + 1;
    }
    attrs.is_empty() || crate::cfg::active(Src { src, plain: toks, ext: &[], parts: None }, &attrs, cfg)
}

fn scan_file(path: &str, dir: &str, mpath: &str, krate: &str, edition: u16, cfg: &CfgSet, context: &crate::source_context::Context) -> Option<Scanned> {
    let _profile = crate::load_profile::span(cfg.session.runtime.profile, "prescan", path);
    scan_text(path, dir, mpath, krate, edition, cfg, context.read_source(path).ok()?.src.to_vec().into(), context)
}

fn scan_text(path: &str, dir: &str, mpath: &str, krate: &str, edition: u16, cfg: &CfgSet, src: ScanBytes, context: &crate::source_context::Context) -> Option<Scanned> {
    scan_text_mode(path, dir, mpath, krate, edition, cfg, src, context, false)
}

fn scan_text_mode(path: &str, dir: &str, mpath: &str, krate: &str, edition: u16, cfg: &CfgSet, src: ScanBytes, context: &crate::source_context::Context, declarations_only: bool) -> Option<Scanned> {
    let macro_surface = !declarations_only || cfg.session.options.rs_lazy;
    let src_len = src.len();
    let mut toks;
    if cfg.session.options.rs_lazy {
        // the scan's lex: the parse's tokens, or (declarations only) the outline's input,
        // whose cfg-active tokens the scan keeps; the outline keeps only its name index
        toks = Vec::with_capacity(src.len() / 4);
        crate::lexer::lex(&src, edition, &mut toks).ok()?;
        let (_, active) = context.rust_outline_lexed(path, &src, edition, cfg, &toks)?;
        if declarations_only { toks = active; }
        toks.shrink_to_fit();
    } else {
        let _profile = crate::load_profile::span(cfg.session.runtime.profile, "source-lex", path);
        toks = Vec::with_capacity(src.len() / 4);
        crate::lexer::lex(&src, edition, &mut toks).ok()?;
        toks.shrink_to_fit();
    }
    if declarations_only && !cfg.session.options.rs_lazy {
        toks = lazy::demand::active_tokens(&src, &toks, cfg);
    }
    scan_tokens(path, dir, mpath, krate, cfg, src_len, src, toks, context, macro_surface)
}

fn scan_tokens(path: &str, dir: &str, mpath: &str, krate: &str, cfg: &CfgSet, src_len: usize,
    src: ScanBytes, toks: Vec<Tok>, context: &crate::source_context::Context, macro_surface: bool,
) -> Option<Scanned> {
    let mut defs = Vec::new();
    let mut children = Vec::new();
    let mut probes = Vec::new();
    let mut aliases = Vec::new();
    let mut macro_use = Vec::new();
    let mut uses: Vec<crate::macros::UseItem> = Vec::new();
    let mut mods: Vec<String> = vec![mpath.to_string()];
    if !file_cfg_active(&src, &toks, cfg) {
        // `#![cfg(false)]` file: removed with its module; nothing in it counts
        return Some(Scanned { src_len, src, toks: toks.into(), defs, children, probes, aliases, macro_use, uses, mods: Vec::new() });
    }
    {
        let text = |i: usize| -> &[u8] { &src[toks[i].lo as usize..toks[i].hi as usize] };
        let fdir = crate::program::dir_of(path).to_string();
        // inline `mod x { .. }` nesting: (brace depth inside it, its dir, its module path, active)
        let mut dirs: Vec<(u32, String, String, bool)> = vec![(0, dir.to_string(), mpath.to_string(), true)];
        let mut depth = 0u32;
        let mut i = 0;
        while i < toks.len() {
            let k = toks[i].kind;
            match k {
                T::OpenBrace => depth += 1,
                T::CloseBrace => {
                    if dirs.len() > 1 && dirs.last().unwrap().0 == depth {
                        dirs.pop();
                    }
                    depth = depth.saturating_sub(1);
                }
                // `use std::{eprintln as log_error};`: a macro renamed by `use` (rustc: macros
                // are importable items; `crate::log_error!` names the import). Recorded per
                // crate as alias -> (first path segment, original name); the parser consults it
                // when no macro has the name. The import's `#[cfg]` is evaluated.
                T::KwUse => {
                    let mut j = i + 1;
                    while j < toks.len() && toks[j].kind != T::Semi {
                        j += 1;
                    }
                    // Demand discovery extracts module bindings in Names::scan;
                    // it never consumes this macro prescan's imports or aliases.
                    if !macro_surface { i = j + 1; continue; }
                    let attrs = decl_attrs(&src, &toks, i, cfg);
                    // its bindings, for the module (an import in a fn body is block-scoped in
                    // rustc; macro calls inside that body see it -- the module's lookups also do
                    // here, which only matters if another module-level binding shadows it)
                    if attrs.active && dirs.last().map_or(true, |l| l.3) {
                        let is_pub = i >= 1 && (toks[i - 1].kind == T::KwPub || toks[i - 1].kind == T::CloseParen);
                        let module = dirs.last().unwrap().2.clone();
                        let tx = |k: usize| String::from_utf8_lossy(text(k)).trim_start_matches("r#").to_string();
                        use_tree(&tx, &toks, i + 1, j, &[], &module, is_pub, &mut uses);
                    }
                    if attrs.active && dirs.last().map_or(true, |l| l.3) {
                        let mut f = i + 1;
                        while f < j && toks[f].kind == T::PathSep {
                            f += 1;
                        }
                        let qual = if f < j { String::from_utf8_lossy(text(f)).into_owned() } else { String::new() };
                        let mut m = i + 1;
                        while m + 2 < j {
                            if matches!(toks[m].kind, T::Ident | T::RawIdent) && text(m + 1) == b"as" && matches!(toks[m + 2].kind, T::Ident | T::RawIdent) {
                                let target = String::from_utf8_lossy(text(m)).trim_start_matches("r#").to_string();
                                let alias = String::from_utf8_lossy(text(m + 2)).trim_start_matches("r#").to_string();
                                if alias != "_" {
                                    aliases.push((alias, qual.clone(), target));
                                }
                            }
                            m += 1;
                        }
                        // `use dep::log::{trace, warn};` from another crate: the names as they are
                        // (the dep may export them under another name: `pub use crate::{__trace as
                        // trace}`, followed through the dep's own aliases)
                        let external = !matches!(qual.as_str(), "" | "std" | "core" | "alloc" | "crate" | "self" | "super");
                        if external {
                            for m in f + 1..j {
                                let leaf = matches!(toks[m].kind, T::Ident | T::RawIdent) && (m + 1 >= j || matches!(toks[m + 1].kind, T::Comma | T::CloseBrace)) && text(m - 1) != b"as";
                                if leaf {
                                    let name = String::from_utf8_lossy(text(m)).trim_start_matches("r#").to_string();
                                    if name != "self" {
                                        aliases.push((name.clone(), qual.clone(), name));
                                    }
                                }
                            }
                        }
                    }
                    i = j;
                }
                // `#[macro_use] extern crate dep;`: dep's exported macros in scope (rustc's
                // macro_use prelude)
                T::KwExtern if macro_surface && i + 2 < toks.len() && toks[i + 1].kind == T::KwCrate && matches!(toks[i + 2].kind, T::Ident | T::RawIdent | T::KwSelfValue) => {
                    let attrs = decl_attrs(&src, &toks, i, cfg);
                    let n = String::from_utf8_lossy(text(i + 2)).trim_start_matches("r#").to_string();
                    if attrs.active && dirs.last().map_or(true, |l| l.3) {
                        // `extern crate x;` / `extern crate x as y;`: binds the crate in the module
                        let bind = if i + 4 < toks.len() && text(i + 3) == b"as" { String::from_utf8_lossy(text(i + 4)).trim_start_matches("r#").to_string() } else { n.clone() };
                        if n != "self" && bind != "_" {
                            let module = dirs.last().unwrap().2.clone();
                            let is_pub = i >= 1 && (toks[i - 1].kind == T::KwPub || toks[i - 1].kind == T::CloseParen);
                            uses.push(crate::macros::UseItem { module, path: vec![String::new(), n.clone()], bind: Some(bind), is_pub });
                        } else if n == "self" && bind != "self" && bind != "_" {
                            // `extern crate self as name;` (makepad-draw): the crate under another
                            // name -- bound in the module, and at the root in the extern prelude
                            // (the collector below adds it, UseItem path ["crate"])
                            let module = dirs.last().unwrap().2.clone();
                            let is_pub = i >= 1 && (toks[i - 1].kind == T::KwPub || toks[i - 1].kind == T::CloseParen);
                            uses.push(crate::macros::UseItem { module, path: vec!["crate".to_string()], bind: Some(bind), is_pub });
                        }
                    }
                    if attrs.active && dirs.last().map_or(true, |l| l.3) && attrs.macro_use {
                        if n != "self" && !macro_use.contains(&n) {
                            macro_use.push(n);
                        }
                    }
                }
                T::KwMod if i + 2 < toks.len() && matches!(toks[i + 1].kind, T::Ident | T::RawIdent) => {
                    let name = String::from_utf8_lossy(text(i + 1)).trim_start_matches("r#").to_owned();
                    let (cur, cur_path, cur_active) = {
                        let l = dirs.last().unwrap();
                        (l.1.clone(), l.2.clone(), l.3)
                    };
                    let child_path = format!("{}::{}", cur_path, name.trim_start_matches("r#"));
                    let attrs = decl_attrs(&src, &toks, i, cfg);
                    let active = attrs.active;
                    let pa = attrs.path;
                    if toks[i + 2].kind == T::Semi {
                        if cur_active && active {
                            let explicit_path = pa.is_some();
                            let cands = match pa {
                                Some(p) => vec![crate::crates::join_path(if dirs.len() > 1 { &cur } else { &fdir }, &p)],
                                None => vec![format!("{}/{}.rs", cur, name), format!("{}/{}/mod.rs", cur, name)],
                            };
                            for c in cands {
                                let exists = context.exists(&c);
                                probes.push((c.clone(), exists));
                                if exists {
                                    let cdir = if explicit_path || c.ends_with("/mod.rs") {
                                        crate::program::dir_of(&c).to_string()
                                    } else {
                                        format!("{}/{}", crate::program::dir_of(&c), crate::program::stem_of(&c))
                                    };
                                    if crate::why::enabled() {
                                        crate::why::edge(&c, crate::why::Cause::new(path, crate::lexer::line_col(&src, toks[i].lo).0, "module-decl", &child_path));
                                    }
                                    children.push((c, cdir, child_path.clone()));
                                    mods.push(child_path.clone());
                                    break;
                                }
                            }
                        }
                    } else if toks[i + 2].kind == T::OpenBrace {
                        let active = active && file_cfg_active(&src, &toks[i + 3..], cfg);
                        if cur_active && active {
                            mods.push(child_path.clone());
                        }
                        let child_dir = pa.map_or_else(|| format!("{}/{}", cur, name), |p| crate::crates::join_path(&cur, &p));
                        dirs.push((depth + 1, child_dir, child_path, cur_active && active));
                    }
                }
                T::Ident
                    if i + 2 < toks.len()
                        && toks[i + 1].kind == T::Not
                        && toks[i + 2].kind == T::OpenParen
                        && dirs.last().map_or(false, |l| l.0 == depth && l.3)
                        && crate::builtins::prescan_hosted(&String::from_utf8_lossy(text(i))) =>
                {
                    // item macros that may define macro_rules!: function-like proc macros of
                    // the JIT host called at item position (run here, their macro_rules! kept;
                    // builtins.rs prescan_defs)
                    let mut close = i + 2;
                    while close < toks.len() && toks[close].kind != T::CloseParen {
                        close += 1;
                    }
                    if close < toks.len() && macro_surface && decl_attrs(&src, &toks, i, cfg).active {
                        let arg = &src[toks[i + 2].hi as usize..toks[close].lo as usize];
                        defs.extend(crate::builtins::prescan_defs(cfg, &String::from_utf8_lossy(text(i)), arg, krate));
                    }
                    i = close;
                }
                T::Ident if text(i) == b"macro_rules" && i + 3 < toks.len() && toks[i + 1].kind == T::Not => {
                    let name = String::from_utf8_lossy(text(i + 2)).trim_start_matches("r#").to_string();
                    let open = i + 3;
                    let mut d = 0i32;
                    let mut close = open;
                    while close < toks.len() {
                        match toks[close].kind {
                            T::OpenParen | T::OpenBracket | T::OpenBrace => d += 1,
                            T::CloseParen | T::CloseBracket | T::CloseBrace => {
                                d -= 1;
                                if d == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        close += 1;
                    }
                    // The demand outline only needs the definition's tokens.
                    // Building matcher trees here duplicates the real preload.
                    if !macro_surface { i = close + 1; continue; }
                    // A cfg-false definition must never reach either textual scope or
                    // the crate-root export table, including inside an inactive module.
                    let attrs = decl_attrs(&src, &toks, i, cfg);
                    if !attrs.active || !dirs.last().unwrap().3 {
                        i = close + 1;
                        continue;
                    }
                    let mt: Vec<crate::macros::MTok> = toks[open + 1..close.min(toks.len())]
                        .iter()
                        .map(|t| crate::macros::MTok { kind: t.kind, text: String::from_utf8_lossy(&src[t.lo as usize..t.hi as usize]).into_owned(), pos: t.lo })
                        .collect();
                    if let Ok(mut def) = crate::macros::parse_def(&name, krate, &mt) {
                        def.exported = attrs.exported;
                        if attrs.local_inner {
                            crate::macros::local_inner_macros(&mut def);
                        }
                        // the definition the file's own parse makes (MacroScope::define reuses
                        // it: lookups made ahead of load order then replay)
                        def.def_file = path.to_string();
                        def.def_pos = toks[i + 2].lo;
                        defs.push(def);
                    }
                    i = close;
                }
                _ => {}
            }
            i += 1;
        }
    }
    Some(Scanned { src_len, src, toks: toks.into(), defs, children, probes, aliases, macro_use, uses, mods })
}

/// Starts the pipeline for `crates` (load order): scans (all crates at once, earliest crate
/// first), then each crate published in order with its parse jobs, on worker threads (none when
/// `jobs()` < 2: the loader scans and parses itself). `base`: the macro table before the first
/// of them loads.
pub fn start(sess: &std::sync::Arc<crate::Session>, crates: Vec<PreCrate>, base: &MacroTable, images: Option<&dyn crate::crate_image::CrateImages>, bounded: bool, sources: Arc<crate::source::SourceRegistry>) -> (Arc<Pool>, Vec<std::thread::JoinHandle<()>>) {
    start_with_plan(sess, crates, base, images, bounded, sources, None)
}

pub fn start_with_plan(sess: &std::sync::Arc<crate::Session>, crates: Vec<PreCrate>, base: &MacroTable, images: Option<&dyn crate::crate_image::CrateImages>, bounded: bool, sources: Arc<crate::source::SourceRegistry>, plan: Option<Arc<DemandPlan>>) -> (Arc<Pool>, Vec<std::thread::JoinHandle<()>>) {
    start_with_contexts_plan(sess, crates, base, images, bounded, sources, None, plan)
}

pub fn start_with_contexts(sess: &std::sync::Arc<crate::Session>, crates: Vec<PreCrate>, base: &MacroTable, images: Option<&dyn crate::crate_image::CrateImages>, bounded: bool, sources: Arc<crate::source::SourceRegistry>, contexts: Option<Vec<Arc<crate::source_context::Context>>>) -> (Arc<Pool>, Vec<std::thread::JoinHandle<()>>) {
    start_with_contexts_plan(sess, crates, base, images, bounded, sources, contexts, None)
}

fn start_with_contexts_plan(sess: &std::sync::Arc<crate::Session>, crates: Vec<PreCrate>, base: &MacroTable, images: Option<&dyn crate::crate_image::CrateImages>, bounded: bool, sources: Arc<crate::source::SourceRegistry>, contexts: Option<Vec<Arc<crate::source_context::Context>>>, plan: Option<Arc<DemandPlan>>) -> (Arc<Pool>, Vec<std::thread::JoinHandle<()>>) {
    let _profile = crate::load_profile::span(sess.runtime.profile, "front-preload-setup", "main");
    let images = if sources.overridden_paths().is_empty() { images } else { None };
    if let Some(images) = images { images.begin_load(&crates); }
    let jobs = jobs();
    if crate::why::enabled() {
        for c in &crates { crate::why::edge(&c.root, crate::why::Cause::new(&c.root, 1, "crate-root", &c.name)); }
    }
    let n = crates.len();
    let plan_offset = plan.as_ref().map_or(0, |p| p.crates.len().checked_sub(n).expect("preload is a suffix of the demand plan"));
    let contexts = plan.as_ref().map(|p| {
        assert!(p.crates[plan_offset..].iter().zip(&crates).all(|(a, b)| a.name == b.name && a.root == b.root));
        p.contexts[plan_offset..].to_vec()
    }).or(contexts).unwrap_or_else(|| crate::source_context::Context::for_crates(&crates, sources, sess));
    let demand_plan = plan.or_else(|| (!sess.options.source_eager && sess.options.rs_lazy).then(|| Arc::new(DemandPlan::new(&crates, contexts.clone(), images, jobs >= 2))))
        .filter(|plan| plan.ready().map_or(true, |p| p.counts().1 != 0));
    let planning = demand_plan.as_ref().is_some_and(|p| p.ready().is_none());
    let mut fps = vec![None; n];
    let mut restored = Vec::with_capacity(n);
    let mut valid = Vec::with_capacity(n);
    for (ci, c) in crates.iter().enumerate() {
        let deps: Option<Vec<u128>> = c.image_deps.iter().map(|&d| fps.get(d).copied().flatten()).collect();
        if images.is_some() && deps.is_none() && trace() {
            let miss: Vec<&str> = c.image_deps.iter().filter(|&&d| fps.get(d).copied().flatten().is_none()).map(|&d| crates.get(d).map_or("?", |x| x.name.as_str())).collect();
            eprintln!("boot-trace: {} parsed: dependencies not restored ({})", c.name, miss.join(", "));
        }
        let plan = demand_plan.as_ref().and_then(|p| p.ready());
        let deferred = plan.map(|p| p.deferred(ci + plan_offset)).unwrap_or_default();
        let plan_id = plan.map_or(0, |p| p.selection_identity(ci + plan_offset));
        let image = deps.and_then(|d| images.and_then(|i| {
            let image = i.crate_image_selected(c, &d, plan_id, &deferred);
            if planning { image.or_else(|| i.crate_image(c, &d)) } else { image }
        }));
        valid.push(!planning && image.is_some());
        if images.is_some() && image.is_none() && trace() && c.image_deps.iter().all(|&d| fps.get(d).copied().flatten().is_some()) {
            eprintln!("boot-trace: {} parsed: no valid crate image", c.name);
        }
        fps[ci] = image.as_ref().map(|i| i.fingerprint);
        restored.push(if planning { image } else { image.filter(|i| !i.fp_only) });
    }
    if let Some(images) = images { images.end_load(); }
    if sess.options.verbose && images.is_some() && (trace() || crate::memstat::enabled()) {
        let count = restored.iter().flatten().count();
        let files: usize = restored.iter().flatten().map(|i| i.files.len()).sum();
        if planning {
            eprintln!("makepad-run front: {count} crate image candidates ({files} files); selection validation on planning worker");
        } else {
            eprintln!("makepad-run front: {} crates from crate images ({} files, no parse), {} crates parsed", count, files, n - count);
        }
    }
    let pending_images = if planning { std::mem::replace(&mut restored, (0..n).map(|_| None).collect()) }
        else { (0..n).map(|_| None).collect() };
    let mut st = PoolState {
        stopped: false,
        plan_pending: planning,
        planning,
        parse_ready: Default::default(),
        images: restored,
        pending_images,
        valid,
        scans: Default::default(),
        scan_seq: 0,
        seen: HashSet::default(),
        scan_left: vec![1; n],
        scanned: (0..n).map(|_| HashMap::default()).collect(),
        publishing: false,
        published: 0,
        through: bounded.then_some(1),
        table: base.clone(),
        ahead: HashMap::default(),
        jobs: Default::default(),
        job_seq: 0,
        done: HashMap::default(),
        running: HashSet::default(),
        cancelled: HashSet::default(),
    };
    // the setup's own reads (the first publication); the loader binds again for each load
    let _reads = contexts.first().map(|c| c.bind_source_reads());
    for (ci, c) in crates.iter().enumerate() {
        if st.pending_images[ci].is_some() { continue; }
        if st.images[ci].is_some() {
            st.scan_left[ci] = 0;
            continue;
        }
        let k = (ci, c.root.clone(), crate::program::dir_of(&c.root).to_string(), c.name.clone());
        st.seen.insert(k.clone());
        st.scan_seq += 1;
        let seq = st.scan_seq;
        st.scans.insert((ci, seq), k);
    }
    let pool = Arc::new(Pool { demand_plan, plan_offset, contexts, crates, parse: jobs >= 2, st: Mutex::new(st), cv: Condvar::new() });
    if let Some(plan) = &pool.demand_plan {
        let weak = Arc::downgrade(&pool);
        plan.on_early_publish(move || {
            if let Some(pool) = weak.upgrade() {
                let st = pool.st.lock().unwrap();
                drop(pool.publish(st));
                pool.cv.notify_all();
            }
        });
    }
    {
        let st = pool.st.lock().unwrap();
        drop(pool.publish(st));
    }
    let mut handles = Vec::new();
    if jobs >= 2 {
        // the macro world's own pool (pmhost) inherits that its threads build it
        let in_world = crate::macro_reg::in_world();
        for worker in 0..jobs {
            let p = pool.clone();
            let sess = sess.clone();
            if let Ok(h) = std::thread::Builder::new().name(format!("run-parse-{worker}")).spawn(move || { let _z = crate::zone_alloc::compiler_thread();
                crate::macro_reg::set_in_world(in_world);
                let _mem = crate::memstat::tag(if in_world { crate::memstat::Tag::PmHost } else { crate::memstat::Tag::Parse });
                p.work(&sess, worker)
            }) {
                handles.push(h);
            }
        }
    }
    (pool, handles)
}

/// Rebuild macro prescans from committed buffers using the startup scanner. A package's
/// library and binary share a macro namespace, so refresh both contexts together.
pub fn check_macros(prog: &mut crate::program::Program, krate: u32, files: &[(u32, Vec<u8>)]) -> Vec<(u32, Vec<String>)> {
    let name = prog.syms.str(prog.crates[krate as usize].name).to_string();
    let mut contexts = Vec::new();
    for (k, c) in prog.crates.iter().enumerate().filter(|(_, c)| prog.syms.str(c.name) == name) {
        let mut mods = crate::macros::CrateMods { root: name.clone(), src: c.root_path().to_string(), ..Default::default() };
        for (alias, dep) in &c.deps { mods.externs.insert(prog.syms.str(*alias).to_string(), prog.syms.str(prog.crates[*dep as usize].name).to_string()); }
        let mut scans = Vec::new();
        let mut reachable = Vec::new();
        let root = c.root_path().to_string();
        let mut todo = vec![(root.clone(), crate::program::dir_of(&root).to_string(), name.clone())];
        while let Some((path, dir, module)) = todo.pop() {
            if reachable.contains(&path) { continue; }
            reachable.push(path.clone());
            let text = files.iter().find(|(f, _)| prog.file_paths[*f as usize] == path).map(|(_, t)| t.clone())
                .or_else(|| crate::vfs::read(&path).ok());
            if let Some(scan) = text.and_then(|t| scan_text(&path, &dir, &module, &name, c.edition, &c.cfg, t.into(), &c.context)) {
                todo.extend(scan.children.iter().rev().cloned());
                scans.push(scan);
            }
        }
        contexts.push((k as u32, reachable, mods, scans));
    }
    let table = std::sync::Arc::make_mut(&mut prog.macros);
    for defs in table.defs.values_mut() { defs.retain(|d| d.krate != name); }
    table.aliases.remove(&name); table.macro_use.remove(&name); table.exp_uses.remove(&name); table.forget_expansion_index(&name); table.sealed.remove(&name);
    table.crates.remove(&name); table.crates.remove(&format!("{}\u{1}lib", name));
    let mut reached = Vec::new();
    for (k, reachable, mut mods, scans) in contexts {
        for scan in scans {
            for d in scan.defs { table.define(d); }
            for (alias, qual, target) in scan.aliases { table.add_alias(name.clone(), alias, (qual, target)); }
            table.macro_use.entry(name.clone()).or_default().extend(scan.macro_use);
            mods.mods.extend(scan.mods);
            for u in scan.uses {
                if let Some(b) = &u.bind {
                    if u.path.last() != Some(b) { mods.renames.insert(b.clone()); }
                    if u.path.len() == 1 && u.path[0] == "crate" && u.module == name { mods.externs.insert(b.clone(), name.clone()); }
                }
                mods.uses.entry(u.module.clone()).or_default().push(u);
            }
        }
        table.add_crate(name.clone(), std::sync::Arc::new(mods));
        reached.push((k, reachable));
    }
    reached
}

#[cfg(test)]
mod cold_tests {
    use super::*;

    #[test]
    fn configured_inner_attributes_do_not_absorb_the_next_items_cfg() {
        let session = Arc::new(crate::Session::default());
        let cfg = CfgSet::host(&session, &[]);
        for source in [
            "#![forbid(unsafe_code)] #[cfg(feature = \"tweaker\")] use super::ShaderField; pub struct ShaderInfo; mod needed;",
            "mod generated { #![allow(dead_code)] #[cfg_attr(not(test), cfg(any()))] use absent::Field; pub struct ShaderInfo; mod needed; }",
        ] {
            let mut toks = Vec::new();
            assert!(crate::lexer::lex(source.as_bytes(), 2021, &mut toks).is_ok());
            let active = configured_tokens(source.as_bytes(), &toks, &cfg);
            let index = crate::rs_outline::expanded_index(source.as_bytes(), &active).unwrap();
            let scope = index.modules.iter().find_map(|module| module.inline.as_deref()).unwrap_or(&index);
            assert!(scope.items.iter().any(|item| item.name == "ShaderInfo"), "{source}: {index:?}");
            assert!(scope.modules.iter().any(|module| module.name == "needed"), "{source}: {index:?}");
            assert!(scope.uses.is_empty(), "disabled import survived: {:?}", scope.uses);
        }
    }

    /// Feed the same source to rustc and the real prescan/outline pipeline. rustc only
    /// compiles fixture libraries; none of their code is launched.
    fn check_glob_cycle(sources: &[(&str, &[&str], &str)], modules: &[&str]) {
        let dir = std::env::temp_dir().join(format!("relax-glob-cycle-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        let absent = dir.join("absent.rs");
        let sess = Arc::new(crate::Session::default());
        let cfg = CfgSet::host(&sess, &[]);
        let mut table = MacroTable::default();
        for &(name, deps, source) in sources {
            let path = dir.join(format!("{name}.rs"));
            std::fs::write(&path, source).unwrap();
            let mut rustc = std::process::Command::new("rustc");
            rustc.args(["--edition=2021", "--crate-type=rlib", "-O", "--crate-name", name])
                .arg(&path).arg("--out-dir").arg(&dir).arg("-L").arg(format!("dependency={}", dir.display()));
            for dep in deps { rustc.arg("--extern").arg(format!("{dep}={}/lib{dep}.rlib", dir.display())); }
            let output = rustc.output().expect("rustc is required for the macro-resolution oracle");
            assert!(output.status.success(), "rustc {name}: {}", String::from_utf8_lossy(&output.stderr));

            let path = path.to_string_lossy().into_owned();
            let context = crate::source_context::Context::new(&path, Default::default(), &sess);
            let scan = scan_text_mode(&path, ".", name, name, 2021, &cfg, source.as_bytes().to_vec().into(), &context, true).unwrap();
            let mut mods = crate::macros::CrateMods { root: name.into(), src: path.clone(), ..Default::default() };
            mods.externs.extend(deps.iter().map(|dep| (dep.to_string(), dep.to_string())));
            mods.mods.extend(scan.mods);
            for u in scan.uses {
                if let Some(bind) = &u.bind {
                    if u.path.last() != Some(bind) { mods.renames.insert(bind.clone()); }
                }
                mods.uses.entry(u.module.clone()).or_default().push(u);
            }
            for def in scan.defs { table.define(def); }
            table.add_crate(name.into(), Arc::new(mods));
            let mut toks = Vec::new();
            assert!(crate::lexer::lex(source.as_bytes(), 2021, &mut toks).is_ok());
            let ctx = std::rc::Rc::new(crate::builtins::FileCtx::new(name, &path, name, &cfg));
            let before = Arc::new(table.clone());
            let surface = crate::parser::rs_expand::expand(source.as_bytes(), toks, 2021, ctx, before.clone());
            assert!(surface.unknown.is_empty(), "{name}: {:?}", surface.unknown);
            assert!(surface.used.replays(&before, name, &path));
            table.add_exp_uses(name, surface.used.uses.clone());
            if name == "app" {
                let pending = Arc::new(table.clone());
                let scope = crate::macros::MacroScope::new(pending.clone());
                for module in modules {
                    let path: Vec<String> = module.split("::").map(str::to_string).collect();
                    let found = scope.lookup("selected", name, &path, name).expect("reachable through the cycle");
                    assert_eq!(found.krate, "defs");
                    for missing in ["unreachable", "missing"] {
                        assert!(scope.lookup(missing, name, &path, name).is_none(), "{module}::{missing}");
                        // rustc must reject both a real macro with no import route and a name
                        // with no definition. Compile-only: successful calls above are in source.
                        std::fs::write(&absent, format!("{source}\n{module}::{missing}!(Absent);\n")).unwrap();
                        let mut rustc = std::process::Command::new("rustc");
                        rustc.args(["--edition=2021", "--crate-type=rlib", "--emit=metadata", "-O", "--crate-name", "app"])
                            .arg(&absent).arg("--out-dir").arg(&dir).arg("-L").arg(format!("dependency={}", dir.display()));
                        for dep in deps { rustc.arg("--extern").arg(format!("{dep}={}/lib{dep}.rlib", dir.display())); }
                        let output = rustc.output().unwrap();
                        assert!(!output.status.success());
                        let errors = String::from_utf8_lossy(&output.stderr);
                        assert!(errors.contains("E0433") && errors.contains(missing), "{errors}");
                    }
                }
                let used = scope.finish();
                assert!(used.replays(&pending, name, &path));
                table.seal_crate(name);
                assert!(used.replays(&Arc::new(table.clone()), name, &path));
                // Once generated parents are published, the prescan's scope fixpoint gives
                // the same answer without the per-parse import resolver.
                for module in modules {
                    let path: Vec<String> = module.split("::").map(str::to_string).collect();
                    assert_eq!(table.lookup("selected", name, &path, name).unwrap().krate, "defs");
                    for missing in ["unreachable", "missing"] { assert!(table.lookup(missing, name, &path, name).is_none()); }
                }
            } else {
                table.seal_crate(name);
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    const CYCLE_DEFS: &str = r#"
        #[macro_export]
        macro_rules! selected { ($name:ident) => { pub struct $name; }; }
        #[macro_export]
        macro_rules! unreachable { ($name:ident) => { pub struct $name; }; }
    "#;

    #[test]
    fn outline_mutual_glob_cycle_matches_rustc_before_and_after_publication() {
        check_glob_cycle(&[
            ("defs", &[], CYCLE_DEFS),
            ("app", &["defs"], r#"
                macro_rules! modules { () => {
                    pub mod generated {
                        pub mod a { pub use super::b::*; }
                        pub mod b { pub use super::a::*; pub use defs::selected; pub use defs as runtime; }
                    }
                }; }
                modules!();
                crate::generated::a::selected!(A);
                crate::generated::b::selected!(B);
                crate::generated::a::runtime::selected!(C);
            "#),
        ], &["crate::generated::a", "crate::generated::b"]);
    }

    #[test]
    fn outline_cross_crate_facade_glob_cycle_matches_rustc() {
        check_glob_cycle(&[
            ("defs", &[], CYCLE_DEFS),
            ("draw", &["defs"], r#"
                macro_rules! modules { () => {
                    pub mod rr { pub use super::bridge::*; }
                    pub mod bridge { pub use super::rr::*; pub use defs::selected; }
                }; }
                modules!();
                crate::rr::selected!(Draw);
            "#),
            ("facade", &["draw"], r#"
                pub use lib_rr::*;
                mod lib_rr { pub use super::*; pub use draw::rr; }
            "#),
            ("app", &["facade"], r#"
                macro_rules! modules { () => {
                    pub mod rr { pub use super::lib_rr::*; pub use facade::rr::*; }
                    mod lib_rr { pub use super::rr::*; }
                }; }
                modules!();
                crate::rr::selected!(App);
                facade::rr::selected!(External);
            "#),
        ], &["crate::rr", "crate::lib_rr", "facade::rr"]);
    }

    #[test]
    fn outline_resolves_exported_macros_through_module_and_glob_reexports() {
        let sess = Arc::new(crate::Session::default());
        let mut cfg = CfgSet::host(&sess, &[]);
        cfg.set.push("relax".into());
        let mut table = MacroTable::default();
        for (name, deps, source) in [
            ("draw", vec![], r#"
                extern crate self as draw;
                pub mod rr {
                    #[cfg(relax)]
                    #[macro_export]
                    macro_rules! enabled { ($($body:tt)*) => { $($body)* }; }
                    #[cfg(not(relax))]
                    #[macro_export]
                    macro_rules! enabled { ($($body:tt)*) => {}; }
                    #[macro_export]
                    macro_rules! disabled { ($($body:tt)*) => {}; }
                    pub use crate::{enabled, disabled};
                }
            "#),
            ("widgets_core", vec!["draw"], "pub extern crate draw; pub use draw::*; pub mod rr { pub use draw::rr::*; }"),
            ("widgets", vec!["widgets_core"], "pub use widgets_core::*; pub use widgets_core::{rr, rr as runtime};"),
            ("app", vec!["widgets", "draw"], "pub use widgets::*; mod ui {}"),
        ] {
            let path = format!("{name}.rs");
            let context = crate::source_context::Context::new(&path, Default::default(), &sess);
            let scan = scan_text_mode(&path, ".", name, name, 2021, &cfg, source.as_bytes().to_vec().into(), &context, true).unwrap();
            let mut mods = crate::macros::CrateMods { root: name.into(), src: path, ..Default::default() };
            mods.externs.extend(deps.into_iter().map(|dep| (dep.into(), dep.into())));
            mods.mods.extend(scan.mods);
            for u in scan.uses {
                if let Some(bind) = &u.bind {
                    if u.path.last() != Some(bind) { mods.renames.insert(bind.clone()); }
                }
                mods.uses.entry(u.module.clone()).or_default().push(u);
            }
            for def in scan.defs { table.define(def); }
            table.add_crate(name.into(), Arc::new(mods));
            table.seal_crate(name);
        }
        let source = br#"
            macro_rules! bridge { () => { pub mod generated { pub use draw::rr::*; } }; }
            bridge!();
            crate::rr::enabled! { pub struct Selected; }
            crate::runtime::enabled! { pub struct Aliased; }
            draw::rr::enabled! { pub struct Direct; }
            crate::rr::disabled! { unknown!(); }
            self::generated::enabled! { pub struct Generated; }
            crate::ui::generated::enabled! { pub struct Absolute; }
        "#;
        let mut toks = Vec::new();
        assert!(crate::lexer::lex(source, 2021, &mut toks).is_ok());
        let ctx = std::rc::Rc::new(crate::builtins::FileCtx::new("app", "ui.rs", "app::ui", &cfg));
        let table = Arc::new(table);
        let surface = crate::parser::rs_expand::expand(source, toks, 2021, ctx, table.clone());
        assert!(surface.unknown.is_empty(), "{:?}", surface.unknown);
        let index = crate::rs_outline::expanded_index(&surface.text(source), &surface.toks).unwrap();
        let names = index.items.iter().map(|item| item.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names, ["bridge", "generated", "Selected", "Aliased", "Direct", "Generated", "Absolute"]);
        assert!(surface.used.replays(&table, "app", "ui.rs"));
        let mut published = (*table).clone();
        published.add_exp_uses("app", surface.used.uses);
        published.seal_crate("app");
        assert!(published.lookup("enabled", "app", &["crate".into(), "ui".into(), "generated".into()], "app").is_some());
    }



    #[test]
    fn cold_preparse_stays_within_load_window_and_drains() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let dir = std::env::temp_dir().join(format!("relax-cold-pool-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        let crates: Vec<_> = (0..5).map(|i| {
            let root = dir.join(format!("c{}.rs", i)).to_string_lossy().into_owned();
            std::fs::write(&root, "pub fn f() -> u32 { 1 }").unwrap();
            PreCrate {  name: format!("c{}", i), root, edition: 2021, cfg: CfgSet::host(sess, &[]), deps: vec![], proc_macro: false, image_deps: vec![], image_dep_names: vec![] }
        }).collect();
        let (pool, workers) = start(sess, crates, &MacroTable::default(), None, true, Default::default());
        for ci in 0..5 {
            let a = pool.take_ahead(ci);
            assert!(pool.st.lock().unwrap().published <= ci as usize + 2);
            for path in a.files {
                // With a single parse job configured, the load would use a.prelexed instead.
                if pool.parse { assert!(pool.wait(sess, ci, &path).is_some()); }
            }
        }
        for w in workers { w.join().unwrap(); }
        assert!(pool.st.lock().unwrap().done.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod lowered_scope_tests {
    use super::*;

    #[test]
    fn cfg_macro_imports_use_configured_attributes() {
        let sess = Arc::new(crate::Session::default());
        let source = br#"
            #[cfg_attr(feature = "selected", macro_use)]
            extern crate enabled;
            #[cfg(not(feature = "selected"))]
            #[macro_use]
            extern crate disabled;
            #[cfg(any())]
            #[macro_use]
            extern crate absent;
            #[cfg_attr(any(), macro_use)]
            extern crate ordinary;
        "#;
        let path = "cfg_macro_imports.rs";
        let context = crate::source_context::Context::new(path, Default::default(), &sess);
        for (features, expected) in [(vec![], "disabled"), (vec!["selected".to_string()], "enabled")] {
            let cfg = CfgSet::host(&sess, &features);
            let scan = scan_text(path, ".", "fixture", "fixture", 2021, &cfg, source.to_vec().into(), &context).unwrap();
            assert_eq!(scan.macro_use, [expected]);
        }
    }


}
