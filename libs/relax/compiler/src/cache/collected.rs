//! A collected Program/Tcx above the immutable per-crate front images. Its certificate
//! covers the ordered unit graph, all authored inputs, module probes and late inputs.
use super::*;
use crate::crate_image::{Codec, CrateImages, GraphRecipe};
use crate::program::Program;
use crate::tcx::Tcx;
use std::sync::Arc;

const KIND_COLLECTED: u32 = 9;

/// Proof passed from a boot certificate or an in-process swap to Cache::open. Fingerprints
/// cover the cache's original crate prefix; a swap's Program may also hold runtime crates.
pub struct CollectedCache {
    pub(super) indexes: Option<MappedIndexes>,
    pub(super) definitions: &'static [u8],
    pub(super) fps: Vec<u128>,
    /// the crates' interface fingerprints the recording run's code was keyed with (its inputs
    /// are the certified ones): the cache opens with them instead of digesting every file
    pub(super) ifaces: Vec<u128>,
    pub(super) defs: usize,
    pub(super) files: usize,
}

fn enabled() -> bool { std::env::var("MAKEPAD_RUN_COLLECTED").map_or(true, |v| v != "0") }
fn key(identity: u128) -> u128 {
    let mut h = Sip128::new();
    h.str("collected-graph-v1"); h.u128(front_graph_key(identity)); h.finish128()
}
pub(super) fn path(dir: &std::path::Path, identity: u128) -> PathBuf {
    dir.join(format!("collected_graph-{:016x}.rcs", key(identity) as u64))
}
fn certificate(graph: &crate::crate_image::GraphImage) -> u128 {
    let mut h = Sip128::new();
    h.write(&crate::crate_image::encode(graph)); h.finish128()
}

/// An immutable boot, captured before a run-time crate can add declarations or intern
/// types. A definition/crate prefix alone is not enough: module compilation can demand
/// boot signatures and deferred boot files between its own additions. Keep their original
/// IDs and lazy state together, without trying to prune the live interner afterwards.
pub(super) struct CollectedBoot {
    recipe: GraphRecipe,
    program: Arc<crate::image::Writer<'static>>,
    types: Arc<crate::image::Writer<'static>>,
    definitions: Arc<crate::image::Writer<'static>>,
    files: Vec<(u32, String, u128)>,
    interfaces: Vec<MappedKey>,
}

impl FrontImages {
    // An edit normally invalidates a leaf or a dependent suffix. Keep the complete
    // certified prefix resident, including its resolved imports and collected types.
    fn collected_prefix(&self, graph: &crate::crate_image::GraphImage, pre: &[crate::preload::PreCrate], plan: Option<&crate::preload::DemandPlan>) -> Option<usize> {
        let recipe = GraphRecipe::new(pre);
        if graph.identity != recipe.identity || graph.crates.len() != pre.len()
            || graph.crates.iter().zip(&recipe.crates).any(|((key, _), want)| key != want) { return None; }
        let present = pre.iter().map(|c| c.name.clone()).collect();
        let mut dirty = Vec::new();
        let mut late = HashMap::new();
        for (ci, (c, ((_, _), fp))) in pre.iter().zip(&graph.crates).enumerate() {
            let valid = (|| {
                let store = Store::open(&self.dir.join(front_store_name(&c.cfg.session.options, &c.name, *fp)))?;
                let (kind, bytes) = store.get(front_crate_key(&c.name, &c.root))?;
                if kind != KIND_FRONT_CRATE { return None; }
                let rec: crate::crate_image::Record = crate::crate_image::decode(bytes)?;
                if rec.ahead.demand_plan != plan.map_or(0, |p| p.selection_identity(ci))
                    || rec.ahead.deferred != plan.map(|p| p.deferred(ci)).unwrap_or_default() { return None; }
                let inputs: Vec<_> = rec.ahead.inputs.iter().map(|(p, h)| (p.as_str(), *h))
                    .chain(rec.files.iter().map(|f| (f.path.as_str(), f.hash))).collect();
                (self.inputs_current(&inputs) && rec.ahead.probes.iter().all(|(p, e)| self.probe_current(p, *e))
                    && ledger_current(&store, &present, &mut late)).then_some(())
            })().is_some();
            dirty.push(!valid || c.image_deps.iter().any(|d| dirty.get(*d).copied().unwrap_or(true)));
        }
        let first = dirty.iter().position(|d| *d)?;
        // Every crate from the first changed one on is collected again: changed ones parse,
        // clean ones load from their crate images (proc-macro and build crates load after the
        // app's, so an app edit leaves clean crates behind it). The std crates may depend on
        // one another despite their load order: a change there uses the ordinary path.
        (first >= crate::std_pre_crates(&pre[0].cfg.session).len()).then_some(first)
    }

    pub fn restore_collected(&self, session: &Arc<crate::Session>, pre: &[crate::preload::PreCrate], swap: Option<&CollectedWritten>) -> Option<(Program, Tcx, Option<CollectedCache>)> {
        if let Some(written) = swap { return self.restore_swap(session, pre, written); }
        let _profile = crate::load_profile::span(session.runtime.profile, "front-collected-restore", "main");
        // This handoff is scoped to one immutable load, never a later edit.
        self.pending_plan.lock().unwrap().take();
        let mut why = "disabled";
        let started = std::time::Instant::now();
        let phase = |name: &str| if crate::preload::trace() {
            eprintln!("makepad-run collected: {} at {:.1} ms", name, started.elapsed().as_secs_f64() * 1e3);
        };
        let restored = std::thread::scope(|scope| (|| {
        if !enabled() || crate::syms::perturb(&session.options) != 0 { return None; }
        self.begin_load(pre);
        why = "certificate";
        let cert = self.certificate.lock().ok()?;
        let stale;
        let mut plan = None;
        let mut plan_task = None;
        let (graph, mut prefix) = match cert.as_ref() {
            Some(graph) => (graph, pre.len()),
            None => {
                stale = read_front_graph(&self.dir, GraphRecipe::new(pre).identity)?;
                if !session.options.source_eager && session.options.rs_lazy {
                    // Restore the immutable Program/Tcx while the demand worker
                    // computes the new selection. Nothing is retained or exposed
                    // to collection until that selection has been validated.
                    let contexts = crate::source_context::Context::for_crates(pre, Arc::default(), session);
                    let pending = Arc::new(crate::preload::DemandPlan::new(pre, contexts, Some(self), true));
                    *self.pending_plan.lock().unwrap() = Some((GraphRecipe::new(pre).identity, pending.clone()));
                    let worker = pending.clone();
                    let in_world = crate::macro_reg::in_world();
                    plan_task = Some(std::thread::Builder::new().name("run-demand".into()).spawn_scoped(scope, move || {
                        crate::macro_reg::set_in_world(in_world);
                        worker.complete();
                    }).ok()?);
                    plan = Some(pending);
                }
                let prefix = if plan.is_some() { 0 } else { self.collected_prefix(&stale, pre, None)? };
                (&stale, prefix)
            }
        };
        phase("validated");
        why = "store";
        let st = Arc::new(Store::open(&path(&self.dir, graph.identity))?);
        // Like a front image: structure only here (bounds; the image reader checks every
        // section), the checksum in the background (verify_front_images), so the boot reads
        // only the pages of the tables it uses, not the whole image.
        let at = st.index.binary_search_by(|e| { let k = e.key; k.cmp(&key(graph.identity)) }).ok()?;
        let (kind, bytes) = st.get_at(at, key(graph.identity))?;
        if kind != KIND_COLLECTED { return None; }
        FRONT_STORES.lock().unwrap_or_else(|e| e.into_inner()).push(st.clone());
        FRONT_USED.lock().unwrap_or_else(|e| e.into_inner()).push((st.clone(), at));
        // The checked, immutable mapping is pinned for the Program's mapped definition arena.
        let bytes: &'static [u8] = unsafe { std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()) };
        let image = crate::image::Reader::new(bytes)?;
        why = "certificate identity";
        if image.len() != 6 || image.bytes(0, 0)? != certificate(graph).to_le_bytes() { return None; }
        let ifaces: &[MappedKey] = image.slice(5, 5)?;
        if ifaces.len() != graph.crates.len() { return None; }
        let program = image.bytes(1, 1)?;
        let types = image.bytes(2, 2)?;
        let definitions = image.bytes(3, 3)?;
        phase("mapped");
        let options = crate::crate_image::parse_options(&session.options);
        let store_paths: Vec<PathBuf> = graph.crates.iter()
            .map(|((name, _), fp)| self.dir.join(front_store_name_with(options, name, *fp))).collect();
        let indexes = MappedIndexes::read(image.bytes(4, 4)?, &self.dir).filter(|ix| {
            ix.stores.len() == graph.crates.len()
                && ix.stores.iter().zip(&store_paths).all(|(s, path)| s.path.as_os_str() == path.as_os_str())
        });
        let mut stores = Vec::new();
        let mut late = HashMap::new();
        let present = graph.crates.iter().map(|((name, _), _)| name.clone()).collect();
        why = "crate stores / late inputs";
        for (i, store_path) in store_paths.iter().enumerate() {
            let store = indexes.as_ref().and_then(|ix| ix.stores.get(i)).cloned()
                .or_else(|| Store::open(store_path).map(Arc::new))?;
            if i < prefix && !ledger_current(&store, &present, &mut late) { return None; }
            stores.push(store);
        }
        phase("crate stores");
        why = "Program";
        let mut file_times = [std::time::Duration::ZERO; 3];
        let timed = crate::preload::trace();
        let mut lap = |slot: usize, at: &mut Option<std::time::Instant>| if let Some(t) = at {
            let now = std::time::Instant::now();
            file_times[slot] += now - *t;
            *t = now;
        };
        let mut prog = Program::restore_collected(session, program, pre, |f, k, cfg, meta| {
            let restored = (|| {
            let mut at = timed.then(std::time::Instant::now);
            let store = stores.get(k as usize)?;
            // The certified index snapshot locates every file's entries, recorded with the
            // graph; without it (a store changed since), search by key.
            let (main, part) = match indexes.as_ref().and_then(|ix| ix.files.get(f)) {
                Some(loc) => (loc.main as usize, (loc.part != u32::MAX).then_some(loc.part as usize)),
                None => {
                    let main = Cache::front_key(&meta.path, meta.hash);
                    let part = Cache::front_part_key(&meta.path, meta.hash);
                    (store.index.binary_search_by_key(&main, |e| e.key).ok()?, store.index.binary_search_by_key(&part, |e| e.key).ok())
                }
            };
            let (loc, img) = front_file_entry(store, main, store.index.get(main)?.key)?;
            lap(0, &mut at);
            let mut pf = crate::front::restore_certified(meta, img)?;
            pf.image = Some(loc);
            pf.sink = self.sink();
            lap(1, &mut at);
            if let Some(i) = part {
                let (loc, img) = front_file_entry(store, i, store.index.get(i)?.key)?;
                if !crate::front::apply_part(&meta.path, &mut pf, img, cfg, true) { return None; }
                pf.part.get_mut()?.image = Some(loc);
            }
            lap(2, &mut at);
            Some(pf)
            })();
            if restored.is_none() && crate::preload::trace() { eprintln!("makepad-run collected: file not restored: {}", meta.path); }
            restored
        })?;
        if crate::preload::trace() {
            eprintln!("makepad-run collected: files lookup {:.1}, restore {:.1}, parts {:.1} ms",
                file_times[0].as_secs_f64() * 1e3, file_times[1].as_secs_f64() * 1e3, file_times[2].as_secs_f64() * 1e3);
        }
        phase("Program");
        prog.front_sink = self.sink();
        why = "crate identities";
        if prog.crates.len() != graph.crates.len() || prog.crates.iter().zip(&graph.crates)
            .any(|(c, ((name, root), _))| prog.syms.str(c.name) != name || c.root_path() != root) { return None; }
        why = "Tcx";
        let mut tcx = Tcx::restore_collected(types, prog.files.len(), prog.crates.len())?;
        phase("Tcx");
        if let Some(task) = plan_task {
            why = "demand selection";
            task.join().ok()?;
            prefix = self.collected_prefix(graph, pre, plan.as_deref())?;
            for store in stores.iter().take(prefix) {
                if !ledger_current(store, &present, &mut late) { return None; }
            }
            phase("selection validated");
        }
        for store in stores.iter().take(prefix) { replay_front_ledger(store)?; }
        prog.front_graph = Some(GraphRecipe::new(pre));
        if prefix != pre.len() {
            let first = prog.retain_collected_prefix(prefix)?;
            tcx.retain_collected_prefix(first);
            if session.options.verbose { eprintln!("makepad-run collected: retained {} crates; rebuilding {} changed/dependent crates", prefix, pre.len() - prefix); }
            // the crates loaded after them find these as their dependencies (full_deps)
            *self.retained.lock().unwrap() = graph.crates[..prefix].iter().map(|((name, _), fp)| (name.clone(), *fp)).collect();
        }
        if let Some(plan) = plan {
            self.pending_plan.lock().unwrap().take();
            // Parsing the remaining crates reuses these exact source snapshots,
            // lowerings and full-graph selection (including retained dependencies).
            for context in &plan.contexts[..prefix] { context.release_prescan(); }
            prog.sources = plan.contexts.first()?.sources().clone();
            prog.demand_plan = Some(plan);
        }
        let cache = (prefix == pre.len()).then(|| CollectedCache {
            indexes, definitions, fps: graph.crates.iter().map(|(_, fp)| *fp).collect(),
            ifaces: ifaces.iter().map(|k| k.get()).collect(),
            defs: prog.defs.len(), files: prog.files.len(),
        });
        drop(cert);
        self.end_load();
        if session.options.verbose {
            eprintln!("makepad-run collected: restored {} crates, {} definitions, {} types", prog.crates.len(), prog.defs.len(), tcx.tys.len());
        }
        Some((prog, tcx, cache))
        })());
        if restored.is_none() && (crate::preload::trace() || session.runtime.profile) {
            eprintln!("makepad-run collected: not restored: {}; demand plan retained={}", why, self.pending_plan.lock().unwrap().is_some());
        }
        restored
    }

    /// Only the quiesced swap supplies this entry. Files are addressed by their immutable run
    /// images, including runtime crates that have no boot crate store or graph certificate.
    fn restore_swap(&self, session: &Arc<crate::Session>, pre: &[crate::preload::PreCrate], written: &CollectedWritten) -> Option<(Program, Tcx, Option<CollectedCache>)> {
        if !enabled() || crate::syms::perturb(&session.options) != 0 { return None; }
        let st = Arc::new(Store::open(&written.dest)?);
        let at = st.index.binary_search_by(|e| { let k = e.key; k.cmp(&written.key) }).ok()?;
        let ix = st.index[at];
        if { ix.len } != written.len || { ix.sum } != written.sum || { ix.kind } != KIND_COLLECTED { return None; }
        let (_, bytes) = st.get_at(at, written.key)?;
        FRONT_STORES.lock().unwrap_or_else(|e| e.into_inner()).push(st.clone());
        FRONT_USED.lock().unwrap_or_else(|e| e.into_inner()).push((st.clone(), at));
        let bytes: &'static [u8] = unsafe { std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()) };
        let image = crate::image::Reader::new(bytes)?;
        let recipe = GraphRecipe::new(pre);
        if image.len() != 8 || image.bytes(0, 0)? != recipe.identity.to_le_bytes() { return None; }
        let ifaces: &[MappedKey] = image.slice(5, 5)?;
        let fps: &[MappedKey] = image.slice(7, 7)?;
        if fps.len() >= pre.len() || ifaces.len() != fps.len() { return None; }
        let mut refs = crate::crate_image::Decode::new(image.bytes(6, 6)?);
        let files = Vec::<(Vec<u8>, Option<Vec<u8>>)>::get(&mut refs)?;
        if !refs.bytes.is_empty() { return None; }
        let mut prog = Program::restore_collected(session, image.bytes(1, 1)?, pre, |f, _, cfg, meta| {
            let (main, part) = files.get(f)?;
            let (loc, img) = front_run::resolve(&self.dir, main, false)?;
            let mut pf = crate::front::restore_certified(meta, img)?;
            pf.image = Some(loc);
            pf.sink = self.sink();
            if let Some(part) = part {
                let (loc, img) = front_run::resolve(&self.dir, part, false)?;
                if !crate::front::apply_part(&meta.path, &mut pf, img, cfg, true) { return None; }
                pf.part.get_mut()?.image = Some(loc);
            }
            Some(pf)
        })?;
        if files.len() != prog.files.len() || prog.crates.iter().zip(&recipe.crates)
            .any(|(c, (name, root))| prog.syms.str(c.name) != name || c.root_path() != root) { return None; }
        prog.front_sink = self.sink();
        // front_graph stays absent: this program must never be published as a boot image.
        let tcx = Tcx::restore_collected(image.bytes(2, 2)?, prog.files.len(), prog.crates.len())?;
        let indexes = MappedIndexes::read(image.bytes(4, 4)?, &self.dir).filter(|ix|
            ix.stores.len() == fps.len() && ix.stores.iter().zip(pre.iter().zip(fps)).all(|(s, (c, fp))|
                s.path == self.dir.join(front_store_name(&session.options, &c.name, fp.get()))));
        let cache = CollectedCache {
            indexes, definitions: image.bytes(3, 3)?, fps: fps.iter().map(|k| k.get()).collect(),
            ifaces: ifaces.iter().map(|k| k.get()).collect(), defs: prog.defs.len(), files: prog.files.len(),
        };
        Some((prog, tcx, Some(cache)))
    }
}

impl Cache {
    /// Both the unit and front-end write locks are held. This only encodes metadata:
    /// crate stores, the graph certificate and the collected store are still published
    /// by the ordinary post-present/exit writer. Repeated module loads reuse the snapshot.
    pub(crate) fn preserve_collected_boot(&mut self, prog: &Program, tcx: &Tcx) {
        if self.collected_committed || self.collected_boot.is_some() || !enabled() || self.mode == Mode::ReadOnly { return; }
        let started = std::time::Instant::now();
        let mut why = "recipe";
        self.collected_boot = self.capture_collected_boot(prog, tcx, &|| false, &mut why);
        if crate::preload::trace() {
            match &self.collected_boot {
                Some(boot) => eprintln!("makepad-run collected: boot snapshot {} crates, {} definitions, {} types, {} files, {} bytes in {:.1} ms",
                    boot.recipe.crates.len(), prog.defs.len(), tcx.tys.len(), boot.files.len(),
                    boot.program.len() + boot.types.len() + boot.definitions.len(), started.elapsed().as_secs_f64() * 1e3),
                None => eprintln!("makepad-run collected: boot snapshot unavailable: {}", why),
            }
        }
    }

    fn capture_collected_boot(&self, prog: &Program, tcx: &Tcx, stop: &dyn Fn() -> bool, why: &mut &'static str) -> Option<CollectedBoot> {
        *why = "recipe";
        let recipe = prog.front_graph.as_ref()?;
        if recipe.crates.len() != prog.crates.len() || recipe.crates.iter().zip(&prog.crates)
            .any(|(key, c)| prog.syms.str(c.name) != key.0 || c.root_path() != key.1) { return None; }
        *why = "Program";
        let program = Arc::new(prog.collected_writer()?.into_owned());
        if stop() { return None; }
        *why = "Tcx";
        let types = Arc::new(tcx.collected_writer_with(stop)?.into_owned());
        if stop() { return None; }
        *why = "definitions";
        let definitions = Arc::new(self.collected_definitions(prog, tcx, stop)?);
        if stop() { return None; }
        *why = "interfaces";
        if self.iface.len() != recipe.crates.len() { return None; }
        let interfaces = self.iface.iter().map(|&k| MappedKey::new(k)).collect();
        let files = prog.files.iter().enumerate().map(|(f, file)| {
            let path = prog.file_paths.get(f)?;
            Some((*prog.file_crate.get(f)?, path.clone(), file.source_hash(path)))
        }).collect::<Option<Vec<_>>>()?;
        Some(CollectedBoot { recipe: recipe.clone(), program, types, definitions, files, interfaces })
    }

    pub(crate) fn collected_crates(&self, prog: &Program) -> usize {
        self.collected_boot.as_ref().map_or(prog.crates.len(), |boot| boot.recipe.crates.len())
    }

    fn collected_definitions(&self, prog: &Program, tcx: &Tcx, stop: &dyn Fn() -> bool) -> Option<crate::image::Writer<'static>> {
        let mut memo = HashMap::new();
        let mut files = HashMap::new();
        let mut maps = vec![std::collections::BTreeMap::new(); prog.crates.len()];
        let mut duplicates = HashSet::new();
        for i in 0..prog.defs.len() {
            if i % 1024 == 1023 && stop() { return None; }
            let d = DefId(i as u32);
            let c = prog.def(d).krate as usize;
            let k = def_key_in(prog, tcx, &self.crate_ident, &mut memo, &mut files, d);
            if maps[c].insert(k, d).is_some() { duplicates.insert((c, k)); }
        }
        for (c, k) in duplicates { maps[c].remove(&k); }
        let mut image = crate::image::Writer::default();
        let keys: Vec<MappedKey> = (0..prog.defs.len()).map(|i| MappedKey::new(memo[&DefId(i as u32)])).collect();
        image.add_owned(0, &keys);
        for (c, map) in maps.into_iter().enumerate() {
            let (keys, defs): (Vec<MappedKey>, Vec<u32>) = map.into_iter().map(|(k, d)| (MappedKey::new(k), d.0)).unzip();
            image.add_owned(1 + c as u32 * 2, &keys);
            image.add_owned(2 + c as u32 * 2, &defs);
        }
        Some(image)
    }

    pub(super) fn restore_collected_definitions(&mut self, prog: &Program, bytes: &'static [u8]) -> bool {
        let Some(mapped) = MappedDefinitions::read(bytes, prog.defs.len(), prog.crates.len()) else { return false; };
        self.mapped_definitions = Some(mapped);
        self.crate_defs_n = prog.defs.len();
        true
    }

    /// Called with compilation quiescent. Never performs collection or forces a lazy query.
    /// Per-crate file images are flushed first, then the complete graph is committed last.
    pub fn record_collected(&mut self, prog: &Program, tcx: &Tcx) -> Option<()> {
        match self.encode_collected(prog, tcx, &|| false) {
            CollectedRecord::Ready(dest, payload) => { Self::commit_collected(prog.session.options.verbose.then_some(self.collected_crates(prog)), dest, payload)?; }
            CollectedRecord::Current => (),
            CollectedRecord::Yielded | CollectedRecord::Failed => return None,
        }
        drop(self.finish_collected());
        Some(())
    }

    /// Mark success before taking the snapshot: later runtime modules must never cause
    /// its boot-only state to be recaptured. The caller drops these bytes outside locks.
    pub(crate) fn finish_collected(&mut self) -> impl Send + 'static {
        self.collected_committed = true;
        self.collected_boot.take()
    }

    /// The collected program's image, encoded under the unit and front-end locks (the caller's).
    /// `stop` is asked between parts and every 1024 definitions: when it says yes (the main
    /// thread waits for the unit lock) the encode is dropped and Yielded comes back, so the
    /// record never holds a frame up; the caller retries later. The store write
    /// (commit_collected) needs no lock.
    pub fn encode_collected(&self, prog: &Program, tcx: &Tcx, stop: &dyn Fn() -> bool) -> CollectedRecord {
        self.encode_collected_as(prog, tcx, stop, false)
    }

    /// `encode_collected` of the program as it is now: neither the committed mark nor the frozen
    /// boot snapshot short-circuits it, so the image covers every type, definition and file
    /// added since the first encode (the settle swap restores from it, and asserts the counts).
    pub(crate) fn encode_collected_now(&self, prog: &Program, tcx: &Tcx, pre: &[crate::preload::PreCrate], extra: &[crate::preload::PreCrate], stop: &dyn Fn() -> bool) -> CollectedRecord {
        if !extra.is_empty() { return self.encode_collected_swap(prog, tcx, pre, extra, stop); }
        self.encode_collected_as(prog, tcx, stop, true)
    }

    /// A whole-program snapshot private to this run. Keep the boot cache's identities and
    /// stores unchanged: runtime files are referenced directly instead of extending that cache.
    fn encode_collected_swap(&self, prog: &Program, tcx: &Tcx, pre: &[crate::preload::PreCrate], extra: &[crate::preload::PreCrate], stop: &dyn Fn() -> bool) -> CollectedRecord {
        if !enabled() || self.mode == Mode::ReadOnly { return CollectedRecord::Failed; }
        let mut why = "recipe";
        let recorded = (|| {
            let crates: Vec<_> = pre.iter().chain(extra).cloned().collect();
            let recipe = GraphRecipe::new(&crates);
            if recipe.crates.len() != prog.crates.len() || recipe.crates.iter().zip(&prog.crates)
                .any(|((name, root), c)| prog.syms.str(c.name) != name || c.root_path() != root)
                || self.fps.len() != pre.len() || self.iface.len() != pre.len() { return None; }
            let mut image = crate::image::Writer::default();
            image.add_bytes(0, recipe.identity.to_le_bytes().to_vec());
            why = "Program";
            image.add_image(1, Arc::new(prog.collected_writer()?.into_owned()));
            if stop() { return None; }
            why = "Tcx";
            image.add_image(2, Arc::new(tcx.collected_writer_with(stop)?.into_owned()));
            if stop() { return None; }
            why = "definitions";
            image.add_image(3, Arc::new(self.collected_definitions(prog, tcx, stop)?));
            if stop() { return None; }
            why = "index snapshot";
            image.add_bytes(4, MappedIndexes::record(&self.crate_path, &[])?);
            image.add_owned(5, &self.iface.iter().map(|&k| MappedKey::new(k)).collect::<Vec<_>>());
            why = "file images";
            let files = prog.files.iter().map(|f| {
                if stop() { return None; }
                let main = front_run::encode_ref(f.image.as_ref()?);
                let part = match f.part.get() {
                    Some(p) => Some(front_run::encode_ref(p.image.as_ref()?)),
                    None => None,
                };
                Some((main, part))
            }).collect::<Option<Vec<_>>>()?;
            image.add_bytes(6, crate::crate_image::encode(&files));
            image.add_owned(7, &self.fps.iter().map(|&k| MappedKey::new(k)).collect::<Vec<_>>());
            if stop() { return None; }
            // One replaceable entry per process, in a namespace boot never reads.
            static NONCE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
            let mut h = Sip128::new();
            h.str("collected-swap-v1"); h.u128(front_graph_key(GraphRecipe::new(pre).identity));
            h.u64(*NONCE.get_or_init(crate::pack::nonce));
            let key = h.finish128();
            let dest = self.dir.join(format!("collected_swap-{:016x}.rcs", key as u64));
            Some(CollectedRecord::Ready((dest, key), image))
        })();
        recorded.unwrap_or_else(|| {
            if stop() { return CollectedRecord::Yielded; }
            if crate::preload::trace() { eprintln!("makepad-run collected: swap not recorded: {why}"); }
            CollectedRecord::Failed
        })
    }

    fn encode_collected_as(&self, prog: &Program, tcx: &Tcx, stop: &dyn Fn() -> bool, fresh: bool) -> CollectedRecord {
        if !enabled() || self.mode == Mode::ReadOnly { return CollectedRecord::Failed; }
        if self.collected_committed && !fresh { return CollectedRecord::Current; }
        let mut why = "recipe";
        let mut yielded = false;
        let recorded = (|| {
        let frozen = self.collected_boot.as_ref().filter(|_| !fresh);
        let recipe = frozen.map(|boot| &boot.recipe).or(prog.front_graph.as_ref())?;
        why = "graph certificate";
        let graph = read_front_graph(&self.dir, recipe.identity)?;
        why = "fingerprints";
        if graph.crates.iter().map(|(key, _)| key).ne(recipe.crates.iter())
            || graph.crates.iter().map(|(_, fp)| fp).ne(self.fps.iter()) { return None; }
        let mut check = || { yielded = yielded || stop(); (!yielded).then_some(()) };
        let captured;
        let boot = match frozen {
            Some(boot) => boot,
            None => { captured = self.capture_collected_boot(prog, tcx, stop, &mut why)?; &captured }
        };
        check()?;
        let mut image = crate::image::Writer::default();
        image.add_bytes(0, certificate(&graph).to_le_bytes().to_vec());
        image.add_image(1, boot.program.clone()); image.add_image(2, boot.types.clone());
        image.add_image(3, boot.definitions.clone());
        why = "index snapshot";
        image.add_bytes(4, MappedIndexes::record(&self.crate_path, &boot.files)?);
        image.add_owned(5, &boot.interfaces);
        check()?;
        let payload = image;
        let dest = path(&self.dir, recipe.identity);
        Some(CollectedRecord::Ready((dest, key(recipe.identity)), payload))
        })();
        match recorded {
            Some(r) => r,
            None if yielded || stop() => CollectedRecord::Yielded,
            None => {
                if crate::preload::trace() {
                    eprintln!("makepad-run collected: not recorded: {} (errors {})", why, prog.errors.len());
                }
                CollectedRecord::Failed
            }
        }
    }

    /// `report`: the crate count to print (verbose). The entry as the store now holds it.
    pub fn commit_collected(report: Option<usize>, (dest, key): (PathBuf, u128), payload: crate::image::Writer<'static>) -> Option<CollectedWritten> {
        // A swap can reference runtime files whose images have no crate-store spill.
        front_run::commit_marks(&front_run::run_marks()).ok()?;
        // Hash/verify outside the compiler locks, alongside the store write. The
        // immutable snapshot is already complete; a current store needs no replacement.
        let (len, sum) = crate::store::entry_sum(&CollectedEntry { key, payload: &payload })?;
        let written = CollectedWritten { dest: dest.clone(), key, len, sum };
        if Store::open(&dest).is_some_and(|s| s.matches_sum(key, KIND_COLLECTED, len, sum)) { return Some(written); }
        if crate::store::append(&dest, vec![CollectedEntry { key, payload: &payload }], true).is_err() {
            if crate::preload::trace() { eprintln!("makepad-run collected: not recorded: write"); }
            return None;
        }
        if let Some(n) = report { eprintln!("makepad-run collected: recorded {} crates", n); }
        Some(written)
    }

    /// The running program's collected tables become views of the image this run wrote
    /// (`written`): a cold run holds what a warm restore would map. Tables that were mutated
    /// since the image was encoded stay as they are. The freed heap tables go to `retired`
    /// (drop them outside the compiler locks); `note` hears each table's outcome. Both the
    /// unit and the front-end write locks are held. None: the image on disk is not the one
    /// written (or unreadable), nothing was changed.
    pub(crate) fn adopt_collected(&self, prog: &mut Program, tcx: &mut Tcx, written: &CollectedWritten,
        retired: &mut Vec<Box<dyn Send>>, mut note: impl FnMut(&'static str, crate::collected::Adopt)) -> Option<()> {
        let st = Arc::new(Store::open(&written.dest)?);
        let at = st.index.binary_search_by(|e| { let k = e.key; k.cmp(&written.key) }).ok()?;
        let ix = st.index[at];
        if { ix.len } != written.len || { ix.sum } != written.sum || { ix.kind } != KIND_COLLECTED { return None; }
        let (_, bytes) = st.get_at(at, written.key)?;
        // the mapping is pinned for the rest of the process: the tables point into it
        FRONT_STORES.lock().unwrap_or_else(|e| e.into_inner()).push(st.clone());
        FRONT_USED.lock().unwrap_or_else(|e| e.into_inner()).push((st.clone(), at));
        let bytes: &'static [u8] = unsafe { std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()) };
        let image = crate::image::Reader::new(bytes)?;
        if image.len() != 6 { return None; }
        let (program, types) = (image.bytes(1, 1)?, image.bytes(2, 2)?);
        prog.adopt_collected(program, retired, &mut note)?;
        tcx.adopt_collected(types, retired, &mut note)?;
        Some(())
    }
}

/// The collected store entry a run wrote: where, and its length and checksum there.
#[derive(Clone)]
pub struct CollectedWritten {
    dest: PathBuf,
    key: u128,
    len: u32,
    sum: u64,
}

pub enum CollectedRecord {
    /// encoded; commit_collected writes it (no lock needed)
    Ready((PathBuf, u128), crate::image::Writer<'static>),
    /// the store already holds this exact image
    Current,
    /// stopped for the main thread: try again later
    Yielded,
    Failed,
}

/// Tables produced by this build and covered by the collected store checksum and input
/// certificate. Only the small section directory is decoded at boot; keys stay mapped.
pub(super) struct MappedDefinitions {
    keys: &'static [MappedKey],
    crates: Vec<(&'static [MappedKey], &'static [u32])>,
}
impl MappedDefinitions {
    fn read(bytes: &'static [u8], defs: usize, crates: usize) -> Option<Self> {
        let image = crate::image::Reader::new(bytes)?;
        if image.len() != 1 + crates * 2 { return None; }
        let keys: &[MappedKey] = image.slice(0, 0)?;
        if keys.len() != defs { return None; }
        let mut tables = Vec::with_capacity(crates);
        for c in 0..crates {
            let ks: &[MappedKey] = image.slice(1 + c * 2, 1 + c as u32 * 2)?;
            let ds: &[u32] = image.slice(2 + c * 2, 2 + c as u32 * 2)?;
            if ks.len() != ds.len() { return None; }
            tables.push((ks, ds));
        }
        Some(Self { keys, crates: tables })
    }
    pub(super) fn key(&self, d: DefId) -> Option<u128> { self.keys.get(d.0 as usize).map(|k| k.get()) }
    pub(super) fn definition(&self, c: u32, key: u128) -> Option<DefId> {
        let (keys, defs) = self.crates.get(c as usize)?;
        let d = *defs.get(find_key(keys, key)?)?;
        ((d as usize) < self.keys.len()).then_some(DefId(d))
    }
}

/// `key`'s position in `keys` (sorted; definition keys are hashes, so evenly spread): a few
/// interpolation steps on the top 64 bits narrow the range, a binary search ends it -- ~4 probes
/// near the key instead of ~16 spread over the table (one per prelinked definition at the start).
fn find_key(keys: &[MappedKey], key: u128) -> Option<usize> {
    let (mut lo, mut hi) = (0usize, keys.len());
    let k = (key >> 64) as u64;
    for _ in 0..4 {
        if hi - lo <= 16 { break; }
        let a = (keys[lo].get() >> 64) as u64;
        let b = (keys[hi - 1].get() >> 64) as u64;
        if k < a || k > b { return None; }
        if a == b { break; }
        let g = lo + ((k - a) as u128 * (hi - 1 - lo) as u128 / (b - a) as u128) as usize;
        match keys[g].get().cmp(&key) {
            std::cmp::Ordering::Equal => return Some(g),
            std::cmp::Ordering::Less => lo = g + 1,
            std::cmp::Ordering::Greater => hi = g,
        }
    }
    keys[lo..hi].binary_search_by_key(&key, |k| k.get()).ok().map(|i| lo + i)
}

// The containing image/store guarantees 8-byte alignment, including on x86 where u128
// itself requires 16. Store the two words without padding and compare as an integer.
#[derive(Clone, Copy)]
#[repr(C)]
struct MappedKey { lo: u64, hi: u64 }
unsafe impl crate::image::Pod for MappedKey {}
impl MappedKey {
    fn new(k: u128) -> Self { Self { lo: k as u64, hi: (k >> 64) as u64 } }
    fn get(self) -> u128 { self.lo as u128 | ((self.hi as u128) << 64) }
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Location { store: u32, entry: u32 }
unsafe impl crate::image::Pod for Location {}

/// A program file's entries in its crate store's index snapshot (part: u32::MAX if none).
#[derive(Clone, Copy)]
#[repr(C)]
struct FileLocation { main: u32, part: u32 }
unsafe impl crate::image::Pod for FileLocation {}

#[derive(Clone)]
pub(super) struct MappedIndexes {
    pub(super) stores: Vec<Arc<Store>>,
    keys: &'static [MappedKey],
    locations: &'static [Location],
    files: &'static [FileLocation],
}
impl MappedIndexes {
    fn record(paths: &[PathBuf], files: &[(u32, String, u128)]) -> Option<Vec<u8>> {
        let mut image = crate::image::Writer::default();
        let mut metadata = Vec::new();
        let mut entries = std::collections::BTreeMap::new();
        let mut opened = Vec::with_capacity(paths.len());
        paths.len().put(&mut metadata);
        for (si, path) in paths.iter().enumerate() {
            opened.push(Store::open(path)?);
            let store = opened.last()?;
            path.file_name()?.to_str()?.to_string().put(&mut metadata);
            store.generation.put(&mut metadata);
            image.add_owned(si as u32, &store.index);
            for (ei, e) in store.index.iter().enumerate() {
                if e.kind != KIND_LEDGER && e.kind != KIND_MANIFEST {
                    entries.insert(e.key, Location { store: si as u32, entry: ei as u32 });
                }
            }
        }
        let (keys, locations): (Vec<_>, Vec<_>) = entries.into_iter().map(|(k, loc)| (MappedKey::new(k), loc)).unzip();
        let find = |store: &Store, key: u128| store.index.binary_search_by_key(&key, |e| e.key).ok().map(|i| i as u32);
        let files = files.iter().map(|(krate, path, hash)| {
            let store = opened.get(*krate as usize)?;
            Some(FileLocation { main: find(store, Cache::front_key(path, *hash))?,
                part: find(store, Cache::front_part_key(path, *hash)).unwrap_or(u32::MAX) })
        }).collect::<Option<Vec<_>>>()?;
        image.add_bytes(paths.len() as u32, metadata);
        image.add_owned(paths.len() as u32 + 1, &keys);
        image.add_owned(paths.len() as u32 + 2, &locations);
        image.add_owned(paths.len() as u32 + 3, &files);
        Some(image.finish())
    }
    fn read(bytes: &'static [u8], dir: &std::path::Path) -> Option<Self> {
        let image = crate::image::Reader::new(bytes)?;
        let n = image.len().checked_sub(4)?;
        let mut r = crate::crate_image::Decode::new(image.bytes(n, n as u32)?);
        if usize::get(&mut r)? != n { return None; }
        let mut stores = Vec::with_capacity(n);
        for i in 0..n {
            let name = String::get(&mut r)?;
            if !crate::store::name_ours(&name) { return None; }
            let generation = u64::get(&mut r)?;
            stores.push(Arc::new(Store::open_certified(&dir.join(name), image.slice(i, i as u32)?, generation)?));
        }
        let keys: &[MappedKey] = image.slice(n + 1, n as u32 + 1)?;
        let locations: &[Location] = image.slice(n + 2, n as u32 + 2)?;
        let files: &[FileLocation] = image.slice(n + 3, n as u32 + 3)?;
        if !r.bytes.is_empty() || keys.len() != locations.len() { return None; }
        Some(Self { stores, keys, locations, files })
    }
    pub(super) fn get(&self, key: u128) -> Option<(u32, u32)> {
        let loc = self.locations.get(find_key(self.keys, key)?)?;
        Some((loc.store, loc.entry))
    }
    pub(super) fn len(&self) -> usize { self.keys.len() }
}

struct CollectedEntry<'a> {
    key: u128,
    payload: &'a crate::image::Writer<'static>,
}
impl crate::store::Entry for CollectedEntry<'_> {
    fn key(&self) -> u128 { self.key }
    fn kind(&self) -> u32 { KIND_COLLECTED }
    fn write_payload(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        self.payload.write_to(out)
    }
}
