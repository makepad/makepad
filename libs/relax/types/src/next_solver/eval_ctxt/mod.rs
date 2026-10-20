// ported from N:rustc_next_trait_solver/src/solve/eval_ctxt/mod.rs @8d1a764
use super::*;
use crate::infer::{InferCtxt, opaque_types::{OpaqueTypeStorageEntries, ProvisionalHiddenType}};
use crate::infer::relate::{TypeRelation, relate_args, solver_relating::SolverRelating};
use crate::infer::region_constraints::ConstraintKind;
use crate::types::{UniverseIndex, variance::V};
use crate::next_solver::search_graph::{SearchGraph, SearchGraphConfig, PathKind, LowerAvailableDepth};
use super::search_graph::SolverDelegate;
use super::canonical::*;
mod probe;
pub mod fast_path;
pub use probe::ProbeCtxt;

#[derive(Clone, Copy)]
pub(super) enum CurrentGoalKind { Misc, CoinductiveTrait, NormalizesTo }

pub struct EvalCtxt<'a, 'tcx> {
    pub(super) inspect: super::inspect::EvaluationStepBuilder,
    pub(super) delegate: &'a InferCtxt<'tcx>,
    pub(super) solver: &'a mut SolverDelegate<'tcx>,
    pub(super) search_graph: &'a mut SearchGraph,
    pub(super) var_values: Vec<GenericArg>,
    pub(super) var_kinds: Vec<CanonicalVarKind>,
    pub(super) max_input_universe: UniverseIndex,
    pub(super) initial_opaque_types_storage_num_entries: OpaqueTypeStorageEntries,
    pub(super) current_goal_kind: CurrentGoalKind,
    pub(super) nested_goals: Vec<(GoalSource, Goal, Option<GoalStalledOn>)>,
    pub(super) opaque_accesses: AccessedOpaques,
    pub(super) tainted: Result<(), NoSolution>,
}

#[derive(Clone, Debug)]
pub struct GoalEvaluation { pub goal: Goal, pub certainty: Certainty, pub has_changed: bool, pub stalled_on: Option<GoalStalledOn> }

impl InferCtxt<'_> {
    pub(crate) fn enter_root<T>(&self, prog: &crate::program::Program, f: impl FnOnce(&mut EvalCtxt<'_, '_>) -> T) -> T {
        self.enter_root_with_depth(prog, self.recursion_limit(prog), f)
    }
    fn enter_root_with_depth<T>(&self, prog: &crate::program::Program, root_depth: usize, f: impl FnOnce(&mut EvalCtxt<'_, '_>) -> T) -> T {
        let mut graph = SearchGraph::new(SearchGraphConfig { recursion_limit: root_depth, ..Default::default() });
        let mut solver = SolverDelegate::new(self.tcx, prog);
        solver.cause = self.cause; solver.limit = self.recursion_limit;
        let mut ecx = EvalCtxt { inspect: Default::default(), delegate: self, solver: &mut solver, search_graph: &mut graph,
            var_values: Vec::new(), var_kinds: Vec::new(), max_input_universe: UniverseIndex::ROOT,
            initial_opaque_types_storage_num_entries: Default::default(), current_goal_kind: CurrentGoalKind::Misc,
            nested_goals: Vec::new(), opaque_accesses: Default::default(), tainted: Ok(()) };
        let result = f(&mut ecx);
        assert!(!ecx.opaque_accesses.might_rerun());
        assert!(ecx.nested_goals.is_empty());
        assert!(graph.is_empty());
        result
    }
    /// rustc_next_trait_solver/src/solve/eval_ctxt/mod.rs:302-313.
    pub fn root_goal_may_hold_with_depth(&self, prog: &crate::program::Program, depth: usize, goal: Goal) -> bool {
        self.probe(|_| self.enter_root_with_depth(prog, depth, |ecx| {
            ecx.evaluate_goal(GoalSource::Misc, goal)
        }).is_ok())
    }
    pub fn evaluate_root_goal(&self, prog: &crate::program::Program, goal: Goal, stalled_on: Option<&GoalStalledOn>) -> Result<GoalEvaluation, NoSolution> {
        if !fast_path::rerunning_stalled_goal_may_make_progress(self, stalled_on) {
            return Ok(GoalEvaluation { goal, certainty: Certainty::Maybe(stalled_on.unwrap().stalled_maybe_info), has_changed: false, stalled_on: stalled_on.cloned() });
        }
        if stalled_on.is_some() {
            if let Some(result) = fast_path::compute_goal_fast_path(self, prog, &goal) { return Ok(result); }
        }
        let evaluate = |ecx: &mut EvalCtxt<'_, '_>| ecx.evaluate_goal_raw(GoalSource::Misc, goal.clone()).map(|(nested, result)| { assert!(nested.is_empty()); result });
        let result = self.enter_root(prog, evaluate);
        result.map_err(NoSolutionOrRerunNonErased::assert_no_rerun)
    }
    /// rustc_next_trait_solver/src/solve/eval_ctxt/mod.rs:283-300.
    pub fn root_goal_may_hold_opaque_types_jank(&self, prog: &crate::program::Program, goal: Goal) -> bool {
        self.probe(|_| self.evaluate_root_goal(prog, goal, None).is_ok_and(|r| match r.certainty {
            Certainty::Yes => true,
            Certainty::Maybe(m) => m.opaque_types_jank == OpaqueTypesJank::AllGood,
        }))
    }

    pub(crate) fn normalize_ty_with_universes(&self, prog: &crate::program::Program, env: &ParamEnvId, ty: TyId) -> Result<(TyId, Vec<Goal>), NoSolution> {
        self.enter_root(prog, |ecx| {
            let result = ecx.normalize_ty(env, ty);
            let goals = std::mem::take(&mut ecx.nested_goals).into_iter().map(|(_, g, _)| g).collect();
            result.map(|ty| (ty, goals)).map_err(NoSolutionOrRerunNonErased::assert_no_rerun)
        })
    }
    pub(crate) fn normalize_predicate_with_universes(&self, prog: &crate::program::Program, env: &ParamEnvId, p: &Predicate) -> Result<(Predicate, Vec<Goal>), NoSolution> {
        self.enter_root(prog, |ecx| {
            let result = ecx.normalize_predicate(env, p);
            let goals = std::mem::take(&mut ecx.nested_goals).into_iter().map(|(_, g, _)| g).collect();
            result.map(|p| (p, goals)).map_err(NoSolutionOrRerunNonErased::assert_no_rerun)
        })
    }

}

impl<'a, 'tcx> EvalCtxt<'a, 'tcx> {
    pub(super) fn enter_canonical(solver: &mut SolverDelegate<'tcx>, graph: &mut SearchGraph, input: &CanonicalQueryInput, proof_tree: &mut super::inspect::ProofTreeBuilder) -> (QueryResult, AccessedOpaques) {
        let mut infcx = InferCtxt::new(solver.tcx, input.typing_mode.0, solver.limit);
        infcx.coherence_crate = input.coherence_crate;

        infcx.cause = solver.cause;
        let values = instantiate_canonical_vars(&infcx, input.canonical.max_universe, &input.canonical.var_kinds);
        let instantiated = instantiate_query_input(infcx.tcx, &input.canonical.value, &values);
        for (key, ty) in instantiated.predefined_opaques_in_body {
            infcx.inner.borrow_mut().opaque_types().register(key, ProvisionalHiddenType::new(ty));
        }
        let initial = infcx.inner.borrow_mut().opaque_types().num_entries();
        let kind = match &instantiated.goal.predicate.kind {
            PredicateKind::Clause(ClauseKind::Trait(p)) if [crate::lang_items::LangItem::Sized, crate::lang_items::LangItem::MetaSized, crate::lang_items::LangItem::PointeeSized].iter().any(|&item| solver.tcx.lang_items.get(item) == Some(p.trait_ref.def_id))
                || trait_is_auto(solver.prog, p.trait_ref.def_id) => CurrentGoalKind::CoinductiveTrait,
            PredicateKind::NormalizesTo(_) => CurrentGoalKind::NormalizesTo,
            _ => CurrentGoalKind::Misc,
        };
        let mut ecx = EvalCtxt { inspect: proof_tree.new_evaluation_step(values.clone()), delegate: &infcx, solver, search_graph: graph, var_values: values,
            var_kinds: input.canonical.var_kinds.clone(), max_input_universe: input.canonical.max_universe,
            initial_opaque_types_storage_num_entries: initial, current_goal_kind: kind,
            nested_goals: Vec::new(), opaque_accesses: Default::default(), tainted: Ok(()) };
        let result = ecx.compute_goal(instantiated.goal);
        let result = match result {
            Ok(r) => Ok(r), Err(NoSolutionOrRerunNonErased::NoSolution) => Err(NoSolution),
            Err(NoSolutionOrRerunNonErased::RerunNonErased) => { assert!(ecx.opaque_accesses.should_bail().is_err()); Err(NoSolution) }
        };
        ecx.inspect.probe_kind(super::inspect::ProbeKind::Root { result: super::inspect::ProbeResult::new(&result) });
        ecx.inspect.probe_final_state(&infcx, ecx.max_input_universe);
        proof_tree.finish_evaluation_step(ecx.inspect);
        (result, ecx.opaque_accesses)
    }
    /// rustc_next_trait_solver/src/solve/eval_ctxt/mod.rs:296-333: the cycle step kind of a
    /// nested goal, from where it comes from and what the current goal is.
    pub(super) fn step_kind_for_source(&self, source: GoalSource) -> PathKind {
        match source {
            GoalSource::Misc => PathKind::Unknown,
            GoalSource::NormalizeGoal(path_kind) => path_kind,
            GoalSource::ImplWhereBound => match self.current_goal_kind {
                // only a where-clause of a coinductive trait is a productive step
                CurrentGoalKind::CoinductiveTrait => PathKind::Coinductive,
                // accessing the associated item steps out of the impl again
                CurrentGoalKind::NormalizesTo => PathKind::Inductive,
                CurrentGoalKind::Misc => PathKind::Unknown,
            },
            GoalSource::TypeRelating => PathKind::Inductive,
            GoalSource::AliasBoundConstCondition | GoalSource::AliasWellFormed => PathKind::Unknown,
        }
    }
    /// rustc relate(): the goals a relation emits (`AliasRelate` is a `Projection` here).
    pub(super) fn add_relate_goals(&mut self, goals: Vec<Goal>) -> Result<(), NoSolutionOrRerunNonErased> {
        for goal in goals {
            let source = match goal.predicate.kind {
                PredicateKind::Subtype(_) | PredicateKind::Clause(ClauseKind::Projection(_)) => GoalSource::TypeRelating,
                PredicateKind::Clause(ClauseKind::WellFormed(_)) => GoalSource::Misc,
                ref p => unreachable!("unexpected nested goal in `relate`: {p:?}"),
            };
            self.add_goal(source, goal)?;
        }
        Ok(())
    }
    pub(super) fn evaluate_goal_raw(&mut self, source: GoalSource, goal: Goal) -> Result<(Vec<(GoalSource, Goal)>, GoalEvaluation), NoSolutionOrRerunNonErased> {
        let step = self.step_kind_for_source(source);
        let goal = fold::fold_goal(self.delegate.tcx, &goal, &mut crate::infer::resolve::DeepVariableResolver::new(self.delegate));
        let opaque_types = self.delegate.resolved_opaque_types();
        let mode = self.delegate.typing_mode.get();
        if mode.is_erased_not_coherence() { assert!(opaque_types.is_empty()); }
        let skip_erased = matches!(mode, crate::tcx::TypingMode::Coherence | crate::tcx::TypingMode::Reflection) ||
            (matches!(goal.predicate.kind, PredicateKind::Clause(ClauseKind::Trait(_))) && opaque_types.iter().any(|(_, t)| matches!(self.delegate.tcx.tys.kind(self.delegate.shallow_resolve(*t)), crate::types::TyKind::Infer(_))));
        let (original, response, previously_succeeded_in_erased, num_opaques_in_storage) = 'canonicalize: {
            if skip_erased {
                if matches!(mode, crate::tcx::TypingMode::ErasedNotCoherence) { return Err(self.opaque_accesses.rerun_always()); }
            } else {
                let (original, input) = canonicalize_goal(self.delegate, &goal, &[], crate::tcx::TypingMode::ErasedNotCoherence);
                let (result, accesses) = self.evaluate_in_search_graph(step, input);
                match typing_mode::should_rerun_after_erased_canonicalization(self.delegate.tcx, &accesses.rerun, mode, &opaque_types) {
                    typing_mode::RerunDecision::No => break 'canonicalize (original, result.as_ref().map_err(|e| *e)?, SucceededInErased::Yes { accessed_opaques: accesses.clone() }, 0),
                    typing_mode::RerunDecision::EagerlyPropagateToParent => { self.opaque_accesses.update(accesses.clone())?; break 'canonicalize (original, result.as_ref().map_err(|e| *e)?, SucceededInErased::No, 0); }
                    typing_mode::RerunDecision::Yes => {},
                }
            }
            let (original, input) = canonicalize_goal(self.delegate, &goal, &opaque_types, mode);
            let (result, accesses) = self.evaluate_in_search_graph(step, input);
            assert!(!accesses.might_rerun());
            (original, result.as_ref().map_err(|e| *e)?, SucceededInErased::No, opaque_types.len())
        };
        let has_changed = !has_only_region_constraints(&self.delegate.tcx.tys, &response);
        let (nested, certainty) = canonical::response::instantiate_and_apply_query_response(self.delegate, self.solver.prog, &original, &response);
        let stalled_on = match certainty {
            Certainty::Maybe(maybe) if !has_changed => Some(fast_path::build_stalled_on(self.delegate, &original, maybe, GoalStalledOnOpaques::Yes { num_opaques_in_storage, previously_succeeded_in_erased })),
            _ => None,
        };
        Ok((nested, GoalEvaluation { goal, certainty, has_changed, stalled_on }))
    }
    fn evaluate_in_search_graph(&mut self, step: PathKind, input: CanonicalQueryInput) -> &'tcx (QueryResult, AccessedOpaques) {
        let lower_depth = match &input.canonical.value.goal.predicate.kind {
            PredicateKind::NormalizesTo(_) => LowerAvailableDepth::No,
            PredicateKind::Clause(ClauseKind::Trait(p)) if trait_is_auto(self.solver.prog, p.trait_ref.def_id) => {
                let GenericArg::Type(ty) = p.trait_ref.args[0] else { unreachable!() };
                if let crate::types::TyKind::Alias(crate::consts::IsRigid::Yes, alias) = self.delegate.tcx.tys.kind(ty) {
                    if let crate::types::AliasTyKind::Opaque { def_id } = alias.kind {
                        // eval_ctxt/mod.rs:645-713: revealing an opaque for an auto
                        // trait consumes no depth unless an item bound proves it.
                        let has_bound = self.delegate.tcx.item_bounds(self.solver.prog, def_id).iter().any(|bound| {
                            matches!(&bound.kind, PredicateKind::Clause(ClauseKind::Trait(t)) if t.trait_ref.def_id == p.trait_ref.def_id && matches!(t.trait_ref.args.first(), Some(GenericArg::Type(t)) if matches!(self.delegate.tcx.tys.kind(*t), crate::types::TyKind::Alias(_, a) if a.kind.def_id() == def_id)))
                        });
                        if has_bound { LowerAvailableDepth::Yes } else { LowerAvailableDepth::No }
                    } else { LowerAvailableDepth::Yes }
                } else { LowerAvailableDepth::Yes }
            }
            _ => LowerAvailableDepth::Yes,
        };
        let id = self.solver.input(input);
        let id = self.search_graph.evaluate_goal(self.solver, id, step, lower_depth, &mut Default::default());
        self.solver.result(id)
    }
    pub(super) fn evaluate_goal(&mut self, source: GoalSource, goal: Goal) -> Result<GoalEvaluation, NoSolutionOrRerunNonErased> {
        if let Some(result) = fast_path::compute_goal_fast_path(self.delegate, self.solver.prog, &goal) { return Ok(result); }
        let (nested, result) = self.evaluate_goal_raw(source, goal)?;
        assert!(nested.is_empty());
        Ok(result)
    }
    pub(super) fn add_goal(&mut self, source: GoalSource, mut goal: Goal) -> Result<(), NoSolutionOrRerunNonErased> { goal.predicate = self.normalize_predicate_for_source(source, &goal.param_env, &goal.predicate)?; self.inspect.add_goal(self.delegate, self.max_input_universe, source, goal.clone()); if let Some(result) = fast_path::compute_goal_fast_path(self.delegate, self.solver.prog, &goal) {
            if !result.certainty.is_yes() { self.nested_goals.push((source, goal, result.stalled_on)); }
        } else { self.nested_goals.push((source, goal, None)); } Ok(()) }
    pub(super) fn add_goals(&mut self, source: GoalSource, goals: Vec<Goal>) -> Result<(), NoSolutionOrRerunNonErased> {
        for goal in goals { self.add_goal(source, goal)?; }
        Ok(())
    }
    pub(super) fn eq(&mut self, env: &ParamEnvId, a: Term, b: Term) -> Result<(), NoSolutionOrRerunNonErased> {
        let mut relate = SolverRelating::new(self.delegate, self.solver.prog, V::Inv, env.clone());
        match (a, b) {
            (Term::Ty(a), Term::Ty(b)) => { relate.tys(a, b).map_err(|_| NoSolution)?; }
            (Term::Const(a), Term::Const(b)) => { relate.consts(a, b).map_err(|_| NoSolution)?; }
            _ => return Err(NoSolution.into()),
        }
        self.add_relate_goals(relate.goals)
    }
    pub(super) fn eq_args(&mut self, env: &ParamEnvId, a: &[GenericArg], b: &[GenericArg]) -> Result<(), NoSolutionOrRerunNonErased> {
        let mut relate = SolverRelating::new(self.delegate, self.solver.prog, V::Inv, env.clone());
        relate_args(&mut relate, a, b).map_err(|_| NoSolution)?;
        self.add_relate_goals(relate.goals)
    }
    pub(super) fn try_evaluate_added_goals(&mut self) -> Result<Certainty, NoSolutionOrRerunNonErased> {
        for _ in 0..8 {
            let mut unchanged_certainty = Some(Certainty::Yes);
            for (source, goal, stalled_on) in std::mem::take(&mut self.nested_goals) {
                if !fast_path::rerunning_stalled_goal_may_make_progress(self.delegate, stalled_on.as_ref()) {
                    unchanged_certainty = unchanged_certainty.map(|c| c.and(Certainty::Maybe(stalled_on.as_ref().unwrap().stalled_maybe_info)));
                    self.nested_goals.push((source, goal, stalled_on)); continue;
                }
                let result = match self.evaluate_goal(source, goal) {
                    Ok(result) => result,
                    Err(e) => { if e == NoSolutionOrRerunNonErased::NoSolution { self.tainted = Err(NoSolution); } return Err(e); }
                };
                if result.has_changed { unchanged_certainty = None; }
                if !result.certainty.is_yes() {
                    self.nested_goals.push((source, result.goal, result.stalled_on));
                    unchanged_certainty = unchanged_certainty.map(|c| c.and(result.certainty));
                }
            }
            if let Some(certainty) = unchanged_certainty { return Ok(certainty); }
        }
        Ok(Certainty::overflow(false))
    }
    pub(super) fn evaluate_added_goals_and_make_canonical_response(&mut self, shallow: Certainty) -> QueryResultOrRerunNonErased {
        self.inspect.make_canonical_response(shallow);
        let nested = self.try_evaluate_added_goals()?;
        assert_eq!(self.tainted, Ok(()));
        self.delegate.leak_check(self.max_input_universe, None).map_err(|_| NoSolution)?;
        let mut external_constraints = ExternalConstraintsData::default();
        let certainty = if matches!(self.current_goal_kind, CurrentGoalKind::NormalizesTo) && shallow.is_yes() {
            debug_assert!(!self.nested_goals.is_empty() || nested.is_yes());
            for (source, goal, _) in std::mem::take(&mut self.nested_goals) { external_constraints.normalization_nested_goals.push((source, goal)); }
            Certainty::Yes
        } else { shallow.and(nested) };
        if matches!(certainty, Certainty::Maybe(MaybeInfo { cause: MaybeCause::Overflow { keep_constraints: false, .. }, .. })) { return Ok(self.make_ambiguous_response_no_constraints(match certainty { Certainty::Maybe(m) => m, _ => unreachable!() })); }
        let mut inner = self.delegate.inner.borrow_mut();
        if certainty.is_yes() {
            for c in inner.unwrap_region_constraints().constraints() {
                let constraint = match c.kind {
                    ConstraintKind::VarEqVar | ConstraintKind::VarEqReg | ConstraintKind::RegEqReg => RegionConstraint::Eq(c.sub, c.sup),
                    _ => RegionConstraint::RegionOutlives(c.sup, c.sub),
                };
                if c.sub != c.sup && !external_constraints.region_constraints.contains(&(constraint, c.visible_for_leak_check)) {
                    external_constraints.region_constraints.push((constraint, c.visible_for_leak_check));
                }
            }
            for &(ty, region, _) in inner.region_obligations() {
                external_constraints.region_constraints.push((RegionConstraint::TypeOutlives(ty, region), false));
            }
        }
        for (key, ty) in inner.opaque_types().opaque_types_added_since(self.initial_opaque_types_storage_num_entries) {
            external_constraints.opaque_types.push((key, ty.ty));
        }
        drop(inner);
        Ok(canonical::response::canonicalize_query_response(self.delegate, self.max_input_universe,
            &Response { certainty, var_values: self.var_values.clone(), external_constraints }))
    }
    pub(super) fn make_ambiguous_response_no_constraints(&self, maybe: MaybeInfo) -> Canonical<Response> {
        response_no_constraints_raw(&self.delegate.tcx.tys, self.max_input_universe,
            &self.var_kinds, Certainty::Maybe(maybe))
    }
}

pub(crate) fn trait_is_auto(prog: &crate::program::Program, def: crate::program::DefId) -> bool {
    let def = prog.def(def);
    { let ast = &prog.files[def.file as usize].ast; ast.as_trait(&ast.item(def.item).kind).is_some_and(|t| t.auto) }
}

impl EvalCtxt<'_, '_> {
    pub(super) fn fresh_args_for_item(&mut self, def: crate::program::DefId) -> Vec<GenericArg> {
        let args = self.delegate.fresh_args_for_item(self.solver.prog, def);
        for &arg in &args { self.inspect.add_var_value(arg); } args
    }
}
impl InferCtxt<'_> {
    pub fn evaluate_root_goal_for_proof_tree(&self, prog: &crate::program::Program, goal: Goal) -> super::inspect::GoalEvaluation {
        let depth = self.recursion_limit(prog);
        self.evaluate_root_goal_for_proof_tree_with_depth(prog, goal, depth)
    }
    fn evaluate_root_goal_for_proof_tree_with_depth(&self, prog: &crate::program::Program, goal: Goal, root_depth: usize) -> super::inspect::GoalEvaluation {
        assert!(!self.typing_mode.get().is_erased_not_coherence());
        let goal = fold::fold_goal(self.tcx, &goal, &mut crate::infer::resolve::DeepVariableResolver::new(self));
        let opaques = self.resolved_opaque_types();
        let (original, input) = canonicalize_goal(self, &goal, &opaques, self.typing_mode.get());
        let mut solver = SolverDelegate::new(self.tcx, prog);
        solver.cause = self.cause; solver.limit = self.recursion_limit;
        let input = solver.input(input);
        let root = solver.evaluate_root_goal_for_proof_tree_raw_provider(input, root_depth);
        let result = solver.result(root.result).0.clone();
        if let Ok(response) = &result {
            let (nested, _) = canonical::response::instantiate_and_apply_query_response(self, prog, &original, response); assert!(nested.is_empty());
        }
        super::inspect::GoalEvaluation { uncanonicalized_goal: goal, orig_values: original, final_revision: root.final_revision, result, required_depth: root.required_depth }
    }
}

impl InferCtxt<'_> {
    /// eval_ctxt/mod.rs:488-515: resolve the complete predefined-opaque tuple,
    /// including arguments, before canonicalizing the next goal.
    fn resolved_opaque_types(&self) -> Vec<(crate::infer::opaque_types::OpaqueTypeKey, crate::types::TyId)> {
        use crate::types::fold::TypeFolder;
        let entries = self.inner.borrow_mut().opaque_types().iter_lookup_table().to_vec();
        let mut resolver = crate::infer::resolve::DeepVariableResolver::new(self);
        let mut result = Vec::new();
        for (key, hidden) in entries {
            let mut args = Vec::with_capacity(key.args.len());
            for &arg in &*key.args { args.push(fold::fold_arg(&self.tcx.tys, arg, &mut resolver, 0)); }
            result.push((crate::infer::opaque_types::OpaqueTypeKey { def_id: key.def_id, args: args.into() }, resolver.fold_ty(&self.tcx.tys, hidden.ty, 0)));
        }
        result
    }
}
