// ported from N:rustc_type_ir/src/search_graph/mod.rs @8d1a764
// ported from N:rustc_type_ir/src/search_graph/stack.rs @8d1a764
// ported from N:rustc_type_ir/src/search_graph/global_cache.rs @8d1a764
//! Concrete port of rustc_type_ir/src/search_graph/{mod,stack,global_cache}.rs.
//! Inputs and responses are interned IDs owned by the caller: equal canonical
//! inputs/responses must have equal IDs, including across fixpoint iterations.
use std::collections::{BTreeMap, HashMap};
use super::SolverDelegate;
use crate::query::DepNodeIndex;

/// rustc_type_ir/src/search_graph/mod.rs:122: forced ambiguity dominates;
/// otherwise a single coinductive step makes the whole path coinductive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathKind {
    Inductive,
    Unknown,
    Coinductive,
    ForcedAmbiguity,
}

impl PathKind {
    pub fn extend(self, rest: Self) -> Self {
        match (self, rest) {
            (Self::ForcedAmbiguity, _) | (_, Self::ForcedAmbiguity) => Self::ForcedAmbiguity,
            (Self::Coinductive, _) | (_, Self::Coinductive) => Self::Coinductive,
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            _ => Self::Inductive,
        }
    }
}

const PATHS: [PathKind; 4] = [
    PathKind::Inductive,
    PathKind::Unknown,
    PathKind::Coinductive,
    PathKind::ForcedAmbiguity,
];

/// mod.rs:272: normalization goals share the parent's depth budget.
#[derive(Debug, Clone, Copy)]
pub enum LowerAvailableDepth { Yes, No }

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequiredDepth(pub usize);

#[derive(Debug, Clone, Copy)]
pub struct SearchGraphConfig {
    pub recursion_limit: usize,
    pub fixpoint_step_limit: usize,
    pub divide_depth_on_overflow: usize,
}

impl Default for SearchGraphConfig {
    fn default() -> Self {
        // rustc_next_trait_solver/src/solve/{mod.rs:46,search_graph.rs:46}.
        Self {
            recursion_limit: 128,
            fixpoint_step_limit: 8,
            divide_depth_on_overflow: 4,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct HeadUsages([u32; 4]);

impl HeadUsages {
    fn add_usage(&mut self, path: PathKind) {
        self.0[path as usize] += 1;
    }
    fn add_nested(&mut self, usages: Self) {
        for i in 0..4 {
            self.0[i] += u32::from(usages.0[i] != 0);
        }
    }
    fn add(&mut self, usages: Self) {
        for i in 0..4 {
            self.0[i] += usages.0[i];
        }
    }
    fn ignore(&mut self, usages: Self) {
        for i in 0..4 {
            self.0[i] = self.0[i].checked_sub(usages.0[i]).unwrap();
        }
    }
    fn is_empty(self) -> bool {
        self.0 == [0; 4]
    }
    fn is_single(self, path: PathKind) -> bool {
        for i in 0..4 {
            if i != path as usize && self.0[i] != 0 {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Default)]
pub struct CandidateHeadUsages {
    usages: BTreeMap<usize, HeadUsages>,
}

impl CandidateHeadUsages {
    pub fn merge_usages(&mut self, other: Self) {
        for (head, usages) in other.usages {
            self.usages.entry(head).or_default().add(usages);
        }
    }
}

/// Empty denotes the zero-length path, distinct from an inductive edge.
/// rustc_type_ir/src/search_graph/mod.rs:366.
#[derive(Debug, Clone, Copy)]
struct PathsToNested(u8);

impl PathsToNested {
    const EMPTY: Self = Self(1);
    fn from_path(path: PathKind) -> Self {
        Self(2 << path as u8)
    }
    fn contains(self, path: PathKind) -> bool {
        self.0 & Self::from_path(path).0 != 0
    }
    fn union(&mut self, other: Self) {
        self.0 |= other.0;
    }
    fn extend(self, path: PathKind) -> Self {
        let bit = Self::from_path(path).0;
        let weaker = bit - 1;
        if self.0 & weaker != 0 {
            Self((self.0 & !weaker) | bit)
        } else {
            self
        }
    }
    fn extend_paths(self, paths: Self) -> Self {
        let mut result = Self(0);
        for path in PATHS {
            if paths.contains(path) {
                result.union(self.extend(path));
            }
        }
        result
    }
}

#[derive(Debug, Clone, Copy)]
struct CycleHead {
    paths: PathsToNested,
    usages: HeadUsages,
}

#[derive(Debug, Default, Clone)]
struct CycleHeads(BTreeMap<usize, CycleHead>);

impl CycleHeads {
    fn highest(&self) -> Option<usize> {
        self.0.last_key_value().map(|(i, _)| *i)
    }
    fn lowest(&self) -> Option<usize> {
        self.0.first_key_value().map(|(i, _)| *i)
    }
    fn insert(&mut self, index: usize, paths: PathsToNested, usages: HeadUsages) {
        match self.0.get_mut(&index) {
            Some(head) => {
                head.paths.union(paths);
                head.usages.add_nested(usages);
            }
            None => {
                self.0.insert(index, CycleHead { paths, usages });
            }
        }
    }
}

#[derive(Debug, Default, Clone)]
struct NestedGoals(HashMap<u32, PathsToNested>);

impl NestedGoals {
    fn insert(&mut self, input: u32, paths: PathsToNested) {
        self.0
            .entry(input)
            .and_modify(|p| p.union(paths))
            .or_insert(paths);
    }
    fn extend_child(&mut self, step: PathKind, nested: NestedRef<'_>) {
        match nested {
            NestedRef::Map(n) => for (&input, &paths) in &n.0 { self.insert(input, paths.extend(step)); },
            NestedRef::Slice(n) => for &(input, paths) in n { self.insert(input, paths.extend(step)); },
        }
    }
    fn compact(&self) -> Box<[(u32, PathsToNested)]> {
        let mut v: Vec<(u32, PathsToNested)> = self.0.iter().map(|(&k, &p)| (k, p)).collect();
        v.sort_unstable_by_key(|e| e.0);
        v.into_boxed_slice()
    }
}

#[derive(Clone, Copy)]
enum NestedRef<'a> { Map(&'a NestedGoals), Slice(&'a [(u32, PathsToNested)]) }
impl NestedRef<'_> {
    fn is_empty(&self) -> bool { match self { NestedRef::Map(n) => n.0.is_empty(), NestedRef::Slice(n) => n.is_empty() } }
}

/// rustc_type_ir/src/search_graph/stack.rs:19: only the top entry is mutated;
/// child metadata is propagated when returning (including cache hits).
#[derive(Debug)]
struct StackEntry {
    input: u32,
    step: PathKind,
    available_depth: usize,
    min_reached_available_depth: usize,
    provisional_result: Option<u32>,
    heads: CycleHeads,
    encountered_overflow: bool,
    usages: Option<HeadUsages>,
    candidate_usages: Option<CandidateHeadUsages>,
    nested_goals: NestedGoals,
}

impl StackEntry {
    fn required_depth(&self) -> RequiredDepth { RequiredDepth(self.available_depth - self.min_reached_available_depth) }
}

#[derive(Debug)]
struct ProvisionalCacheEntry {
    encountered_overflow: bool,
    heads: CycleHeads,
    path_from_head: PathKind,
    result: u32,
}

struct EvaluationResult {
    encountered_overflow: bool,
    required_depth: RequiredDepth,
    heads: CycleHeads,
    nested_goals: NestedGoals,
    result: u32,
}

impl EvaluationResult {
    fn finalize(entry: StackEntry, encountered_overflow: bool, result: u32) -> Self {
        Self {
            encountered_overflow,
            required_depth: entry.required_depth(),
            heads: entry.heads,
            nested_goals: entry.nested_goals,
            result,
        }
    }
}

struct Tracked { result: u32, dep_node: DepNodeIndex }

struct CachedResult {
    required_depth: RequiredDepth,
    // the nested goals as a sorted slice: a cached entry is only ever scanned, never extended
    // (a hash table here cost ~2x the elements plus its header on every entry)
    nested_goals: Box<[(u32, PathsToNested)]>,
    result: Tracked,
    misses: std::sync::Arc<Vec<(u32, String)>>,
}

#[derive(Default)]
struct CacheEntry {
    success: Option<CachedResult>,
    // overflow results are rare: allocated only when one exists
    with_overflow: Option<Box<Vec<(RequiredDepth, CachedResult)>>>,
}

/// rustc_type_ir/src/search_graph/global_cache.rs:19,80: success requires enough
/// depth; stack overflow requires exactly the same available depth.
#[derive(Default)]
pub(in crate::next_solver) struct GlobalCache {
    entries: HashMap<u32, CacheEntry>,
}

impl GlobalCache {
    pub(super) fn mem_stats(&self) -> (usize, usize) {
        fn hm<K, V, S>(m: &std::collections::HashMap<K, V, S>) -> usize { m.capacity() * (std::mem::size_of::<(K, V)>() + 1) * 8 / 7 }
        let mut b = hm(&self.entries);
        let mut n = 0;
        for e in self.entries.values() {
            if let Some(o) = &e.with_overflow { b += std::mem::size_of::<Vec<(RequiredDepth, CachedResult)>>() + o.capacity() * std::mem::size_of::<(RequiredDepth, CachedResult)>(); }
            let all = e.success.iter().chain(e.with_overflow.iter().flat_map(|o| o.iter().map(|x| &x.1)));
            for s in all { n += 1; b += s.nested_goals.len() * std::mem::size_of::<(u32, PathsToNested)>(); }
        }
        (n, b)
    }
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
    pub(super) fn remove_misses(&mut self) {
        self.entries.retain(|_, e| {
            if e.success.as_ref().is_some_and(|v| !v.misses.is_empty()) { e.success = None; }
            if let Some(o) = &mut e.with_overflow {
                o.retain(|(_, v)| v.misses.is_empty());
                if o.is_empty() { e.with_overflow = None; }
            }
            e.success.is_some() || e.with_overflow.is_some()
        });
    }
}

pub struct SearchGraph {
    config: SearchGraphConfig,
    stack: Vec<StackEntry>,
    provisional_cache: HashMap<u32, Vec<ProvisionalCacheEntry>>,
}

enum UpdateParent<'a> {
    Ordinary(NestedRef<'a>),
    CycleOnStack(u32),
    ProvisionalCacheHit,
}

#[derive(Clone, Copy)]
enum RebaseReason {
    NoCycleUsages,
    Ambiguity(crate::next_solver::Certainty),
    ReachedFixpoint(Option<PathKind>),
}

impl SearchGraph {
    pub fn recursion_limit(&self) -> usize { self.config.recursion_limit }
    pub fn new(config: SearchGraphConfig) -> Self {
        assert!(config.fixpoint_step_limit > 0);
        assert!(config.divide_depth_on_overflow > 0);
        Self {
            config,
            stack: Vec::new(),
            provisional_cache: HashMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        if self.stack.is_empty() {
            assert!(self.provisional_cache.is_empty());
            true
        } else {
            false
        }
    }

    pub fn debug_current_depth(&self) -> usize {
        self.stack.len()
    }

    fn cycle_path_kind(stack: &[StackEntry], step: PathKind, head: usize) -> PathKind {
        let mut path = step;
        for entry in &stack[head + 1..] {
            path = path.extend(entry.step);
        }
        path
    }

    /// rustc_type_ir/src/search_graph/mod.rs:619. A provisional hit intentionally
    /// carries no nested goals: its applicability is already deterministic.
    fn update_parent_goal(
        stack: &mut [StackEntry],
        step: PathKind,
        min_reachable_available_depth: usize,
        heads: &CycleHeads,
        overflow: bool,
        context: UpdateParent<'_>,
    ) {
        let parent_index = match stack.len().checked_sub(1) {
            Some(i) => i,
            None => return,
        };
        let parent = &mut stack[parent_index];
        parent.min_reached_available_depth = parent.min_reached_available_depth.min(min_reachable_available_depth);
        parent.encountered_overflow |= overflow;
        for (&index, head) in &heads.0 {
            if let Some(candidate) = &mut parent.candidate_usages {
                candidate.usages.entry(index).or_default().add_nested(head.usages);
            }
            if index < parent_index {
                parent.heads.insert(index, head.paths.extend(step), head.usages);
            } else {
                assert_eq!(index, parent_index);
                parent.usages.get_or_insert_default().add_nested(head.usages);
            }
        }
        let depends_on_cycle = match context {
            UpdateParent::Ordinary(nested) => {
                parent.nested_goals.extend_child(step, nested);
                !nested.is_empty()
            }
            UpdateParent::CycleOnStack(head) => {
                parent.nested_goals.insert(head, PathsToNested::from_path(step));
                true
            }
            UpdateParent::ProvisionalCacheHit => true,
        };
        if depends_on_cycle {
            parent.nested_goals.insert(parent.input, PathsToNested::EMPTY);
        }
    }

    pub fn enter_single_candidate(&mut self) {
        let previous = self
            .stack
            .last_mut()
            .unwrap()
            .candidate_usages
            .replace(CandidateHeadUsages::default());
        debug_assert!(previous.is_none());
        #[cfg(test)]
        {
            assert!(previous.is_none(), "candidate entered before previous candidate finished");
            CANDIDATE_PAIRS.with(|pairs| { let (enter, finish) = pairs.get(); pairs.set((enter + 1, finish)); });
        }
    }

    pub fn finish_single_candidate(&mut self) -> CandidateHeadUsages {
        let usages = self.stack.last_mut().unwrap().candidate_usages.take().unwrap();
        #[cfg(test)]
        CANDIDATE_PAIRS.with(|pairs| { let (enter, finish) = pairs.get(); pairs.set((enter, finish + 1)); });
        usages
    }

    /// Deliberately mirror rustc_type_ir/src/search_graph/mod.rs:721's copy
    /// semantics: self-head usages are kept; ancestor usages are removed.
    pub fn ignore_candidate_head_usages(&mut self, candidate: CandidateHeadUsages) {
        let index = self.stack.len() - 1;
        let entry = &mut self.stack[index];
        for (head_index, usages) in candidate.usages {
            if head_index == index {
                entry.usages.unwrap().ignore(usages);
            } else {
                entry.heads.0.get_mut(&head_index).unwrap().usages.ignore(usages);
            }
        }
    }

    /// Explicit paths support coherence's unknown steps and forced ambiguity.
    /// search_graph/mod.rs:774: inspect the root directly; nested goals use the
    /// ordinary search graph, while the root has its own dependency-tracked cache.
    pub(crate) fn evaluate_root_goal_for_proof_tree(delegate: &mut SolverDelegate<'_>, root_depth: usize, input: u32, inspect: &mut crate::next_solver::inspect::ProofTreeBuilder) -> (u32, RequiredDepth) {
        let mut graph = Self::new(SearchGraphConfig { recursion_limit: root_depth, ..Default::default() });
        graph.stack.push(StackEntry { input, step: PathKind::Inductive, available_depth: root_depth,
            min_reached_available_depth: root_depth, provisional_result: None, heads: Default::default(),
            encountered_overflow: false, usages: None, candidate_usages: None, nested_goals: Default::default() });
        let evaluation = graph.evaluate_goal_in_task(delegate, input, inspect);
        assert!(graph.is_empty()); (evaluation.result, evaluation.required_depth)
    }

    /// rustc_type_ir/src/search_graph/mod.rs:759: depth, provisional cache,
    /// global cache, stack cycle, computation, then cache insertion in that order.
    pub(crate) fn evaluate_goal(
        &mut self,
        delegate: &mut SolverDelegate<'_>,
        input: u32,
        step: PathKind,
        lower_available_depth: LowerAvailableDepth,
        inspect: &mut crate::next_solver::inspect::ProofTreeBuilder,
    ) -> u32 {
        crate::solver_work::count(crate::solver_work::SEARCH_GOALS);
        crate::preempt::point();
        let available_depth = match self.stack.last() {
            None => self.config.recursion_limit,
            Some(parent) if matches!(lower_available_depth, LowerAvailableDepth::No) => parent.available_depth,
            Some(parent) if parent.available_depth == 0 => {
                let parent = self.stack.last_mut().unwrap();
                parent.encountered_overflow = true;
                parent.nested_goals.insert(parent.input, PathsToNested::EMPTY);
                return delegate.stack_overflow_result(input);
            }
            Some(parent) if parent.encountered_overflow => {
                parent.available_depth / self.config.divide_depth_on_overflow
            }
            Some(parent) => parent.available_depth - 1,
        };
        if let Some(result) = self.lookup_provisional_cache(input, step, available_depth) { return result; }
        if inspect.is_noop() {
            if let Some(result) = self.lookup_global_cache(delegate, input, step, available_depth) { return result; }
        }
        if let Some(result) = self.check_cycle_on_stack(delegate, input, step, available_depth) {
            return result;
        }
        debug_assert!(!self.stack.iter().any(|entry| entry.input == input));
        self.stack.push(StackEntry {
            input,
            step,
            available_depth,
            min_reached_available_depth: available_depth,
            provisional_result: None,
            heads: CycleHeads::default(),
            encountered_overflow: false,
            usages: None,
            candidate_usages: None,
            nested_goals: NestedGoals::default(),
        });
        let engine = &delegate.tcx.queries.engine;
        let ((evaluation, misses), dep_node) = engine.with_anon_task(&mut || crate::type_queries::with_misses(|| self.evaluate_goal_in_task(delegate, input, inspect)));
        Self::update_parent_goal(
            &mut self.stack,
            step,
            available_depth.checked_sub(evaluation.required_depth.0).unwrap(),
            &evaluation.heads,
            evaluation.encountered_overflow,
            UpdateParent::Ordinary(NestedRef::Map(&evaluation.nested_goals)),
        );
        let result = evaluation.result;
        if evaluation.heads.0.is_empty() {
            if inspect.is_noop() {
            let mut cache = delegate.tcx.queries.solver.global(input);
            let entry = cache.entries.entry(input).or_default();
            let cached = CachedResult {
                result: delegate.mk_tracked(result, dep_node),
                misses,
                required_depth: evaluation.required_depth,
                nested_goals: evaluation.nested_goals.compact(),
            };
            let previous = if evaluation.encountered_overflow {
                let o = entry.with_overflow.get_or_insert_with(Default::default);
                match o.iter_mut().find(|(d, _)| *d == evaluation.required_depth) {
                    Some((_, slot)) => Some(std::mem::replace(slot, cached)),
                    None => { o.push((evaluation.required_depth, cached)); None }
                }
            } else {
                entry.success.replace(cached)
            };
            if let Some(previous) = previous {
                assert_eq!(delegate.get_tracked(&previous.result), result);
            }
            }
        } else {
            let path_from_head =
                Self::cycle_path_kind(&self.stack, step, evaluation.heads.highest().unwrap());
            self.provisional_cache
                .entry(input)
                .or_default()
                .push(ProvisionalCacheEntry {
                    encountered_overflow: evaluation.encountered_overflow,
                    heads: evaluation.heads,
                    path_from_head,
                    result,
                });
        }
        result
    }

    fn check_cycle_on_stack(
        &mut self,
        delegate: &mut SolverDelegate<'_>,
        input: u32,
        step: PathKind,
        available_depth: usize,
    ) -> Option<u32> {
        let mut head_index = None;
        for i in 0..self.stack.len() {
            if self.stack[i].input == input {
                head_index = Some(i);
                break;
            }
        }
        let index = head_index?;
        let path = Self::cycle_path_kind(&self.stack, step, index);
        let mut usages = HeadUsages::default();
        usages.add_usage(path);
        let mut heads = CycleHeads::default();
        heads.insert(index, PathsToNested::from_path(step), usages);
        Self::update_parent_goal(
            &mut self.stack,
            step,
            available_depth,
            &heads,
            false,
            UpdateParent::CycleOnStack(input),
        );
        Some(match self.stack[index].provisional_result {
            Some(result) => result,
            None => delegate.initial_provisional_result(path, input),
        })
    }

    fn lookup_provisional_cache(&mut self, input: u32, step: PathKind, available_depth: usize) -> Option<u32> {
        let entries = self.provisional_cache.get(&input)?;
        for entry in entries {
            let head_index = entry.heads.highest().unwrap();
            if entry.encountered_overflow {
                match self.stack.last().unwrap().heads.lowest() {
                    Some(lowest) if lowest <= head_index => {}
                    _ => continue,
                }
            }
            if entry.path_from_head == Self::cycle_path_kind(&self.stack, step, head_index) {
                Self::update_parent_goal(
                    &mut self.stack,
                    step,
                    available_depth,
                    &entry.heads,
                    entry.encountered_overflow,
                    UpdateParent::ProvisionalCacheHit,
                );
                return Some(entry.result);
            }
        }
        None
    }

    /// rustc_type_ir/src/search_graph/mod.rs:1135: replaying a global entry must
    /// neither encounter a stack cycle nor use an applicable provisional entry.
    fn candidate_is_applicable(&self, step: PathKind, nested: &[(u32, PathsToNested)]) -> bool {
        if nested.is_empty() {
            return true;
        }
        for entry in &self.stack {
            if nested.binary_search_by_key(&entry.input, |e| e.0).is_ok() {
                return false;
            }
        }
        for &(input, paths) in nested {
            if let Some(entries) = self.provisional_cache.get(&input) {
                for entry in entries {
                    if entry.encountered_overflow {
                        continue;
                    }
                    let path = Self::cycle_path_kind(&self.stack, step, entry.heads.highest().unwrap());
                    if paths.extend(path).contains(entry.path_from_head) {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn lookup_global_cache(&mut self, delegate: &SolverDelegate<'_>, input: u32, step: PathKind, depth: usize) -> Option<u32> {
        let cache = delegate.tcx.queries.solver.global(input);
        let entry = cache.entries.get(&input)?;
        let mut selected = None;
        if let Some(success) = &entry.success {
            if depth >= success.required_depth.0 && self.candidate_is_applicable(step, &success.nested_goals) {
                selected = Some((success, false));
            }
        }
        if selected.is_none() {
            if let Some(overflow) = entry.with_overflow.as_ref().and_then(|o| o.iter().find(|(d, _)| *d == RequiredDepth(depth)).map(|e| &e.1)) {
                if self.candidate_is_applicable(step, &overflow.nested_goals) {
                    selected = Some((overflow, true));
                }
            }
        }
        let (cached, overflow) = selected?;
        crate::program::note_misses(&cached.misses);
        Self::update_parent_goal(
            &mut self.stack,
            step,
            depth.checked_sub(cached.required_depth.0).unwrap(),
            &CycleHeads::default(),
            overflow,
            UpdateParent::Ordinary(NestedRef::Slice(&cached.nested_goals)),
        );
        Some(delegate.get_tracked(&cached.result))
    }

    fn clear_dependent_provisional_results_for_rerun(&mut self) {
        let rerun_index = self.stack.len();
        self.provisional_cache.retain(|_, entries| {
            entries.retain(|entry| {
                let (&head, data) = entry.heads.0.last_key_value().unwrap();
                head != rerun_index || data.usages.is_empty()
            });
            !entries.is_empty()
        });
    }

    /// rustc_type_ir/src/search_graph/mod.rs:962: rebase all dependencies onto
    /// the popped head's own heads, checking that every cycle keeps its kind.
    /// mod.rs:1100: retain dependent responses only when already ambiguous
    /// with the same ambiguity kind; never synthesize a replacement response.
    fn rebase_provisional_cache_entries(&mut self, delegate: &mut SolverDelegate<'_>, popped: &StackEntry, reason: RebaseReason) {
        let popped_index = self.stack.len();
        let stack = &self.stack;
        self.provisional_cache.retain(|_, entries| {
            entries.retain_mut(|entry| {
                if entry.heads.highest().unwrap() != popped_index {
                    assert!(entry.heads.highest().unwrap() < popped_index);
                    return true;
                }
                let (_, popped_head) = entry.heads.0.pop_last().unwrap();
                if popped_head.usages.is_empty() {
                    for &index in popped.heads.0.keys() {
                        entry
                            .heads
                            .insert(index, PathsToNested::EMPTY, HeadUsages::default());
                    }
                } else {
                    let ep = popped_head.paths;
                    for (&index, head) in &popped.heads.0 {
                        let ph = head.paths;
                        let hp = Self::cycle_path_kind(stack, popped.step, index);
                        let he = hp.extend(entry.path_from_head);
                        for ph_path in PATHS {
                            if !ph.contains(ph_path) {
                                continue;
                            }
                            let hph = hp.extend(ph_path);
                            for ep_path in PATHS {
                                if !ep.contains(ep_path) {
                                    continue;
                                }
                                let heph = ep_path.extend(he).extend(ph_path);
                                if hph != heph {
                                    return false;
                                }
                            }
                        }
                        entry.heads.insert(index, ep.extend_paths(ph), head.usages);
                    }
                    match reason {
                        RebaseReason::NoCycleUsages => return false,
                        RebaseReason::Ambiguity(info) => {
                            if delegate.is_ambiguous_result(entry.result) != Some(info) { return false; }
                        }
                        RebaseReason::ReachedFixpoint(None) => {}
                        RebaseReason::ReachedFixpoint(Some(path)) => {
                            if !popped_head.usages.is_single(path) {
                                return false;
                            }
                        }
                    }
                }
                let Some(head) = entry.heads.highest() else {
                    return false;
                };
                entry.path_from_head =
                    entry
                        .path_from_head
                        .extend(Self::cycle_path_kind(stack, popped.step, head));
                true
            });
            !entries.is_empty()
        });
    }

    /// rustc_type_ir/src/search_graph/mod.rs:1314: rerun every cycle head, carry
    /// depth/nested-goal/dependency metadata across iterations, and reset usages.
    fn evaluate_goal_in_task(&mut self, delegate: &mut SolverDelegate<'_>, input: u32, inspect: &mut crate::next_solver::inspect::ProofTreeBuilder) -> EvaluationResult {
        let mut encountered_overflow = false;
        let mut iteration = 0;
        loop {
            let result = delegate.compute(self, input, inspect);
            let mut entry = self.stack.pop().unwrap();
            assert_eq!(entry.input, input);
            assert!(entry.candidate_usages.is_none(), "unfinished candidate");
            encountered_overflow |= entry.encountered_overflow;
            let Some(usages) = entry.usages else {
                return EvaluationResult::finalize(entry, encountered_overflow, result);
            };
            let fixpoint = match entry.provisional_result {
                Some(previous) if previous == result => Some(None),
                Some(_) => None,
                None => match delegate.is_initial_provisional_result(result) {
                    Some(path) if usages.is_single(path) => Some(Some(path)),
                    _ => None,
                },
            };
            let reason = if let Some(fixpoint) = fixpoint {
                Some(RebaseReason::ReachedFixpoint(fixpoint))
            } else if usages.is_empty() {
                Some(RebaseReason::NoCycleUsages)
            } else {
                delegate.is_ambiguous_result(result).map(RebaseReason::Ambiguity)
            };
            if let Some(reason) = reason {
                self.rebase_provisional_cache_entries(delegate, &entry, reason);
                return EvaluationResult::finalize(entry, encountered_overflow, result);
            }
            iteration += 1;
            if iteration >= self.config.fixpoint_step_limit {
                let result = delegate.fixpoint_overflow_result(input);
                let kind = delegate.is_ambiguous_result(result).expect("fixpoint overflow must be ambiguous");
                self.rebase_provisional_cache_entries(delegate, &entry, RebaseReason::Ambiguity(kind));
                return EvaluationResult::finalize(entry, encountered_overflow, result);
            }
            self.clear_dependent_provisional_results_for_rerun();
            entry.provisional_result = Some(result);
            entry.encountered_overflow = false;
            entry.usages = Some(HeadUsages::default());
            self.stack.push(entry);
        }
    }
}



impl SolverDelegate<'_> {
    fn mk_tracked(&self, result: u32, dep_node: DepNodeIndex) -> Tracked { Tracked { result, dep_node } }
    fn get_tracked(&self, tracked: &Tracked) -> u32 {
        self.tcx.queries.engine.read_index(tracked.dep_node);
        tracked.result
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static CANDIDATE_PAIRS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}
