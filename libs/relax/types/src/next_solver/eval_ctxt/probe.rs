// ported from N:rustc_next_trait_solver/src/solve/eval_ctxt/probe.rs @8d1a764
use super::*;
use crate::next_solver::search_graph::CandidateHeadUsages;

pub struct ProbeCtxt<'me, 'a, 'tcx> { ecx: &'me mut EvalCtxt<'a, 'tcx> }
impl<'a, 'tcx> EvalCtxt<'a, 'tcx> {
    pub fn probe(&mut self) -> ProbeCtxt<'_, 'a, 'tcx> { ProbeCtxt { ecx: self } }
}
impl ProbeCtxt<'_, '_, '_> {
    pub fn enter_single_candidate(self, source: super::super::assembly::CandidateSource, f: impl FnOnce(&mut EvalCtxt<'_, '_>) -> QueryResultOrRerunNonErased) -> (QueryResultOrRerunNonErased, CandidateHeadUsages) {
        if let Err(error) = self.ecx.opaque_accesses.should_bail() { return (Err(error), Default::default()); }
        self.ecx.search_graph.enter_single_candidate();
        let mut usages = CandidateHeadUsages::default();
        let result = self.enter_with_kind(|ecx| {
            let result = f(ecx);
            usages = ecx.search_graph.finish_single_candidate();
            ecx.inspect.probe_kind(super::super::inspect::ProbeKind::TraitCandidate {
                source, result: super::super::inspect::ProbeResult::new(&result),
            });
            result
        });
        (result, usages)
    }
    pub fn enter<T>(self, f: impl FnOnce(&mut EvalCtxt<'_, '_>) -> Result<T, NoSolutionOrRerunNonErased>) -> Result<T, NoSolutionOrRerunNonErased> {
        self.enter_with_kind(|ecx| {
            let result = f(ecx);
            ecx.inspect.probe_kind(super::super::inspect::ProbeKind::ProjectionCompatibility);
            result
        })
    }
    pub fn enter_with_kind<T>(self, f: impl FnOnce(&mut EvalCtxt<'_, '_>) -> Result<T, NoSolutionOrRerunNonErased>) -> Result<T, NoSolutionOrRerunNonErased> {
        let nested_goals = self.ecx.nested_goals.clone();
        self.enter_inner(f, nested_goals)
    }
    pub fn enter_without_propagated_nested_goals<T>(self, f: impl FnOnce(&mut EvalCtxt<'_, '_>) -> Result<T, NoSolutionOrRerunNonErased>) -> Result<T, NoSolutionOrRerunNonErased> {
        self.enter_inner(|ecx| {
            let result = f(ecx);
            ecx.inspect.probe_kind(super::super::inspect::ProbeKind::ProjectionCompatibility);
            result
        }, Vec::new())
    }
    fn enter_inner<T>(self, f: impl FnOnce(&mut EvalCtxt<'_, '_>) -> Result<T, NoSolutionOrRerunNonErased>, nested_goals: Vec<(GoalSource, Goal, Option<GoalStalledOn>)>) -> Result<T, NoSolutionOrRerunNonErased> {
        self.ecx.opaque_accesses.should_bail()?;
        let outer = self.ecx;
        let mut nested = EvalCtxt { inspect: outer.inspect.take_and_enter_probe(), delegate: outer.delegate, solver: outer.solver, search_graph: outer.search_graph,
            var_values: outer.var_values.clone(), var_kinds: outer.var_kinds.clone(),
            max_input_universe: outer.max_input_universe,
            initial_opaque_types_storage_num_entries: outer.initial_opaque_types_storage_num_entries,
            current_goal_kind: outer.current_goal_kind, nested_goals, opaque_accesses: Default::default(), tainted: outer.tainted };
        let result = nested.delegate.probe(|_| {
            let result = f(&mut nested);
            nested.inspect.probe_final_state(nested.delegate, nested.max_input_universe);
            result
        });
        outer.inspect = nested.inspect.finish_probe();
        outer.opaque_accesses.update(nested.opaque_accesses)?;
        result
    }
}
