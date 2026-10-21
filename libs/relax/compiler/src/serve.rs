//! Warm, single-writer checking. Transport-independent state for `check --watch` and serve.
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::time::{Duration, Instant};
use crate::diag::Diagnostic;
use crate::jit::Unit;
use crate::program::{DefId, DefKind};
use crate::reload::Change;

pub struct Session {
    pub unit: Box<Unit>,
    pub root: u32,
    pub own_crates: Vec<u32>,
    pub items: HashMap<DefId, Vec<Diagnostic>>,
    pub front: HashMap<String, Vec<Diagnostic>>,
    /// Dense reverse index; updated after each checked owner (including its instances).
    pub dependents: Vec<Vec<u32>>,
    edges: HashMap<DefId, Vec<DefId>>,
    deep: bool,
    lazy: bool,
    deep_errors: Vec<Diagnostic>,
    files: HashMap<String, u32>,
    parse_errors: HashMap<String, Diagnostic>,
    failed_probes: Vec<(String, bool)>,
    sources: HashMap<String, Vec<u8>>,
    pub rechecked: usize,
    pub rss_before_trim_mb: f64,
    pub rss_mb: f64,
    pub ms: f64,
}

impl Session {
    pub fn query_ready(&self) -> bool { self.parse_errors.is_empty() }
    pub fn paths(&self) -> Vec<String> {
        let mut paths: Vec<_> = self.files.keys().cloned().collect();
        paths.extend(self.unit.prog.module_probes.iter().filter(|(p, _)| p.ends_with(".rs")).map(|(p, _)| p.clone()));
        paths.extend(self.failed_probes.iter().map(|(p, _)| p.clone()));
        if let Some(plan) = &self.unit.prog.demand_plan { paths.extend(plan.indexed_paths()); }
        paths.extend(crate::native_macros::paths());
        paths.sort(); paths.dedup(); paths
    }
    pub fn file(&self, path: &str) -> Option<u32> {
        self.files.get(path).copied().or_else(|| {
            let target = crate::vfs::canonicalize(path).ok()?;
            self.files.iter().find(|(p,_)| crate::vfs::canonicalize(p).ok().as_ref() == Some(&target)).map(|(_,f)|*f)
        })
    }
    pub fn contains(&self, path: &str) -> bool {
        if self.file(path).is_some() { return true; }
        let target = Self::source_spelling(path);
        self.paths().iter().any(|p| Self::source_spelling(p) == target)
    }
    fn source_spelling(path: &str) -> String {
        let path = std::path::Path::new(path);
        path.ancestors().find_map(|parent| crate::vfs::canonicalize(parent).ok()
            .map(|root| root.join(path.strip_prefix(parent).unwrap())))
            .map(|p| crate::vfs::normalize_path(&p.to_string_lossy()))
            .unwrap_or_else(|| crate::vfs::normalize_path(&path.to_string_lossy()))
    }

    pub fn load(sess: &std::sync::Arc<crate::Session>, args: &[String]) -> Result<Self, String> { crate::run::load_check_session(sess, args) }

    pub fn new(mut unit: Box<Unit>, root: u32, mut errors: Vec<String>) -> Self {
        crate::borrowck::lazy::disable_gandalf();
        crate::jit::COMPILE_ERRORS_FATAL.store(false, std::sync::atomic::Ordering::Relaxed);
        crate::layout::set_root_crate(root);
        // the app scope was set from the unit graph by run::cmd_run_inner (gandalf_app_roots);
        // the root crate's own errors are reported even when the rule leaves it out
        let own_crates: Vec<_> = (0..unit.prog.crates.len() as u32)
            .filter(|k| *k == root || crate::borrowck::event::app_crate(&unit.prog, *k)).collect();
        for &k in &own_crates {
            errors.extend(unit.tcx.check_signature_errors(&unit.prog, k));
            if k != root {
                errors.extend(crate::check::own_errors(&unit.prog, k, &unit.prog.errors, "resolve"));
                errors.extend(crate::check::own_errors(&unit.prog, k, &unit.tcx.errors, "typeck"));
            }
        }
        let mut files = HashMap::new();
        for (p, i) in unit.prog.current_source_files() { files.insert(p, i); }
        let mut front: HashMap<String, Vec<Diagnostic>> = HashMap::new();
        let mut parse_errors = HashMap::new();
        for e in errors {
            let d = crate::diag::from_legacy(&e, "resolve");
            if d.phase == "parse" { parse_errors.insert(d.file.clone(), d.clone()); }
            front.entry(d.file.clone()).or_default().push(d);
        }
        let sources: HashMap<String, Vec<u8>> = files.iter().map(|(p, f)| {
            let text = if unit.prog.files[*f as usize].src_len == 0 { crate::vfs::read(p).ok() } else { None };
            (p.clone(), text.unwrap_or_else(|| unit.prog.files[*f as usize].src[..unit.prog.files[*f as usize].src_len as usize].to_vec()))
        }).collect();
        // Startup parses are not reload parses. Prime expansion reuse before readiness:
        // otherwise the first body edit rebuilds the proc-macro world. Keep that world
        // warm for context-sensitive expansions whose spans move in subsequent edits.
        let mut hot: Vec<_> = files.iter().filter(|(p, f)| p.ends_with(".rs") && own_crates.contains(&unit.prog.file_crate[**f as usize])).map(|(p, f)| (*f, p.clone())).collect();
        hot.sort_by_key(|(f, _)| *f);
        let mut normalize = Vec::new();
        let mut primed = Vec::new();
        for (file, path) in hot {
            if let Ok(v) = unit.prog.parse_version(file, &path, sources[&path].clone()) {
                if let Ok(plan) = crate::reload::classify(&unit.prog, file, &v.pf, &path) {
                    if plan.changes.is_empty() && plan.added.is_empty() && plan.removed.is_empty() && plan.removed_uses.is_empty() && !plan.attributes_changed {
                        if let Ok(f) = unit.prog.commit_body_version(v, &plan.pairs) {
                            unit.prog.lower_body_declarations(&[], &[f]);
                            primed.push(file);
                            primed.push(f);
                            files.insert(path, f);
                        }
                    } else { normalize.push(path); }
                } else { normalize.push(path); }
            }
        }
        // The new versions' body inputs (round-47 per-file inputs), as reload.rs publish_reload.
        if !primed.is_empty() {
            unit.tcx.queries.register_files(unit.prog.files.len());
            unit.tcx.publish_file_bodies(&primed);
        }
        let lazy = !unit.prog.session.options.source_eager;
        let mut session = Self { unit, root, own_crates, sources, deep: false, lazy, deep_errors: Vec::new(), items: HashMap::new(), front, dependents: Vec::new(), edges: HashMap::new(), files,
            parse_errors, failed_probes: Vec::new(), rechecked: 0, rss_before_trim_mb: 0.0, rss_mb: 0.0, ms: 0.0 };
        // Late-resolved item macros initially live in separate expansion files. Normalize
        // those crates once, before readiness, so the first body edit pairs the same items.
        if !normalize.is_empty() {
            let edits = normalize.into_iter().map(|p| { let text = session.sources[&p].clone(); (p, Ok(text)) }).collect();
            session.apply_texts_inner(edits, false);
        }
        session
    }

    fn functions(&self) -> Vec<DefId> {
        self.unit.prog.defs.iter().enumerate().filter_map(|(i, d)| {
            if !self.own_crates.contains(&d.krate) || !matches!(d.kind, DefKind::Fn | DefKind::AssocFn) { return None; }
            match &self.unit.prog.files[d.file as usize].ast.item(d.item).kind {
                crate::ast::ItemKind::Fn(s, b, _) if b.is_some() || self.unit.prog.files[d.file as usize].ast.fn_sig(*s).lazy_body.is_some() => Some(DefId(i as u32)),
                _ => None,
            }
        }).collect()
    }

    pub fn check(&mut self) {
        if let Err(error) = crate::native_macros::verdict() {
            self.parse_errors.insert("native-macros".into(), Diagnostic::new("macro", "", 0, 0, error));
            return;
        }
        let sess = &self.unit.prog.session.clone();
        let options = &sess.options;
        let start = Instant::now();
        self.rechecked = 0;
        for d in self.functions() { self.check_item(options, d); }
        if self.deep { self.deep_errors = crate::check::collect_deep(&mut self.unit, self.root); }
        self.trim();
        self.ms = start.elapsed().as_secs_f64() * 1000.0;
    }

    fn check_item(&mut self, options: &crate::options::Options, d: DefId) {
        let mut borrow_diagnostics = Vec::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let u = &mut self.unit;
            let generic = u.tcx.fn_sig(&u.prog, d).map_or(true, |s| s.n_generics > 0)
                || u.prog.def(u.prog.def(d).parent).kind == DefKind::Trait;
            let args = if generic { crate::borrowck::identity_args(&u.tcx, &u.prog, d).unwrap_or_default() } else { Vec::new() };
            let mut queue = vec![(d, args)];
            let mut seen = HashSet::new();
            let mut errors = Vec::new();
            let mut edges = Vec::new();
            while let Some((item, args)) = queue.pop() {
                if !seen.insert((item, args.clone())) { continue; }
                if seen.len() > 50_000 {
                    errors.push("internal compiler error: check instance limit exceeded".to_string());
                    break;
                }
                // Body diagnostics are returned on Body. Do not let speculative type
                // queries leak into the signature collector's next flush_errs call.
                let state = crate::tcx::EvalState::new();
                let check = |u: &crate::jit::Unit| {
                    let cx = crate::tcx::EvalCx::new(&u.prog, &u.tcx, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), &state);
                    u.tcx.with_errors(&cx, || crate::typeck::instantiate_body(&cx, &u.prog, &u.tcx, item, &args)).0
                };
                let body = check(u);
                edges.extend_from_slice(&body.dependencies);
                for r in body.res.values().chain(body.pat_res.values()).chain(body.fmt_named.values()) {
                    if let crate::typeck::Res::Def(d) = r { edges.push(*d); }
                }
                for m in body.methods.values().chain(body.binops.values()) { edges.push(m.def); }
                for (d, _) in body.overloaded_derefs().map(|(_, _, ov)| ov).collect::<Vec<_>>().iter().chain(body.ov_index.values()).chain(body.for_next.values()) { edges.push(*d); }
                for (d, _, _) in body.for_into.values() { edges.push(*d); }
                for (d, _, _) in body.try_branch.values() { edges.push(*d); }
                for (d, _) in body.try_from_residual.values() { edges.push(*d); }
                for t in body.expr_ty.iter().chain(body.pat_ty.iter()) { crate::typeck::type_dependencies(&u.tcx, *t, &mut edges); }
                // Check concrete instances reached in the package, with diagnostics owned by
                // the caller. Closure bodies are already checked in their enclosing Body.
                let cx = crate::tcx::EvalCx::new(&u.prog, &u.tcx, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), &state);
                for (callee, args) in u.tcx.with_errors(&cx, || crate::jit::body_callees(&u.prog, &u.tcx, &body, false, None)).0 {
                    edges.push(callee);
                    if self.own_crates.contains(&u.prog.def(callee).krate) && !args.is_empty() { queue.push((callee, args)); }
                }
                let polymorphic = args.iter().any(|a| !u.tcx.tys.is_concrete(*a));
                if body.errors.is_empty() && crate::borrowck::enabled(options) {
                    let verdict = u.tcx.with_errors(&cx, || crate::borrowck::check_body(&cx, &u.prog, &u.tcx, &body)).0;
                    borrow_diagnostics.extend(crate::borrowck::lazy::report_checked(&u.prog, 0, u.tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst), item, &args, verdict.errors, crate::borrowck::event::Origin::Serve).iter().map(|e| e.error.to_diagnostic()));
                }
                // Match check_crate's treatment of polymorphic bodies: concrete instances
                // supply type errors; identity checking supplies borrow errors when supported.
                if !polymorphic { errors.extend(body.errors); }
            }
            (errors, edges)
        }));
        let (errors, mut edges) = match result {
            Ok(v) => v,
            Err(_) => {
                let p = &self.unit.prog;
                let def = p.def(d);
                let pos = p.files[def.file as usize].ast.item(def.item).lo;
                (vec![p.record_error(def.file, pos, "typeck", format!("internal compiler error while type-checking {} -- makepad run bug, report it", p.def_path(d)))], Vec::new())
            }
        };
        edges.sort_by_key(|d| d.0); edges.dedup();
        if let Some(old) = self.edges.insert(d, edges.clone()) {
            for dep in old { if let Some(v) = self.dependents.get_mut(dep.0 as usize) { v.retain(|i| *i != d.0); } }
        }
        self.dependents.resize_with(self.unit.prog.defs.len(), Vec::new);
        for dep in edges { if let Some(v) = self.dependents.get_mut(dep.0 as usize) { v.push(d.0); } }
        let mut diagnostics: Vec<_> = errors.iter().map(|e| crate::diag::from_legacy(e, "typeck")).collect();
        diagnostics.extend(borrow_diagnostics);
        self.items.insert(d, diagnostics);
        self.rechecked += 1;
    }

    fn refresh_front(&mut self) {
        self.front.clear();
        let p = &self.unit.prog;
        for (errors, phase) in [(&p.errors, "resolve"), (&self.unit.tcx.errors, "typeck")] {
            for &krate in &self.own_crates {
                for e in crate::check::own_errors(p, krate, errors, phase) {
                    let d = crate::diag::from_legacy(&e, phase);
                    self.front.entry(d.file.clone()).or_default().push(d);
                }
            }
        }
        for e in crate::macro_reg::derive_failures() {
            let d = crate::diag::from_legacy(&e, "macro");
            self.front.entry(d.file.clone()).or_default().push(d);
        }
    }

    /// Apply disk versions. `did_change` uses exactly the same path for unsaved buffers.
    pub fn apply(&mut self, paths: &[String]) {
        let edits = paths.iter().map(|p| (p.clone(), crate::vfs::read(p))).collect();
        self.apply_texts(edits);
    }

    pub fn did_change(&mut self, path: String, text: String) {
        self.apply_texts(vec![(path, Ok(text.into_bytes()))]);
    }

    fn apply_texts(&mut self, edits: Vec<(String, std::io::Result<Vec<u8>>)>) {
        self.apply_texts_inner(edits, true);
    }

    fn apply_texts_inner(&mut self, edits: Vec<(String, std::io::Result<Vec<u8>>)>, check_bodies: bool) {
        let changes: Vec<_> = edits.iter().map(|(p, text)| (p.as_str(), text.as_ref().ok().map(|s| s.as_slice()))).collect();
        if let Err(error) = crate::native_macros::reload(&changes, self.unit.prog.session.runtime.profile) {
            self.parse_errors.insert("native-macros".into(), Diagnostic::new("macro", "", 0, 0, error));
            return;
        }
        self.parse_errors.remove("native-macros");
        let edits: Vec<_> = edits.into_iter().filter(|(p, _)| !crate::native_macros::owns_path(p)).collect();
        if edits.is_empty() { return; }
        // A skipped/appearing file has no old DefId to invalidate. Recompute the
        // demand closure and publish a canonical program before any query runs.
        if !self.unit.prog.load_recipe.is_empty() && (self.lazy
            || edits.iter().any(|(p, _)| self.file(p).is_none())) {
            self.rebuild_sources(edits, check_bodies, false);
            return;
        }
        let sess = &self.unit.prog.session.clone();
        let options = &sess.options;
        let start = Instant::now();
        let trace = std::env::var_os("MAKEPAD_RUN_RELOAD_TRACE").is_some();
        if trace { crate::parse_work::begin(); }
        if crate::diag::enabled() {
            // Retained front-end errors still need their structured spans/suggestions
            // after the previous body's diagnostic registry is released.
            let retained: Vec<_> = self.unit.prog.errors.iter().chain(&self.unit.tcx.errors)
                .map(|e| (e.clone(), crate::diag::from_legacy(e, "resolve"))).collect();
            crate::diag::enable();
            for (e, d) in retained { crate::diag::record(e, d); }
        }
        self.rechecked = 0;
        let mut dirty = HashSet::new();
        let mut legacy_owners = Vec::new();
        let mut rebuild = HashSet::new();
        let mut macro_rebuild = false;
        let mut versions = Vec::new();
        for (path, src) in edits {
            let path = if self.files.contains_key(&path) { path } else {
                let canonical = crate::vfs::canonicalize(&path).map(|p| p.to_string_lossy().into_owned()).unwrap_or(path);
                if self.files.contains_key(&canonical) { canonical } else {
                    self.files.keys().find(|p| crate::vfs::canonicalize(p).ok().is_some_and(|p| p.to_string_lossy() == canonical)).cloned().unwrap_or(canonical)
                }
            };
            let Some(&file) = self.files.get(&path) else {
                self.parse_errors.insert(path.clone(), Diagnostic::new("parse", path, 0, 0, "file is not loaded in this package"));
                continue;
            };
            let readable = src.is_ok();
            if let Ok(text) = &src { self.sources.insert(path.clone(), text.clone()); }
            let parsed = src.map_err(|e| format!("{}:1:1: {}", path, e)).and_then(|s| self.unit.prog.parse_version(file, &path, s));
            let v = match parsed {
                Ok(v) => { self.parse_errors.remove(&path); v },
                Err(e) => {
                    let e = e.trim_end_matches(" -- fix the syntax; the running code is unchanged");
                    self.parse_errors.insert(path.clone(), crate::diag::from_legacy(e, "parse"));
                    // A changed macro/import can make parsing against the old macro table
                    // fail. Retry with this crate's fresh prescan before declaring failure.
                    if readable { rebuild.insert(self.unit.prog.file_crate[file as usize]); }
                    continue;
                }
            };
            let krate = self.unit.prog.file_crate[file as usize];
            if self.front.get(&path).is_some_and(|v| !v.is_empty()) { rebuild.insert(krate); }
            let plan = crate::reload::classify(&self.unit.prog, file, &v.pf, &path);
            let pairs = match plan {
                Ok(plan) => {
                    if trace { eprintln!("makepad-run check plan: {}: {} changes, {} added, {} removed, {} uses removed", path, plan.changes.len(), plan.added.len(), plan.removed.len(), plan.removed_uses.len()); }
                    if !plan.added.is_empty() || !plan.removed.is_empty() || !plan.removed_uses.is_empty() || plan.attributes_changed { rebuild.insert(krate); }
                    for change in plan.changes {
                        match change {
                            Change::Body(d) => {
                                legacy_owners.push(d);
                                dirty.insert(d);
                                let def = self.unit.prog.def(d);
                                let pf = &self.unit.prog.files[def.file as usize];
                                if let crate::ast::ItemKind::Fn(sig, _, _) = pf.ast.item(def.item).kind {
                                    if pf.ast.fn_sig(sig).konst { rebuild.insert(krate); }
                                }
                            },
                            Change::Const(d) | Change::StaticInit(d) => {
                                // Consts also affect signatures, array lengths and CTFE. A
                                // crate recollection is the conservative transitive invalidation.
                                dirty.insert(d); rebuild.insert(krate);
                            }
                            Change::Layout(_) => { rebuild.insert(krate); }
                            Change::Macro(_) => { rebuild.insert(krate); macro_rebuild = true; }
                        }
                    }
                    plan.pairs
                }
                Err(why) => {
                    if trace { eprintln!("makepad-run check rebuild: {}: {:?}", path, why); }
                    macro_rebuild |= why.iter().any(|e| e.contains("macro"));
                    rebuild.insert(krate);
                    crate::reload::pair_items(&self.unit.prog.files[file as usize], &v.pf).0
                }
            };
            // Retained errors in this file need new source coordinates even if their item
            // is unchanged. Re-derive them instead of guessing span offsets through macros.
            for (&d, errors) in &self.items {
                if !errors.is_empty() && errors.iter().any(|e| e.file == path) { dirty.insert(d); }
            }
            versions.push((v, pairs));
        }
        let mut body_files = Vec::new();
        for (v, pairs) in versions {
            let path = v.path.clone();
            let old = v.old;
            let committed = if rebuild.is_empty() { self.unit.prog.commit_body_version(v, &pairs) }
                else { self.unit.prog.commit_version(v, &pairs) };
            match committed {
                Ok(f) => { body_files.push(old); body_files.push(f); self.files.insert(path, f); },
                Err(e) => { self.parse_errors.insert(path.clone(), crate::diag::from_legacy(&e, "parse")); }
            }
        }
        let parsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        if rebuild.is_empty() {
            self.unit.prog.lower_body_declarations(&legacy_owners, &body_files);
            // The edited versions' body inputs, as reload.rs publish_reload.
            self.unit.tcx.queries.register_files(self.unit.prog.files.len());
            self.unit.tcx.publish_file_bodies(&body_files);
            self.refresh_front();
            // Include callers before invalidating hidden opaque types: an unchanged
            // caller's inferred return can change when its callee's body changes.
            let mut queue: Vec<_> = dirty.iter().copied().collect();
            while let Some(d) = queue.pop() {
                if let Some(v) = self.dependents.get(d.0 as usize) {
                    for &dep in v { if dirty.insert(DefId(dep)) { queue.push(DefId(dep)); } }
                }
            }
        }
        if rebuild.is_empty() { self.unit.tcx.invalidate_check_bodies(&self.unit.prog, &dirty.iter().copied().collect::<Vec<_>>()); }
        else { self.unit.tcx.invalidate_check(&[]); }
        for i in 0..self.unit.fns.len() {
            let f = &mut self.unit.fns[i];
            f.compiled = false; f.rir = None; f.build_err = None; f.building = false;
        }
        if !rebuild.is_empty() {
            self.parse_errors.remove(""); // Retry any failed crate recollection.
            // Recollect only edited crates. All package functions are rechecked because new
            // items/impls can change previously failed lookups (no old edge exists for those).
            let mut crates: Vec<_> = rebuild.into_iter().collect(); crates.sort();
            let names: Vec<_> = crates.iter().map(|k| self.unit.prog.crates[*k as usize].name).collect();
            for (k, c) in self.unit.prog.crates.iter().enumerate() {
                if names.contains(&c.name) && !crates.contains(&(k as u32)) { crates.push(k as u32); }
            }
            crates.sort();
            let mut affected = crates.clone();
            loop {
                let n = affected.len();
                for (k, c) in self.unit.prog.crates.iter().enumerate() {
                    if !affected.contains(&(k as u32)) && c.deps.iter().any(|(_, d)| affected.contains(d)) { affected.push(k as u32); }
                }
                if affected.len() == n { break; }
            }
            // Consumers contain expanded syntax, so changing a macro also requires
            // reparsing dependent crates, in their original dependency/load order.
            if macro_rebuild { crates = affected.clone(); crates.sort(); }
            let mut prescanned = HashMap::new();
            for &krate in &crates {
                let old: Vec<_> = self.unit.prog.defs.iter().enumerate().filter(|(_, d)| d.krate == krate).map(|(i, _)| DefId(i as u32)).collect();
                let impls: Vec<_> = old.iter().copied().filter(|d| self.unit.prog.def(*d).kind == DefKind::Impl).collect();
                self.unit.tcx.uncollect_impls(&impls);
                self.unit.tcx.invalidate_check(&old);
                let mut texts: Vec<_> = self.files.iter().filter(|(_, f)| self.unit.prog.file_crate[**f as usize] == krate)
                    .filter_map(|(p, f)| self.sources.get(p).map(|t| (*f, t.clone()))).collect();
                texts.sort_by_key(|(f, _)| *f);
                let paths: Vec<_> = texts.iter().map(|(f, _)| self.unit.prog.file_paths[*f as usize].clone()).collect();
                crate::macro_reg::clear_derive_failures(&paths);
                if !prescanned.contains_key(&krate) {
                    let name = self.unit.prog.crates[krate as usize].name;
                    let inputs: Vec<_> = self.files.iter().filter(|(_, f)| self.unit.prog.crates[self.unit.prog.file_crate[**f as usize] as usize].name == name)
                        .filter_map(|(p, f)| self.sources.get(p).map(|t| (*f, t.clone()))).collect();
                    prescanned.extend(crate::preload::check_macros(&mut self.unit.prog, krate, &inputs));
                }
                let reachable = &prescanned[&krate];
                texts.retain(|(f, _)| reachable.contains(&self.unit.prog.file_paths[*f as usize]));
                self.parse_errors.retain(|p, _| !self.files.get(p).is_some_and(|f| self.unit.prog.file_crate[*f as usize] == krate && !reachable.contains(p)));
                for (f, text) in texts {
                    let path = self.unit.prog.file_paths[f as usize].clone();
                    let mut snapshot = self.unit.prog.parse_snapshot(f, &path);
                    snapshot.reuse = Default::default();
                    match snapshot.parse(text) {
                        Ok(v) => {
                            self.parse_errors.remove(&path);
                            let pairs = crate::reload::pair_items(&self.unit.prog.files[f as usize], &v.pf).0;
                            match self.unit.prog.commit_version(v, &pairs) {
                                Ok(n) => { self.files.insert(path, n); },
                                Err(e) => { self.parse_errors.insert(path, crate::diag::from_legacy(e.trim_end_matches(" -- fix the syntax; the running code is unchanged"), "parse")); }
                            }
                        }
                        Err(e) => { self.parse_errors.insert(path, crate::diag::from_legacy(e.trim_end_matches(" -- fix the syntax; the running code is unchanged"), "parse")); }
                    }
                }
                let files = self.files.iter().filter(|(_, f)| self.unit.prog.file_crate[**f as usize] == krate).map(|(p, f)| (p.clone(), *f)).collect();
                match self.unit.prog.recollect_check_crate(krate, files) {
                    Ok(defs) => {
                        self.unit.tcx.errors.retain(|e| !self.files.iter().any(|(p, f)| self.unit.prog.file_crate[*f as usize] == krate && e.starts_with(p)));
                        self.unit.consts.clear(); self.unit.statics.clear(); self.unit.static_spans.clear();
                        self.unit.tcx.collect_check(&self.unit.prog, &defs);
                    }
                    Err(e) => { self.parse_errors.insert(String::new(), crate::diag::from_legacy(&e, "resolve")); }
                }
            }
            // Dependent crates keep their files and DefIds, but signatures may name the
            // replaced definitions and must be collected again.
            for krate in affected.into_iter().filter(|k| !crates.contains(k)) {
                let defs: Vec<_> = self.unit.prog.defs.iter().enumerate().filter(|(_, d)| d.krate == krate).map(|(i, _)| DefId(i as u32)).collect();
                let impls: Vec<_> = defs.iter().copied().filter(|d| self.unit.prog.def(*d).kind == DefKind::Impl).collect();
                self.unit.tcx.uncollect_impls(&impls);
                self.unit.tcx.invalidate_check(&defs);
                self.unit.tcx.errors.retain(|e| !self.files.iter().any(|(p, f)| self.unit.prog.file_crate[*f as usize] == krate && e.starts_with(p)));
                self.unit.tcx.collect_check(&self.unit.prog, &defs);
            }
            for (path, i) in self.unit.prog.current_source_files() {
                self.files.insert(path.clone(), i);
                self.sources.entry(path).or_insert_with(|| self.unit.prog.files[i as usize].src[..self.unit.prog.files[i as usize].src_len as usize].to_vec());
            }
            self.items.clear(); self.edges.clear(); self.dependents.clear();
            dirty = self.functions().into_iter().collect();
            self.refresh_front();
        }
        let mut dirty: Vec<_> = dirty.into_iter().filter(|d| self.own_crates.contains(&self.unit.prog.def(*d).krate) && matches!(self.unit.prog.def(*d).kind, DefKind::Fn | DefKind::AssocFn)).collect();
        dirty.sort_by_key(|d| d.0);
        if !check_bodies {
            if trace { crate::parse_work::finish().print("check startup"); }
            return;
        }
        let invalidated_ms = start.elapsed().as_secs_f64() * 1000.0;
        for d in dirty { self.check_item(options, d); }
        if self.deep { self.deep_errors = crate::check::collect_deep(&mut self.unit, self.root); }
        let checked_ms = start.elapsed().as_secs_f64() * 1000.0;
        self.trim();
        self.ms = start.elapsed().as_secs_f64() * 1000.0;
        if trace {
            crate::parse_work::finish().print("check");
            eprintln!("makepad-run check timing: parse/classify/commit {:.3}, invalidate/front {:.3}, check {:.3}, trim {:.3} ms",
                parsed_ms, invalidated_ms - parsed_ms, checked_ms - invalidated_ms, self.ms - checked_ms);
        }
    }

    fn rebuild_sources(&mut self, edits: Vec<(String, std::io::Result<Vec<u8>>)>, check_bodies: bool, force_eager: bool) {
        let start = Instant::now();
        for (path, text) in edits {
            let path = self.file(&path).map(|f| self.unit.prog.file_paths[f as usize].clone()).unwrap_or_else(|| crate::vfs::normalize_path(&path));
            let canonical = Self::source_spelling(&path);
            // Skipped files have no file ID yet. Remove old aliases too, so an
            // earlier overlay cannot overwrite the newest edit.
            self.sources.retain(|old, _| Self::source_spelling(old) != canonical);
            match text {
                Ok(text) => { self.sources.insert(path, text); }
                Err(_) => { self.sources.remove(&path); }
            }
        }
        let mut sources = crate::source::SourceRegistry::default();
        let mut saved: Vec<_> = self.sources.iter().collect(); saved.sort_by_key(|(p, _)| *p);
        for (path, text) in saved { sources = sources.overlay(path, text.clone().into()); }
        let eager = force_eager || !self.lazy;
        let mut probes = Vec::new();
        crate::macro_reg::clear_derive_failures(&self.paths());
        let replacement = self.unit.prog.replay_watched_sources(std::sync::Arc::new(sources), eager, None, &mut probes).and_then(|mut prog| {
            // A missing module was demanded by collection, even if its new
            // outline has no used items. Check the recovered file before
            // clearing the parse error; leave unrelated modules skipped.
            let missing: HashSet<_> = self.failed_probes.iter().filter(|(_, exists)| !exists)
                .map(|(path, _)| Self::source_spelling(path)).collect();
            if !missing.is_empty() {
                let (f0, d0) = (prog.files.len() as u32, prog.defs.len());
                loop {
                    let mut modules: Vec<_> = prog.skipped_files.iter()
                        .filter(|(_, (path, ..))| missing.contains(&Self::source_spelling(path))).map(|(m, _)| *m).collect();
                    if modules.is_empty() { break; }
                    modules.sort_unstable();
                    for module in modules {
                        if let Err(error) = prog.load_deferred_module(module) {
                            probes.extend(self.failed_probes.iter().cloned());
                            probes.extend(prog.module_probes.iter().cloned());
                            return Err(error);
                        }
                    }
                }
                if prog.files.len() as u32 != f0 { prog.link_deferred_modules(f0, d0); }
            }
            Ok(prog)
        });
        let prog = match replacement {
            Ok(prog) => prog,
            Err(error) => {
                self.failed_probes = probes;
                self.parse_errors.insert(String::new(), crate::diag::from_legacy(&error, "parse"));
                self.rechecked = 0;
                self.ms = start.elapsed().as_secs_f64() * 1000.0;
                return;
            }
        };
        let mut tcx = crate::tcx::Tcx::new(); tcx.collect(&prog);
        let mut front: HashMap<String, Vec<Diagnostic>> = HashMap::new();
        for &krate in &self.own_crates {
            for error in crate::check::own_errors(&prog, krate, &prog.errors, "resolve").into_iter()
                .chain(crate::check::own_errors(&prog, krate, &tcx.errors, "typeck")) {
                let diagnostic = crate::diag::from_legacy(&error, "resolve");
                front.entry(diagnostic.file.clone()).or_default().push(diagnostic);
            }
        }
        let core = self.unit.core_crate;
        // No checking job can retain the old definition/type IDs after a graph
        // replacement. Use the existing auxiliary-unit retirement protocol.
        unsafe { crate::jit::retire_unit(&mut *self.unit); }
        self.unit = Unit::new(prog, tcx, core);
        self.files = self.unit.prog.current_source_files().into_iter().collect();
        for (path, file) in &self.files {
            self.sources.entry(path.clone()).or_insert_with(|| {
                let pf = &self.unit.prog.files[*file as usize]; pf.src[..pf.src_len as usize].to_vec()
            });
        }
        self.items.clear(); self.edges.clear(); self.dependents.clear();
        self.parse_errors.clear(); self.failed_probes.clear(); self.deep_errors.clear(); self.front = front;
        if check_bodies { self.check(); }
        self.ms = start.elapsed().as_secs_f64() * 1000.0;
    }

    /// An editor query explicitly reaches a previously unlowered module. Queries
    /// run between checks, so rebuilding here cannot invalidate an active reader.
    pub fn reach_query_files(&mut self) -> Result<(), String> {
        if self.unit.prog.demand_plan.as_ref().is_some_and(|p| { let (n, total) = p.counts(); n != total }) {
            self.rebuild_sources(Vec::new(), false, true);
        }
        if self.query_ready() { Ok(()) } else { Err("fix parse errors before querying this package".into()) }
    }

    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        // A failed parse has no new semantic snapshot. As in a one-shot load, publish
        // parse failures alone until recovery; never label old semantic errors current.
        let mut out: Vec<_> = if self.parse_errors.is_empty() {
            self.front.values().chain(self.items.values()).flatten().chain(self.deep_errors.iter()).cloned().collect()
        } else { self.parse_errors.values().cloned().collect() };
        out.sort_by(|a, b| (&a.file, a.line, a.col, &a.message, a.phase).cmp(&(&b.file, b.line, b.col, &b.message, b.phase)));
        out.dedup(); out
    }

    pub fn emit_diagnostics(&self) {
        for d in self.diagnostics() {
            if crate::diag::enabled() { crate::diag::emit(&d); }
            else { println!("{}:{}:{}: {}", d.file, d.line, d.col, d.message); }
        }
    }

    fn emit(&self) {
        self.emit_diagnostics();
        let errors = self.diagnostics().len();
        if crate::diag::enabled() {
            println!("{{\"type\":\"summary\",\"errors\":{},\"warnings\":0,\"ms\":{:.3},\"rechecked\":{},\"rss_mb\":{:.3},\"rss_before_trim_mb\":{:.3}}}", errors, self.ms, self.rechecked, self.rss_mb, self.rss_before_trim_mb);
        } else {
            println!("makepad check: {} errors, {} rechecked, {:.3} ms, {:.1} MB RSS", errors, self.rechecked, self.ms, self.rss_mb);
        }
        let _ = std::io::stdout().flush();
    }

    fn trim(&mut self) {
        let sess = &self.unit.prog.session.clone();
        self.rss_before_trim_mb = crate::memstat::resident() as f64 / 1048576.0;
        // Constants may have used the JIT, and --deep certainly did. Recreate only the
        // execution shell so all code/data allocations and function slots are released;
        // the Program, interner and collected types remain warm at stable IDs.
        if self.unit.fns.len() != 0 {
            unsafe { crate::jit::retire_unit(&mut *self.unit); }
            let core = self.unit.prog.prelude_crate.unwrap_or(0);
            let prog = std::mem::replace(&mut self.unit.prog, crate::program::Program::new(sess));
            let tcx = std::mem::replace(&mut self.unit.tcx, crate::tcx::Tcx::new());
            self.unit = Unit::new(prog, tcx, core);
        }
        self.unit.bodies.clear(); self.unit.old_bodies.clear();

        for f in self.unit.fns.iter_mut() { f.rir = None; f.spec = None; }
        self.unit.reload_specs.clear();
        // No worker can read these retired tables: this session has a single writer.
        unsafe {
            self.unit.tcx.alias_tys.reclaim();
            self.unit.prog.reclaim_caches();
        }
        if self.rss_mb == 0.0 || self.rechecked > 8 { crate::memstat::trim(); }
        self.rss_mb = crate::memstat::resident() as f64 / 1048576.0;
    }

    pub fn watch(&mut self, deep: bool) -> i32 {
        self.deep = deep;
        self.check();
        self.emit();
        let (tx, context) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                match line { Ok(line) => if tx.send(line).is_err() { break; }, Err(_) => break }
            }
        });
        let std = crate::std_dir();
        let mut watched: HashMap<String, _> = self.paths().into_iter().filter(|p| crate::vfs::relative_path(p, &std).is_none() && !p.contains("/.cargo/") && !p.contains("/rustlib/"))
            .map(|p| { let stamp = crate::reload::stamp(&p); (p, stamp) }).collect();
        let mut hot: HashSet<String> = self.files.iter().filter(|(_, f)| self.own_crates.contains(&self.unit.prog.file_crate[**f as usize])).map(|(p, _)| p.clone()).collect();
        let mut tick = 0u32;
        loop {
            match context.recv_timeout(Duration::from_millis(5)) {
                Ok(line) => {
                    let request = crate::json::parse(line.as_bytes());
                    match request {
                        Ok(r) => match r.get("method").map(|m| m.str()) {
                            Some("shutdown") => return 0,
                            Some("check") => { self.check(); self.emit(); },
                            Some("didChange") => {
                                if let Some(crate::json::Json::Str(path)) = r.get("path") {
                                    match r.get("text") {
                                        Some(crate::json::Json::Str(text)) => self.did_change(path.clone(), text.clone()),
                                        _ => self.apply(&[path.clone()]),
                                    }
                                    self.emit();
                                }
                            }
                            _ => { self.protocol_error("unknown method"); }
                        },
                        Err(e) => self.protocol_error(&e),
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return 0,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            tick = tick.wrapping_add(1);
            let mut changed = Vec::new();
            for (path, previous) in &mut watched {
                if tick % 20 != 0 && !hot.contains(path) { continue; }
                let stamp = crate::reload::stamp(path);
                if stamp != *previous { *previous = stamp; hot.insert(path.clone()); changed.push(path.clone()); }
            }
            if !changed.is_empty() { self.apply(&changed); self.emit(); }
            for p in self.paths() { if crate::vfs::relative_path(&p, &std).is_none() && !p.contains("/.cargo/") { watched.entry(p.clone()).or_insert_with(|| crate::reload::stamp(&p)); } }
        }
    }

    fn protocol_error(&mut self, message: &str) {
        let d = Diagnostic::new("run", "", 0, 0, message);
        if crate::diag::enabled() { crate::diag::emit(&d); } else { eprintln!("makepad check: {}", message); }
    }
}

impl Drop for Session {
    fn drop(&mut self) { unsafe { crate::jit::retire_unit(&mut *self.unit); } }
}

#[cfg(test)]
mod lazy_tests {
    use super::*;

    #[test]
    fn warm_indexes_queries_and_appearing_files_follow_the_current_sources() {
        isolated_test!(warm_indexes_queries_and_appearing_files_follow_the_current_sources);
        let dir = std::env::temp_dir().join(format!("relax-warm-context-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname=\"warm_rx\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[lib]\npath=\"main.rs\"\n").unwrap();
        let root = dir.join("main.rs").to_string_lossy().into_owned();
        let idle = dir.join("idle.rs").to_string_lossy().into_owned();
        let added = dir.join("added.rs").to_string_lossy().into_owned();
        let original = "mod idle; pub struct Base; pub trait Mark {}";
        std::fs::write(&root, original).unwrap();
        std::fs::write(&idle, "pub struct Unused;").unwrap();
        let mut lazy = crate::Session::default(); lazy.options.source_eager = false;
        let sess = std::sync::Arc::new(lazy);
        let mut prog = crate::program::Program::new(&sess);
        let cfg = crate::cfg::CfgSet::host(&sess, &[]);
        prog.preload(vec![crate::preload::PreCrate {  name: "warm_rx".into(), root: root.clone(), edition: 2021, cfg: cfg.clone(), deps: Vec::new(), proc_macro: false, image_deps: Vec::new(), image_dep_names: Vec::new() }]);
        let krate = prog.load_crate("warm_rx", &root, 2021, cfg, Vec::new()).unwrap();
        prog.resolve_imports();
        let mut tcx = crate::tcx::Tcx::new(); tcx.collect(&prog);
        let mut warm = Session::new(Unit::new(prog, tcx, 0), krate, Vec::new());
        assert!(warm.file(&idle).is_none());
        assert!(warm.contains(&idle));
        warm.did_change(idle.clone(), "pub struct StillUnused;".into());
        let alias = std::fs::canonicalize(&idle).unwrap().to_string_lossy().into_owned();
        assert!(warm.contains(&alias), "RPC canonical paths must find skipped files");
        warm.did_change(alias.clone(), "pub struct NewUnused;".into());
        assert!(warm.file(&idle).is_none());
        warm.reach_query_files().unwrap();
        assert!(warm.file(&idle).is_some());
        assert!(warm.unit.prog.defs.iter().any(|d| warm.unit.prog.syms.str(d.name) == "NewUnused"));
        // A changed impl index can reach a previously unused file. An alias of
        // the same authored path must not resurrect the old overlay bytes.
        warm.did_change(alias, "impl crate::Mark for crate::Base {}".into());
        assert!(warm.query_ready(), "{:?}", warm.diagnostics());
        // The plan indexes both the crate root and the newly reached sibling.
        assert_eq!(warm.unit.prog.demand_plan.as_ref().unwrap().counts(), (2, 2));
        assert_eq!(warm.unit.prog.impls.len(), 1);
        warm.did_change(root.clone(), format!("{original} mod added; pub use added::value;"));
        assert!(!warm.query_ready());
        assert!(warm.contains(&added), "failed edit lost the missing-file probe");
        let added_alias = std::fs::canonicalize(&dir).unwrap().join("added.rs").to_string_lossy().into_owned();
        assert!(warm.contains(&added_alias), "a missing file must match its canonical parent spelling");
        std::fs::write(&added, "pub fn value() {}").unwrap();
        warm.apply(&[added.clone()]);
        assert!(warm.query_ready(), "{:?}", warm.diagnostics());
        assert!(warm.file(&added).is_some());
        assert!(warm.diagnostics().is_empty(), "{:?}", warm.diagnostics());
        drop(warm);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sibling_impl_index_survives_edits_and_eager_queries() {
        isolated_test!(sibling_impl_index_survives_edits_and_eager_queries);
        let dir = std::env::temp_dir().join(format!("relax-warm-impl-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname=\"warm_impl\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[lib]\npath=\"main.rs\"\n").unwrap();
        let root = dir.join("main.rs").to_string_lossy().into_owned();
        let sibling = dir.join("sibling.rs").to_string_lossy().into_owned();
        let idle = dir.join("idle.rs").to_string_lossy().into_owned();
        // Serve checks this uncalled body too. Nothing refers to `sibling`.
        let source = "mod sibling; mod idle; pub struct Base; pub trait Mark {} fn require<T: Mark>(_: T) {} pub fn caller() { require(Base); }";
        let implementation = "impl crate::Mark for crate::Base {}";
        std::fs::write(&root, source).unwrap();
        std::fs::write(&sibling, implementation).unwrap();
        // Identical unqualified names must not demand unrelated impls.
        std::fs::write(&idle, "pub struct Base; pub trait Mark {} impl Mark for Base {}").unwrap();
        let sess = std::sync::Arc::new(crate::Session::default());
        let mut prog = crate::program::Program::new(&sess);
        let cfg = crate::cfg::CfgSet::host(&sess, &[]);
        prog.preload(vec![crate::preload::PreCrate { name: "warm_impl".into(), root: root.clone(), edition: 2021, cfg: cfg.clone(), deps: Vec::new(), proc_macro: false, image_deps: Vec::new(), image_dep_names: Vec::new() }]);
        let krate = prog.load_crate("warm_impl", &root, 2021, cfg, Vec::new()).unwrap();
        prog.resolve_imports();
        let mut tcx = crate::tcx::Tcx::new(); tcx.collect(&prog);
        let mut warm = Session::new(Unit::new(prog, tcx, 0), krate, Vec::new());
        warm.check();
        assert!(warm.diagnostics().is_empty(), "{:?}", warm.diagnostics());
        assert!(warm.file(&sibling).is_some());
        assert!(warm.file(&idle).is_none());
        assert_eq!(warm.unit.prog.demand_plan.as_ref().unwrap().counts(), (2, 3));

        warm.reach_query_files().unwrap();
        assert!(warm.file(&idle).is_some());
        assert!(warm.unit.prog.session.options.source_eager);
        assert!(warm.unit.prog.session.options.rs_lazy);
        warm.did_change(root.clone(), format!("{source}\n"));
        assert!(warm.diagnostics().is_empty(), "{:?}", warm.diagnostics());
        assert!(!warm.unit.prog.session.options.source_eager);
        assert!(warm.file(&idle).is_none());
        assert!(warm.file(&sibling).is_some());
        assert_eq!(warm.unit.prog.demand_plan.as_ref().unwrap().counts(), (2, 3));

        warm.did_change(sibling.clone(), String::new());
        assert!(warm.diagnostics().iter().any(|d| d.code == "E0277"), "{:?}", warm.diagnostics());
        assert!(warm.file(&sibling).is_none());
        assert!(warm.file(&idle).is_none());
        warm.did_change(sibling, implementation.into());
        assert!(warm.diagnostics().is_empty(), "{:?}", warm.diagnostics());
        assert_eq!(warm.unit.prog.impls.len(), 1);
        assert!(warm.file(&idle).is_none());
        assert_eq!(warm.unit.prog.demand_plan.as_ref().unwrap().counts(), (2, 3));

        let added = dir.join("added.rs").to_string_lossy().into_owned();
        warm.did_change(root, format!("{source} mod added; pub use added::value;"));
        assert!(!warm.query_ready());
        std::fs::write(&added, "pub fn value() {}").unwrap();
        warm.apply(&[added.clone()]);
        assert!(warm.query_ready(), "{:?}", warm.diagnostics());
        assert!(warm.diagnostics().is_empty(), "{:?}", warm.diagnostics());
        assert!(warm.file(&added).is_some());
        assert!(warm.file(&idle).is_none(), "recovering a missing module must stay lazy");
        drop(warm);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sibling_blanket_impl_is_reached_by_its_trait_index() {
        isolated_test!(sibling_blanket_impl_is_reached_by_its_trait_index);
        let dir = std::env::temp_dir().join(format!("relax-warm-blanket-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname=\"warm_blanket\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[lib]\npath=\"main.rs\"\n").unwrap();
        let root = dir.join("main.rs").to_string_lossy().into_owned();
        let sibling = dir.join("sibling.rs").to_string_lossy().into_owned();
        let idle = dir.join("idle.rs").to_string_lossy().into_owned();
        std::fs::write(&root, "mod sibling; mod idle; pub trait Mark {} fn require<T: Mark>(_: T) {} pub fn caller() { require(1u8); }").unwrap();
        std::fs::write(&sibling, "impl<T> crate::Mark for T {}").unwrap();
        std::fs::write(&idle, "pub trait Mark {} impl<T> Mark for T {}").unwrap();
        let sess = std::sync::Arc::new(crate::Session::default());
        let mut prog = crate::program::Program::new(&sess);
        let cfg = crate::cfg::CfgSet::host(&sess, &[]);
        prog.preload(vec![crate::preload::PreCrate { name: "warm_blanket".into(), root: root.clone(), edition: 2021, cfg: cfg.clone(), deps: Vec::new(), proc_macro: false, image_deps: Vec::new(), image_dep_names: Vec::new() }]);
        let krate = prog.load_crate("warm_blanket", &root, 2021, cfg, Vec::new()).unwrap();
        prog.resolve_imports();
        let mut tcx = crate::tcx::Tcx::new(); tcx.collect(&prog);
        let mut warm = Session::new(Unit::new(prog, tcx, 0), krate, Vec::new());
        warm.check();
        assert!(warm.diagnostics().is_empty(), "{:?}", warm.diagnostics());
        assert!(warm.file(&sibling).is_some());
        assert!(warm.file(&idle).is_none());
        assert_eq!(warm.unit.prog.impls.len(), 1);
        assert_eq!(warm.unit.prog.demand_plan.as_ref().unwrap().counts(), (2, 3));
        drop(warm);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod round45_tests {
    use super::*;

    #[test]
    fn checking_body_edits_publish_owner_legacy_results() {
        isolated_test!(checking_body_edits_publish_owner_legacy_results);
        let dir = std::env::temp_dir().join(format!("relax-check-r45-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.join("lib.rs").to_string_lossy().into_owned();
        let dep = dir.join("dep.rs").to_string_lossy().into_owned();
        let source = "#![no_std] pub fn a() -> i32 { dep::f(1, 2) } pub fn b() -> i32 { dep::f(3, 4) }";
        std::fs::write(&root, source).unwrap();
        std::fs::write(&dep, "#![no_std] #[rustc_legacy_const_generics(1)] pub fn f<const N: i32>(x: i32) -> i32 { x }").unwrap();
        let session = std::sync::Arc::new(crate::Session::default());
        let mut prog = crate::program::Program::new(&session);
        let cfg = crate::cfg::CfgSet::host(&session, &[]);
        let dependency = prog.load_crate("dep", &dep, 2021, cfg.clone(), vec![]).unwrap();
        let sym = prog.syms.intern("dep");
        let krate = prog.load_crate("edited", &root, 2021, cfg, vec![(sym, dependency)]).unwrap();
        prog.resolve_imports();
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        let mut warm = Session::new(Unit::new(prog, tcx, 0), krate, Vec::new());
        warm.check();
        assert!(warm.diagnostics().is_empty(), "{:?}", warm.diagnostics());
        let b = warm.unit.prog.defs.iter().enumerate().find(|(_, d)| warm.unit.prog.syms.str(d.name) == "b").map(|(i, _)| DefId(i as u32)).unwrap();
        let child = warm.unit.prog.legacy_const_args[&b].children[0].1;
        warm.did_change(root.clone(), source.replace("dep::f(1, 2)", "let x = 5; dep::f(x, 6)"));
        assert!(warm.diagnostics().is_empty(), "{:?}", warm.diagnostics());
        let result = &warm.unit.prog.legacy_const_args[&b];
        assert_eq!(result.file, warm.unit.prog.def(b).file);
        assert_eq!(result.children[0].1, child);
        assert_eq!(result.calls.len(), 1);
        warm.did_change(root.clone(), source.into());
        assert!(warm.diagnostics().is_empty(), "{:?}", warm.diagnostics());
        drop(warm);
        std::fs::remove_file(root).unwrap();
        std::fs::remove_file(dep).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
