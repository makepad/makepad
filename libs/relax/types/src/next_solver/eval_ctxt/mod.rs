// ported from N:rustc_next_trait_solver/src/solve/eval_ctxt/mod.rs @8d1a764
use super::*;
use crate::infer::{InferCtxt, opaque_types::{OpaqueTypeStorageEntries, ProvisionalHiddenType}};
use crate::infer::relate::{TypeRelation, relate_args, solver_relating::SolverRelating};
use crate::infer::region_constraints::ConstraintKind;
use crate::types::{TyKind, UniverseIndex, variance::V};
use crate::next_solver::search_graph::{SearchGraph, SearchGraphConfig, PathKind, LowerAvailableDepth};
use super::search_graph::SolverDelegate;
use super::canonical::*;
mod probe;
pub mod fast_path;
pub use probe::ProbeCtxt;

std::thread_local! {
    static CODEGEN_CAUSE: std::cell::Cell<(crate::program::DefId, u32)> = const { std::cell::Cell::new((crate::program::NO_DEF, 0)) };
}

pub struct CodegenCauseGuard {
    prev: (crate::program::DefId, u32),
}

impl Drop for CodegenCauseGuard {
    fn drop(&mut self) {
        CODEGEN_CAUSE.with(|c| c.set(self.prev));
    }
}

pub fn enter_codegen_cause(def: crate::program::DefId, span: u32) -> CodegenCauseGuard {
    let prev = CODEGEN_CAUSE.with(|c| c.replace((def, span)));
    CodegenCauseGuard { prev }
}

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
        let limit = self.recursion_limit(prog);
        let evaluate = |ecx: &mut EvalCtxt<'_, '_>| ecx.evaluate_goal_raw(GoalSource::Misc, goal.clone()).map(|(nested, result)| { assert!(nested.is_empty()); result });
        let result = self.enter_root_with_depth(prog, limit, evaluate).map_err(NoSolutionOrRerunNonErased::assert_no_rerun);
        if !self.typing_mode.get().is_coherence() && result.as_ref().is_ok_and(|r| r.certainty.is_overflow()) {
            let snapshot = self.start_snapshot();
            let retry = self.enter_root_with_depth(prog, limit.saturating_mul(2), evaluate).map_err(NoSolutionOrRerunNonErased::assert_no_rerun);
            if let Ok(eval) = retry {
                if eval.certainty == Certainty::Yes {
                    self.commit_from(snapshot);
                    self.emit_next_solver_overflow_fcw(prog, &goal, limit);
                    return Ok(eval);
                }
            }
            self.rollback_to(snapshot);
        }
        result
    }
    pub(crate) fn emit_next_solver_overflow_fcw(&self, prog: &crate::program::Program, goal: &Goal, limit: usize) {
        let ty_str = |t: TyId| crate::typeck::ty_to_string(false, prog, self.tcx, self.deeply_resolve_ignoring_regions(t));
        let pred = match &goal.predicate.kind {
            PredicateKind::Clause(ClauseKind::Trait(p)) => {
                let mut args = Vec::new();
                for arg in &p.trait_ref.args {
                    args.push(match arg {
                        crate::consts::GenericArg::Type(t) => ty_str(*t),
                        crate::consts::GenericArg::Const(c) => self.tcx.tys.const_to_string(*c),
                        crate::consts::GenericArg::Lifetime(r) => format!("{r:?}"),
                    });
                }
                let name = prog.name(p.trait_ref.def_id).to_string();
                let self_ty = args.first().cloned().unwrap_or_default();
                let tr = if args.len() > 1 { format!("{}<{}>", name, args[1..].join(", ")) } else { name };
                format!("{self_ty}: {tr}")
            }
            PredicateKind::Clause(ClauseKind::Projection(p)) => match (&p.projection_term, &p.term) {
                (AliasTerm::Ty(a), Term::Ty(t)) => format!("{} == {}", ty_str(*a), ty_str(*t)),
                _ => format!("{:?}", goal.predicate),
            },
            PredicateKind::NormalizesTo(p) => match (&p.alias, &p.term) {
                (AliasTerm::Ty(a), Term::Ty(t)) => format!("{} == {}", ty_str(*a), ty_str(*t)),
                _ => format!("{:?}", goal.predicate),
            },
            PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(t))) => {
                let resolved = self.deeply_resolve_ignoring_regions(*t);
                let s = match self.tcx.tys.kind(resolved) {
                    TyKind::FnDef(d, args, _) => {
                        let args_list = self.tcx.tys.list(*args);
                        let mut name = prog.name(*d).to_string();
                        if !args_list.is_empty() {
                            let rendered: Vec<String> = args_list.iter().map(|&a| ty_str(a)).collect();
                            name.push_str(&format!("::<{}>", rendered.join(", ")));
                        }
                        if let Some(sig) = self.tcx.fn_sig(prog, *d) {
                            let params: Vec<String> = sig.params.iter().map(|&p| ty_str(self.tcx.tys.subst(p, args_list))).collect();
                            let ret = self.tcx.tys.subst(sig.ret, args_list);
                            if ret == self.tcx.tys.unit {
                                format!("fn({}) {{{name}}}", params.join(", "))
                            } else {
                                format!("fn({}) -> {} {{{name}}}", params.join(", "), ty_str(ret))
                            }
                        } else {
                            format!("fn() {{{name}}}")
                        }
                    }
                    _ => ty_str(*t),
                };
                format!("{s} well-formed")
            }
            _ => format!("{:?}", goal.predicate),
        };
        let (def, file, span) = if self.cause.body_id != crate::program::NO_DEF && (self.cause.body_id.0 as usize) < prog.defs.len() {
            let d = prog.def(self.cause.body_id);
            let span = if self.origin_span.get() != 0 { self.origin_span.get() } else { self.cause.span };
            (self.cause.body_id, d.file, span)
        } else if let Some(site) = self.goal_codegen_site(prog, goal) {
            site
        } else if !prog.crates.is_empty() {
            let krate = (crate::layout::root_crate() as usize).min(prog.crates.len() - 1);
            let m = &prog.mods[prog.crates[krate].root_mod as usize];
            (m.def, m.file, 0)
        } else {
            return;
        };
        let krate = prog.file_crate.get(file as usize).copied().unwrap_or_else(crate::layout::root_crate) as usize;
        let krate = krate.min(prog.crates.len().saturating_sub(1));
        let crate_root_def = prog.mods[prog.crates[krate].root_mod as usize].def;
        let limit_krate = if self.cause.body_id != crate::program::NO_DEF && (self.cause.body_id.0 as usize) < prog.defs.len() {
            prog.def(self.cause.body_id).krate as usize
        } else {
            crate::layout::root_crate() as usize
        }.min(prog.crates.len().saturating_sub(1));
        let crate_name = prog.syms.str(prog.crates[limit_krate].name);
        let next = limit.saturating_mul(2);
        let msg = format!(
            "overflow evaluating the requirement `{pred}`\n  = help: consider increasing the recursion limit by adding a `#![recursion_limit = \"{next}\"]` attribute to your crate (`{crate_name}`)\n  = help: or consider adding a manual `impl` of auto traits like `Send` for intermediate types, if auto traits are involved\n  = note: this lint is attached to the whole crate and can't be disabled on a per-function basis\n  = warning: this was previously accepted by the compiler but is being phased out; it will become a hard error in a future release!\n  = note: for more information, see issue #159228 <https://github.com/rust-lang/rust/issues/159228>"
        );
        let _ = def;
        if crate::lint::enabled() {
            crate::lint::emit(prog, crate_root_def, &crate::lint::RECURSION_DEPTH_EXCEEDING_LIMIT, file, span, &msg);
        } else {
            let mut spec = crate::lint::resolve_in(prog, crate_root_def, file, &[], &crate::lint::RECURSION_DEPTH_EXCEEDING_LIMIT, None);
            let root_krate = (crate::layout::root_crate() as usize).min(prog.crates.len().saturating_sub(1));
            if spec.level == crate::lint::Level::Warn && root_krate != krate {
                let root_def = prog.mods[prog.crates[root_krate].root_mod as usize].def;
                let root_spec = crate::lint::resolve_in(prog, root_def, prog.mods[prog.crates[root_krate].root_mod as usize].file, &[], &crate::lint::RECURSION_DEPTH_EXCEEDING_LIMIT, None);
                if root_spec.level != crate::lint::Level::Warn {
                    spec = root_spec;
                }
            }
            if spec.level >= crate::lint::Level::Deny {
                self.errors.report(self.cause, format!("{} [{}]", msg, crate::lint::RECURSION_DEPTH_EXCEEDING_LIMIT.name));
            } else if spec.level == crate::lint::Level::Warn {
                let (l, c) = prog.files[file as usize].line_col(span);
                let line = match msg.split_once('\n') {
                    Some((head, tail)) => format!("{}:{}:{}: warning: {} [{}]\n{}", prog.file_paths[file as usize], l, c, head, crate::lint::RECURSION_DEPTH_EXCEEDING_LIMIT.name, tail),
                    None => format!("{}:{}:{}: warning: {} [{}]", prog.file_paths[file as usize], l, c, msg, crate::lint::RECURSION_DEPTH_EXCEEDING_LIMIT.name),
                };
                static EMITTED: relax_session::sync::Mutex<Option<std::collections::HashSet<String>>> = relax_session::sync::Mutex::new(None);
                let _k = crate::zone_alloc::kept();
                if EMITTED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(line.clone()) {
                    eprintln!("{line}");
                }
            }
        }
    }

    fn goal_codegen_site(&self, prog: &crate::program::Program, goal: &Goal) -> Option<(crate::program::DefId, u32, u32)> {
        let mut visited = std::collections::HashSet::new();
        let mut fallback = None;
        let mut roots = Vec::new();
        match &goal.predicate.kind {
            PredicateKind::Clause(ClauseKind::Trait(p)) => {
                for arg in &p.trait_ref.args {
                    if let crate::consts::GenericArg::Type(t) = arg { roots.push(*t); }
                }
            }
            PredicateKind::Clause(ClauseKind::Projection(p)) => {
                if let AliasTerm::Ty(a) = p.projection_term { roots.push(a); }
                if let Term::Ty(t) = p.term { roots.push(t); }
            }
            PredicateKind::NormalizesTo(p) => {
                if let AliasTerm::Ty(a) = p.alias { roots.push(a); }
                if let Term::Ty(t) = p.term { roots.push(t); }
            }
            PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(t))) | PredicateKind::Clause(ClauseKind::ConstArgHasType(_, t)) => {
                roots.push(*t);
            }
            PredicateKind::Subtype(p) => { roots.push(p.a); roots.push(p.b); }
            PredicateKind::Coerce(p) => { roots.push(p.a); roots.push(p.b); }
            _ => {}
        }
        for root in roots {
            if let Some(hit) = self.closure_site_in_ty(prog, root, &mut visited, &mut fallback) {
                return Some(hit);
            }
        }
        if fallback.is_some() {
            return fallback;
        }
        let (c_def, c_span) = CODEGEN_CAUSE.with(|c| c.get());
        if c_def != crate::program::NO_DEF && (c_def.0 as usize) < prog.defs.len() {
            let d = prog.def(c_def);
            return Some((c_def, d.file, c_span));
        }
        None
    }

    fn closure_site_in_ty(
        &self,
        prog: &crate::program::Program,
        ty: TyId,
        visited: &mut std::collections::HashSet<TyId>,
        fallback: &mut Option<(crate::program::DefId, u32, u32)>,
    ) -> Option<(crate::program::DefId, u32, u32)> {
        let ty = self.deeply_resolve_ignoring_regions(ty);
        if !visited.insert(ty) {
            return None;
        }
        match self.tcx.tys.kind(ty) {
            TyKind::Closure(file, expr, args) => {
                for child in [args.tupled_upvars_ty, args.parent_args, args.closure_sig_as_fn_ptr_ty] {
                    if let Some(hit) = self.closure_site_in_ty(prog, child, visited, fallback) {
                        return Some(hit);
                    }
                }
                let node = crate::def_collector::NodeId::Closure(crate::ast::ExprId(*expr & !crate::tcx::ASYNC_FN_BODY));
                let def = prog.node_id_to_def_id.get(&(*file, node)).copied().or_else(|| {
                    match self.tcx.tys.kind(args.parent_args) {
                        TyKind::FnDef(d, _, _) => Some(*d),
                        _ => None,
                    }
                })?;
                if (def.0 as usize) >= prog.defs.len() {
                    return None;
                }
                let mut top = def;
                while top != crate::program::NO_DEF && matches!(prog.def(top).kind, crate::program::DefKind::Closure | crate::program::DefKind::AnonConst) {
                    top = prog.def(top).parent;
                }
                let body_loaded = top != crate::program::NO_DEF
                    && prog.def(top).file == *file
                    && prog.def(top).item.0 != u32::MAX
                    && prog.fn_body(*file, prog.def(top).item).is_ok();
                let span = if *expr & crate::tcx::ASYNC_FN_BODY != 0 {
                    if top != crate::program::NO_DEF && prog.def(top).file == *file && prog.def(top).item.0 != u32::MAX {
                        prog.files[*file as usize].ast.item(prog.def(top).item).lo
                    } else {
                        0
                    }
                } else if body_loaded {
                    prog.files[*file as usize].ast.expr(crate::ast::ExprId(*expr)).lo
                } else {
                    0
                };
                let krate = prog.def(def).krate;
                let site = (def, *file, span);
                if (krate as usize) < prog.crates.len() && (crate::borrowck::event::app_crate(prog, krate) || prog.crates[krate as usize].validate) {
                    return Some(site);
                }
                if fallback.is_none() {
                    *fallback = Some(site);
                }
                None
            }
            TyKind::Adt(_, args, _) | TyKind::FnDef(_, args, _) => {
                for &arg in self.tcx.tys.list(*args) {
                    if let Some(hit) = self.closure_site_in_ty(prog, arg, visited, fallback) {
                        return Some(hit);
                    }
                }
                None
            }
            TyKind::Tuple(tys) | TyKind::FnPtr(tys, _, _, _) => {
                for &arg in self.tcx.tys.list(*tys) {
                    if let Some(hit) = self.closure_site_in_ty(prog, arg, visited, fallback) {
                        return Some(hit);
                    }
                }
                if let TyKind::FnPtr(_, ret, _, _) = self.tcx.tys.kind(ty) {
                    if let Some(hit) = self.closure_site_in_ty(prog, *ret, visited, fallback) {
                        return Some(hit);
                    }
                }
                None
            }
            TyKind::Ref(_, inner, _) | TyKind::Ptr(_, inner) | TyKind::Slice(inner) | TyKind::Array(inner, _) => {
                self.closure_site_in_ty(prog, *inner, visited, fallback)
            }
            TyKind::Alias(_, alias) => {
                for &arg in self.tcx.tys.list(alias.args) {
                    if let Some(hit) = self.closure_site_in_ty(prog, arg, visited, fallback) {
                        return Some(hit);
                    }
                }
                None
            }
            _ => None,
        }
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
        infcx.impl_view = input.impl_view;

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
        let eval = self.evaluate_root_goal_for_proof_tree_with_depth(prog, goal.clone(), depth, true);
        if !self.typing_mode.get().is_coherence() && eval.result.as_ref().is_ok_and(|r| r.value.certainty.is_overflow()) {
            let snapshot = self.start_snapshot();
            let retry = self.evaluate_root_goal_for_proof_tree_with_depth(prog, goal.clone(), depth.saturating_mul(2), true);
            if retry.result.as_ref().is_ok_and(|r| r.value.certainty == Certainty::Yes) {
                self.commit_from(snapshot);
                self.emit_next_solver_overflow_fcw(prog, &goal, depth);
                return retry;
            }
            self.rollback_to(snapshot);
        }
        eval
    }
    /// rustc_trait_selection/src/solve/inspect/analyse.rs: diagnostics inspect failed candidates too.
    /// Keep their nested goals for this report only; the selection cache prunes them.
    pub(crate) fn evaluate_root_goal_for_diagnostics(&self, prog: &crate::program::Program, goal: Goal) -> super::inspect::GoalEvaluation {
        self.evaluate_root_goal_for_proof_tree_with_depth(prog, goal, self.recursion_limit(prog), false)
    }
    fn evaluate_root_goal_for_proof_tree_with_depth(&self, prog: &crate::program::Program, goal: Goal, root_depth: usize, cached: bool) -> super::inspect::GoalEvaluation {
        assert!(!self.typing_mode.get().is_erased_not_coherence());
        let goal = fold::fold_goal(self.tcx, &goal, &mut crate::infer::resolve::DeepVariableResolver::new(self));
        let opaques = self.resolved_opaque_types();
        let (original, input) = canonicalize_goal(self, &goal, &opaques, self.typing_mode.get());
        let mut solver = SolverDelegate::new(self.tcx, prog);
        solver.cause = self.cause; solver.limit = self.recursion_limit;
        let input = solver.input(input);
        let (result, final_revision, required_depth) = if cached {
            let root = solver.evaluate_root_goal_for_proof_tree_raw_provider(input, root_depth);
            (root.result, root.final_revision, root.required_depth)
        } else {
            let mut inspect = super::inspect::ProofTreeBuilder::new();
            let (result, depth) = SearchGraph::evaluate_root_goal_for_proof_tree(&mut solver, root_depth, input, &mut inspect);
            (result, inspect.unwrap(), depth)
        };
        let result = solver.result(result).0.clone();
        if let Ok(response) = &result {
            let (nested, _) = canonical::response::instantiate_and_apply_query_response(self, prog, &original, response); assert!(nested.is_empty());
        }
        super::inspect::GoalEvaluation { uncanonicalized_goal: goal, orig_values: original, final_revision, result, required_depth }
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
