// ported from N:rustc_next_trait_solver/src/solve/search_graph.rs @8d1a764
use super::*;
use super::canonical::*;
use super::eval_ctxt::EvalCtxt;
use crate::program::Program;
mod graph;
pub use graph::{SearchGraph, SearchGraphConfig, PathKind, LowerAvailableDepth, CandidateHeadUsages, RequiredDepth};
use crate::tcx::Tcx;

// Kept audit design: rustc_type_ir/src/search_graph/global_cache.rs:47-72.
// Interned IDs preserve equality and dep-node reads; storage lives for this Tcx revision.
// Shared by every thread that type-checks (a parallel `makepad check`): interned inputs and
// results are append-only arenas read without a lock with sharded lookup maps, a lock only to
// intern a new one; the global cache is sharded by input. One Mutex over all of it was the top
// contended lock of the check after the region interner.
pub(crate) struct SolverCache {
    pub(super) proof_trees: crate::conc::ShardMap<(u32, usize), ProofTreeCacheEntry>,
    inputs: crate::conc::AppendVec<CanonicalQueryInput>,
    input_ids: IdIndex,
    results: crate::conc::AppendVec<(QueryResult, AccessedOpaques)>,
    result_ids: IdIndex,
    intern: relax_session::sync::Mutex<()>,
    global_cache: Box<[relax_session::sync::Mutex<graph::GlobalCache>]>,
}
/// hash -> arena index of an interned value: the value lives once, in its arena; a lookup
/// compares against the arena entry (rustc's interner shape). Entries whose 64-bit hashes are
/// equal beyond the first go to `more` (checked only when one exists).
#[derive(Default)]
struct IdIndex {
    first: crate::conc::ShardMap<u64, u32>,
    more: relax_session::sync::Mutex<std::collections::HashMap<u64, Vec<u32>>>,
    has_more: std::sync::atomic::AtomicBool,
}
impl IdIndex {
    fn hash_of<T: std::hash::Hash>(v: &T) -> u64 {
        use std::hash::Hasher;
        let mut h = crate::fxhash::FxHasher::default(); v.hash(&mut h); h.finish()
    }
    fn mem_stats(&self) -> (usize, usize) {
        let (n, mut b) = self.first.mem_stats(|_, _| 0);
        for v in self.more.lock().unwrap_or_else(|e| e.into_inner()).values() { b += v.capacity() * 4 + 64; }
        (n, b)
    }
    fn get(&self, h: u64, is: impl Fn(u32) -> bool) -> Option<u32> {
        if let Some(id) = self.first.get(&h) {
            if is(id) { return Some(id); }
            if self.has_more.load(std::sync::atomic::Ordering::Acquire) {
                let more = self.more.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(ids) = more.get(&h) { return ids.iter().copied().find(|&i| is(i)); }
            }
        }
        None
    }
    /// Caller holds the intern lock and has seen `get` fail.
    fn insert(&self, h: u64, id: u32) {
        if self.first.get(&h).is_none() { self.first.insert(h, id); return; }
        self.more.lock().unwrap_or_else(|e| e.into_inner()).entry(h).or_default().push(id);
        self.has_more.store(true, std::sync::atomic::Ordering::Release);
    }
}
const SOLVER_CAP: usize = 1 << 26;
const GLOBAL_SHARDS: usize = 64;
impl Default for SolverCache {
    fn default() -> Self {
        SolverCache { proof_trees: Default::default(), inputs: crate::conc::AppendVec::with_capacity(SOLVER_CAP), input_ids: Default::default(),
            results: crate::conc::AppendVec::with_capacity(SOLVER_CAP), result_ids: Default::default(), intern: Default::default(),
            global_cache: (0..GLOBAL_SHARDS).map(|_| Default::default()).collect() }
    }
}

#[derive(Clone)]
pub(super) struct ProofTreeCacheEntry {
    pub result: u32, pub final_revision: super::inspect::Probe,
    pub required_depth: graph::RequiredDepth, pub dep_node: crate::query::DepNodeIndex,
    pub misses: std::sync::Arc<Vec<(u32, String)>>,
}
impl SolverCache {
    /// memory report (MAKEPAD_RUN_MEM_REPORT): exact bytes per solver structure
    pub(crate) fn mem_lines(&self) -> Vec<(String, u64)> {
        use std::mem::size_of;
        let mut out: Vec<(String, u64)> = Vec::new();
        let vk = size_of::<CanonicalVarKind>();
        let input_deep = |i: &CanonicalQueryInput| i.canonical.var_kinds.capacity() * vk + i.canonical.value.predefined_opaques_in_body.capacity() * size_of::<(crate::infer::opaque_types::OpaqueTypeKey, TyId)>();
        let n = self.inputs.len();
        let chunks = (n + 4095) / 4096 * 4096 * size_of::<CanonicalQueryInput>();
        let deep: usize = self.inputs.iter().map(|i| input_deep(i)).sum();
        out.push((format!("solver inputs AppendVec ({}; CanonicalQueryInput {} B; chunks {} B + vecs)", n, size_of::<CanonicalQueryInput>(), chunks), (chunks + deep) as u64));
        let (m, b) = self.input_ids.mem_stats();
        out.push((format!("solver input_ids map ({})", m), b as u64));
        let res_deep = |r: &(QueryResult, AccessedOpaques)| inspect::query_result_bytes(&r.0) + match &r.1.rerun { typing_mode::RerunCondition::OpaqueInStorage(v) | typing_mode::RerunCondition::OpaqueInStorageOrAnyOpaqueHasInferAsHidden(v) => v.capacity() * 4, _ => 0 };
        let n = self.results.len();
        let chunks = (n + 4095) / 4096 * 4096 * size_of::<(QueryResult, AccessedOpaques)>();
        let deep: usize = self.results.iter().map(|r| res_deep(r)).sum();
        out.push((format!("solver results AppendVec ({}; entry {} B; chunks {} B + vecs)", n, size_of::<(QueryResult, AccessedOpaques)>(), chunks), (chunks + deep) as u64));
        let (m, b) = self.result_ids.mem_stats();
        out.push((format!("solver result_ids map ({})", m), b as u64));
        let (m, b) = self.proof_trees.mem_stats(|_, e| inspect::probe_bytes(&e.final_revision));
        out.push((format!("solver proof_trees ({}; ProofTreeCacheEntry {} B + Probe tree; Probe {} B, ProbeStep {} B, CanonicalState {} B, canonical Response {} B)", m, size_of::<ProofTreeCacheEntry>(), size_of::<inspect::Probe>(), size_of::<inspect::ProbeStep>(), size_of::<inspect::CanonicalState>(), size_of::<canonical::Canonical<Response>>()), b as u64));
        let (mut ge, mut gb) = (0usize, 0usize);
        for g in self.global_cache.iter() { let g = g.lock().unwrap_or_else(|e| e.into_inner()); let (a, b) = g.mem_stats(); ge += a; gb += b; }
        out.push((format!("solver global_cache ({} results)", ge), gb as u64));
        out
    }
    pub(crate) fn len(&self) -> usize { self.inputs.len() + self.proof_trees.len() }
    /// The global cache shard holding `input`'s entries.
    fn global(&self, input: u32) -> relax_session::sync::MutexGuard<'_, graph::GlobalCache> {
        crate::solver_work::count(crate::solver_work::SOLVER_GLOBAL_LOCKS);
        self.global_cache[input as usize % GLOBAL_SHARDS].lock().unwrap_or_else(|e| e.into_inner())
    }
    pub(crate) fn cached_len(&self) -> usize {
        self.global_cache.iter().map(|g| g.lock().unwrap_or_else(|e| e.into_inner()).len()).sum::<usize>() + self.proof_trees.len()
    }
    pub(crate) fn remove_misses(&mut self) {
        for g in self.global_cache.iter_mut() { g.get_mut().unwrap_or_else(|e| e.into_inner()).remove_misses(); }
        self.proof_trees.retain(|_, entry| entry.misses.is_empty());
    }
}
pub struct SolverDelegate<'a> { pub limit: crate::tcx::RecursionLimit, pub tcx: &'a Tcx, pub prog: &'a Program, pub cause: super::fulfill::ObligationCause }
impl<'a> SolverDelegate<'a> {
    pub fn new(tcx: &'a Tcx, prog: &'a Program) -> Self { Self { tcx, prog, limit: Default::default(), cause: Default::default() } }
    pub fn input(&mut self, input: CanonicalQueryInput) -> u32 {
        let cache = &self.tcx.queries.solver;
        crate::solver_work::count(crate::solver_work::SOLVER_MAP_LOCKS);
        let h = IdIndex::hash_of(&input);
        let is = |i: u32| cache.inputs[i as usize] == input;
        if let Some(id) = cache.input_ids.get(h, is) { return id; }
        crate::solver_work::count(crate::solver_work::SOLVER_INTERN_LOCKS);
        let _g = cache.intern.lock().unwrap_or_else(|e| e.into_inner());
        crate::solver_work::count(crate::solver_work::SOLVER_MAP_LOCKS);
        if let Some(id) = cache.input_ids.get(h, is) { return id; }
        let id = cache.inputs.push(input) as u32;
        crate::solver_work::count(crate::solver_work::SOLVER_MAP_LOCKS);
        cache.input_ids.insert(h, id); id
    }
    fn result_id(&mut self, result: (QueryResult, AccessedOpaques)) -> u32 {
        let cache = &self.tcx.queries.solver;
        crate::solver_work::count(crate::solver_work::SOLVER_MAP_LOCKS);
        let h = IdIndex::hash_of(&result);
        let is = |i: u32| cache.results[i as usize] == result;
        if let Some(id) = cache.result_ids.get(h, is) { return id; }
        crate::solver_work::count(crate::solver_work::SOLVER_INTERN_LOCKS);
        let _g = cache.intern.lock().unwrap_or_else(|e| e.into_inner());
        crate::solver_work::count(crate::solver_work::SOLVER_MAP_LOCKS);
        if let Some(id) = cache.result_ids.get(h, is) { return id; }
        let id = cache.results.push(result) as u32;
        crate::solver_work::count(crate::solver_work::SOLVER_MAP_LOCKS);
        cache.result_ids.insert(h, id); id
    }
    // Entries are immutable and live for the borrowed Tcx, including across nested evaluations.
    pub fn result(&self, id: u32) -> &'a (QueryResult, AccessedOpaques) { crate::solver_work::count(crate::solver_work::RESULT_READS); &self.tcx.queries.solver.results[id as usize] }
    fn query_input(&self, id: u32) -> &'a CanonicalQueryInput { &self.tcx.queries.solver.inputs[id as usize] }
    fn unconstrained(&mut self, input: u32, certainty: Certainty) -> u32 {
        let input = self.query_input(input);
        let response = response_no_constraints_raw(&self.tcx.tys, input.canonical.max_universe,
            &input.canonical.var_kinds, certainty);
        self.result_id((Ok(response), Default::default()))
    }
}
impl SolverDelegate<'_> {
    fn initial_provisional_result(&mut self, kind: PathKind, input: u32) -> u32 {
        let certainty = match kind {
            PathKind::Coinductive => Certainty::Yes,
            PathKind::Unknown | PathKind::ForcedAmbiguity => Certainty::overflow(false),
            PathKind::Inductive => unreachable!(),
        };
        self.unconstrained(input, certainty)
    }
    fn is_initial_provisional_result(&self, result: u32) -> Option<PathKind> {
        match &self.result(result).0 {
            Ok(response) if has_no_inference_or_external_constraints(&self.tcx.tys, &response) => {
                if response.value.certainty == Certainty::Yes { Some(PathKind::Coinductive) }
                else if response.value.certainty == Certainty::overflow(false) { Some(PathKind::Unknown) }
                else { None }
            }
            Err(NoSolution) => Some(PathKind::Inductive),
            _ => None,
        }
    }
    fn stack_overflow_result(&mut self, input: u32) -> u32 { self.unconstrained(input, Certainty::overflow(true)) }
    fn fixpoint_overflow_result(&mut self, input: u32) -> u32 { self.unconstrained(input, Certainty::overflow(false)) }
    fn is_ambiguous_result(&self, result: u32) -> Option<Certainty> {
        let response = self.result(result).0.as_ref().ok()?;
        if !has_no_inference_or_external_constraints(&self.tcx.tys, &response) { return None; }
        match response.value.certainty {
            Certainty::Yes => None,
            certainty @ Certainty::Maybe(_) => Some(certainty),
        }
    }
    fn compute(&mut self, graph: &mut SearchGraph, input: u32, inspect: &mut super::inspect::ProofTreeBuilder) -> u32 {
        let input = self.query_input(input);
        // Append-only publication preserves this cache only when no existing impl set
        // changes. Entries with deferred misses (including nested hits) are evicted.
        let result = EvalCtxt::enter_canonical(self, graph, &input, inspect);
        self.result_id(result)
    }
}

impl SolverDelegate<'_> {
    pub(super) fn evaluate_root_goal_for_proof_tree_raw_provider(&mut self, input: u32, root_depth: usize) -> ProofTreeCacheEntry {
        let cached = self.tcx.queries.solver.proof_trees.get(&(input, root_depth));
        if let Some(cached) = cached { crate::program::note_misses(&cached.misses); self.tcx.queries.engine.read_index(cached.dep_node); return cached; }
        let engine = &self.tcx.queries.engine;
        let (((result, final_revision, required_depth), misses), dep_node) = engine.with_anon_task(&mut || crate::type_queries::with_misses(|| {
            let mut inspect = super::inspect::ProofTreeBuilder::new();
            let (id, required_depth) = SearchGraph::evaluate_root_goal_for_proof_tree(self, root_depth, input, &mut inspect);
            let (_, accessed_opaques) = self.result(id); assert!(!accessed_opaques.might_rerun());
            let mut tree = inspect.unwrap(); tree.prune_unreachable();
            (id, tree, required_depth)
        }));
        let entry = ProofTreeCacheEntry { result, final_revision, required_depth, dep_node, misses };
        self.tcx.queries.solver.proof_trees.insert((input, root_depth), entry.clone()); entry
    }
}

#[cfg(test)]
pub(crate) use graph::CANDIDATE_PAIRS;
