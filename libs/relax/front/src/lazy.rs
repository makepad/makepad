//! A conservative demand closure, completed before dense program IDs are assigned.
//! This is a child of preload so discovery uses exactly its module/cfg scanner.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
#[path = "demand.rs"]
pub(super) mod demand;
#[path = "rs_items.rs"]
pub(crate) mod items;
use demand::Names;
#[path = "demand_part.rs"]
mod part;
pub use part::DemandPart;

#[derive(Clone)]
pub struct FileIndex {
    pub ci: usize,
    pub path: String,
    pub module: String,
    pub names: BTreeSet<String>,
    pub glob_names: BTreeSet<String>,
    pub impls: Vec<(Option<String>, String)>,
    pub blankets: BTreeSet<String>,
    pub force: bool,
}

/// A certified demand closure. Per-file index projections are retained beside
/// their paths; a warm load decodes these rather than indexing again.
#[derive(Clone)]
pub struct DemandImage {
    pub graph: u128,
    pub fingerprint: u128,
    pub inputs: Vec<(String, u128)>,
    pub probes: Vec<(String, bool)>,
    pub late_inputs: Vec<(String, u8, String, u128)>,
    pub files: Vec<FileIndex>,
    pub reached: BTreeSet<(usize, String)>,
    pub opaque_crates: usize,
    /// A load-order Rust prefix retained in full to overlap collection and lowering.
    pub eager_prefix: usize,
    pub parts: Vec<Arc<DemandPart>>,
    pub(crate) recording: Option<Arc<part::Recording>>,
}

#[derive(Default)]
struct EarlyCrates {
    prefix: std::sync::atomic::AtomicUsize,
    notify: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>>,
}
impl EarlyCrates {
    fn publish(&self, prefix: usize) {
        self.prefix.fetch_max(prefix, std::sync::atomic::Ordering::Release);
        if let Some(notify) = self.notify.get() { notify(); }
    }
}

/// Cold planning runs on a preload worker. A conservative Rust-only prefix can
/// collect once its expansion surfaces are published.
/// The final closure retains that prefix in full. A restored certificate is
/// ready synchronously so ordinary crate-image restoration remains unchanged.
pub struct PendingPlan {
    pending_crates: Vec<PreCrate>,
    pending_contexts: Vec<Arc<crate::source_context::Context>>,
    plan: std::sync::OnceLock<Plan>,
    early: EarlyCrates,
    parts: Option<crate::crate_image::DemandParts>,
}
impl PendingPlan {
    pub fn new(crates: &[PreCrate], contexts: Vec<Arc<crate::source_context::Context>>, images: Option<&dyn crate::crate_image::CrateImages>, parallel: bool) -> Self {
        let image = if crates.iter().any(|c| c.cfg.session.runtime.lazy_verify) { None }
            else { images.and_then(|images| images.demand_image(crates, recipe_identity(crates))) };
        let parts = if image.is_none() && crates.first().is_some_and(|c| c.cfg.session.options.rs_lazy)
            && !crates.iter().any(|c| c.cfg.session.runtime.lazy_verify) {
            images.and_then(|i| i.demand_parts(crates, recipe_identity(crates)))
        } else { None };
        let pending = Self { pending_crates: crates.to_vec(), pending_contexts: contexts, plan: std::sync::OnceLock::new(), early: EarlyCrates::default(), parts };
        if let Some(parts) = &pending.parts { pending.early.publish(parts.eager_prefix); }
        if image.is_some() || !parallel {
            let plan = Plan::build(&pending.pending_crates, pending.pending_contexts.clone(), image, &pending.early, pending.parts.as_ref());
            let _ = pending.plan.set(plan);
        }
        pending
    }
    pub fn complete(&self) {
        let _reads = self.pending_contexts.first().map(|c| c.bind_source_reads());
        let _ = std::ops::Deref::deref(self);
    }
    pub fn ready(&self) -> Option<&Plan> { self.plan.get() }
    pub fn eagerly_selected(&self, ci: usize) -> bool {
        ci < self.early.prefix.load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn on_early_publish(&self, notify: impl Fn() + Send + Sync + 'static) {
        let _ = self.early.notify.set(Box::new(notify));
    }
    pub fn skips(&self, root: &str, path: &str) -> bool {
        // MAKEPAD_RUN_LAZY_DEFER=<path suffix>,..: these files are skipped whatever the
        // closure says; the post-boot loader brings them in when code needs them (its test)
        if self.pending_crates.first().is_some_and(|c| c.cfg.session.options.rs_lazy) && forced_deferral(path) {
            return true;
        }
        if self.pending_crates.iter().position(|c| c.root == root).is_some_and(|ci| self.eagerly_selected(ci)) {
            return false;
        }
        self.pending_crates.first().is_some_and(|c| c.cfg.session.options.rs_lazy)
            && std::ops::Deref::deref(self).skips(root, path)
    }
}
impl std::ops::Deref for PendingPlan {
    type Target = Plan;
    fn deref(&self) -> &Plan {
        self.plan.get_or_init(|| Plan::build(&self.pending_crates, self.pending_contexts.clone(), None, &self.early, self.parts.as_ref()))
    }
}

pub struct Plan {
    pub crates: Vec<PreCrate>,
    pub contexts: Vec<Arc<crate::source_context::Context>>,
    pub probes: Vec<(String, bool)>,
    pub image: Arc<DemandImage>,
    indexed: BTreeSet<(usize, String)>,
    reached: BTreeSet<(usize, String)>,
    selection: Vec<u128>,
}

impl Plan {
    fn build(crates: &[PreCrate], contexts: Vec<Arc<crate::source_context::Context>>, image: Option<Arc<DemandImage>>, early: &EarlyCrates, stored: Option<&crate::crate_image::DemandParts>) -> Self {
        let _mem = crate::memstat::tag(crate::memstat::Tag::Planner);
        let rust_lazy = crates.first().is_some_and(|c| c.cfg.session.options.rs_lazy);
        let profile = crate::why::enabled() || crates.first().is_some_and(|c| c.cfg.session.runtime.profile);
        let _profile = crate::load_profile::span(profile, "rust-demand", "load");
        let graph = recipe_identity(crates);
        if !crates.iter().any(|c| c.cfg.session.runtime.lazy_verify) {
            if let Some(image) = image {
                let indexed = image.files.iter().map(|f| (f.ci, f.path.clone())).collect::<BTreeSet<_>>();
                early.publish(image.eager_prefix);
                let mut plan = Self { crates: crates.to_vec(), contexts,
                    probes: image.probes.clone(),
                    reached: image.reached.clone(), indexed, image, selection: Vec::new() };
                plan.set_selection_identities();
                if profile {
                    eprintln!("rust-lazy: reached={} total={} opaque_scopes={}", plan.reached.len(), plan.indexed.len(), plan.image.opaque_crates);
                    eprintln!("rust-lazy: restored demand; indexes=0 schemas=0");
                    plan.report_counts();
                }
                return plan;
            }
        }
        if profile {
            if let Some(c) = crates.last() { eprintln!("context-graph-root: {:?}", c.root); }
            for c in crates {
                let deps: Vec<_> = c.image_deps.iter().filter_map(|&i| crates.get(i)).map(|c| c.root.clone()).collect();
                eprintln!("context-crate: {:?} {:?}", c.root, deps);
            }
        }
        let previous = stored.map(|parts| (parts.load)()).unwrap_or_default();
        let predecoded = demand::Predecoded::start(previous.clone(), jobs());
        let mut parts = vec![DemandPart::default(); crates.len()];
        let mut identities = Vec::new();
        let dependency_names = Names::new(crates);
        let dependency_crates = (0..crates.len()).map(|ci| dependency_names.reference_crates(ci).into_iter().filter(|&d| d < ci).collect::<Vec<_>>()).collect::<Vec<_>>();
        let mut saved = previous.clone();
        // Keep stale parts only for boundary comparison, never as current input.
        for ci in 0..saved.len() {
            if !stored.is_some_and(|s| s.valid[ci]) { saved[ci] = None; }
        }
        let saved = &saved;
        let mut reused_outline = BTreeSet::new();
        let verify_restore = std::env::var_os("MAKEPAD_RUN_DEMAND_RESTORE_CHECK").is_some_and(|v| v == "1");
        let mut restored_surfaces = BTreeSet::new();
        let mut demands = BTreeSet::new();
        let mut origins = BTreeMap::<String, (String, String)>::new();
        let mut force_origin = BTreeMap::<String, (String, String)>::new();
        let mut names = Names::new(crates);
        let mut rust = Vec::new();
        let mut sites = BTreeMap::new();
        let mut inputs = BTreeMap::new();
        let mut probes = BTreeSet::new();
        let mut forced = BTreeSet::new();
        let discovery_profile = crate::load_profile::span(profile, "rust-demand-discovery", "load");
        // Crate scans share only immutable source snapshots. Publish the
        // results in crate/DFS order, preserving lexical macro precedence.
        let next_crate = std::sync::atomic::AtomicUsize::new(0);
        let workers_profile = crate::load_profile::span(profile, "rust-demand-discovery-workers", "load");
        let mut discovered = crate::pool::scope(|scope| {
            let handles = (0..jobs().min(crates.len())).map(|_| {
                let contexts = &contexts;
                let template = &names;
                let next_crate = &next_crate;
                scope.spawn(move || {
                    let _mem = crate::memstat::tag(crate::memstat::Tag::Planner);
                    let mut output = Vec::new();
                    loop {
                        let ci = next_crate.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(c) = crates.get(ci) else { break };
                        let _profile = crate::load_profile::span(profile, "rust-demand-crate", &c.name);
                        if let Some(part) = saved.get(ci).and_then(Option::as_ref) {
                            output.push((ci, part.scans.clone(), part.aliases.clone(), template.fork()));
                            continue;
                        }
                        let (scans, aliases, local_names) = discover_crate(c, ci, &contexts[ci], template, rust_lazy, profile);
                        output.push((ci, scans, aliases, local_names));
                    }
                    output
                })
            }).collect::<Vec<_>>();
            handles.into_iter().flat_map(|h| h.join().expect("context discovery worker")).collect::<Vec<_>>()
        });
        drop(workers_profile);
        let merge_profile = crate::load_profile::span(profile, "rust-demand-discovery-merge", "load");
        discovered.sort_by_key(|entry| entry.0);
        let relevant = (0..crates.len()).collect::<BTreeSet<_>>();
        let mut expanded_indexes = BTreeMap::new();
        let mut expansion_failures = BTreeMap::new();
        let mut eager_bodies = BTreeMap::new();
        let mut prepared_items = BTreeMap::new();
        let mut eager_prefix = 0;
        let mut prepared_files = Vec::new();
        let mut unindexed_files = Vec::new();
        if rust_lazy {
            let _profile = crate::load_profile::span(profile, "rs-outline-expand", "load");
            // Shared like Program::macros: snapshots are Arc clones; a write copies
            // the table only while a snapshot still shares it.
            let mut table = Arc::new(crate::macros::MacroTable::default());
            // Crates expand on shared workers: a crate whose dependencies are all published
            // starts expanding (against a copy of the table as its publication will see it)
            // while earlier crates still publish. Publication stays in crate order and
            // replays every surface against the real table.
            let queue = OutlineQueue::default();
            let newest_dep = (0..crates.len()).map(|ci| dependency_crates[ci].iter().copied().max()).collect::<Vec<_>>();
            let mut started: Vec<Option<(Arc<OutlineJob>, Vec<Vec<crate::macros::MacroRef>>, Arc<crate::macros::CrateMods>)>> = (0..crates.len()).map(|_| None).collect();
            crate::pool::scope(|workers| {
            // (the workers end with the queue, also when publication panics)
            let finish = OutlineFinish(&queue);
            for _ in 0..jobs() {
                let (queue, contexts) = (&queue, &contexts);
                workers.spawn(move || {
                    let _mem = crate::memstat::tag(crate::memstat::Tag::Planner);
                    while let Some((job, i)) = queue.next() {
                        let c = &crates[job.ci];
                        let _reads = contexts[job.ci].bind_source_reads();
                        let (path, module, scan) = job.inputs[i].lock().unwrap().take().expect("each input is expanded once");
                        let _profile = crate::load_profile::span(profile, "rust-demand-outline-worker", &path);
                        let ctx = std::rc::Rc::new(crate::builtins::FileCtx::new(&c.name, &path, &module, &c.cfg));
                        let surface = {
                            let src = &scan.src[..];
                            let mut toks = crate::parser::lazy_bodies::Toks::from(&scan.toks[..]);
                            if !toks.last().is_some_and(|t| t.kind == T::Eof) {
                                toks.push(Tok { kind: T::Eof, lo: src.len() as u32, hi: src.len() as u32 });
                            }
                            Some(crate::parser::rs_expand::expand(src, toks, c.edition, ctx, job.base.clone()))
                        };
                        let surface = surface.map(|surface| OutlineIndexes::new(surface, &scan.src, profile, &path));
                        drop(scan);
                        job.ready.0.lock().unwrap().insert((path.clone(), module.clone()), surface);
                        job.ready.1.notify_all();
                    }
                });
            }
            for k in 0..discovered.len() {
                for j in k + 1..(k + 1 + OUTLINE_AHEAD).min(discovered.len()) {
                    let ready = newest_dep[j].is_none_or(|d| d < k);
                    if started[j].is_some() || !ready || saved.get(j).and_then(Option::as_ref).is_some() { continue; }
                    let c = &crates[j];
                    let scans = &mut discovered[j].1;
                    let modules = Arc::new(crate_modules(c, scans));
                    let mut base = table.clone();
                    let mut defs = Vec::with_capacity(scans.len());
                    {
                        let t = Arc::make_mut(&mut base);
                        for (_, _, _, scan, _, _) in scans.iter_mut() {
                            let Some(scan) = scan else { defs.push(Vec::new()); continue; };
                            let scan = Arc::make_mut(scan);
                            let own: Vec<crate::macros::MacroRef> = std::mem::take(&mut scan.defs).into_iter().map(Arc::new).collect();
                            publish_scan_names(t, &c.name, scan, &own);
                            defs.push(own);
                        }
                        t.add_crate(c.name.clone(), modules.clone());
                        if c.name == "core" { t.s2_fmt = crate::vfs::exists(format!("{}/fmt/mod.rs", crate::program::dir_of(&c.root))); }
                    }
                    let job = OutlineJob::new(j, base, scans);
                    queue.push(job.clone());
                    started[j] = Some((job, defs, modules));
                }
                let (ci, scans, aliases, _) = &mut discovered[k];
                let c = &crates[*ci];
                let _crate_profile = crate::load_profile::span(profile, "rust-demand-outline-crate", &c.name);
                let dependencies = dependency_crates[*ci].iter().map(|&d| identities[d]).collect::<Vec<_>>();
                let reusable = saved.get(*ci).and_then(Option::as_ref)
                    .filter(|part| part.dependencies == dependencies);
                if profile && reusable.is_none() && saved.get(*ci).and_then(Option::as_ref).is_some() {
                    eprintln!("rust-demand-part-invalid: {} dependency surface", c.name);
                }
                if let Some(part) = reusable.filter(|_| predecoded.take(*ci).and_then(|image| names.restore_decoded(*ci, image, false)).is_some()) {
                    *scans = part.scans.clone();
                    part.macros.publish(Arc::make_mut(&mut table), &c.name);
                    Arc::make_mut(&mut table).seal_crate(&c.name);
                    prepared_files.extend(part.files.iter().cloned());
                    unindexed_files.extend(part.unindexed.iter().copied());
                    for ((path, module), bodies) in &part.eager { eager_bodies.insert((*ci, path.clone(), module.clone()), bodies.clone()); }
                    for ((path, module), failed) in &part.failures { expansion_failures.insert((*ci, path.clone(), module.clone()), *failed); }
                    parts[*ci] = (**part).clone();
                    identities.push(part.surface_id);
                    reused_outline.insert(*ci);
                    restored_surfaces.extend(part.outlined.iter().map(|(p, m)| (*ci, p.clone(), m.clone())));
                    if *ci == eager_prefix && *ci + 1 < crates.len() {
                        eager_prefix += 1;
                        early.publish(eager_prefix);
                    }
                    if profile { eprintln!("rust-demand-part: {} restored outline", c.name); }
                    continue;
                }
                if saved.get(*ci).and_then(Option::as_ref).is_some() {
                    // Its own source is unchanged but an imported surface is
                    // different. Expanded scans cannot serve as authored input.
                    let (fresh, fresh_aliases, _) = discover_crate(c, *ci, &contexts[*ci], &names, rust_lazy, profile);
                    *scans = fresh; *aliases = fresh_aliases;
                }
                if let Some(previous) = (0..*ci).rev().find(|&i| crates[i].name == c.name) {
                    let (before, after) = parts.split_at_mut(*ci);
                    after[0].macros.inherit_definitions(&before[previous].macros);
                }
                let files_start = prepared_files.len();
                let mut namespace_restored: Option<Arc<DemandPart>> = None;
                let job = match started[*ci].take() {
                    // expanding since its dependencies were published: the names it published
                    // to its copy of the table go to the real one now, in the same order
                    Some((job, defs, modules)) => {
                        for ((_, _, _, scan, _, _), own) in scans.iter().zip(&defs) {
                            let Some(scan) = scan else { continue; };
                            for def in own { parts[*ci].macros.note_definition(&c.name, def); }
                            publish_scan_names(Arc::make_mut(&mut table), &c.name, scan, own);
                        }
                        Arc::make_mut(&mut table).add_crate(c.name.clone(), modules);
                        if c.name == "core" { Arc::make_mut(&mut table).s2_fmt = crate::vfs::exists(format!("{}/fmt/mod.rs", crate::program::dir_of(&c.root))); }
                        job
                    }
                    None => {
                        let modules = Arc::new(crate_modules(c, scans));
                        for (_, _, _, scan, _, _) in scans.iter_mut() {
                            let Some(scan) = scan else { continue; };
                            let scan = Arc::make_mut(scan);
                            let own: Vec<crate::macros::MacroRef> = std::mem::take(&mut scan.defs).into_iter().map(Arc::new).collect();
                            for def in &own { parts[*ci].macros.note_definition(&c.name, def); }
                            publish_scan_names(Arc::make_mut(&mut table), &c.name, scan, &own);
                        }
                        Arc::make_mut(&mut table).add_crate(c.name.clone(), modules);
                        if c.name == "core" { Arc::make_mut(&mut table).s2_fmt = crate::vfs::exists(format!("{}/fmt/mod.rs", crate::program::dir_of(&c.root))); }
                        // Expand against the prescanned environment on the shared workers.
                        // Publish in source order, replaying macro lookups before using
                        // a speculative surface, just as the real preload parser does.
                        let job = OutlineJob::new(*ci, table.clone(), scans);
                        queue.push(job.clone());
                        job
                    }
                };
                let expected = scans.iter().filter(|entry| entry.3.is_some()).map(|(path, module, ..)| (path.clone(), module.clone())).collect::<BTreeSet<_>>();
                {
                    // Publish in source order as soon as each file's speculative
                    // surface exists, while later files of the crate still expand.
                    let take = |key: (String, String)| {
                        if !expected.contains(&key) { return None; }
                        let _profile = crate::load_profile::span(profile, "rust-demand-outline-wait", &key.0);
                        let mut map = job.ready.0.lock().unwrap();
                        loop {
                            if let Some(surface) = map.remove(&key) { return surface; }
                            map = job.ready.1.wait(map).unwrap();
                        }
                    };
                    let mut directories = BTreeMap::new();
                    directories.insert(c.root.clone(), crate::program::dir_of(&c.root).to_owned());
                    for (_, _, _, scan, _, _) in scans.iter() {
                        if let Some(scan) = scan { for (path, dir, _) in &scan.children { directories.insert(path.clone(), dir.clone()); } }
                    }
                    let mut seen = scans.iter().map(|(path, module, ..)| (path.clone(), module.clone())).collect::<BTreeSet<_>>();
                    // the module files each scanned file was reached through (rustc_expand: a file
                    // that includes itself through `mod` items is the loader's circular-modules error)
                    let mut chains: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
                    let mut pending = std::collections::VecDeque::from(std::mem::take(scans));
                    while let Some(mut entry) = pending.pop_front() {
                        let (path, module, _, scan, parameters, _) = &mut entry;
                        let Some(scan) = scan else { scans.push(entry); continue; };
                        let _file_profile = crate::load_profile::span(profile, "rust-demand-outline-publish", path);
                        // the worker's surface (the worker has let go of the scan)
                        let speculative = take((path.clone(), module.clone()));
                        let scan = Arc::make_mut(scan);
                        {
                            for def in std::mem::take(&mut scan.defs) {
                                let def = Arc::new(def);
                                parts[*ci].macros.note_definition(&c.name, &def);
                                Arc::make_mut(&mut table).push(def);
                            }
                            // As Program::load_file: a speculative surface whose lookups
                            // replay against the current table is kept; only a miss
                            // snapshots the table to expand again.
                            let surface = speculative
                                .filter(|surface| surface.replays(&table, &c.name, path, profile))
                                .unwrap_or_else(|| {
                                    let _profile = crate::load_profile::span(profile, "rust-demand-outline-replay", path);
                                    let ctx = std::rc::Rc::new(crate::builtins::FileCtx::new(&c.name, path, module, &c.cfg));
                                    let surface = crate::parser::rs_expand::expand(&scan.src, &scan.toks[..], c.edition, ctx, table.clone());
                                    OutlineIndexes::new(surface, &scan.src, profile, path)
                                });
                            let OutlineIndexes { surface, index, parameters: found_parameters } = surface;
                            // an expansion that added no text keeps the file's own bytes, shared
                            let expanded_src: ScanBytes = if surface.ext.is_empty() { scan.src.clone() } else { surface.text(&scan.src).into_owned().into() };
                            *parameters = found_parameters;
                            if let Some(index) = index { expanded_indexes.insert((*ci, path.clone(), module.clone()), index); }
                            contexts[*ci].set_outline_reuse(path, surface.reuse.clone());
                            eager_bodies.insert((*ci, path.clone(), module.clone()), surface.eager_bodies);
                            prepared_items.insert((*ci, path.clone(), module.clone()), surface.items);
                            if profile {
                                for (name, reason) in &surface.unknown { eprintln!("rs-outline-fallback: {path:?} {name:?} {reason:?}"); }
                            }
                            expansion_failures.insert((*ci, path.clone(), module.clone()), !surface.unknown.is_empty());
                            for def in &surface.used.local {
                                if !table.defs.get(&def.name).is_some_and(|defs| defs.iter().any(|d| d.def_file == def.def_file && d.def_pos == def.def_pos)) { parts[*ci].macros.note_definition(&c.name, def); Arc::make_mut(&mut table).push(def.clone()); }
                            }
                            if !surface.used.uses.is_empty() { Arc::make_mut(&mut table).add_exp_uses(&c.name, surface.used.uses); }
                            let dir = directories.get(path).cloned().unwrap_or_else(|| crate::program::dir_of(path).into());
                            let _profile = crate::load_profile::span(profile, "rust-demand-outline-scan", path);
                            if let Some(expanded) = scan_tokens(path, &dir, module, &c.name, c.edition, &c.cfg, scan.src_len,
                                expanded_src, surface.toks, &contexts[*ci], false) { *scan = expanded; }
                        }
                        let mut chain = chains.get(&(path.clone(), module.clone())).cloned().unwrap_or_default();
                        chain.push(path.clone());
                        for (child, dir, child_module) in &scan.children {
                            directories.insert(child.clone(), dir.clone());
                            if chain.contains(child) { continue; }
                            if !seen.insert((child.clone(), child_module.clone())) { continue; }
                            chains.insert((child.clone(), child_module.clone()), chain.clone());
                            let source = contexts[*ci].read_source(child).ok();
                            let hash = source.as_ref().map(|s| crate::crate_image::file_hash(child, &s.src));
                            let child_scan = source.and_then(|s| scan_text_mode(child, dir, child_module, &c.name, c.edition, &c.cfg, (&s.src).into(), &contexts[*ci], true)).map(Arc::new);
                            pending.push_back((child.clone(), child_module.clone(), hash, child_scan, BTreeSet::new(), Vec::new()));
                        }
                        {
                            let _profile = crate::load_profile::span(profile, "rust-demand-outline-names", path);
                            names.add(*ci, module, &scan.src, &scan.toks, &scan.uses, &scan.mods);
                        }
                        scans.push(entry);
                    }
                }
                Arc::make_mut(&mut table).seal_crate(&c.name);
                {
                    // The crate's declarations are final: bind them and evaluate its
                    // imports now, while later crates still lower and expand.
                    let _profile = crate::load_profile::span(profile, "rust-demand-namespace-crate", &c.name);
                    let mut raw = aliases.iter().map(|(path, module)| (*ci, path.clone(), module.clone(), false, BTreeSet::new()))
                        .chain(scans.iter().map(|(path, module, _, scan, parameters, _)| match scan {
                            Some(_) => (*ci, path.clone(), module.clone(), false, parameters.clone()),
                            None => (*ci, path.clone(), module.clone(), true, BTreeSet::new()),
                        })).collect::<Vec<_>>();
                    raw.sort();
                    for entry in &raw {
                        let mut file = file_index(crates, &contexts, &expanded_indexes, profile, entry);
                        names.qualify_index(&mut file);
                        prepared_files.push(file);
                    }
                    // An edit that left this crate's namespace inputs as recorded (a
                    // comment, a function body) changes no binding: restore the recorded
                    // namespace as an unchanged crate's part does. The recorded bindings
                    // are the prepare_crate result for equal tables and equal
                    // dependency surfaces (see Names::snapshot).
                    let stale = previous.get(*ci).and_then(Option::as_ref).filter(|_| !saved.is_empty());
                    let input_id = stale.map(|_| names.input_id(*ci));
                    let recorded = stale.filter(|old| Some(old.input_id) == input_id && old.input_id != 0
                        && !old.namespace.is_empty() && old.dependencies == dependencies);
                    if let Some(old) = recorded {
                        if verify_restore {
                            names.prepare_crate(*ci);
                            let (now, _) = names.snapshot(*ci);
                            eprintln!("rust-demand-restore-check: crate={} {}", c.name, if now == old.namespace { "identical" } else { "MISMATCH" });
                        } else if predecoded.take(*ci).and_then(|image| names.restore_decoded(*ci, image, true)).is_some() {
                            namespace_restored = Some(old.clone());
                            if profile { eprintln!("rust-demand-restore: crate={} namespace inputs unchanged", c.name); }
                        } else { names.prepare_crate(*ci); }
                    } else {
                        names.prepare_crate(*ci);
                        if stale.is_some() && profile { eprintln!("rust-demand-restore: crate={} namespace inputs changed", c.name); }
                    }
                    // Its namespace is final too: qualify impl heads and check
                    // for unindexed items before the closure needs them.
                    let start = prepared_files.len() - raw.len();
                    let sources = scans.iter().filter_map(|(path, module, _, scan, parameters, _)| scan.as_ref()
                        .map(|scan| ((path.as_str(), module.as_str()), (&scan.src[..], &scan.toks[..], parameters)))).collect::<BTreeMap<_, _>>();
                    let files = &mut prepared_files[start..];
                    let chunk = files.len().div_ceil(jobs().max(1)).max(1);
                    let (names, contexts, expanded_indexes, sources) = (&names, &contexts, &expanded_indexes, &sources);
                    let mut flags = crate::pool::scope(|scope| {
                        let workers = files.chunks_mut(chunk).map(|files| scope.spawn(move || { let _mem = crate::memstat::tag(crate::memstat::Tag::Planner); files.iter_mut().map(|file| {
                            let source = sources.get(&(file.path.as_str(), file.module.as_str())).copied();
                            macro_surface(names, crates, contexts, expanded_indexes, profile, file, source.map(|(src, toks, _)| (src, toks)), source.map(|(_, _, parameters)| parameters))
                        }).collect::<Vec<_>>() })).collect::<Vec<_>>();
                        workers.into_iter().flat_map(|w| w.join().expect("context macro surface worker")).collect::<Vec<_>>()
                    });
                    unindexed_files.append(&mut flags);
                    let range = (*ci, String::new(), String::new())..(*ci + 1, String::new(), String::new());
                    for (_, items) in prepared_items.range_mut(range) { items::qualify(names, *ci, items); }
                }
                let part = &mut parts[*ci];
                part.aliases = aliases.clone();
                part.dependencies = dependencies;
                if !saved.is_empty() {
                    let _snapshot = crate::load_profile::span(profile, "rust-demand-namespace-save", &c.name);
                    if let Some(old) = &namespace_restored {
                        (part.namespace, part.namespace_id, part.input_id) = (old.namespace.clone(), old.namespace_id, old.input_id);
                    } else {
                        (part.namespace, part.input_id) = names.snapshot(*ci);
                        part.namespace_id = crate::crate_image::file_hash("", &part.namespace);
                    }
                }
                part.macros.finish_capture(&table, &c.name);
                part.files = prepared_files[files_start..].to_vec();
                part.unindexed = unindexed_files[files_start..].to_vec();
                let range = (*ci, String::new(), String::new())..(*ci + 1, String::new(), String::new());
                part.items = prepared_items.range_mut(range.clone()).map(|((_, p, m), v)| ((p.clone(), m.clone()), Arc::new(std::mem::take(v)))).collect();
                part.eager = eager_bodies.range(range.clone()).map(|((_, p, m), v)| ((p.clone(), m.clone()), v.clone())).collect();
                part.outlined = expanded_indexes.range(range.clone()).map(|((_, p, m), _)| (p.clone(), m.clone())).collect();
                part.failures = expansion_failures.range(range.clone()).map(|((_, p, m), v)| ((p.clone(), m.clone()), *v)).collect();
                if !saved.is_empty() { part.seal_identity(); }
                identities.push(part.surface_id);
                if profile { eprintln!("rust-demand-part: {} rebuilt outline", c.name); }
                if *ci == eager_prefix && *ci + 1 < crates.len()
 {
                    eager_prefix += 1;
                    early.publish(eager_prefix);
                }
            }
            drop(finish);
            });
        }
        for (ci, scans, _aliases, local_names) in discovered {
            use std::fmt::Write as _;
            let c = &crates[ci];
            let mut trace = String::new();
            let keep_bindings = relevant.iter().any(|&i| demand::within(&c.name, &crates[i].name));
            {
                let _profile = crate::load_profile::span(profile, "rust-demand-merge-names", &c.name);
                names.merge(local_names, keep_bindings);
            }
            let _profile = crate::load_profile::span(profile, "rust-demand-merge-files", &c.name);
            if rust_lazy { parts[ci].scans = scans.clone(); }
            for (path, module, hash, scan, _parameters, opaque) in scans {
                if let Some(hash) = hash { inputs.insert(path.clone(), hash); }
                let Some(scan) = scan else { forced.insert(module.clone()); continue; };
                probes.extend(scan.probes.iter().cloned());
                if crate::why::enabled() { sites.extend(reference_sites(&path, &scan.src, &scan.toks, &[], &contexts[ci])); }
                if profile {
                    for (child, _, _) in &scan.children { let _ = writeln!(trace, "context-parent: {:?} {:?}", child, path); }
                    if path == c.root { let _ = writeln!(trace, "context-root: {:?} {:?}", path, c.name); }
                    for (name, line) in opaque { let _ = writeln!(trace, "context-opaque: {:?} {:?} {}", path, name, line); }
                }
                rust.push((ci, path, module, scan));
            }
            // Preserve source order without making every formatted fragment a
            // separate stderr write on the profiled cold path.
            if !trace.is_empty() { eprint!("{trace}"); }
        }
        drop(merge_profile);
        drop(discovery_profile);
        let mut files = std::mem::take(&mut prepared_files);
        names.finish_prepare();
        let sources = rust.iter().map(|(ci, path, module, scan)|
            ((*ci, path.as_str(), module.as_str()), (&scan.src, &scan.toks))).collect::<BTreeMap<_, _>>();
        if rust_lazy {
            // Index compiler-owned derives and supported local macro output.
            // Unknown output still retains the crate and dependencies: its
            // references need not occur in the invocation's input. Global items
            // and certified output need only their file and indexed references.
            for file in &mut files {
                let mut outputs = Vec::new();
                let reason = if let Some(failed) = expansion_failures.get(&(file.ci, file.path.clone(), file.module.clone())) {
                    if *failed { Some(("unindexed item expansion", true)) }
                    else { Some(("outlined expansion surface", false)).filter(|_| global_items(&sources[&(file.ci, file.path.as_str(), file.module.as_str())].0, &sources[&(file.ci, file.path.as_str(), file.module.as_str())].1)) }
                } else { match sources.get(&(file.ci, file.path.as_str(), file.module.as_str())) {
                    Some((src, toks)) => rust_eager_reason(src, toks, &names, file.ci, &file.module, &mut outputs, &file.path, profile),
                    None => Some(("unavailable or multiply mounted outline", true)),
                }};
                for (module, text, tokens) in outputs {
                    for name in names.references(file.ci, &module, text.as_bytes(), &tokens) {
                        origins.entry(name.clone()).or_insert((file.path.clone(), "macro-output".into()));
                        demands.insert(name);
                    }
                    crate::why::event("rs-output-index", &file.path, 0, &module, None);
                }
                if let Some((why, dependencies)) = reason {
                    file.force = true;
                    crate::why::event("rs-eager", &file.path, 0, "", Some(crate::why::Cause::new(&file.path, 1, "outline-fallback", why)));
                    if profile { eprintln!("rs-eager: {:?} {why}; dependencies={dependencies}", file.path); }
                    if dependencies {
                        let mut todo = vec![file.ci];
                        let mut seen = BTreeSet::new();
                        while let Some(ci) = todo.pop() {
                            if !seen.insert(ci) { continue; }
                            forced.insert(crates[ci].name.clone());
                            force_origin.entry(crates[ci].name.clone()).or_insert((file.path.clone(), why.into()));
                            todo.extend(crates[ci].image_deps.iter().copied());
                            for (_, name) in &crates[ci].deps {
                                todo.extend(crates.iter().enumerate().filter(|(_, c)| c.name == *name).map(|(i, _)| i));
                            }
                        }
                    }
                }
            }
        }
        if rust_lazy {
            for name in names.implicit_demands() {
                origins.entry(name.clone()).or_insert((String::new(), "implicit".into()));
                demands.insert(name);
            }
        }
        for c in crates.iter() {
            // A crate built with `--test` (cfg(test): `makepad test`) is entered at its `#[test]`
            // fns, wherever they are: all of its own items are demanded, its dependencies' only
            // as they reach them.
            if c.cfg.set.iter().any(|x| x == "test") && forced.insert(c.name.clone()) {
                force_origin.entry(c.name.clone()).or_insert((c.root.clone(), "test harness".into()));
            }
        }
        for (ci, c) in crates.iter().enumerate() {
            // The runtime looks up main even when the root only glob-reexports
            // it. No authored identifier at that root seeds this demand.
            for name in names.entry(ci, "main") {
                origins.entry(name.clone()).or_insert((c.root.clone(), "entry-point".into()));
                demands.insert(name);
            }
        }
        let references_profile = crate::load_profile::span(profile, "rust-demand-rust-refs", "load");
        let references = crate::pool::scope(|scope| {
            let chunk = rust.len().div_ceil(jobs().max(1)).max(1);
            let handles = rust.chunks(chunk).map(|files| {
                let names = &names;
                let relevant = &relevant;
                scope.spawn(move || { let _mem = crate::memstat::tag(crate::memstat::Tag::Planner); files.iter().filter(|(ci, ..)| relevant.contains(ci)).map(|(ci, path, module, scan)| {
                    let (src, toks) = (&scan.src, &scan.toks);
                    let opaque = names.opaque_scopes(*ci, src, toks, module);
                    let references = BTreeSet::<String>::new();
                    (path, opaque, references)
                }).collect::<Vec<_>>() })
            }).collect::<Vec<_>>();
            handles.into_iter().flat_map(|h| h.join().expect("context reference worker")).collect::<Vec<_>>()
        });
        // Workers return in source order so first-edge diagnostics stay stable.
        for (path, opaque, references) in references {
            for scope in opaque {
                force_origin.entry(scope.clone()).or_insert((path.clone(), "module-expansion".into()));
                forced.insert(scope);
            }
            for name in references {
                origins.entry(name.clone()).or_insert((path.clone(), "scoped-name".into()));
                demands.insert(name);
            }
        }
        drop(references_profile);
        // A valid complete certificate was restored above. After invalidation,
        // recompute references: reusing a leaf's old references here could
        // conceal a changed external schema source.
        let mut plan = Self {
            crates: crates.to_vec(), contexts,
            probes: probes.iter().cloned().collect(),
            image: Arc::new(DemandImage { graph, fingerprint: 0, inputs: Vec::new(), probes: Vec::new(), late_inputs: Vec::new(), files: Vec::new(), reached: BTreeSet::new(), opaque_crates: 0, eager_prefix, parts: Vec::new(), recording: None }),
            indexed: files.iter().map(|f| (f.ci, f.path.clone())).collect(), reached: BTreeSet::new(),
            selection: Vec::new(),
        };
        // Configured sibling tokens already supplied the outline. Their name
        // references need neither mapped origins nor a second lowering stream.
        let surfaces = rust.iter().map(|(ci, path, module, scan)| ((*ci, path.as_str(), module.as_str()), (&scan.src, &scan.toks))).collect::<BTreeMap<_, _>>();
        let closure_profile = crate::load_profile::span(profile, if rust_lazy { "rust-demand-selection" } else { "rust-demand-closure" }, "load");
        if rust_lazy {
            // Item-level closure: a reached file declares its items; only used
            // items (and their impls/members) add references.
            let inputs_profile = crate::load_profile::span(profile, "rust-demand-closure-inputs", "load");
            let mut inputs = files.iter().zip(&unindexed_files).map(|(file, &unindexed)| {
                let surface = surfaces.get(&(file.ci, file.path.as_str(), file.module.as_str())).map(|(src, toks)| (&src[..], &toks[..]));
                let force = forced.iter().any(|scope| demand::within(&file.module, scope)) || surface.is_some() && unindexed;
                let items = parts[file.ci].items.get(&(file.path.clone(), file.module.clone())).cloned().unwrap_or_else(|| {
                    let mut items = Vec::new();
                    if let Some((src, toks)) = surface { items::split(src, toks, 0, toks.len(), &file.module, &mut items); }
                    items::qualify(&names, file.ci, &mut items);
                    Arc::new(items)
                });
                items::FileInput { ci: file.ci, path: &file.path, module: &file.module, index: file, surface, items,
                    
                    eager_bodies: eager_bodies.get(&(file.ci, file.path.clone(), file.module.clone())), force, root: file.path == crates[file.ci].root || file.ci < eager_prefix }
            }).collect::<Vec<_>>();
            let implicit = names.implicit_demands().into_iter().filter(|name| names.is_trait(name)).collect::<BTreeSet<_>>();
            drop(inputs_profile);
            let select = |names: &mut Names, inputs: &mut [items::FileInput<'_>], parts: &mut [DemandPart], incoming: Option<&part::Boundary>| {
                items::closure(names, inputs, demands.clone(), &implicit, jobs(), profile, parts, incoming)
            };
            let edited = (0..crates.len()).filter(|ci| !reused_outline.contains(ci)).collect::<BTreeSet<_>>();
            let local_candidate = previous.len() == crates.len() && previous.iter().all(Option::is_some)
                && !edited.is_empty() && edited.iter().all(|&ci| previous[ci].as_ref().is_some_and(|old| old.surface_id == parts[ci].surface_id && parts[ci].same_transfer(old)))
                && previous[0].as_ref().and_then(|p| p.roots.as_ref()).is_some_and(|(old_seeds, old_forced, prefix)|
                    old_seeds == &demands && old_forced == &forced && *prefix == eager_prefix);
            let local_result = if local_candidate {
                let _local = crate::load_profile::span(profile, "rust-demand-local-selection", "edited crates");
                if let Some(delta) = previous[0].as_ref().and_then(|p| p.closure_delta.as_deref()).and_then(demand::ClosureDelta::decoded) {
                    names.apply_closure_delta(&delta);
                    let mut incoming = part::Boundary::default();
                    for (_, old) in previous.iter().enumerate().filter(|(ci, _)| !edited.contains(ci)) {
                        incoming.extend(&old.as_ref().unwrap().boundary);
                    }
                    let selected = files.iter().enumerate().filter_map(|(f, file)| edited.contains(&file.ci).then_some(f)).collect::<Vec<_>>();
                    let mut local_inputs = selected.iter().map(|&f| inputs[f].clone()).collect::<Vec<_>>();
                    let result = select(&mut names, &mut local_inputs, &mut parts, Some(&incoming));
                    Some((selected, result))
                } else { None }
            } else { None };
            // All names are compared, even if another crate also supplied them.
            // Equal aggregate outputs alone do not prove the least fixed point:
            // old incoming demands can keep a cycle alive after its root is
            // removed. The transfer check certifies the unchanged subscriptions
            // too. Uncertified structural edits restart from current roots.
            let tried_local = local_result.is_some();
            let local_result = local_result.filter(|_| edited.iter().all(|&ci|
                parts[ci].boundary.same_outgoing(&previous[ci].as_ref().unwrap().boundary, &crates[ci].name)));
            let result = if let Some((selected, local)) = local_result {
                let mut reached = files.iter().map(|file| !edited.contains(&file.ci) && previous[file.ci].as_ref().unwrap().reached.contains(&file.path)).collect::<Vec<_>>();
                let mut reasons = vec![("stored-boundary".into(), String::new()); files.len()];
                for (i, &f) in selected.iter().enumerate() { reached[f] = local.reached[i]; reasons[f] = local.reasons[i].clone(); }
                if profile { eprintln!("rust-demand-boundary: unchanged; edited={} retained={} closure=skipped", edited.len(), crates.len() - edited.len()); }
                items::Closure { reached, reasons, items: parts.iter().map(|p| p.counts.0).sum(), used_items: parts.iter().map(|p| p.counts.1).sum(), parents: Vec::new() }
            } else {
                if profile && !previous.is_empty() { eprintln!("rust-demand-boundary: changed or unavailable; full closure"); }
                let _full = crate::load_profile::span(profile, "rust-demand-closure", "load");
                if tried_local {
                    names = Names::new(crates);
                    for (ci, part) in parts.iter().enumerate() { names.restore(ci, &part.namespace, false).expect("current outline namespace"); }
                    names.finish_prepare();
                }
                select(&mut names, &mut inputs, &mut parts, None)
            };
            parts[0].roots = Some((demands.clone(), forced.clone(), eager_prefix));
            let _publish_profile = crate::load_profile::span(profile, "rust-demand-closure-publish", "load");
            for (f, file) in files.iter().enumerate() {
                if !result.reached[f] { continue; }
                if profile { eprintln!("context-reach: {:?} {:?} {:?} {:?}", file.path, result.reasons[f].0, result.reasons[f].1, if inputs[f].force { "forced" } else { "" }); }
                if let Some(Some((from, key))) = result.parents.get(f) {
                    eprintln!("context-reach-edge: {:?} {:?} {:?}", file.path, from.map_or("<seed>", |g| files[g].path.as_str()), key);
                }
                plan.reached.insert((file.ci, file.path.clone()));
            }
            if profile { eprintln!("rs-items: items={} used={}", result.items, result.used_items); }
        }
        drop(closure_profile);
        let finish_profile = crate::load_profile::span(profile, "rust-demand-certify", "load");
        if profile || crates.first().is_some_and(|c| c.cfg.session.runtime.lazy_verify) {
            eprintln!("rust-lazy: reached={} total={} opaque_scopes={}", plan.reached.len(), plan.indexed.len(), forced.len());
        }
        // Lowering may discover additional include/schema inputs. Certify those
        // too, and validate them before the warm demand/image restoration path.
        // Read each crate's dependency set once for both certificates. The old
        // consumer pass and per-part pass repeated the shared host-path filtering.
        let inputs_profile = crate::load_profile::span(profile, "rust-demand-certify-inputs", "load");
        for &ci in &reused_outline {
            inputs.extend(parts[ci].inputs.iter().cloned());
            probes.extend(parts[ci].probes.iter().cloned());
        }
        plan.probes = probes.into_iter().collect();
        let late_inputs = crate::sip::late_inputs();
        let mut hash = crate::sip::Sip128::new();
        hash.str("source-demand-v24-impl-declarations");
        hash.u128(graph);
        for (p, h) in &inputs { hash.str(p); hash.u128(*h); }
        for (p, exists) in &plan.probes { hash.str(p); hash.u8(*exists as u8); }
        for (c, kind, name, content) in &late_inputs { hash.str(c); hash.u8(*kind); hash.str(name); hash.u128(*content); }
        let fingerprint = hash.finish128();
        drop(inputs_profile);
        // The closure is certified. Transient glob lookup results are not part
        // of the per-file index and need not be decoded on a warm load.
        for f in &mut files { f.glob_names.clear(); }
        if rust_lazy {
            let _profile = crate::load_profile::span(profile, "rust-demand-certify-parts", "load");
            for (ci, part) in parts.iter_mut().enumerate() {
                let _profile = crate::load_profile::span(profile, "rust-demand-certify-part", &crates[ci].root);
                let mut own = part.scans.iter().filter_map(|(p, _, hash, ..)| hash.map(|h| (p.clone(), h))).collect::<BTreeMap<_, _>>();
                let mut probes = part.scans.iter().filter_map(|(_, _, _, scan, ..)| scan.as_ref())
                    .flat_map(|s| s.probes.iter().cloned()).collect::<BTreeSet<_>>();
                // Retain validated schema reads even when restored outlines did
                // not ask the Context to read them again this invocation.
                if reused_outline.contains(&ci) {
                    own.extend(part.inputs.iter().cloned());
                    probes.extend(part.probes.iter().cloned());
                }
                let own = own.into_iter().collect::<Vec<_>>();
                let probes = probes.into_iter().collect::<Vec<_>>();
                let late = late_inputs.iter().filter(|(c, ..)| c == &crates[ci].name).cloned().collect::<Vec<_>>();
                if part.inputs != own || part.probes != probes || part.late_inputs != late { part.key = 0; }
                part.inputs = own;
                part.probes = probes;
                part.late_inputs = late;
            }
        }
        let image_profile = crate::load_profile::span(profile, "rust-demand-certify-image", "load");
        let headers = parts.iter().map(|p| Arc::new(DemandPart { key: p.key, ..DemandPart::default() })).collect();
        let recording = rust_lazy.then(|| Arc::new(part::Recording::new(crates, names, parts)));
        plan.image = Arc::new(DemandImage { graph, fingerprint, inputs: inputs.into_iter().collect(),
            probes: plan.probes.clone(), late_inputs, files, reached: plan.reached.clone(),
            opaque_crates: forced.len(), eager_prefix,
            parts: if rust_lazy { headers } else { Vec::new() }, recording });
        plan.set_selection_identities();
        plan.dump();
        if profile { plan.report_counts(); }
        drop(image_profile);
        drop(finish_profile);
        {
            let _profile = crate::load_profile::span(profile, "rust-demand-discovery-release", "load");
            drop((surfaces, sources));
            drop((expanded_indexes, expansion_failures, eager_bodies, prepared_items, rust));
        }
        plan
    }

    pub fn deferred(&self, ci: usize) -> Vec<String> {
        self.indexed.range((ci, String::new())..(ci + 1, String::new()))
            .filter(|k| !self.reached.contains(*k)).map(|k| k.1.clone()).collect()
    }

    pub fn skips(&self, root: &str, path: &str) -> bool {
        self.crates.iter().position(|c| c.root == root).is_some_and(|ci| {
            let key = (ci, path.to_owned());
            self.indexed.contains(&key) && !self.reached.contains(&key)
        })
    }

    pub fn indexed_paths(&self) -> Vec<String> { self.indexed.iter().map(|(_, p)| p.clone()).collect() }
    pub fn counts(&self) -> (usize, usize) { (self.reached.len(), self.indexed.len()) }
    fn report_counts(&self) {
        if self.image.eager_prefix != 0 { eprintln!("source-lazy: early_crates={}", self.image.eager_prefix); }
        for extension in ["rs"] {
            let suffix = format!(".{extension}");
            let indexed = self.indexed.iter().filter(|(_, path)| path.ends_with(&suffix)).count();
            let reached = self.reached.iter().filter(|(_, path)| path.ends_with(&suffix)).count();
            eprintln!("source-lazy: language={extension} indexed={indexed} reached={reached} deferred={}", indexed - reached);
        }
    }
    /// MAKEPAD_RUN_DEMAND_DUMP=<file>: the plan's observable result (per-crate selection
    /// identity and deferred list, reached/indexed counts), to diff an incremental
    /// plan against one built from scratch (MAKEPAD_RUN_LAZY_VERIFY=1) for the same sources.
    fn dump(&self) {
        let Some(path) = std::env::var_os("MAKEPAD_RUN_DEMAND_DUMP") else { return; };
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(out, "reached={} indexed={}", self.reached.len(), self.indexed.len());
        for (ci, c) in self.crates.iter().enumerate() {
            let _ = writeln!(out, "{ci} {} {:032x} deferred={:?}", c.name, self.selection[ci], self.deferred(ci));
        }
        let _ = std::fs::write(path, out);
    }
    pub fn selection_identity(&self, ci: usize) -> u128 {
        self.selection[ci]
    }

    fn set_selection_identities(&mut self) {
        // Collection loads whole files. Item demand, forced/opaque scopes and
        // expansion fallbacks affect that final choice, not the contents of a
        // reached file. Hash exactly the indexed/reached projection consumed by
        // skips(), in path order, without any other crate's sources or index.
        // Early crates are retained in full and can fingerprint before the
        // closure finishes: zero is their eager identity (also for unindexed
        // crates). Becoming selective gives the crate a nonzero identity.
        let mut hashes = vec![None; self.crates.len()];
        for key in &self.indexed {
            if key.0 < self.image.eager_prefix { continue; }
            let hash = hashes[key.0].get_or_insert_with(|| {
                let mut hash = crate::sip::Sip128::new();
                hash.str("source-demand-crate-v1");
                hash
            });
            hash.str(&key.1);
            hash.u8(self.reached.contains(key) as u8);
        }
        self.selection = hashes.into_iter().map(|h| h.map_or(0, |h| h.finish128())).collect();
        // A crate whose indexed files are all reached loads exactly what an eager crate loads: its
        // identity is the eager one. Otherwise the same crate would carry an identity that depends
        // on whether the early prefix had reached it when its image was recorded or looked up (a
        // timing accident), and no two programs could share its image or its fingerprint.
        for ci in 0..self.selection.len() {
            if self.selection[ci] != 0 && self.indexed.range((ci, String::new())..(ci + 1, String::new())).all(|k| self.reached.contains(k)) {
                self.selection[ci] = 0;
            }
        }
        for (context, &identity) in self.contexts.iter().zip(&self.selection) {
            if identity != 0 { context.set_demand_identity(identity); }
        }
    }

    pub fn force_remaining(&self) -> Result<(), String> {
        for (ci, path) in &self.indexed {
            {
                let source = self.contexts[*ci].read_source(path)?;
                let cfg = &self.crates[*ci].cfg;
                let outline = self.contexts[*ci].rust_outline(path, &source.src, self.crates[*ci].edition, cfg)
                    .ok_or_else(|| format!("{path}: Rust outline unavailable during verification"))?;
                let mut tokens = Vec::new();
                crate::lexer::lex(&source.src, self.crates[*ci].edition, &mut tokens).map_err(|e| format!("{path}: {}", e.msg))?;
                let active = crate::preload::configured_tokens(&source.src, &tokens, cfg);
                // Independent full token-tree indexing checks that skipping body
                // groups did not erase a module, import, impl head or declaration.
                let stream = crate::pm_tokens::from_toks(&source.src, &active, &|lo, hi| crate::pm_tokens::Span {
                    lo, hi, ..crate::pm_tokens::Span::DUMMY
                }).map_err(|e| e.to_string())?;
                let eager = crate::rs_index::of_tokens(&stream);
                if outline.index != eager { return Err(format!("MAKEPAD_RUN_LAZY_VERIFY: Rust outline differs: {path}")); }
            }
        }
        Ok(())
    }
}

/// Crates a dependency-ready crate may start expanding ahead of publication (`Plan::build`).
const OUTLINE_AHEAD: usize = 32;

/// One crate's outline expansion on the shared workers (`Plan::build`): its files expand against
/// `base`, the table as the crate's publication will most likely see it, while earlier crates
/// still publish; publication replays each surface against the real table (a miss expands again).
struct OutlineJob {
    ci: usize,
    base: Arc<crate::macros::MacroTable>,
    /// Each worker reads its scan in place and lets go of it before handing back the surface:
    /// publication then owns the scan alone (Arc::make_mut, no copy).
    inputs: Vec<relax_session::sync::Mutex<Option<(String, String, Arc<Scanned>)>>>,
    next: std::sync::atomic::AtomicUsize,
    ready: (relax_session::sync::Mutex<BTreeMap<(String, String), Option<OutlineIndexes>>>, relax_session::sync::Condvar),
}
impl OutlineJob {
    fn new(ci: usize, base: Arc<crate::macros::MacroTable>, scans: &[part::Scan]) -> Arc<Self> {
        let inputs = scans.iter().filter_map(|(path, module, _, scan, ..)| scan.as_ref().map(|scan|
            relax_session::sync::Mutex::new(Some((path.clone(), module.clone(), scan.clone()))))).collect();
        Arc::new(Self { ci, base, inputs, next: std::sync::atomic::AtomicUsize::new(0),
            ready: (relax_session::sync::Mutex::new(BTreeMap::new()), relax_session::sync::Condvar::new()) })
    }
}

/// The shared expansion workers' queue: the started crates' jobs (the lowest crate first: the
/// one publication waits for), and whether the load is done.
#[derive(Default)]
struct OutlineQueue {
    jobs: relax_session::sync::Mutex<(BTreeMap<usize, Arc<OutlineJob>>, bool)>,
    wake: relax_session::sync::Condvar,
}
impl OutlineQueue {
    fn push(&self, job: Arc<OutlineJob>) {
        self.jobs.lock().unwrap().0.insert(job.ci, job);
        self.wake.notify_all();
    }
    /// The next file to expand (lowest crate first); None once the load is done.
    fn next(&self) -> Option<(Arc<OutlineJob>, usize)> {
        let mut g = self.jobs.lock().unwrap();
        loop {
            while let Some((ci, job)) = g.0.iter().next().map(|(&ci, job)| (ci, job.clone())) {
                let i = job.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if i < job.inputs.len() { return Some((job, i)); }
                g.0.remove(&ci);
            }
            if g.1 { return None; }
            g = self.wake.wait(g).unwrap();
        }
    }
}
/// Ends the queue when dropped.
struct OutlineFinish<'a>(&'a OutlineQueue);
impl Drop for OutlineFinish<'_> {
    fn drop(&mut self) {
        self.0.jobs.lock().unwrap_or_else(|e| e.into_inner()).1 = true;
        self.0.wake.notify_all();
    }
}

/// A crate's modules and imports as its scans declare them (MacroTable::add_crate).
fn crate_modules(c: &PreCrate, scans: &[part::Scan]) -> crate::macros::CrateMods {
    let mut modules = crate::macros::CrateMods { root: c.name.clone(), src: c.root.clone(), ..Default::default() };
    modules.externs.extend(c.deps.iter().cloned());
    for (_, _, _, scan, _, _) in scans {
        let Some(scan) = scan else { continue; };
        modules.mods.extend(scan.mods.iter().cloned());
        modules.items.extend(scan.mod_items.iter().cloned());
        for u in &scan.uses {
            if let Some(bind) = &u.bind {
                if u.path.last() != Some(bind) { modules.renames.insert(bind.clone()); }
                if u.path == ["crate"] && u.module == c.name { modules.externs.insert(bind.clone(), c.name.clone()); }
            }
            modules.uses.entry(u.module.clone()).or_default().push(u.clone());
        }
    }
    modules
}

/// One scanned file's macro definitions (`defs`, taken from the scan), aliases and macro_use
/// crates into `table`, in the scan's order.
fn publish_scan_names(table: &mut crate::macros::MacroTable, krate: &str, scan: &Scanned, defs: &[crate::macros::MacroRef]) {
    for def in defs { table.push(def.clone()); }
    for (alias, qual, target) in &scan.aliases { table.add_alias(krate.to_string(), alias.clone(), (qual.clone(), target.clone())); }
    for name in &scan.macro_use { table.macro_use.entry(krate.to_string()).or_default().push(name.clone()); }
}

type RawFile = (usize, String, String, bool, BTreeSet<String>);

/// Pure projections of an expanded file. Build these on its expansion worker,
/// before handing the result back to ordered macro/namespace publication. A
/// failed macro replay discards both the surface and its projections together.
struct OutlineIndexes {
    surface: crate::parser::rs_expand::Surface,
    index: Option<Arc<crate::rs_index::NameIndex>>,
    parameters: BTreeSet<String>,
}
impl OutlineIndexes {
    /// `input` is the source the surface was expanded from.
    fn new(surface: crate::parser::rs_expand::Surface, input: &[u8], profile: bool, path: &str) -> Self {
        let _profile = crate::load_profile::span(profile, "rust-demand-outline-index", path);
        let text = surface.text(input);
        let index = crate::rs_outline::expanded_index(&text, &surface.toks).map(Arc::new);
        let parameters = impl_parameters(&text, &surface.toks);
        drop(text);
        Self { surface, index, parameters }
    }
    fn replays(&self, table: &Arc<crate::macros::MacroTable>, krate: &str, path: &str, profile: bool) -> bool {
        let _profile = crate::load_profile::span(profile, "rust-demand-outline-replay-check", path);
        self.surface.used.replays(table, krate, path)
    }
}

/// One file's declaration index projection (unqualified names, impl heads).
fn file_index(crates: &[PreCrate], contexts: &[Arc<crate::source_context::Context>],
    expanded_indexes: &BTreeMap<(usize, String, String), Arc<crate::rs_index::NameIndex>>, profile: bool,
    (ci, path, module, force, parameters): &RawFile,
) -> FileIndex {
    let mut file = FileIndex { ci: *ci, path: path.clone(), module: module.clone(), names: BTreeSet::new(), glob_names: BTreeSet::new(), impls: Vec::new(), blankets: BTreeSet::new(), force: *force };
    let index = if let Some(index) = expanded_indexes.get(&(*ci, path.clone(), module.clone())) {
        Ok(index.clone())
    } else {
        contexts[*ci].read_source(path).ok()
            .and_then(|source| contexts[*ci].rust_outline(path, &source.src, crates[*ci].edition, &crates[*ci].cfg))
            .map(|outline| Arc::new(outline.index.clone()))
            .ok_or_else(|| "Rust outline unavailable; collect the file to report its error".to_owned())
    };
    match index {
        Ok(index) => {
            index_names(&index, parameters, &mut file, &crates[*ci].cfg);
            if profile && file.force { eprintln!("rs-force: {path:?} index {:?}", index.impls.iter().map(|i| (&i.trait_path, &i.self_path)).collect::<Vec<_>>()); }
            // Nested modules may be generated, have #[path], or carry
            // textual macro scope. Their parent must be present.
            // Prescan already followed the sibling's configured
            // modules. A child demand also reaches its parent.
        }
        // An index is an optimization, never a reason to swallow an error.
        Err(error) => {
            file.force = true;
            if profile { eprintln!("rs-index-fallback: {path:?} {error}"); }
            crate::why::event("index-fallback", path, 0, "", Some(crate::why::Cause::new(path, 1, "index-unavailable", &error)));
        },
    }
    file.force |= *path == crates[*ci].root;
    file
}

/// Scan authored inputs; also used when a changed dependency invalidates an outline.
fn discover_crate(c: &PreCrate, ci: usize, context: &crate::source_context::Context, template: &Names, rust_lazy: bool, profile: bool)
    -> (Vec<part::Scan>, Vec<(String, String)>, Names) {
    let mut todo = vec![(c.root.clone(), crate::program::dir_of(&c.root).to_owned(), c.name.clone())];
    let mut seen = BTreeSet::new();
    let mut scans = Vec::new();
    let mut local_names = template.fork();
    let mut aliases = Vec::new();
    while let Some((path, dir, module)) = todo.pop() {
        if !seen.insert(path.clone()) {
            aliases.push((path, module));
            continue;
        }
        let source = context.read_source(&path).ok();
        let hash = source.as_ref().map(|source| crate::crate_image::file_hash(&path, &source.src));
        let scan = source.and_then(|source| scan_text_mode(&path, &dir, &module, &c.name, c.edition, &c.cfg, (&source.src).into(), context, true)).map(Arc::new);
        let parameters = BTreeSet::new();
        let mut opaque = Vec::new();
        if let Some(scan) = &scan {
            todo.extend(scan.children.iter().rev().cloned());
            {
                let _profile = crate::load_profile::span(profile, "rust-demand-names", &path);
                if !rust_lazy { local_names.add(ci, &module, &scan.src, &scan.toks, &scan.uses, &scan.mods); }
            }
            // These token walks belong to the file worker;
            // only their stable publication needs the merge.
            let _profile = crate::load_profile::span(profile, "rust-demand-file-metadata", &path);
            if profile { opaque = opaque_sites(&scan.src, &scan.toks); }
        }
        scans.push((path, module, hash, scan, parameters, opaque));
    }
    (scans, aliases, local_names)
}

/// Namespace-dependent impl heads, compiler derive heads and the unindexed-item
/// check of one file. Returns whether an item is invisible to the index.
fn macro_surface(names: &Names, crates: &[PreCrate], contexts: &[Arc<crate::source_context::Context>],
    expanded_indexes: &BTreeMap<(usize, String, String), Arc<crate::rs_index::NameIndex>>, profile: bool,
    file: &mut FileIndex, source: Option<(&[u8], &[Tok])>, parameters: Option<&BTreeSet<String>>,
) -> bool {
    let _profile = crate::load_profile::span(profile, "rust-demand-macro-surface", &file.path);
    names.qualify_impls(file);
    let initial_force = file.force;
    let Some((src, toks)) = source else { return false; };
    {
        let c = &crates[file.ci];
        let index = expanded_indexes.get(&(file.ci, file.path.clone(), file.module.clone())).cloned()
            .or_else(|| contexts[file.ci].rust_outline(&file.path, src, c.edition, &c.cfg).map(|o| Arc::new(o.index.clone())));
        if let Some(index) = index {
            let scanned = parameters.is_none().then(|| impl_parameters(src, toks));
            let parameters = parameters.or(scanned.as_ref()).unwrap();
            names.qualify_rust_impls(file, &index, parameters, &c.cfg);
            let module = file.module.clone();
            index_builtin_derives(file, &index, &module, src, toks, &c.cfg);
        }
    }
    let unindexed = unindexed_items(src, toks, names, file.ci, &file.module);
    if profile && unindexed { eprintln!("rs-force: {:?} unindexed-item", file.path); }
    file.force |= unindexed;
    if profile && file.force && !initial_force { eprintln!("rs-force: {:?} impl-or-item", file.path); }
    unindexed
}

fn reference_sites(path: &str, src: &[u8], toks: &[Tok], origins: &[crate::source::TokenOrigin], context: &crate::source_context::Context) -> BTreeMap<(String, String), (String, usize)> {
    let mut sites = BTreeMap::new();
    let lines = crate::source::line_starts(src);
    for (i, token) in toks.iter().enumerate() {
        if !matches!(token.kind, T::Ident | T::RawIdent) || i > 0 && toks[i - 1].kind == T::KwMod { continue; }
        let name = String::from_utf8_lossy(&src[token.lo as usize..token.hi as usize]).trim_start_matches("r#").to_owned();
        let site = if let Some(span) = crate::source::origin_at(origins, token.lo) {
            context.source_site(path, span)
        } else { (path.to_owned(), crate::source::line_col(src, &lines, token.lo).0) };
        sites.entry((path.into(), name)).or_insert(site);
    }
    for (name, line) in opaque_sites(src, toks) {
        sites.entry((path.into(), "opaque-expansion".into())).or_insert((path.into(), line));
        sites.entry((path.into(), "opaque-attribute".into())).or_insert((path.into(), line));
        let _ = name;
    }
    sites
}

fn opaque_sites(src: &[u8], toks: &[Tok]) -> Vec<(String, usize)> {
    let lines = crate::lexer::LineIndex::new(src);
    let mut out = Vec::new();
    let mut depth = 0;
    for (i, t) in toks.iter().enumerate() {
        match t.kind {
            T::OpenBracket if depth != 0 || i > 0 && toks[i - 1].kind == T::Pound
                || i > 1 && toks[i - 1].kind == T::Not && toks[i - 2].kind == T::Pound => depth += 1,
            T::CloseBracket if depth != 0 => depth -= 1,
            _ => {},
        }
        if !matches!(t.kind, T::Ident | T::RawIdent) || depth == 0 && !toks.get(i + 1).is_some_and(|t| t.kind == T::Not) { continue; }
        if let Ok(name) = std::str::from_utf8(&src[t.lo as usize..t.hi as usize]) {
            if opaque_name(name) { out.push((name.to_owned(), lines.line_col(t.lo).0)); }
        }
    }
    out
}

fn index_cfg_active(stream: Option<&crate::pm_tokens::TokenStream>, cfg: &CfgSet) -> bool {
    let Some(stream) = stream else { return true; };
    let Ok(input) = crate::builtins::Expansion::from_stream(stream, 2021) else { return true; };
    let toks = &input.toks;
    let mut attrs = Vec::new();
    let mut at = 0;
    while at < toks.len() {
        if toks[at].kind != T::Pound { return true; }
        let inner = toks.get(at + 1).is_some_and(|t| t.kind == T::Not);
        let open = at + 1 + usize::from(inner);
        let mut depth = 1;
        let mut end = open + 1;
        while end < toks.len() {
            match toks[end].kind { T::OpenBracket => depth += 1, T::CloseBracket => depth -= 1, _ => {} }
            if depth == 0 { break; }
            end += 1;
        }
        if end == toks.len() { return true; }
        attrs.push(Attr { inner, toks: TokRange { lo: (open + 1) as u32, hi: end as u32 } });
        at = end + 1;
    }
    crate::cfg::active(Src { src: input.text.as_bytes(), plain: toks, ext: &[], parts: None }, &attrs, cfg)
}

fn index_names(index: &crate::rs_index::NameIndex, parameters: &BTreeSet<String>, file: &mut FileIndex, cfg: &CfgSet) {
    if !index.cfgs.iter().all(|tokens| index_cfg_active(Some(tokens), cfg)) { return; }
    for item in index.items.iter().filter(|i| index_cfg_active(i.cfg.as_ref(), cfg)) { file.names.insert(item.name.clone()); if item.vis != crate::rs_index::Vis::Private { file.glob_names.insert(item.name.clone()); } }
    for mac in index.macros.iter().filter(|i| index_cfg_active(i.cfg.as_ref(), cfg)) { file.names.insert(mac.name.clone()); }
    let trait_key = |tr: &String| if cfg.session.options.rs_lazy { tr.split('<').next().unwrap_or("").trim().to_owned() } else { tr.clone() };
    for imp in index.impls.iter().filter(|i| index_cfg_active(i.cfg.as_ref(), cfg)) {
        let head = imp.self_path.split('<').next().unwrap_or("").trim();
        if parameters.contains(head) {
            if let Some(tr) = &imp.trait_path { file.blankets.insert(trait_key(tr)); }
            else { file.force = true; }
        } else if let Some(tr) = head.strip_prefix("dyn ") {
            file.blankets.insert(tr.to_owned());
        } else if head.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ':') && !head.is_empty() {
            // rustc candidate_assembly::assemble_candidates_from_impls calls
            // for_each_relevant_impl(trait, self_ty, ...). A trait spelling alone
            // cannot activate every concrete implementor's module.
            file.impls.push((imp.trait_path.clone(), head.to_owned()));
        } else if let Some(tr) = imp.trait_path.as_ref().filter(|_| cfg.session.options.rs_lazy) {
            // A structural self type (slice, array, reference, tuple, fn
            // pointer) can arise through inference; such an impl can only be
            // selected through its trait, which then must be demanded.
            file.blankets.insert(trait_key(tr));
        } else {
            // Inherent impls of structural types/projections stay with the file.
            file.force = true;
        }
    }
    for module in index.modules.iter().filter(|i| index_cfg_active(i.cfg.as_ref(), cfg)) {
        if let Some(inner) = &module.inline { index_names(inner, parameters, file, cfg); }
    }
    // Anonymous constants have no runtime initialization. Their nested impls,
    // when any, are handled by the explicit unindexed-item check below.
    if !cfg.session.options.rs_lazy {
        file.force |= index.items.iter().any(|i| i.name == "_" && index_cfg_active(i.cfg.as_ref(), cfg));
    }
}

fn impl_parameters(src: &[u8], toks: &[Tok]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (i, t) in toks.iter().enumerate() {
        if t.kind != T::KwImpl || !toks.get(i + 1).is_some_and(|t| t.kind == T::Lt) { continue; }
        let mut depth = 0i32;
        let mut parameter = true;
        for t in &toks[i + 1..] {
            match t.kind {
                T::Lt => depth += 1,
                T::Gt => depth -= 1,
                T::Shr => depth -= 2,
                T::Comma if depth == 1 => parameter = true,
                T::Ident | T::RawIdent if depth == 1 && parameter => {
                    if let Ok(name) = std::str::from_utf8(&src[t.lo as usize..t.hi as usize]) { out.insert(name.trim_start_matches("r#").to_owned()); }
                    parameter = false;
                }
                T::Lifetime | T::KwConst if depth == 1 => parameter = false,
                _ => {},
            }
            if depth <= 0 { break; }
        }
    }
    out
}

pub fn recipe_identity(crates: &[PreCrate]) -> u128 {
    let mut hash = crate::sip::Sip128::new();
    hash.str("source-demand-v24-impl-declarations");
    hash.u128(crate::macro_reg::outline_identity());
    for c in crates {
        hash.str(&c.name); hash.str(&c.root); hash.u32(c.edition as u32);
        hash.u128(c.cfg.session.options.fingerprint()); hash.u8(c.proc_macro as u8);
        let mut cfg = c.cfg.set.clone(); cfg.sort();
        for value in cfg { hash.str(&value); }
        for (alias, name) in &c.deps { hash.str(alias); hash.str(name); }
        for d in &c.image_deps { hash.u32(*d as u32); }
        for (alias, name) in &c.image_dep_names { hash.str(alias); hash.str(name); }
    }
    hash.finish128()
}

fn unindexed_items(src: &[u8], toks: &[Tok], names: &Names, ci: usize, module: &str) -> bool {
    let fixed = demand::fixed_function_macros(src, toks);
    let mut depth = 0u32;
    let mut local_types = Vec::<(u32, String)>::new();
    let mut i = 0;
    while i < toks.len() {
        let token = &toks[i];
        // A transcriber is not an item scope until its macro is invoked.
        if src.get(token.lo as usize..token.hi as usize) == Some(b"macro_rules") && i + 3 < toks.len() {
            i = demand::close(toks, i + 3) + 1;
            continue;
        }
        if depth == 0 && token.kind == T::KwMod && toks.get(i + 2).is_some_and(|t| t.kind == T::OpenBrace) {
            let end = demand::close(toks, i + 2);

            let name = &toks[i + 1];
            let module = format!("{module}::{}", String::from_utf8_lossy(&src[name.lo as usize..name.hi as usize]));
            if unindexed_items(src, &toks[i + 3..end], names, ci, &module) { return true; }
            i = end + 1;
            continue;
        }
        if depth != 0 && (matches!(token.kind, T::KwStruct | T::KwEnum) || src.get(token.lo as usize..token.hi as usize) == Some(b"union")) {
            if let Some(name) = toks.get(i + 1) {
                local_types.push((depth, String::from_utf8_lossy(&src[name.lo as usize..name.hi as usize]).into_owned()));
            }
        }
        if token.kind == T::KwImpl && depth != 0 && i > 0
            && matches!(toks[i - 1].kind, T::OpenBrace | T::Semi | T::CloseBrace | T::CloseBracket) {
            let Some(open) = (i + 1..toks.len()).find(|&at| toks[at].kind == T::OpenBrace) else { return true; };
            let end = demand::close(toks, open);
            let local = crate::pm_tokens::from_toks(src, &toks[i..=end], &|_, _| crate::pm_tokens::Span::DUMMY).ok()
                .map(|tokens| crate::rs_index::of_tokens(&tokens))
                .is_some_and(|index| index.impls.len() == 1 && local_types.iter().any(|(_, name)|
                    index.impls[0].self_path.split('<').next().is_some_and(|head| head.trim() == name)));
            // An impl of a type declared in this body cannot be observed until
            // its enclosing function is reached. External/blanket impls retain
            // the fallback because they affect other files' candidate sets.
            if !local { return true; }
            i = end + 1;
            continue;
        }
        if depth == 0 && matches!(token.kind, T::Ident | T::RawIdent | T::KwCrate | T::KwSelfValue | T::KwSuper) {
            let mut path = vec![String::from_utf8_lossy(&src[token.lo as usize..token.hi as usize]).into_owned()];
            let mut end = i + 1;
            while end + 1 < toks.len() && toks[end].kind == T::PathSep {
                let t = &toks[end + 1]; path.push(String::from_utf8_lossy(&src[t.lo as usize..t.hi as usize]).into_owned()); end += 2;
            }
            if toks.get(end).is_some_and(|t| t.kind == T::Not) {
                if toks.get(end + 1).is_some_and(|t| matches!(t.kind, T::OpenBrace | T::OpenParen | T::OpenBracket)) {
                    if !fixed.contains_key(path.last().unwrap()) && !names.empty_macro(ci, module, &path)
                        && !(path.len() == 1 && names.local_impl_macro(ci, module, src, toks, i, end + 1)) { return true; }
                    i = demand::close(toks, end + 1) + 1;
                    continue;
                }
                return true;
            }
        }
        match token.kind {
            T::OpenBrace | T::OpenParen | T::OpenBracket => depth += 1,
            T::CloseBrace | T::CloseParen | T::CloseBracket => { depth = depth.saturating_sub(1); local_types.retain(|(scope, _)| *scope <= depth); },
            _ => {},
        }
        i += 1;
    }
    false
}

fn opaque_name(name: &str) -> bool {
    crate::macro_reg::has_macro(name) || crate::macro_reg::has_attr(name)
        || matches!(name, "include" | "concat_idents" | "paste" | "global_asm" | "link_section" | "global_allocator" | "used" | "no_mangle" | "export_name" | "link" | "lang")
}

fn global_items(src: &[u8], tokens: &[Tok]) -> bool {
    let mut at = 0;
    while at < tokens.len() {
        let t = tokens[at];
        // Ordinary statics have const initializers, not startup constructors.
        // They and foreign declarations follow name demand. Linkage/lang-item
        // attributes below are the roots observable without a Rust reference.
        if t.kind == T::KwMod && tokens.get(at + 2).is_some_and(|t| t.kind == T::OpenBrace) {
            let end = demand::close(tokens, at + 2);
            if global_items(src, &tokens[at + 3..end]) { return true; }
            at = end + 1;
            continue;
        }
        if t.kind == T::Pound {
            let open = at + 1 + usize::from(tokens.get(at + 1).is_some_and(|t| t.kind == T::Not));
            if tokens.get(open).is_some_and(|t| t.kind == T::OpenBracket) {
                let end = demand::close(tokens, open);
                if tokens[open + 1..end].iter().any(|t| matches!(src.get(t.lo as usize..t.hi as usize),
                    Some(b"lang" | b"no_mangle" | b"export_name" | b"link" | b"link_section" | b"used" | b"global_allocator" | b"alloc_error_handler" | b"panic_handler"))) { return true; }
                at = end + 1;
                continue;
            }
        }
        if matches!(t.kind, T::OpenBrace | T::OpenParen | T::OpenBracket) {
            at = demand::close(tokens, at) + 1;
            continue;
        }
        at += 1;
    }
    false
}

// These compiler-owned outputs are either trait impls or marker/glue impls.
// A hosted derive with the same name is still opaque. No user macro body is
// inferred from its spelling.
fn builtin_derive_list(src: &[u8], tokens: &[Tok]) -> Option<Vec<&'static str>> {
    let mut traits = Vec::new();
    for token in tokens {
        match token.kind {
            T::OpenParen | T::CloseParen | T::Comma => {}
            T::Ident => {
                let name = std::str::from_utf8(&src[token.lo as usize..token.hi as usize]).ok()?;
                if crate::macro_reg::has_derive(name) {
                    if crate::macro_reg::outline_native(name, crate::macro_reg::Kind::Derive) { continue; }
                    return None;
                }
                traits.push(crate::parser::derive::trait_path(name)?);
            }
            _ => return None,
        }
    }
    Some(traits)
}

fn index_builtin_derives(file: &mut FileIndex, index: &crate::rs_index::NameIndex, module: &str, src: &[u8], tokens: &[Tok], cfg: &CfgSet) {
    let view = Src { src, plain: tokens, ext: &[], parts: None };
    for item in &index.items {
        if !matches!(item.kind, crate::rs_index::ItemKind::Struct | crate::rs_index::ItemKind::Enum | crate::rs_index::ItemKind::Union) { continue; }
        let Some(at) = tokens.iter().position(|t| t.lo == item.span.lo) else { continue; };
        let Some(keyword) = at.checked_sub(1) else { continue; };
        for attr in crate::cfg::attrs_before(view, keyword, cfg) {
            let lo = attr.toks.lo as usize;
            let hi = attr.toks.hi as usize;
            if lo >= hi || crate::cfg::tok_str(view, lo) != b"derive" { continue; }
            if let Some(traits) = builtin_derive_list(src, &tokens[lo + 1..hi]) {
                for tr in traits {
                    file.impls.push((Some(format!("core::{tr}")), format!("{module}::{}", item.name)));
                    crate::why::event("rs-derive-index", &file.path, 0, &item.name, None);
                }
            }
        }
    }
    for child in &index.modules {
        if let Some(inner) = &child.inline {
            index_builtin_derives(file, inner, &format!("{module}::{}", child.name), src, tokens, cfg);
        }
    }
}

fn rust_eager_reason(src: &[u8], tokens: &[Tok], names: &Names, ci: usize, module: &str, outputs: &mut Vec<(String, String, Vec<Tok>)>, path: &str, profile: bool) -> Option<(&'static str, bool)> {
    let mut file = None;
    let mut i = 0;
    while i < tokens.len() {
        let t = &tokens[i];
        let spelling = std::str::from_utf8(&src[t.lo as usize..t.hi as usize]).unwrap_or("");
        if matches!(t.kind, T::KwStatic | T::KwExtern) || spelling == "auto" {
            file = Some(("static, extern or auto-trait declaration", false));
        }
        if spelling == "macro_rules" {
            file = Some(("textual macro definition", false));
            if i + 3 < tokens.len() {
                i = demand::close(tokens, i + 3) + 1;
                continue;
            }
        }
        if t.kind == T::KwMod && tokens.get(i + 2).is_some_and(|t| t.kind == T::OpenBrace) {
            let end = demand::close(tokens, i + 2);
            let name = &tokens[i + 1];
            let module = format!("{module}::{}", String::from_utf8_lossy(&src[name.lo as usize..name.hi as usize]));
            if let Some(reason) = rust_eager_reason(src, &tokens[i + 3..end], names, ci, &module, outputs, path, profile) {
                if reason.1 { return Some(reason); }
                file = Some(reason);
            }
            i = end + 1;
            continue;
        }
        if t.kind == T::Pound {
            let open = i + 1 + usize::from(tokens.get(i + 1).is_some_and(|t| t.kind == T::Not));
            if !tokens.get(open).is_some_and(|t| t.kind == T::OpenBracket) { i += 1; continue; }
            let end = demand::close(tokens, open);
            let Some(name) = tokens.get(open + 1).filter(|_| open + 1 < end) else { i = end + 1; continue; };
            let name = std::str::from_utf8(&src[name.lo as usize..name.hi as usize]).unwrap_or("");
            match name {
                "lang" | "no_mangle" | "export_name" | "link" | "link_name" | "link_section" | "used" | "global_allocator" | "alloc_error_handler" | "panic_handler" => file = Some(("language or linkage attribute", false)),
                "derive" if builtin_derive_list(src, &tokens[open + 2..end]).is_some() => {
                    file = Some(("indexed compiler derive output", false));
                }
                "default" if !crate::macro_reg::has_attr(name) => {}
                "cfg" | "cfg_attr" | "doc" | "path" | "allow" | "warn" | "deny" | "forbid" | "expect"
                | "inline" | "cold" | "repr" | "deprecated" | "must_use" | "track_caller" | "non_exhaustive"
                | "target_feature" | "automatically_derived" | "macro_export" | "macro_use"
                | "no_std" | "no_core" | "no_implicit_prelude" | "feature" | "crate_name" | "crate_type"
                | "recursion_limit" | "type_length_limit" | "register_tool" | "windows_subsystem"
                | "test" | "bench" | "ignore" | "should_panic" => {}
                _ => {
                    if profile {
                        let attribute = tokens[open + 1..end].iter().map(|t| String::from_utf8_lossy(&src[t.lo as usize..t.hi as usize])).collect::<Vec<_>>().join(" ");
                        eprintln!("rs-fallback-trigger: {path:?} attribute {attribute:?}");
                    }
                    return Some(("derive or unindexed attribute expansion", true));
                }
            }
        }
        if matches!(t.kind, T::Ident | T::RawIdent) && spelling != "macro_rules"
            && tokens.get(i + 1).is_some_and(|t| t.kind == T::Not) {
            if tokens.get(i + 2).is_some_and(|t| matches!(t.kind, T::OpenBrace | T::OpenParen | T::OpenBracket))
                && (i == 0 || tokens[i - 1].kind != T::PathSep) {
                if names.empty_macro(ci, module, &[spelling]) {
                    i = demand::close(tokens, i + 2) + 1;
                    continue;
                }
                if let Some((text, expanded)) = names.local_impl_output(ci, module, src, tokens, i, i + 2) {
                    if let Some(reason) = rust_eager_reason(text.as_bytes(), &expanded, names, ci, module, outputs, path, profile) {
                        if reason.1 { return Some(reason); }
                    }
                    outputs.push((module.into(), text, expanded));
                    file = Some(("indexed local macro impl output", false));
                    i = demand::close(tokens, i + 2) + 1;
                    continue;
                }
            }
            match spelling {
                "stringify" | "concat" | "env" | "option_env" | "file" | "line" | "column" | "module_path" | "cfg" | "include_str" | "include_bytes" => {}
                _ => {
                    if profile { eprintln!("rs-fallback-trigger: {path:?} macro {spelling:?}"); }
                    return Some(("unindexed macro output (including include/generated files)", true));
                }
            }
        }
        i += 1;
    }
    file
}

fn forced_deferral(path: &str) -> bool {
    static LIST: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| std::env::var("MAKEPAD_RUN_LAZY_DEFER").map(|v| v.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect()).unwrap_or_default())
        .iter().any(|suffix| path.ends_with(suffix.as_str()))
}
