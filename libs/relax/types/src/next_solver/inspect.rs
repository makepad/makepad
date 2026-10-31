// ported from N:rustc_type_ir/src/solve/inspect.rs @8d1a764
// ported from N:rustc_next_trait_solver/src/solve/inspect/build.rs @8d1a764
// ported from N:rustc_trait_selection/src/solve/inspect/analyse.rs @8d1a764
use super::*;
use super::assembly::CandidateSource;
use crate::infer::InferCtxt;
use crate::types::UniverseIndex;

#[derive(Clone)]
pub enum StateData { Unit, Goal(Goal), Args(Vec<GenericArg>) }
#[derive(Clone)]
pub struct State { pub var_values: Vec<GenericArg>, pub data: StateData }
pub type CanonicalState = canonical::Canonical<State>;
#[derive(Clone)]
pub enum ProbeKind {
    Root { result: ProbeResult },
    TraitCandidate { source: CandidateSource, result: ProbeResult },
    NormalizedSelfTyAssembly, UnsizeAssembly, ProjectionCompatibility, ShadowedEnvProbing,
    OpaqueTypeStorageLookup { result: ProbeResult }, RigidAlias { result: ProbeResult },
}
/// A probe's query result as its readers see it: the certainty, or no solution (the full
/// response of a `QueryResult` is 136 bytes inline plus its vectors; no reader looks at it).
#[derive(Clone, Copy)]
pub enum ProbeResult { NoSolution, Ok(Certainty) }
impl ProbeResult {
    pub fn new<E>(r: &Result<canonical::Canonical<Response>, E>) -> Self {
        match r { Ok(c) => ProbeResult::Ok(c.value.certainty), Err(_) => ProbeResult::NoSolution }
    }
    pub fn certainty(&self) -> Result<Certainty, NoSolution> {
        match self { ProbeResult::Ok(c) => Ok(*c), ProbeResult::NoSolution => Err(NoSolution) }
    }
}
// Steps are small: states and nested probes live behind boxes (an inline nested probe made
// every step as large as a whole probe, 296 bytes, for the many steps that are not one).
#[derive(Clone)]
pub enum ProbeStep {
    AddGoal(GoalSource, Box<CanonicalState>), NestedProbe(Box<Probe>),
    RecordImplArgs { impl_args: Box<CanonicalState> }, MakeCanonicalResponse { shallow_certainty: Certainty },
}
/// `final_state` is None for the pruned leaf of a failed candidate (see `prune_unreachable`).
#[derive(Clone)]
pub struct Probe { pub steps: Box<[ProbeStep]>, pub kind: ProbeKind, pub final_state: Option<Box<CanonicalState>> }
use crate::infer::opaque_types::OpaqueTypeKey;
use super::canonical::{Canonical, CanonicalVarKind};
pub fn canonical_state_bytes<T>(c: &Canonical<T>) -> usize { c.var_kinds.capacity() * std::mem::size_of::<CanonicalVarKind>() }
pub fn query_result_bytes(r: &QueryResult) -> usize {
    match r {
        Ok(c) => c.var_kinds.capacity() * std::mem::size_of::<CanonicalVarKind>() + c.value.var_values.capacity() * std::mem::size_of::<GenericArg>()
            + c.value.external_constraints.region_constraints.capacity() * std::mem::size_of::<(RegionConstraint, bool)>()
            + c.value.external_constraints.opaque_types.capacity() * std::mem::size_of::<(OpaqueTypeKey, TyId)>()
            + c.value.external_constraints.normalization_nested_goals.capacity() * std::mem::size_of::<(GoalSource, Goal)>(),
        Err(_) => 0,
    }
}
fn state_bytes(c: &CanonicalState) -> usize {
    16 + std::mem::size_of::<CanonicalState>() + canonical_state_bytes(c) + c.value.var_values.capacity() * std::mem::size_of::<GenericArg>() + match &c.value.data { StateData::Args(v) => v.capacity() * std::mem::size_of::<GenericArg>(), _ => 0 }
}
/// memory report: heap bytes of a proof tree below its root (step slices, boxed nested probes
/// and states, results; the allocator's 16-byte block header counted per box)
pub fn probe_bytes(p: &Probe) -> usize {
    let mut n = p.steps.len() * std::mem::size_of::<ProbeStep>() + p.final_state.as_ref().map_or(0, |s| state_bytes(s));
    for s in p.steps.iter() {
        n += match s { ProbeStep::AddGoal(_, c) => state_bytes(c), ProbeStep::NestedProbe(q) => 16 + std::mem::size_of::<Probe>() + probe_bytes(q), ProbeStep::RecordImplArgs { impl_args } => state_bytes(impl_args), _ => 0 };
    }
    n
}
impl Probe {
    /// Drops what no reader of `InspectGoal::candidates` can reach, for the cached copy of a
    /// proof tree: probes `candidates_recur` skips outright (projection compatibility and
    /// shadowed-env probes), and the goals / impl arguments of a failed candidate with no nested
    /// probes, and the final state of every probe that is not an applicable candidate (only an
    /// applicable candidate is ever instantiated).
    pub(super) fn prune_unreachable(&mut self) {
        let mut steps = std::mem::take(&mut self.steps).into_vec();
        steps.retain(|s| !matches!(s, ProbeStep::NestedProbe(p) if matches!(p.kind, ProbeKind::ProjectionCompatibility | ProbeKind::ShadowedEnvProbing)));
        let mut leaf = true;
        for s in &mut steps { if let ProbeStep::NestedProbe(p) = s { leaf = false; p.prune_unreachable(); } }
        let failed = matches!(&self.kind, ProbeKind::Root { result: ProbeResult::NoSolution } | ProbeKind::TraitCandidate { result: ProbeResult::NoSolution, .. } | ProbeKind::OpaqueTypeStorageLookup { result: ProbeResult::NoSolution } | ProbeKind::RigidAlias { result: ProbeResult::NoSolution });
        if leaf && failed {
            steps.retain(|s| matches!(s, ProbeStep::MakeCanonicalResponse { .. }));
        }
        // the final state is read only to instantiate an applicable candidate
        if !matches!(&self.kind, ProbeKind::Root { result: ProbeResult::Ok(_) } | ProbeKind::TraitCandidate { result: ProbeResult::Ok(_), .. } | ProbeKind::OpaqueTypeStorageLookup { result: ProbeResult::Ok(_) } | ProbeKind::RigidAlias { result: ProbeResult::Ok(_) }) {
            self.final_state = None;
        }
        self.steps = steps.into_boxed_slice();
    }
}
pub struct GoalEvaluation {
    pub uncanonicalized_goal: Goal, pub orig_values: Vec<GenericArg>, pub final_revision: Probe, pub result: QueryResult, pub required_depth: super::search_graph::RequiredDepth,
}
#[derive(Default)]
pub(crate) struct ProofTreeBuilder { state: Option<Option<Probe>> }
impl ProofTreeBuilder {
    pub fn new() -> Self { Self { state: Some(None) } }
    pub fn is_noop(&self) -> bool { self.state.is_none() }
    pub fn new_evaluation_step(&self, var_values: Vec<GenericArg>) -> EvaluationStepBuilder {
        EvaluationStepBuilder { state: self.state.as_ref().map(|_| Box::new(WipEvaluationStep {
            evaluation: WipProbe::new(var_values.len()), var_values, probe_depth: 0,
        })) }
    }
    pub fn finish_evaluation_step(&mut self, step: EvaluationStepBuilder) {
        if let Some(state) = &mut self.state { *state = Some(step.state.unwrap().evaluation.finalize()); }
    }
    pub fn unwrap(self) -> Probe { self.state.unwrap().unwrap() }
}
#[derive(Default)]
pub(crate) struct EvaluationStepBuilder { state: Option<Box<WipEvaluationStep>> }
struct WipEvaluationStep { var_values: Vec<GenericArg>, probe_depth: usize, evaluation: WipProbe }
struct WipProbe { initial_num_var_values: usize, steps: Vec<WipProbeStep>, kind: Option<ProbeKind>, final_state: Option<CanonicalState> }
enum WipProbeStep { Step(ProbeStep), NestedProbe(WipProbe) }
impl WipProbe {
    fn new(initial_num_var_values: usize) -> Self { Self { initial_num_var_values, steps: Vec::new(), kind: None, final_state: None } }
    fn finalize(self) -> Probe {
        let mut steps = Vec::new();
        for step in self.steps { steps.push(match step { WipProbeStep::Step(s) => s, WipProbeStep::NestedProbe(p) => ProbeStep::NestedProbe(Box::new(p.finalize())) }); }
        Probe { steps: steps.into_boxed_slice(), kind: self.kind.expect("probe kind"), final_state: Some(Box::new(self.final_state.expect("probe final state"))) }
    }
}
impl WipEvaluationStep {
    fn current_evaluation_scope(&mut self) -> &mut WipProbe {
        let mut current = &mut self.evaluation;
        for _ in 0..self.probe_depth { let Some(WipProbeStep::NestedProbe(p)) = current.steps.last_mut() else { unreachable!() }; current = p; }
        current
    }
}
impl EvaluationStepBuilder {
    pub fn take_and_enter_probe(&mut self) -> Self {
        let mut nested = Self { state: self.state.take() };
        if let Some(s) = &mut nested.state {
            let probe = WipProbe::new(s.var_values.len());
            s.current_evaluation_scope().steps.push(WipProbeStep::NestedProbe(probe)); s.probe_depth += 1;
        }
        nested
    }
    pub fn candidate_source(&mut self, source: CandidateSource) {
        if let Some(s) = &mut self.state {
            let Some(WipProbeStep::NestedProbe(p)) = s.current_evaluation_scope().steps.last_mut() else { unreachable!() };
            let Some(ProbeKind::TraitCandidate { source: slot, .. }) = &mut p.kind else { unreachable!() };
            *slot = source;
        }
    }
    pub fn add_var_value(&mut self, value: GenericArg) { if let Some(s) = &mut self.state { s.var_values.push(value); } }
    pub fn probe_kind(&mut self, kind: ProbeKind) {
        if let Some(s) = &mut self.state { assert!(s.current_evaluation_scope().kind.replace(kind).is_none()); }
    }
    pub fn probe_final_state(&mut self, infcx: &InferCtxt<'_>, universe: UniverseIndex) {
        if let Some(s) = &mut self.state {
            let state = canonical::make_canonical_state(infcx, &s.var_values, universe, StateData::Unit);
            assert!(s.current_evaluation_scope().final_state.replace(state).is_none());
        }
    }
    pub fn add_goal(&mut self, infcx: &InferCtxt<'_>, universe: UniverseIndex, source: GoalSource, goal: Goal) {
        if let Some(s) = &mut self.state {
            let state = canonical::make_canonical_state(infcx, &s.var_values, universe, StateData::Goal(goal));
            s.current_evaluation_scope().steps.push(WipProbeStep::Step(ProbeStep::AddGoal(source, Box::new(state))));
        }
    }
    pub fn record_impl_args(&mut self, infcx: &InferCtxt<'_>, universe: UniverseIndex, args: Vec<GenericArg>) {
        if let Some(s) = &mut self.state {
            let impl_args = canonical::make_canonical_state(infcx, &s.var_values, universe, StateData::Args(args));
            s.current_evaluation_scope().steps.push(WipProbeStep::Step(ProbeStep::RecordImplArgs { impl_args: Box::new(impl_args) }));
        }
    }
    pub fn make_canonical_response(&mut self, shallow_certainty: Certainty) {
        if let Some(s) = &mut self.state { s.current_evaluation_scope().steps.push(WipProbeStep::Step(ProbeStep::MakeCanonicalResponse { shallow_certainty })); }
    }
    pub fn finish_probe(mut self) -> Self {
        if let Some(s) = &mut self.state {
            assert_ne!(s.probe_depth, 0); let n = s.current_evaluation_scope().initial_num_var_values;
            s.var_values.truncate(n); s.probe_depth -= 1;
        }
        self
    }
}

pub struct InspectGoal<'a, 'tcx> {
    pub infcx: &'a InferCtxt<'tcx>, pub prog: &'a crate::program::Program, pub goal: Goal, orig_values: Vec<GenericArg>, prev_universe: UniverseIndex,
    final_revision: Probe, pub result: Result<Certainty, NoSolution>,
}
pub struct InspectCandidate<'a, 'tcx> {
    pub goal: &'a InspectGoal<'a, 'tcx>, pub kind: &'a ProbeKind, steps: Vec<&'a ProbeStep>,
    final_state: Option<&'a CanonicalState>, pub shallow_certainty: Certainty, pub result: Result<Certainty, NoSolution>,
}
impl<'a, 'tcx> InspectGoal<'a, 'tcx> {
    pub fn new(infcx: &'a InferCtxt<'tcx>, prog: &'a crate::program::Program, root: GoalEvaluation) -> Self {
        Self { infcx, prog, prev_universe: infcx.universe(), goal: fold::fold_goal(infcx.tcx, &root.uncanonicalized_goal, &mut crate::infer::resolve::DeepVariableResolver::new(infcx)), orig_values: root.orig_values,
            final_revision: root.final_revision, result: root.result.map(|r| r.value.certainty) }
    }
    fn candidates_recur(&'a self, candidates: &mut Vec<InspectCandidate<'a, 'tcx>>, steps: &mut Vec<&'a ProbeStep>, probe: &'a Probe) {
        let mut shallow_certainty = None;
        for step in &probe.steps {
            match step {
                ProbeStep::AddGoal(..) | ProbeStep::RecordImplArgs { .. } => steps.push(step),
                ProbeStep::MakeCanonicalResponse { shallow_certainty: c } => { assert!(shallow_certainty.replace(*c).is_none()); }
                ProbeStep::NestedProbe(p) => {
                    if matches!(p.kind, ProbeKind::ProjectionCompatibility | ProbeKind::ShadowedEnvProbing) { continue; }
                    let n = steps.len(); self.candidates_recur(candidates, steps, p); steps.truncate(n);
                }
            }
        }
        let result = match &probe.kind {
            ProbeKind::Root { result } | ProbeKind::TraitCandidate { result, .. } | ProbeKind::OpaqueTypeStorageLookup { result } | ProbeKind::RigidAlias { result } => result,
            ProbeKind::NormalizedSelfTyAssembly | ProbeKind::UnsizeAssembly => return,
            ProbeKind::ProjectionCompatibility | ProbeKind::ShadowedEnvProbing => panic!("non-candidate probe"),
        };
        if let Some(shallow_certainty) = shallow_certainty { candidates.push(InspectCandidate {
            goal: self, kind: &probe.kind, steps: steps.clone(), final_state: probe.final_state.as_deref(),
            shallow_certainty, result: result.certainty(),
        }); }
    }
    /// rustc_trait_selection/src/solve/inspect/analyse.rs:325-331.
    pub fn unique_applicable_candidate(&'a self) -> Option<InspectCandidate<'a, 'tcx>> {
        let mut candidates = self.candidates();
        candidates.retain(|c| c.result.is_ok());
        let candidate = candidates.pop();
        if candidates.is_empty() { candidate } else { None }
    }
    pub fn candidates(&'a self) -> Vec<InspectCandidate<'a, 'tcx>> {
        let mut candidates = Vec::new(); self.candidates_recur(&mut candidates, &mut Vec::new(), &self.final_revision); candidates
    }
}
impl InspectCandidate<'_, '_> {
    /// A failed candidate's proof tree is pruned to its leaf: nothing to instantiate.
    pub fn has_final_state(&self) -> bool { self.final_state.is_some() }
    pub fn instantiate_nested_goals(&self) -> Vec<Goal> {
        let mut values = self.goal.orig_values.clone(); let mut goals = Vec::new();
        for step in &self.steps {
            if let ProbeStep::AddGoal(_, state) = step {
                let StateData::Goal(goal) = canonical::instantiate_canonical_state(self.goal.infcx, self.goal.prog, self.goal.prev_universe, &mut values, state) else { unreachable!() };
                goals.push(goal);
            }
        }
        canonical::instantiate_canonical_state(self.goal.infcx, self.goal.prog, self.goal.prev_universe, &mut values, self.final_state.expect("final state of an applicable candidate"));
        goals
    }
    pub fn instantiate_impl_args(&self) -> Vec<GenericArg> {
        let infcx = self.goal.infcx; let mut values = self.goal.orig_values.clone();
        for step in &self.steps {
            if let ProbeStep::RecordImplArgs { impl_args } = step {
                let StateData::Args(args) = canonical::instantiate_canonical_state(infcx, self.goal.prog, self.goal.prev_universe, &mut values, impl_args) else { unreachable!() };
                canonical::instantiate_canonical_state(infcx, self.goal.prog, self.goal.prev_universe, &mut values, self.final_state.expect("final state of an applicable candidate"));
                let mut resolver = crate::infer::resolve::DeepVariableResolver::new(infcx);
                let mut resolved = Vec::with_capacity(args.len());
                for arg in args { resolved.push(fold::fold_arg(&infcx.tcx.tys, arg, &mut resolver, 0)); }
                return resolved;
            }
        }
        panic!("impl candidate without recorded arguments")
    }
}
