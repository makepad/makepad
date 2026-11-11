//! A collected Program/Tcx above the immutable per-crate front images. Its certificate
//! covers the ordered unit graph, all authored inputs, module probes and late inputs.
use super::*;
use crate::crate_image::{Codec, CrateImages, GraphRecipe};
use crate::program::Program;
use crate::tcx::Tcx;
use std::sync::Arc;

const KIND_COLLECTED: u32 = 9;

/// Proof passed from a boot certificate to Cache::open. Fingerprints cover the cache's crates.
pub struct CollectedCache {
    pub(super) indexes: Option<MappedIndexes>,
    pub(super) definitions: &'static [u8],
    pub(super) fps: Vec<u128>,
    /// the crates' interface fingerprints the recording run's code was keyed with (its inputs
    /// are the certified ones): the cache opens with them instead of digesting every file
    pub(super) ifaces: Vec<u128>,
    pub(super) defs: usize,
    pub(super) files: usize,
    /// a partial restore (an edited start): the crates and definitions it kept of the image --
    /// only their definition tables (`definitions`, of `defs` definitions and `fps.len()`
    /// crates) hold for this program
    pub(super) kept: Option<(usize, usize)>,
}

impl CollectedCache {
    /// The cache opens for the first `n` crates only: the ones a boot loaded (crates added at run
    /// time have no fingerprint in the cache, so nothing of theirs is cached; their front entries
    /// are not in its index).
    pub(crate) fn boot_crates(&mut self, n: usize) {
        self.fps.truncate(n);
        self.ifaces.truncate(n);
        if let Some(ix) = &mut self.indexes { ix.stores.truncate(n); }
    }
}

/// MAKEPAD_RUN_COLLECTED_TRACE: the collected encode's parts, timed.
struct Parts(Option<(std::time::Instant, String)>);
impl Parts {
    fn new() -> Self {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        Parts(ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_COLLECTED_TRACE").is_some()).then(|| (std::time::Instant::now(), format!(" at t={:.0} ms:", crate::clock::ms_since_start()))))
    }
    fn lap(&mut self, what: &str, image: &crate::image::Writer<'_>) {
        if let Some((t, line)) = &mut self.0 {
            let (rss, _, extra) = crate::memstat::process_rss();
            line.push_str(&format!(" {what} {:.1} ms ({:.1} MB, {:.1} MB owned; rss {} MB, {});", t.elapsed().as_secs_f64() * 1e3,
                image.len() as f64 / 1048576.0, image.owned_len() as f64 / 1048576.0, rss >> 20, extra.split(';').next().unwrap_or("")));
            *t = std::time::Instant::now();
        }
    }
}
impl Drop for Parts {
    fn drop(&mut self) {
        if let Some((_, line)) = &self.0 { if line.contains(';') { eprintln!("makepad-run collected: encode parts{line}"); } }
    }
}
/// `dirty` grown by the crates that load again with a dirty one: the ones depending on it and the
/// ones of its name (a lib and its bin share a name: collected_prefix keeps neither when one
/// changes).
fn with_dependents(pre: &[crate::preload::PreCrate], dirty: &mut [bool]) {
    loop {
        let names: std::collections::HashSet<&str> = pre.iter().zip(dirty.iter()).filter(|(_, d)| **d).map(|(c, _)| c.name.as_str()).collect();
        let mut changed = false;
        for j in 0..pre.len() {
            if !dirty[j] && (names.contains(pre[j].name.as_str()) || pre[j].image_deps.iter().any(|&k| dirty[k])) { dirty[j] = true; changed = true; }
        }
        if !changed { break; }
    }
}

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
/// `'p`: the program and types sections borrow the arrays of the program they were captured
/// from (`'static` once frozen: CollectedBoot::into_owned).
pub(super) struct CollectedBoot<'p> {
    recipe: GraphRecipe,
    program: Arc<crate::image::Writer<'p>>,
    types: Arc<crate::image::Writer<'p>>,
    definitions: Arc<crate::image::Writer<'static>>,
    files: Vec<(u32, String, u128)>,
    interfaces: Vec<MappedKey>,
}

impl CollectedBoot<'_> {
    /// The boot with its borrowed arrays copied: it outlives the program's locks. None: `stop`
    /// said yes between two copies (image::Writer::into_owned).
    fn into_owned(self, stop: &dyn Fn() -> bool) -> Option<CollectedBoot<'static>> {
        let own = |w: Arc<crate::image::Writer<'_>>| Some(Arc::new(Arc::unwrap_or_clone(w).into_owned(stop)?));
        Some(CollectedBoot { recipe: self.recipe, program: own(self.program)?, types: own(self.types)?,
            definitions: self.definitions, files: self.files, interfaces: self.interfaces })
    }
}

impl FrontImages {
    // An edit normally invalidates a leaf or a dependent suffix. Keep the complete
    // certified prefix resident, including its resolved imports and collected types.
    /// Each crate's recorded front record as the collected prefix needs it: None when its
    /// store, record, inputs, probes or late inputs no longer hold; else the demand selection
    /// (identity, deferred files) it was recorded under, to compare with this load's.
    /// `opened`: the graph's crate stores, already open (in graph order).
    fn prefix_records(&self, graph: &crate::crate_image::GraphImage, pre: &[crate::preload::PreCrate], opened: Option<&[Arc<Store>]>) -> Option<Vec<Result<(u128, Vec<String>), &'static str>>> {
        let recipe = GraphRecipe::new(pre);
        if graph.identity != recipe.identity || graph.crates.len() != pre.len()
            || graph.crates.iter().zip(&recipe.crates).any(|((key, _), want)| key != want) { return None; }
        let present = pre.iter().map(|c| c.name.clone()).collect();
        let mut late = HashMap::new();
        Some(pre.iter().zip(&graph.crates).enumerate().map(|(i, (c, (_, fp)))| {
            let reopened;
            let store = match opened.and_then(|o| o.get(i)) {
                Some(store) => &**store,
                None => { reopened = Store::open(&self.dir.join(front_store_name(&c.cfg.session.options, &c.name, *fp))).ok_or("store")?; &reopened }
            };
            if self.reloads(&c.name, &c.root) { return Err("reloaded (a checked crate)"); }
            let (kind, bytes) = store.get(front_crate_key(&c.name, &c.root)).ok_or("record")?;
            if kind != KIND_FRONT_CRATE { return Err("record"); }
            // (its head: the inputs and the selection, not its tables and file images)
            let rec = crate::crate_image::decode_record_head(bytes).ok_or("record")?;
            let inputs: Vec<_> = rec.inputs.iter().chain(&rec.files).map(|(p, h)| (p.as_str(), *h)).collect();
            if !(self.inputs_current(&inputs) && rec.probes.iter().all(|(p, e)| self.probe_current(p, *e))
                && ledger_current(store, &present, &mut late)) { return Err("inputs"); }
            Ok((rec.demand_plan, rec.deferred))
        }).collect())
    }

    /// The demand plan `graph` recorded, when this load can take it as it is (DemandPlan::carried):
    /// for these crates, it selected every file of the last crate (it deferred nothing: the load
    /// takes every crate in full, which is always sound), and its module probes hold. Edits of
    /// file contents do not plan again; a module file that comes or goes does (a selection can
    /// become partial again only after such a change or a load without a plan). A reordered load
    /// (`base`: the stored graph's crates in its order, load_order) takes the plan with its crate
    /// positions moved to this load's: a plan names crates by position only (its files, its
    /// reached set, its parts; DemandPart::read takes a part at any position).
    pub(super) fn carried_demand(&self, graph: &crate::crate_image::GraphImage, pre: &[crate::preload::PreCrate], base: Option<&[crate::preload::PreCrate]>) -> Option<Arc<crate::preload::DemandImage>> {
        let no = |why: &str| { if crate::preload::trace() { eprintln!("makepad-run front: demand plan not carried: {why}"); } None };
        if pre.iter().any(|c| c.cfg.session.runtime.lazy_verify) { return no("verify"); }
        let Some(stored) = graph.demand_image.clone() else { return no("no stored plan") };
        let last = pre.len().checked_sub(1)?;
        if stored.graph != crate::preload::demand_recipe_identity(base.unwrap_or(pre)) || stored.parts.len() != pre.len() { return no("another graph"); }
        let image = match base {
            None => stored,
            Some(base) => {
                // this load's crate k is the stored plan's crate from[k]
                if base.len() != pre.len() { return no("another graph"); }
                let at: HashMap<(&str, &str), usize> = base.iter().enumerate().map(|(j, c)| ((c.name.as_str(), c.root.as_str()), j)).collect();
                let Some(from) = pre.iter().map(|c| at.get(&(c.name.as_str(), c.root.as_str())).copied()).collect::<Option<Vec<usize>>>() else { return no("another crate set") };
                let mut to = vec![0; from.len()];
                for (k, &j) in from.iter().enumerate() { to[j] = k; }
                let mut image = (*stored).clone();
                image.graph = crate::preload::demand_recipe_identity(pre);
                for f in &mut image.files { f.ci = to[f.ci]; }
                image.files.sort_by_key(|f| f.ci);
                image.reached = stored.reached.iter().map(|(ci, p)| (to[*ci], p.clone())).collect();
                image.parts = from.iter().map(|&j| stored.parts[j].clone()).collect();
                Arc::new(image)
            }
        };
        if !image.files.iter().filter(|f| f.ci == last).all(|f| image.reached.contains(&(last, f.path.clone()))) { return no("the last crate was selected in part"); }
        // the module files it chose between are as they were: no module came or went
        if !image.probes.iter().all(|(p, e)| self.probe_current(p, *e)) { return no("a module probe changed"); }
        if crate::preload::trace() && base.is_some() { eprintln!("makepad-run collected: demand plan carried into the reordered load"); }
        Some(image)
    }

    /// prefix_records of `graph` for a load whose first `common` crates are `pre`'s (the
    /// others: "load order"), the ones load_order read when it read this graph.
    fn records_for(&self, graph: &crate::crate_image::GraphImage, pre: &[crate::preload::PreCrate], common: usize, opened: Option<&[Arc<Store>]>) -> Option<PrefixRecords> {
        let stashed = self.records.lock().unwrap().take().filter(|(identity, _)| *identity == graph.identity);
        let mut records = match stashed { Some((_, r)) => r, None => self.prefix_records(graph, pre, opened)? };
        for r in records.iter_mut().skip(common) { *r = Err("load order"); }
        Some(records)
    }

    // An edit normally invalidates a leaf or a dependent suffix. Keep the complete
    // certified prefix resident, including its resolved imports and collected types.
    // `selection(ci)`: this load's demand selection of crate ci, None while it is not known yet.
    // Ok((prefix, unknown)): `unknown` when the prefix ends at a crate whose selection is not
    // known yet (a later look may find it longer). `trace`: MAKEPAD_RUN_BOOT_TRACE names the prefix.
    fn collected_prefix(pre: &[crate::preload::PreCrate], records: &[Result<(u128, Vec<String>), &'static str>],
        selection: &dyn Fn(usize) -> Option<(u128, Vec<String>)>, trace: bool) -> Option<(usize, bool)> {
        let mut dirty = Vec::new();
        let mut unknown = false;
        let mut first_why = "";
        for (ci, c) in pre.iter().enumerate() {
            let why = match &records[ci] {
                Err(why) => Some(*why),
                Ok(recorded) => match selection(ci) {
                    None => Some("selection not known yet"),
                    Some(now) => (now != *recorded).then_some("demand selection"),
                },
            };
            let dep_dirty = c.image_deps.iter().any(|d| dirty.get(*d).copied().unwrap_or(true));
            if (why.is_some() || dep_dirty) && first_why.is_empty() {
                first_why = why.unwrap_or("a dependency");
                unknown = why == Some("selection not known yet");
            }
            dirty.push(why.is_some() || dep_dirty);
        }
        let mut first = dirty.iter().position(|d| *d)?;
        // A package's lib and bin share their name (cargo names both after the package), and the
        // macro table keys its per-crate state by name (the bin's load moves the lib's modules to
        // `<name>\u{1}lib` and replaces the rest): a retained crate named as a collected-again one
        // cannot be told apart from it there, so the suffix starts at the first such crate (an app
        // edit in its bin collects its lib again too, not every crate below it).
        while let Some(i) = pre[..first].iter().position(|c| pre[first..].iter().any(|d| d.name == c.name)) { first = i; }
        if trace {
            eprintln!("makepad-run collected: prefix {} of {} crates, ends at {} ({}{})", first, pre.len(), pre[dirty.iter().position(|d| *d)?].name, first_why, if unknown { "; waiting" } else { "" });
        }
        // Every crate from the first changed one on is collected again: changed ones parse,
        // clean ones load from their crate images (proc-macro and build crates load after the
        // app's, so an app edit leaves clean crates behind it). The std crates may depend on
        // one another despite their load order: a change there uses the ordinary path.
        (first >= crate::std_pre_crates(&pre[0].cfg.session, false).len()).then_some((first, unknown))
    }

    /// How many leading crates of `pre` a restore keeps when every record holds but the
    /// reloaded crates' (reload_crates): collected_prefix with only those refused, so the crates
    /// depending on them and the ones named as them load again too.
    pub fn reload_prefix(&self, pre: &[crate::preload::PreCrate]) -> usize {
        let records: PrefixRecords = pre.iter().map(|c| if self.reloads(&c.name, &c.root) { Err("reloaded (a checked crate)") } else { Ok((0, Vec::new())) }).collect();
        match Self::collected_prefix(pre, &records, &|_| Some((0, Vec::new())), false) {
            Some((prefix, _)) => prefix,
            // (nothing loads again, or the std crates would)
            None if !records.iter().any(Result::is_err) => pre.len(),
            None => 0,
        }
    }

    /// The load order for `pre` (std first, every crate after its dependencies), as positions
    /// in `pre`: the order the last collected image of these crates was recorded in, with the
    /// crates whose records no longer hold, the crates of their names and every crate depending
    /// on them moved to its end (in that order). The restore keeps every crate before them
    /// (restore_collected, BaseOrder), and the next edit of the same crates finds them last
    /// already: an edit of a library collects it and its dependents again, not every crate a
    /// dependency order happened to put after it. With no stored order, a check's reloaded crates
    /// and their dependents last (reload_crates). None: `pre`'s own order.
    pub fn load_order(&self, pre: &[crate::preload::PreCrate], std_count: usize) -> Option<Vec<usize>> {
        let recipe = GraphRecipe::new(pre);
        let Some(stored) = read_front_order(&self.dir, recipe.set) else {
            // a first check: the crates that load again at every check (the reloaded ones and
            // the ones depending on them) go last from the start, so the image it records is
            // the one the next check restores
            if !self.reloading() { return None; }
            let mut dirty: Vec<bool> = pre.iter().map(|c| self.reloads(&c.name, &c.root)).collect();
            with_dependents(pre, &mut dirty);
            let moved: Vec<usize> = (0..pre.len()).filter(|&j| !dirty[j]).chain((0..pre.len()).filter(|&j| dirty[j])).collect();
            return (moved.iter().take(std_count).copied().eq(0..std_count) && moved.iter().enumerate().any(|(j, k)| j != *k)).then_some(moved);
        };
        let at: HashMap<_, usize> = pre.iter().enumerate().map(|(i, c)| (c.key(), i)).collect();
        let order: Vec<usize> = stored.iter().map(|key| at.get(key).copied()).collect::<Option<_>>()?;
        if !order.iter().copied().take(std_count).eq(0..std_count) { return None; }
        let base = crate::preload::reorder(pre, &order)?;
        self.begin_load(&base);
        if self.certificate.lock().unwrap().is_some() && !self.reloading() { return Some(order); }
        let graph = self.graph_read.lock().unwrap().clone()?;
        let records = self.prefix_records(&graph, &base, None)?;
        let mut dirty: Vec<bool> = records.iter().map(Result::is_err).collect();
        with_dependents(&base, &mut dirty);
        let moved: Vec<usize> = (0..base.len()).filter(|&j| !dirty[j]).chain((0..base.len()).filter(|&j| dirty[j])).collect();
        let common = moved.iter().enumerate().take_while(|(j, k)| j == *k).count();
        if common == base.len() {
            *self.records.lock().unwrap() = Some((graph.identity, records));
            return Some(order);
        }
        let reordered: Vec<usize> = moved.iter().map(|&j| order[j]).collect();
        let identity = GraphRecipe::new(&crate::preload::reorder(pre, &reordered)?).identity;
        if crate::preload::trace() {
            eprintln!("makepad-run collected: load order keeps {} of {} crates in place, {} changed or dependent ones last ({})", common, base.len(), dirty.iter().filter(|d| **d).count(),
                (0..base.len()).filter(|&j| dirty[j]).map(|j| base[j].name.as_str()).collect::<Vec<_>>().join(" "));
        }
        // that order recorded before: its own graph decides (begin_load)
        if read_front_graph(&self.dir, identity).is_some() { return Some(reordered); }
        *self.records.lock().unwrap() = Some((graph.identity, records));
        *self.base.lock().unwrap() = Some(BaseOrder { set: recipe.set, identity: graph.identity, pre: base, common });
        // the reordered load begins here, with the inputs validated against the stored graph
        *self.begun.lock().unwrap() = Some(identity);
        Some(reordered)
    }

    /// The stored graph a load of `pre` restores from, the crates in its order and how many
    /// leading ones of `pre` it describes: `pre`'s own, else a reordered load's base (load_order).
    fn graph_for(&self, pre: &[crate::preload::PreCrate]) -> Option<(Arc<crate::crate_image::GraphImage>, Option<(Vec<crate::preload::PreCrate>, usize)>)> {
        let recipe = GraphRecipe::new(pre);
        if let Some(g) = self.front_graph(recipe.identity) { return Some((g, None)); }
        let base = self.base.lock().unwrap();
        let base = base.as_ref().filter(|b| b.set == recipe.set)?;
        Some((self.front_graph(base.identity)?, Some((base.pre.clone(), base.common))))
    }

    /// The program and type context restored from the collected image of `pre`'s graph. Err:
    /// which check refused it (the settle rebuild's deferral line and the boot's trace name it).
    pub fn restore_collected(&self, session: &Arc<crate::Session>, pre: &[crate::preload::PreCrate]) -> Result<(Program, Tcx, Option<CollectedCache>), String> {
        let _profile = crate::load_profile::span(session.runtime.profile, "front-collected-restore", "main");
        // This handoff is scoped to one immutable load, never a later edit.
        self.pending_plan.lock().unwrap().take();
        let mut why = "symbol perturbation on";
        // (the crate or file the refusing check was about)
        let mut detail = String::new();
        let started = std::time::Instant::now();
        let phase = |name: &str| if crate::preload::trace() {
            eprintln!("makepad-run collected: {} at {:.1} ms", name, started.elapsed().as_secs_f64() * 1e3);
        };
        let restored = (|| {
        if crate::syms::perturb(&session.options) != 0 { return None; }
        self.begin_load(pre);
        why = "certificate lock poisoned";
        let cert = self.certificate.lock().ok()?;
        let stale;
        // a reordered load's base graph: its crates in its order, of which `common` lead `pre` too
        let mut base = None;
        let mut plan = None;
        // the prefix whose contexts' prescans the plan worker releases once it is done
        let release_after: Arc<relax_session::sync::Mutex<Option<usize>>> = Arc::default();
        // (crates to load again: the prefix before them, as for an edit)
        let (graph, mut prefix) = match cert.as_ref().filter(|_| !self.reloading()) {
            Some(graph) => (graph, pre.len()),
            None => {
                why = "no stored graph of this crate set";
                let (graph, b) = self.graph_for(pre)?;
                stale = graph;
                base = b;
                if !session.options.source_eager && session.options.rs_lazy {
                    let contexts = crate::source_context::Context::for_crates(pre, Arc::default(), session);
                    if let Some(image) = self.carried_demand(&stale, pre, base.as_ref().map(|(p, _)| p.as_slice())) {
                        plan = Some(Arc::new(crate::preload::DemandPlan::carried(pre, contexts, image)));
                    } else {
                        // Restore the immutable Program/Tcx while the demand worker
                        // computes the new selection. Nothing is retained or exposed
                        // to collection until that selection has been validated.
                        let pending = Arc::new(crate::preload::DemandPlan::new(pre, contexts, Some(self), true));
                        *self.pending_plan.lock().unwrap() = Some((GraphRecipe::new(pre).identity, pending.clone()));
                        let worker = pending.clone();
                        let in_world = crate::macro_reg::in_world();
                        let release = release_after.clone();
                        // (detached: the restore takes its prefix as soon as the plan's early
                        // selection covers it, and the load goes on while the plan completes)
                        why = "demand worker not started";
                        crate::run_threads::spawn("run-demand", move || {
                            crate::macro_reg::set_in_world(in_world);
                            worker.complete();
                            let mut r = release.lock().unwrap_or_else(|e| e.into_inner());
                            if let Some(prefix) = r.take() { for context in &worker.contexts()[..prefix] { context.release_prescan(); } }
                        }).ok()?;
                        plan = Some(pending);
                    }
                }
                let prefix = if plan.is_some() { 0 } else {
                    why = "the graph's crate records";
                    let (rpre, common) = base.as_ref().map_or((pre, pre.len()), |(p, n): &(Vec<_>, usize)| (p.as_slice(), *n));
                    Self::collected_prefix(rpre, &self.records_for(&stale, rpre, common, None)?, &|_| Some((0, Vec::new())), crate::preload::trace())?.0
                };
                (&stale, prefix)
            }
        };
        let (rpre, common) = base.as_ref().map_or((pre, pre.len()), |(p, n)| (p.as_slice(), *n));
        phase("validated");
        why = "collected store missing or unreadable";
        let st = Arc::new(Store::open(&path(&self.dir, graph.identity))?);
        // Like a front image: structure only here (bounds; the image reader checks every
        // section), the checksum in the background (verify_front_images), so the boot reads
        // only the pages of the tables it uses, not the whole image.
        why = "no collected entry in its store";
        let at = st.index.binary_search_by(|e| { let k = e.key; k.cmp(&key(graph.identity)) }).ok()?;
        why = "collected entry unreadable";
        let (kind, bytes) = st.get_at(at, key(graph.identity))?;
        why = "collected entry of another kind";
        if kind != KIND_COLLECTED { return None; }
        pin_front(&st, Some(at));
        // The checked, immutable mapping is pinned for the Program's mapped definition arena.
        let bytes: &'static [u8] = unsafe { std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()) };
        why = "collected image header";
        let image = crate::image::Reader::new(bytes)?;
        why = "collected image sections (format)";
        if image.len() != 6 { return None; }
        why = "certificate mismatch (the crate graph changed since the image)";
        if image.bytes(0, 0)? != certificate(graph).to_le_bytes() { return None; }
        why = "interface count differs from the graph's crates";
        let ifaces: &[MappedKey] = image.slice(5, 5)?;
        if ifaces.len() != graph.crates.len() { return None; }
        why = "collected image sections (format)";
        let program = image.bytes(1, 1)?;
        let types = image.bytes(2, 2)?;
        let definitions = image.bytes(3, 3)?;
        phase("mapped");
        let options = crate::crate_image::parse_options(&session.options);
        let store_paths: Vec<PathBuf> = graph.crates.iter()
            .map(|((name, _, _), fp)| self.dir.join(front_store_name_with(options, name, *fp))).collect();
        let indexes = MappedIndexes::read(image.bytes(4, 4)?, &self.dir).filter(|ix| {
            ix.stores.len() == graph.crates.len()
                && ix.stores.iter().zip(&store_paths).all(|(s, path)| s.path.as_os_str() == path.as_os_str())
        });
        let mut stores = Vec::new();
        let mut late = HashMap::new();
        let present = graph.crates.iter().map(|((name, _, _), _)| name.clone()).collect();
        for (i, store_path) in store_paths.iter().enumerate() {
            detail = graph.crates[i].0 .0.clone();
            why = "crate store missing or unreadable";
            let store = indexes.as_ref().and_then(|ix| ix.stores.get(i)).cloned()
                .or_else(|| Store::open(store_path).map(Arc::new))?;
            why = "late inputs changed (crate ledger)";
            if i < prefix && !ledger_current(&store, &present, &mut late) { return None; }
            stores.push(store);
        }
        detail.clear();
        phase("crate stores");
        why = "Program";
        // (the first file whose front image did not restore)
        let mut failed_file: Option<String> = None;
        let mut file_times = [std::time::Duration::ZERO; 3];
        let timed = crate::preload::trace();
        let mut lap = |slot: usize, at: &mut Option<std::time::Instant>| if let Some(t) = at {
            let now = std::time::Instant::now();
            file_times[slot] += now - *t;
            *t = now;
        };
        let prog = Program::restore_collected(session, program, rpre, |f, k, cfg, meta| {
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
            if restored.is_none() {
                if crate::preload::trace() { eprintln!("makepad-run collected: file not restored: {}", meta.path); }
                failed_file.get_or_insert_with(|| meta.path.to_string());
            }
            restored
        });
        let Some(mut prog) = prog else {
            if let Some(f) = failed_file {
                why = "a file's front image did not restore";
                detail = f;
            }
            return None;
        };
        if crate::preload::trace() {
            eprintln!("makepad-run collected: files lookup {:.1}, restore {:.1}, parts {:.1} ms",
                file_times[0].as_secs_f64() * 1e3, file_times[1].as_secs_f64() * 1e3, file_times[2].as_secs_f64() * 1e3);
        }
        phase("Program");
        prog.front_sink = self.sink();
        why = "crate count differs from the graph's";
        if prog.crates.len() != graph.crates.len() { return None; }
        why = "crate identity differs from the graph's";
        if let Some(c) = prog.crates.iter().zip(&graph.crates)
            .find(|(c, (key, _))| crate::preload::crate_key(prog.syms.str(c.name), c.root_path(), &c.cfg) != *key) {
            detail = prog.syms.str(c.0.name).to_string();
            return None;
        }
        why = "type context (Tcx)";
        let mut tcx = Tcx::restore_collected(types, prog.files.len(), prog.crates.len())?;
        phase("Tcx");
        if let Some(plan) = plan.as_deref() {
            why = "demand selection";
            let records = self.records_for(graph, rpre, common, Some(&stores))?;
            phase("prefix records");
            // the plan's selection where it is complete, else its early prefix (crates selected
            // in full: identity 0, nothing deferred; preload resolve_images reads it the same way)
            prefix = loop {
                let (ready, seen) = (plan.ready(), plan.early_prefix());
                let selection = |ci: usize| match ready {
                    Some(r) => Some((r.selection_identity(ci), r.deferred(ci))),
                    None => (ci < seen).then(Vec::new).map(|d| (0, d)),
                };
                let (p, unknown) = Self::collected_prefix(rpre, &records, &selection, crate::preload::trace())?;
                if !unknown || ready.is_some() { break p; }
                plan.wait_progress(seen);
            };
            phase("prefix chosen");
            why = "late inputs changed (crate ledger, demand selection)";
            for (i, store) in stores.iter().take(prefix).enumerate() {
                detail = graph.crates[i].0 .0.clone();
                if !ledger_current(store, &present, &mut late) { return None; }
            }
            detail.clear();
            phase("selection validated");
        }
        why = "front ledger replay";
        for (i, store) in stores.iter().take(prefix).enumerate() {
            detail = graph.crates[i].0 .0.clone();
            replay_front_ledger(store)?;
        }
        detail.clear();
        phase("ledger replayed");
        prog.front_graph = Some(GraphRecipe { base: base.is_some().then_some(graph.identity), ..GraphRecipe::new(pre) });
        let image_defs = prog.defs.len();
        let mut kept = None;
        if prefix != pre.len() {
            why = "retain prefix";
            let first = prog.retain_collected_prefix(prefix)?;
            kept = Some((prefix, first));
            phase("Program prefix retained");
            tcx.retain_collected_prefix(first);
            phase("Tcx prefix retained");
            if session.options.verbose { eprintln!("makepad-run collected: retained {} crates; rebuilding {} changed/dependent crates", prefix, pre.len() - prefix); }
            // the crates loaded after them find these as their dependencies (full_deps)
            *self.retained.lock().unwrap() = graph.crates[..prefix].iter().map(|((name, _, _), fp)| (name.clone(), *fp)).collect();
            *self.retained_proc.lock().unwrap() = graph.crates[..prefix].iter().enumerate().filter(|(k, _)| prog.crates.get(*k).is_some_and(|c| c.proc_macro))
                .map(|(_, ((name, _, _), fp))| (name.clone(), *fp)).collect();
        }
        if let Some(plan) = plan {
            self.pending_plan.lock().unwrap().take();
            // Parsing the remaining crates reuses these exact source snapshots,
            // lowerings and full-graph selection (including retained dependencies).
            why = "demand contexts";
            {
                // the retained crates' prescans go once the plan no longer reads them
                let mut r = release_after.lock().unwrap_or_else(|e| e.into_inner());
                if plan.ready().is_some() { for context in &plan.contexts()[..prefix] { context.release_prescan(); } } else { *r = Some(prefix); }
            }
            prog.sources = plan.contexts().first()?.sources().clone();
            prog.demand_plan = Some(plan);
        }
        let cache = Some(CollectedCache {
            indexes: indexes.filter(|_| kept.is_none()), definitions, fps: graph.crates.iter().map(|(_, fp)| *fp).collect(),
            ifaces: ifaces.iter().map(|k| k.get()).collect(),
            defs: if kept.is_some() { image_defs } else { prog.defs.len() }, files: prog.files.len(), kept,
        });
        phase("demand contexts");
        drop(cert);
        self.end_load();
        phase("restored");
        if session.options.verbose {
            eprintln!("makepad-run collected: restored {} crates, {} definitions, {} types", prog.crates.len(), prog.defs.len(), tcx.tys.len());
        }
        Some((prog, tcx, cache))
        })();
        restored.ok_or_else(|| {
            let why = if detail.is_empty() { why.to_string() } else { format!("{why} ({detail})") };
            if crate::preload::trace() || session.runtime.profile {
                eprintln!("makepad-run collected: not restored: {}; demand plan retained={}", why, self.pending_plan.lock().unwrap().is_some());
            }
            why
        })
    }

}

/// What the collected encode reads of a Cache: `Cache::encoder` borrows the cache's own fields
/// (the owner's records), `CollectedInputs::encoder` an owned copy of them.
struct Encoder<'a> {
    mode: Mode,
    dir: &'a std::path::Path,
    fps: &'a [u128],
    iface_plain: &'a [u128],
    crate_path: &'a [PathBuf],
    crate_ident: &'a [u128],
    boot: Option<&'a CollectedBoot<'static>>,
    committed: bool,
    /// the definition tables the cache restored, with the definition count they were restored for
    mapped: Option<(&'a MappedDefinitions, usize)>,
    /// the graph the image is of when it is not the program's own (crates added at run time:
    /// CollectedInputs::with_added)
    recipe: Option<&'a GraphRecipe>,
}



/// The cache fields the collected encode reads, copied in one owner request: the settle rebuild
/// (jit settle_rebuild.rs) then encodes the program on its own thread under a front read. The
/// owner may `take()` the cache meanwhile.
pub(crate) struct CollectedInputs {
    mode: Mode,
    dir: PathBuf,
    fps: Vec<u128>,
    iface_plain: Vec<u128>,
    crate_path: Vec<PathBuf>,
    crate_ident: Vec<u128>,
    mapped: Option<(MappedDefinitions, usize)>,
    recipe: Option<GraphRecipe>,
}

impl CollectedInputs {
    /// The collected image of the program as it is now: neither a committed mark nor a frozen
    /// boot snapshot short-circuits it, so the image covers every type, definition and file
    /// added since the first encode (the settle rebuild restores from it, and compares the counts).
    /// It borrows the program's arrays: commit it while the caller's front read holds them.
    pub(crate) fn encode_now<'p>(&self, prog: &'p Program, tcx: &'p Tcx, stop: &dyn Fn() -> bool) -> CollectedRecord<'p> {
        Encoder { mode: self.mode, dir: &self.dir, fps: &self.fps, iface_plain: &self.iface_plain,
            crate_path: &self.crate_path, crate_ident: &self.crate_ident, boot: None, committed: false,
            mapped: self.mapped.as_ref().map(|(m, n)| (m, *n)), recipe: self.recipe.as_ref() }
            .encode_as(prog, tcx, stop, true, Some)
    }

    /// These inputs for a program whose crates from `boot` on were added at run time
    /// (modules::add_crates: an app domain's), described by `pre`: the image covers them too.
    /// Their fingerprints and interfaces are computed as a boot's (from their files only), their
    /// stores and the graph of `pre` written (record_added_crates). The cache's own tables stay
    /// the boot's (the crate identities: definition keys are the ones it computes). Under the
    /// caller's front read.
    pub(crate) fn with_added(&self, prog: &Program, pre: &[crate::preload::PreCrate], boot: usize) -> Result<CollectedInputs, String> {
        if self.fps.len() != boot || self.iface_plain.len() != boot || self.crate_path.len() != boot || pre.len() != prog.crates.len() || boot > pre.len() {
            return Err("the cache does not describe the boot's crates".into());
        }
        let fps = crate::stable::crate_fingerprints_from(prog, &self.fps);
        let want: Vec<usize> = (0..prog.files.len()).filter(|&f| prog.file_crate.get(f).is_some_and(|&k| k as usize >= boot)).collect();
        let mut digests = vec![None; prog.files.len()];
        super::items::file_digests_of(prog, &want, &mut digests);
        let iface_plain = super::items::crate_ifaces_from(prog, &fps, &digests, &self.iface_plain);
        let recipe = GraphRecipe::new(pre);
        let fps = record_added_crates(prog, &self.dir, &GraphRecipe::new(&pre[..boot]), &recipe, &fps)?;
        let crate_path = self.crate_path.iter().cloned().chain((boot..pre.len())
            .map(|ci| self.dir.join(front_store_name(&prog.session.options, &pre[ci].name, fps[ci])))).collect();
        Ok(CollectedInputs { mode: self.mode, dir: self.dir.clone(), fps, iface_plain, crate_path, crate_ident: self.crate_ident.clone(),
            mapped: self.mapped.clone(), recipe: Some(recipe) })
    }
}

impl Cache {
    pub(crate) fn collected_inputs(&self) -> CollectedInputs {
        CollectedInputs { mode: self.mode, dir: self.dir.clone(), fps: self.fps.clone(), iface_plain: self.iface_plain.clone(),
            crate_path: self.crate_path.clone(), crate_ident: self.crate_ident.clone(),
            mapped: self.mapped_definitions.clone().map(|m| (m, self.crate_defs_n)), recipe: None }
    }

    fn encoder(&self) -> Encoder<'_> {
        Encoder { mode: self.mode, dir: &self.dir, fps: &self.fps, iface_plain: &self.iface_plain,
            crate_path: &self.crate_path, crate_ident: &self.crate_ident, boot: self.collected_boot.as_ref(),
            committed: self.collected_committed, mapped: self.mapped_definitions.as_ref().map(|m| (m, self.crate_defs_n)), recipe: None }
    }


    /// Both the unit and front-end write locks are held. This only encodes metadata:
    /// crate stores, the graph certificate and the collected store are still published
    /// by the ordinary post-present/exit writer. Repeated module loads reuse the snapshot.
    pub(crate) fn preserve_collected_boot(&mut self, prog: &Program, tcx: &Tcx) {
        if self.collected_committed || self.collected_boot.is_some() || self.mode == Mode::ReadOnly { return; }
        let started = std::time::Instant::now();
        let mut why = "recipe";
        self.collected_boot = self.encoder().capture_boot(prog, tcx, &|| false, &mut why).and_then(|boot| boot.into_owned(&|| false));
        if crate::preload::trace() {
            match &self.collected_boot {
                Some(boot) => eprintln!("makepad-run collected: boot snapshot {} crates, {} definitions, {} types, {} files, {} bytes in {:.1} ms",
                    boot.recipe.crates.len(), prog.defs.len(), tcx.tys.len(), boot.files.len(),
                    boot.program.len() + boot.types.len() + boot.definitions.len(), started.elapsed().as_secs_f64() * 1e3),
                None => eprintln!("makepad-run collected: boot snapshot unavailable: {}", why),
            }
        }
    }

    pub(crate) fn collected_crates(&self, prog: &Program) -> usize {
        self.collected_boot.as_ref().map_or(prog.crates.len(), |boot| boot.recipe.crates.len())
    }

    pub(super) fn restore_collected_definitions(&mut self, prog: &Program, bytes: &'static [u8]) -> bool {
        let Some(mapped) = MappedDefinitions::read(bytes, prog.defs.len(), prog.crates.len(), None) else { return false; };
        self.mapped_definitions = Some(mapped);
        self.crate_defs_n = prog.defs.len();
        true
    }

    /// A partial restore's definition tables: those of the `crates` crates it kept, for their
    /// definitions below `defs` (an image of `image_defs` definitions and `image_crates` crates).
    pub(super) fn restore_kept_definitions(&mut self, prog: &Program, bytes: &'static [u8], image_defs: usize, image_crates: usize, crates: usize, defs: usize) -> bool {
        let Some(mapped) = MappedDefinitions::read(bytes, image_defs, image_crates, Some((crates, defs))) else { return false; };
        self.mapped_definitions = Some(mapped);
        self.crate_defs_n = prog.defs.len();
        true
    }

    /// Called with compilation quiescent. Never performs collection or forces a lazy query.
    /// Per-crate file images are flushed first, then the complete graph is committed last.
    pub fn record_collected(&mut self, prog: &Program, tcx: &Tcx) -> Option<()> {
        // committed under the caller's quiescence: the image borrows the program's arrays
        match self.encoder().encode_as(prog, tcx, &|| false, false, Some) {
            CollectedRecord::Ready(dest, payload) => { Self::commit_collected(prog.session.options.verbose.then_some(self.collected_crates(prog)), dest, payload, &|| false)?; }
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
    /// `stop` is asked between parts and every 1024 definitions: when it says yes (a program
    /// request waits for the owner) the encode is dropped and Yielded comes back, so the record
    /// never holds a frame or a live edit up; the caller retries later. The image owns its arrays: the
    /// store write (commit_collected) needs no lock.
    pub fn encode_collected(&self, prog: &Program, tcx: &Tcx, stop: &dyn Fn() -> bool) -> CollectedRecord<'static> {
        self.encoder().encode_as(prog, tcx, stop, false, |boot| boot.into_owned(stop))
    }
}

impl Encoder<'_> {
    fn capture_boot<'p>(&self, prog: &'p Program, tcx: &'p Tcx, stop: &dyn Fn() -> bool, why: &mut &'static str) -> Option<CollectedBoot<'p>> {
        // a live edit's versions (a refused or undone one's too, with their definitions) are in
        // the tables: they are no boot's (a boot captured before them still is: preserve_collected_boot)
        *why = "the program holds a live edit's file versions";
        if prog.has_edit_versions() { return None; }
        *why = "recipe";
        let recipe = self.recipe.or(prog.front_graph.as_ref())?;
        if recipe.crates.len() != prog.crates.len() || recipe.crates.iter().zip(&prog.crates)
            .any(|(key, c)| crate::preload::crate_key(prog.syms.str(c.name), c.root_path(), &c.cfg) != *key) { return None; }
        *why = "Program";
        let mut parts = Parts::new();
        crate::run::yield_section("collected Program");
        let program = Arc::new(prog.collected_writer_with(stop)?);
        parts.lap("Program", &program);
        if stop() { return None; }
        *why = "Tcx";
        crate::run::yield_section("collected Tcx");
        let types = Arc::new(tcx.collected_writer_with(stop)?);
        parts.lap("Tcx", &types);
        if stop() { return None; }
        *why = "definitions";
        crate::run::yield_section("collected definitions");
        let definitions = Arc::new(self.definitions(prog, tcx, stop)?);
        parts.lap("definitions", &definitions);
        if stop() { return None; }
        *why = "interfaces";
        if self.iface_plain.len() != recipe.crates.len() { return None; }
        crate::run::yield_section("collected files");
        let interfaces = self.iface_plain.iter().map(|&k| MappedKey::new(k)).collect();
        let files = prog.files.iter().enumerate().map(|(f, file)| {
            let path = prog.file_paths.get(f)?;
            Some((*prog.file_crate.get(f)?, path.clone(), file.source_hash(path)))
        }).collect::<Option<Vec<_>>>()?;
        Some(CollectedBoot { recipe: recipe.clone(), program, types, definitions, files, interfaces })
    }

    fn definitions(&self, prog: &Program, tcx: &Tcx, stop: &dyn Fn() -> bool) -> Option<crate::image::Writer<'static>> {
        // The keys the cache restored hold while the program has the definitions they were
        // restored for (Cache::def_key): taken as they are below `dk` (a partial restore's kept
        // definitions); only the definitions after them are keyed here.
        let reuse = self.mapped.filter(|(m, n)| *n == prog.defs.len() && m.keys.len() <= prog.defs.len()).map(|(m, _)| m);
        let dk = reuse.map_or(0, |m| m.defs());
        let mut memo = HashMap::new();
        let mut files = HashMap::new();
        let mut keys: Vec<MappedKey> = Vec::with_capacity(prog.defs.len());
        if let Some(m) = reuse { keys.extend_from_slice(&m.keys[..dk]); }
        for i in dk..prog.defs.len() {
            if i % 128 == 127 && stop() { return None; }
            keys.push(MappedKey::new(def_key_in(prog, tcx, self.crate_ident, &mut memo, &mut files, DefId(i as u32))));
        }
        // per crate: its definitions by key, without the keys two of them have (asked between
        // crates and every 4096 definitions too: a large app's sorts held the owner 64-70 ms between checks)
        let mut maps: Vec<Vec<(u128, u32)>> = vec![Vec::new(); prog.crates.len()];
        for (i, k) in keys.iter().enumerate() {
            if i % 4096 == 4095 && stop() { return None; }
            maps[prog.def(DefId(i as u32)).krate as usize].push((k.get(), i as u32));
        }
        let mut image = crate::image::Writer::default();
        image.add_owned(0, &keys);
        for (c, mut map) in maps.into_iter().enumerate() {
            if stop() { return None; }
            map.sort_unstable();
            let unique = |i: usize| (i == 0 || map[i - 1].0 != map[i].0) && map.get(i + 1).is_none_or(|n| n.0 != map[i].0);
            let (keys, defs): (Vec<MappedKey>, Vec<u32>) = (0..map.len()).filter(|&i| unique(i)).map(|i| (MappedKey::new(map[i].0), map[i].1)).unzip();
            image.add_owned(1 + c as u32 * 2, &keys);
            image.add_owned(2 + c as u32 * 2, &defs);
        }
        Some(image)
    }

    /// `keep`: what a boot captured now becomes before the image takes it (CollectedBoot::into_owned
    /// for an image that outlives the program's locks; as it is for one committed under them).
    /// None: it stopped for `stop`.
    fn encode_as<'p, 'b>(&self, prog: &'p Program, tcx: &'p Tcx, stop: &dyn Fn() -> bool, fresh: bool,
        keep: impl FnOnce(CollectedBoot<'p>) -> Option<CollectedBoot<'b>>) -> CollectedRecord<'b> {
        if self.mode == Mode::ReadOnly { return CollectedRecord::Failed; }
        if self.committed && !fresh { return CollectedRecord::Current; }
        let mut why = "recipe";
        let mut yielded = false;
        let recorded = (|| {
        let frozen = self.boot.filter(|_| !fresh);
        let recipe = frozen.map(|boot| &boot.recipe).or(self.recipe).or(prog.front_graph.as_ref())?;
        why = "graph certificate";
        let graph = read_front_graph(self.dir, recipe.identity)?;
        why = "fingerprints";
        if graph.crates.iter().map(|(key, _)| key).ne(recipe.crates.iter())
            || graph.crates.iter().map(|(_, fp)| fp).ne(self.fps.iter()) { return None; }
        let mut check = || { yielded = yielded || stop(); (!yielded).then_some(()) };
        let captured;
        let boot: &CollectedBoot<'b> = match frozen {
            Some(boot) => boot,
            None => {
                let boot = self.capture_boot(prog, tcx, stop, &mut why)?;
                why = "copy";
                crate::run::yield_section("collected copy");
                captured = keep(boot)?;
                &captured
            }
        };
        check()?;
        crate::run::yield_section("collected image");
        let mut image = crate::image::Writer::default();
        image.add_bytes(0, certificate(&graph).to_le_bytes().to_vec());
        image.add_image(1, boot.program.clone()); image.add_image(2, boot.types.clone());
        image.add_image(3, boot.definitions.clone());
        why = "index snapshot";
        image.add_bytes(4, MappedIndexes::record(self.crate_path, &boot.files)?);
        image.add_owned(5, &boot.interfaces);
        check()?;
        let payload = image;
        let dest = path(self.dir, recipe.identity);
        Some(CollectedRecord::Ready(CollectedDest { dest, key: key(recipe.identity), recipe: recipe.clone() }, payload))
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
}

impl Cache {
    /// `report`: the crate count to print (verbose). `stop` is asked every few MB of the image
    /// (its write holds the caller's locks when it borrows the program): yes drops the write
    /// and None comes back. The entry as the store now holds it.
    pub fn commit_collected(report: Option<usize>, CollectedDest { dest, key, recipe }: CollectedDest, payload: crate::image::Writer<'_>, stop: &dyn Fn() -> bool) -> Option<CollectedWritten> {
        front_run::commit_marks(&front_run::run_marks()).ok()?;
        // The image streams from its sections twice, for the sum and into the store: neither
        // pass assembles it.
        let (len, sum) = crate::store::entry_sum(&CollectedEntry { key, payload: &payload, stop })?;
        let written = CollectedWritten { dest: dest.clone(), key, len, sum };
        // the next start of these crates loads in this order (load_order)
        let order = |written| { if let Some(dir) = dest.parent() { write_front_order(dir, &recipe); } Some(written) };
        if Store::open(&dest).is_some_and(|s| s.matches_sum(key, KIND_COLLECTED, len, sum)) { return order(written); }
        if crate::store::append(&dest, vec![CollectedEntry { key, payload: &payload, stop }], true).is_err() {
            if crate::preload::trace() { eprintln!("makepad-run collected: not recorded: write"); }
            return None;
        }
        if let Some(n) = report { eprintln!("makepad-run collected: recorded {} crates", n); }
        order(written)
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
        pin_front(&st, Some(at));
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

/// Where a collected image goes, and the graph recipe whose load order it is recorded in.
pub struct CollectedDest {
    dest: PathBuf,
    key: u128,
    recipe: GraphRecipe,
}

/// `'p`: what the image borrows (`'static`: nothing; else commit it under the program's locks).
pub enum CollectedRecord<'p> {
    /// encoded; commit_collected writes it
    Ready(CollectedDest, crate::image::Writer<'p>),
    /// the store already holds this exact image
    Current,
    /// stopped for a waiting request: try again later
    Yielded,
    Failed,
}

/// Tables produced by this build and covered by the collected store checksum and input
/// certificate. Only the small section directory is decoded at boot; keys stay mapped.
#[derive(Clone)]
pub(super) struct MappedDefinitions {
    keys: &'static [MappedKey],
    crates: Vec<(&'static [MappedKey], &'static [u32])>,
    /// a partial restore's: the tables hold for the definitions below `.1` of the first `.0`
    /// crates only (the others go by the maps the cache builds)
    kept: Option<(usize, usize)>,
}
impl MappedDefinitions {
    fn read(bytes: &'static [u8], defs: usize, crates: usize, kept: Option<(usize, usize)>) -> Option<Self> {
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
        Some(Self { keys, crates: tables, kept })
    }
    pub(super) fn partial(&self) -> bool { self.kept.is_some() }
    /// the definitions below this hold (all of them but in a partial restore)
    fn defs(&self) -> usize { self.kept.map_or(self.keys.len(), |(_, d)| d) }
    /// whether crate `c`'s table holds (its definitions below `defs()` are found there)
    pub(super) fn has_crate(&self, c: u32) -> bool { self.kept.is_none_or(|(n, _)| (c as usize) < n) }
    pub(super) fn key(&self, d: DefId) -> Option<u128> { if (d.0 as usize) < self.defs() { self.keys.get(d.0 as usize).map(|k| k.get()) } else { None } }
    /// None also for a definition past `defs()` (a kept crate's on-demand tail: its map has it)
    pub(super) fn definition(&self, c: u32, key: u128) -> Option<DefId> {
        if !self.has_crate(c) { return None; }
        let (keys, defs) = self.crates.get(c as usize)?;
        let d = *defs.get(find_key(keys, key)?)?;
        ((d as usize) < self.defs()).then_some(DefId(d))
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
        // (an entry of a store the cache does not open: CollectedCache::boot_crates)
        ((loc.store as usize) < self.stores.len()).then_some((loc.store, loc.entry))
    }
    pub(super) fn len(&self) -> usize { self.keys.len() }
}

struct CollectedEntry<'a, 'p> {
    key: u128,
    payload: &'a crate::image::Writer<'p>,
    stop: &'a dyn Fn() -> bool,
}
impl crate::store::Entry for CollectedEntry<'_, '_> {
    fn key(&self) -> u128 { self.key }
    fn kind(&self) -> u32 { KIND_COLLECTED }
    fn write_payload(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        self.payload.write_to(&mut Stoppable { out, stop: self.stop, since: 0 })
    }
}

/// A write that asks `stop` every STOP_EVERY bytes: yes fails it.
struct Stoppable<'a> {
    out: &'a mut dyn std::io::Write,
    stop: &'a dyn Fn() -> bool,
    since: usize,
}
const STOP_EVERY: usize = 4 << 20;
impl std::io::Write for Stoppable<'_> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.since >= STOP_EVERY {
            self.since = 0;
            if (self.stop)() { return Err(std::io::Error::other("the collected image write gave way")); }
        }
        let n = self.out.write(&b[..b.len().min(STOP_EVERY)])?;
        self.since += n;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> { self.out.flush() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reordered load (load_order) takes the stored plan with its crate positions moved: files,
    /// reached set and parts follow each crate to its new position; a plan whose new last crate was
    /// selected in part is not carried.
    #[test]
    fn reordered_load_carries_demand_plan() {
        let sess = std::sync::Arc::new(crate::Session::default());
        let cfg = crate::cfg::CfgSet::host(&sess, &[]);
        let krate = |name: &str| crate::preload::PreCrate { name: name.into(), root: format!("/w/{name}/lib.rs"), edition: 2021, cfg: cfg.clone(),
            deps: Vec::new(), proc_macro: false, image_deps: Vec::new(), image_dep_names: Vec::new() };
        let base = vec![krate("a"), krate("b"), krate("c")];
        let pre = vec![krate("a"), krate("c"), krate("b")];
        let file = |ci: usize, path: &str| crate::preload::FileIndex { ci, path: path.into(), module: String::new(), names: Default::default(),
            glob_names: Default::default(), impls: Vec::new(), blankets: Default::default(), force: false };
        let files = vec![file(0, "/w/a/lib.rs"), file(1, "/w/b/lib.rs"), file(1, "/w/b/x.rs"), file(2, "/w/c/lib.rs")];
        let part = |key: u128| { let mut p = crate::preload::DemandPart::default(); p.key = key; Arc::new(p) };
        let stored = |reached: &[(usize, &str)]| {
            let mut image = crate::preload::DemandImage::default();
            image.graph = crate::preload::demand_recipe_identity(&base);
            image.files = files.clone();
            image.reached = reached.iter().map(|(ci, p)| (*ci, p.to_string())).collect();
            image.eager_prefix = 2;
            image.parts = vec![part(10), part(11), part(12)];
            crate::crate_image::GraphImage { identity: 1, crates: Vec::new(), inputs: Vec::new(), stamps: Vec::new(), probes: Vec::new(), demand_image: Some(Arc::new(image)) }
        };
        let images = FrontImages::default();
        let all = [(0, "/w/a/lib.rs"), (1, "/w/b/lib.rs"), (1, "/w/b/x.rs"), (2, "/w/c/lib.rs")];
        let image = images.carried_demand(&stored(&all), &pre, Some(&base)).expect("a full selection is carried");
        assert_eq!(image.graph, crate::preload::demand_recipe_identity(&pre));
        let at: Vec<(usize, &str)> = image.files.iter().map(|f| (f.ci, f.path.as_str())).collect();
        assert_eq!(at, [(0, "/w/a/lib.rs"), (1, "/w/c/lib.rs"), (2, "/w/b/lib.rs"), (2, "/w/b/x.rs")]);
        assert!(image.reached.contains(&(2, "/w/b/x.rs".to_string())) && image.reached.contains(&(1, "/w/c/lib.rs".to_string())));
        assert_eq!(image.parts.iter().map(|p| p.key).collect::<Vec<_>>(), [10, 12, 11]);
        // `b` (last now) had a file the stored plan deferred: the load plans again
        let partial = [(0, "/w/a/lib.rs"), (1, "/w/b/lib.rs"), (2, "/w/c/lib.rs")];
        assert!(images.carried_demand(&stored(&partial), &pre, Some(&base)).is_none());
        // the stored order itself: carried as it is
        assert!(images.carried_demand(&stored(&all), &base, None).is_some_and(|i| i.files.iter().map(|f| f.ci).eq([0, 1, 1, 2])));
    }
}
