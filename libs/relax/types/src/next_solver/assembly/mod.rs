// ported from N:rustc_next_trait_solver/src/solve/assembly/mod.rs @8d1a764
// ported from N:rustc_next_trait_solver/src/solve/trait_goals.rs @8d1a764
use super::*;
use super::eval_ctxt::EvalCtxt;
use crate::program::DefId;
use crate::types::{TyKind, AliasTyKind};
use crate::consts::IsRigid;
use crate::next_solver::search_graph::CandidateHeadUsages;
pub mod structural_traits;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuiltinImplSource { Misc, Trivial, Object(usize), TraitUpcasting(usize) }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateSource { Impl(DefId), ParamEnv(bool), AliasBound(bool), BuiltinImpl(BuiltinImplSource) }
pub struct Candidate { pub source: CandidateSource, pub result: canonical::Canonical<Response>, pub head_usages: CandidateHeadUsages }
#[derive(Clone)]
pub enum GoalKind { Trait(TraitClause), NormalizesTo(NormalizesTo) }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraitGoalProvenVia { ParamEnv, AliasBound, Misc }
#[derive(Clone, Copy)]
pub enum AssembleCandidatesFrom { All, EnvAndBounds }
impl GoalKind {
    pub fn trait_ref(&self, ecx: &EvalCtxt<'_, '_>) -> TraitRef {
        match self { Self::Trait(p) => p.trait_ref.clone(), Self::NormalizesTo(p) => ecx.alias_trait_ref(&p.alias) }
    }
    pub fn self_ty(&self, ecx: &EvalCtxt<'_, '_>) -> TyId {
        let GenericArg::Type(ty) = self.trait_ref(ecx).args[0] else { panic!("trait self type") }; ty
    }
    fn replace_self(&mut self, ecx: &EvalCtxt<'_, '_>, ty: TyId) {
        match self {
            Self::Trait(p) => p.trait_ref.args[0] = GenericArg::Type(ty),
            Self::NormalizesTo(p) => match &mut p.alias {
                AliasTerm::Ty(t) => { let tys = &ecx.delegate.tcx.tys; let TyKind::Alias(r, mut a) = *tys.kind(*t) else { unreachable!() }; let mut args = tys.list(a.args).to_vec(); args[0] = ty; a.args = tys.mk_list(&args); *t = tys.intern(TyKind::Alias(r, a)); }
                AliasTerm::Const(a) => { let mut args = a.args.to_vec(); args[0] = GenericArg::Type(ty); a.args = args.into(); }
            }
        }
    }
}
impl EvalCtxt<'_, '_> {
    pub(super) fn alias_args(&self, alias: &AliasTerm) -> (DefId, Vec<GenericArg>) {
        match alias {
            AliasTerm::Ty(ty) => { let TyKind::Alias(_, a) = self.delegate.tcx.tys.kind(*ty) else { panic!("alias term") }; (a.kind.def_id(), self.delegate.tcx.complete_args(self.solver.prog, a.kind.def_id(), self.delegate.tcx.tys.list(a.args), a.regions)) }
            AliasTerm::Const(a) => (a.kind.def_id(), a.args.to_vec()),
        }
    }
    pub(super) fn alias_trait_ref(&self, alias: &AliasTerm) -> TraitRef {
        let (def, mut args) = self.alias_args(alias); let parent = self.solver.prog.def(def).parent;
        let indices = self.delegate.tcx.generic_indices_of(self.solver.prog, parent);
        let n = indices.lifetimes.len() + indices.types_consts.len();
        args.truncate(n); TraitRef { def_id: parent, args }
    }
    pub(super) fn assemble_and_evaluate_candidates(&mut self, env: &ParamEnvId, mut goal: GoalKind, from: AssembleCandidatesFrom) -> Result<(Vec<Candidate>, CandidateHeadUsages), NoSolutionOrRerunNonErased> {
        let mut candidates = Vec::new(); let mut failed = CandidateHeadUsages::default();
        let self_ty = match map_err_to_rerun(self.structurally_normalize_term(env, Term::Ty(goal.self_ty(self))))? { Ok(Term::Ty(t)) => t, _ => return Ok((candidates, failed)) };
        goal.replace_self(self, self_ty);
        let predicate = Predicate::dummy(match goal { GoalKind::Trait(p) => PredicateKind::Clause(ClauseKind::Trait(p)), GoalKind::NormalizesTo(p) => PredicateKind::NormalizesTo(p) });
        let predicate = fold::fold_predicate(&self.delegate.tcx.tys, &predicate, &mut crate::infer::resolve::DeepResolverIgnoringRegions::new(self.delegate));
        let goal = match predicate.kind { PredicateKind::Clause(ClauseKind::Trait(p)) => GoalKind::Trait(p), PredicateKind::NormalizesTo(p) => GoalKind::NormalizesTo(p), _ => unreachable!() };
        if matches!(self.delegate.tcx.tys.kind(self_ty), TyKind::Infer(_)) {
            self.try_assemble_bounds_via_registered_opaques(env, &goal, from, &mut candidates)?;
            return Ok((candidates, failed));
        }
        if self.delegate.typing_mode.is_coherence() {
            let local_crate = self.delegate.coherence_crate.expect("coherence crate context");
            if coherence::trait_ref_is_knowable(self,env,&goal.trait_ref(self),local_crate)?.is_err() {
                // assembly/mod.rs:1022-1060: a known-false supertrait disproves
                // an otherwise unknowable downstream implementation.
                let source = CandidateSource::BuiltinImpl(BuiltinImplSource::Misc);
                let (result, head_usages) = self.probe().enter_single_candidate(source, |ecx| {
                    let tr = goal.trait_ref(ecx);
                    let elaborated = ecx.delegate.tcx.supertraits(ecx.solver.prog, collect::trait_predicate(tr.def_id, tr.args));
                    for predicate in elaborated.into_iter().skip(1) {
                        ecx.add_goal(GoalSource::Misc, Goal { param_env: env.clone(), predicate })?;
                    }
                    ecx.evaluate_added_goals_and_make_canonical_response(Certainty::AMBIGUOUS)
                });
                if let Ok(result) = map_err_to_rerun(result)? { candidates.push(Candidate { source, result, head_usages }); }
            }
        }
        self.probe().enter_with_kind(|ecx| {
            ecx.inspect.probe_kind(super::inspect::ProbeKind::NormalizedSelfTyAssembly);
            ecx.assemble_alias_bound_candidates_recur(env, &goal, self_ty, true, &mut candidates)
        })?;
        for p in &*self.delegate.tcx.param_env_by_id(*env).clauses {
            if !self.assumption_matches_kind(&goal, p) { continue; }
            let mut global = false;
            let (result, usages) = self.probe().enter_single_candidate(CandidateSource::ParamEnv(false), |ecx| {
                ecx.match_assumption(env, &goal, p)?;
                ecx.try_evaluate_added_goals()?;
                let (is_global, certainty) = ecx.characterize_param_env_assumption(env, p)?;
                global = is_global;
                ecx.evaluate_added_goals_and_make_canonical_response(certainty)
            });
            self.inspect.candidate_source(CandidateSource::ParamEnv(global));
            match map_err_to_rerun(result)? { Ok(result) => candidates.push(Candidate { source: CandidateSource::ParamEnv(global), result, head_usages: usages }), Err(_) => failed.merge_usages(usages) }
        }
        if matches!(from, AssembleCandidatesFrom::All) {
            self.assemble_builtin_impl_candidates(env, &goal, &mut candidates)?;
            let mut shadowed = false;
            if !self.delegate.typing_mode.is_coherence() {
                for c in &candidates { if matches!(c.source, CandidateSource::ParamEnv(false) | CandidateSource::AliasBound(_)) && has_no_inference_or_external_constraints(&self.delegate.tcx.tys, &c.result) { shadowed = true; } }
            }
            if !shadowed { self.assemble_impl_candidates(env, &goal, &mut candidates)?; self.assemble_object_bound_candidates(env, &goal, &mut candidates)?; }
        } else {
            let mut param = false; for c in &candidates { param |= matches!(c.source, CandidateSource::ParamEnv(_)); }
            if !param { self.assemble_object_bound_candidates(env, &goal, &mut candidates)?; }
        }
        Ok((candidates, failed))
    }
    pub(super) fn forced_ambiguity(&mut self, maybe: MaybeInfo) -> Result<Candidate, NoSolutionOrRerunNonErased> {
        let source = CandidateSource::BuiltinImpl(BuiltinImplSource::Misc);
        let (result, head_usages) = self.probe().enter_single_candidate(source, |ecx| ecx.evaluate_added_goals_and_make_canonical_response(Certainty::Maybe(maybe)));
        Ok(Candidate { source, result: result?, head_usages })
    }
    pub(super) fn assumption_matches_kind(&self, goal: &GoalKind, p: &Predicate) -> bool {
        match (goal, &p.kind) {
            (GoalKind::Trait(goal), PredicateKind::Clause(ClauseKind::Trait(p))) => goal.polarity == p.polarity && goal.trait_ref.def_id == p.trait_ref.def_id,
            (GoalKind::NormalizesTo(goal), PredicateKind::Clause(ClauseKind::Projection(p))) => self.alias_args(&goal.alias).0 == self.alias_args(&p.projection_term).0,
            _ => false,
        }
    }
    pub(super) fn match_assumption(&mut self, env: &ParamEnvId, goal: &GoalKind, p: &Predicate) -> Result<(), NoSolutionOrRerunNonErased> {
        let kind = self.delegate.instantiate_predicate_binder_with_infer(p);
        let p = self.normalize_predicate(env, &Predicate::dummy(kind))?;
        match (goal, p.kind) {
            (GoalKind::Trait(goal), PredicateKind::Clause(ClauseKind::Trait(p))) => self.eq_args(env, &goal.trait_ref.args, &p.trait_ref.args),
            (GoalKind::NormalizesTo(goal), PredicateKind::Clause(ClauseKind::Projection(p))) => {
                self.eq_args(env, &self.alias_args(&goal.alias).1, &self.alias_args(&p.projection_term).1)?;
                self.instantiate_normalizes_to_term(env, goal, p.term)?;
                let (def, args) = self.alias_args(&goal.alias);
                self.add_item_clauses(env, def, &args, GoalSource::AliasWellFormed, true)
            }
            _ => Err(NoSolution.into()),
        }
    }
    pub(super) fn probe_and_consider_implied_clause(&mut self, env: &ParamEnvId, goal: &GoalKind, p: &Predicate, source: CandidateSource, candidates: &mut Vec<Candidate>) -> Result<(), NoSolutionOrRerunNonErased> {
        if !self.assumption_matches_kind(goal, p) { return Ok(()); }
        let (result, head_usages) = self.probe().enter_single_candidate(source, |ecx| { ecx.match_assumption(env, goal, p)?; ecx.evaluate_added_goals_and_make_canonical_response(Certainty::Yes) });
        if let Ok(result) = map_err_to_rerun(result)? { candidates.push(Candidate { source, result, head_usages }); }
        Ok(())
    }
    fn assemble_alias_bound_candidates_recur(&mut self, env: &ParamEnvId, goal: &GoalKind, ty: TyId, self_bounds: bool, candidates: &mut Vec<Candidate>) -> Result<(), NoSolutionOrRerunNonErased> {
        let tcx = self.delegate.tcx;
        if matches!(tcx.tys.kind(ty), TyKind::Infer(_)) {
            candidates.push(self.forced_ambiguity(MaybeInfo::AMBIGUOUS)?); return Ok(());
        }
        let TyKind::Alias(IsRigid::Yes, a) = tcx.tys.kind(ty) else { return Ok(()); };
        let def = a.kind.def_id(); let args = tcx.complete_args(self.solver.prog, def, tcx.tys.list(a.args), a.regions);
        for bound in tcx.item_bounds(self.solver.prog, def) {
            let bound = tcx.instantiate_predicate(self.solver.prog, def, &args, &bound);
            for p in tcx.supertraits(self.solver.prog, bound) {
                let subject = match &p.kind {
                    PredicateKind::Clause(ClauseKind::Trait(t)) => t.trait_ref.args.first().copied(),
                    PredicateKind::Clause(ClauseKind::Projection(p)) => self.alias_trait_ref(&p.projection_term).args.first().copied(),
                    _ => None,
                };
                let same = match subject { Some(GenericArg::Type(t)) => matches!(tcx.tys.kind(t), TyKind::Alias(_, b) if b == a), _ => false };
                if same == self_bounds { self.probe_and_consider_implied_clause(env, goal, &p, CandidateSource::AliasBound(self_bounds), candidates)?; }
            }
        }
        if matches!(a.kind, AliasTyKind::Projection { .. }) {
            if let Ok(Term::Ty(t)) = map_err_to_rerun(self.structurally_normalize_term(env, Term::Ty(self.delegate.tcx.tys.list(a.args)[0])))? { self.assemble_alias_bound_candidates_recur(env, goal, t, false, candidates)?; }
        }
        Ok(())
    }
    pub(super) fn assemble_impl_candidates(&mut self, env: &ParamEnvId, goal: &GoalKind, candidates: &mut Vec<Candidate>) -> Result<(), NoSolutionOrRerunNonErased> {
        crate::solver_work::count(crate::solver_work::IMPL_GOALS);
        let tr = goal.trait_ref(self); let tcx = self.delegate.tcx;
        let self_ty = goal.self_ty(self);
        if matches!(tcx.tys.kind(self_ty), TyKind::IntVar(_) | TyKind::FloatVar(_)) {
            // rustc_middle::ty::trait_def::for_each_relevant_impl: inference only
            // visits this trait's impls; deep rejection filters concrete heads.
            if let Some(impls) = tcx.trait_impls_of.get(&tr.def_id) { for &imp in impls {
                if let Some(candidate) = self.consider_impl_candidate(env, goal, imp, |ecx| ecx.evaluate_added_goals_and_make_canonical_response(Certainty::Yes))? { candidates.push(candidate); }
            } }
        } else {
            let key = crate::tcx::ty_key(&tcx.tys, self_ty);
            let keys = [key, crate::tcx::TyKey::Other];
            for &key in &keys[..if key == crate::tcx::TyKey::Other { 1 } else { 2 }] {
                if let Some(impls) = tcx.trait_impls.get(&(key, tr.def_id)) { for &imp in impls {
                    if let Some(candidate) = self.consider_impl_candidate(env, goal, imp, |ecx| ecx.evaluate_added_goals_and_make_canonical_response(Certainty::Yes))? { candidates.push(candidate); }
                } }
            }
        }
        Ok(())
    }
    pub(super) fn consider_impl_candidate(&mut self, env: &ParamEnvId, goal: &GoalKind, imp: DefId,
        then: impl FnOnce(&mut EvalCtxt<'_, '_>) -> QueryResultOrRerunNonErased) -> Result<Option<Candidate>, NoSolutionOrRerunNonErased> {
        let tcx = self.delegate.tcx; let tr = goal.trait_ref(self);
        let d = self.solver.prog.def(imp);
        let negative = { let ast = &self.solver.prog.files[d.file as usize].ast; ast.as_impl(&ast.item(d.item).kind).is_some_and(|i| i.negative) };
        let wanted_negative = matches!(goal, GoalKind::Trait(TraitClause { polarity: ClausePolarity::Negative, .. }));
        if negative != wanted_negative { return Ok(None); }
        // Shared by trait and projection candidates, before any inference probe.
        let impl_ref = tcx.identity_impl_trait_ref(self.solver.prog, imp);
        if !super::fast_reject::DeepRejectCtxt::relate_rigid_infer(&tcx.tys).args_may_unify(&tr.args, &impl_ref.args) {
            crate::solver_work::count(crate::solver_work::IMPL_REJECTED);
            return Ok(None);
        }
        crate::solver_work::count(crate::solver_work::IMPL_PROBES);
        let (result, head_usages) = self.probe().enter_single_candidate(CandidateSource::Impl(imp), |ecx| {
                let args = ecx.fresh_args_for_item(imp);
                ecx.inspect.record_impl_args(ecx.delegate, ecx.max_input_universe, args.clone());
                let impl_ref = tcx.impl_trait_ref(ecx.solver.prog, imp, &args);
                ecx.eq_args(env, &tr.args, &impl_ref.args)?;
                ecx.add_item_clauses(env, imp, &args, GoalSource::ImplWhereBound, false)?;
                // trait_goals.rs:104-120: elaborated supertrait outlives obligations.
                for predicate in tcx.impl_super_outlives(ecx.solver.prog, imp) {
                    let predicate = tcx.instantiate_predicate(ecx.solver.prog, imp, &args, predicate);
                    ecx.add_goal(GoalSource::Misc, Goal { param_env: env.clone(), predicate })?;
                }
                if let GoalKind::NormalizesTo(p) = goal { ecx.confirm_projection_impl(env, p, imp, &args)?; }
            then(ecx)
        });
        Ok(match map_err_to_rerun(result)? {
            Ok(result) => Some(Candidate { source: CandidateSource::Impl(imp), result, head_usages }),
            Err(_) => None,
        })
    }
    pub(super) fn add_item_clauses(&mut self, env: &ParamEnvId, def: DefId, args: &[GenericArg], source: GoalSource, own: bool) -> Result<(), NoSolutionOrRerunNonErased> {
        let tcx = self.delegate.tcx;
        let inherited;
        let ps = if own { tcx.own_clauses_of(self.solver.prog, def) } else { inherited = tcx.clauses_of(self.solver.prog, def); &inherited };
        for p in ps { let p = tcx.instantiate_predicate(self.solver.prog, def, args, &p); self.add_goal(source, Goal { param_env: env.clone(), predicate: p })?; }
        Ok(())
    }
    pub(super) fn try_merge_candidates(&self, candidates: &[Candidate]) -> Option<(canonical::Canonical<Response>, Option<usize>)> {
        for (i, c) in candidates.iter().enumerate() {
            if c.result.value.certainty.is_yes() && has_no_inference_or_external_constraints(&self.delegate.tcx.tys, &c.result) { return Some((c.result.clone(), Some(i))); }
        }
        let one = candidates.first()?;
        let mut one_no_regions = one.result.clone(); one_no_regions.value.external_constraints.region_constraints.clear();
        let mut equal_regions = true;
        for c in &candidates[1..] {
            let mut no_regions = c.result.clone(); no_regions.value.external_constraints.region_constraints.clear();
            if no_regions != one_no_regions { return None; }
            equal_regions &= c.result.value.external_constraints.region_constraints == one.result.value.external_constraints.region_constraints;
        }
        if equal_regions { return Some((one.result.clone(), None)); }
        for c in candidates { if c.result.value.external_constraints.region_constraints.is_empty() { return Some((c.result.clone(), None)); } }
        None
    }
    pub(super) fn flounder(&self, candidates: &[Candidate]) -> QueryResultOrRerunNonErased {
        if candidates.is_empty() { return Err(NoSolution.into()); }
        let mut maybe = MaybeInfo::AMBIGUOUS;
        for c in candidates { if let Certainty::Maybe(m) = c.result.value.certainty { maybe = maybe.or(m); } }
        Ok(self.make_ambiguous_response_no_constraints(maybe))
    }
    pub(super) fn merge_trait_candidates(&mut self, tr: &TraitRef, mut candidates: Vec<Candidate>, failed: CandidateHeadUsages) -> Result<(canonical::Canonical<Response>, Option<TraitGoalProvenVia>), NoSolutionOrRerunNonErased> {
        if candidates.is_empty() { return Err(NoSolution.into()); }
        // trait_goals.rs:1628-1633: coherence does not prefer environment/object candidates.
        if self.delegate.typing_mode.is_coherence() {
            return Ok(match self.try_merge_candidates(&candidates) {
                Some((response, _)) => (response, Some(TraitGoalProvenVia::Misc)),
                None => (self.flounder(&candidates)?, None),
            });
        }
        for c in &candidates { if c.source == CandidateSource::BuiltinImpl(BuiltinImplSource::Trivial) { return Ok((c.result.clone(), Some(TraitGoalProvenVia::Misc))); } }
        let marker = super::eval_ctxt::trait_is_auto(self.solver.prog, tr.def_id) || self.delegate.tcx.lang_items.get(crate::lang_items::LangItem::Sized) == Some(tr.def_id) || self.delegate.tcx.lang_items.get(crate::lang_items::LangItem::MetaSized) == Some(tr.def_id);
        let mut alias_self = false; let mut alias = false; let mut non_global = false;
        for c in &candidates { alias_self |= c.source == CandidateSource::AliasBound(true); alias |= matches!(c.source, CandidateSource::AliasBound(_)); non_global |= c.source == CandidateSource::ParamEnv(false); }
        let via = if marker && alias_self { Some(TraitGoalProvenVia::AliasBound) } else if non_global { Some(TraitGoalProvenVia::ParamEnv) } else if alias { Some(TraitGoalProvenVia::AliasBound) } else { None };
        if let Some(via) = via {
            candidates.retain(|c| match via { TraitGoalProvenVia::ParamEnv => matches!(c.source, CandidateSource::ParamEnv(_)), TraitGoalProvenVia::AliasBound => matches!(c.source, CandidateSource::AliasBound(_)), _ => false });
            if let Some((response, always)) = self.try_merge_candidates(&candidates) {
                if via == TraitGoalProvenVia::ParamEnv { if let Some(i) = always {
                    for (j, c) in candidates.into_iter().enumerate() { if i != j { self.search_graph.ignore_candidate_head_usages(c.head_usages); } }
                    self.search_graph.ignore_candidate_head_usages(failed);
                } }
                return Ok((response, Some(via)));
            }
            return Ok((self.flounder(&candidates)?, None));
        }
        let mut object = false; let mut seen_object = false; let mut all_global = true;
        for c in &candidates { if !seen_object && matches!(c.source, CandidateSource::BuiltinImpl(BuiltinImplSource::Object(_))) { seen_object = true; object = has_only_region_constraints(&self.delegate.tcx.tys, &c.result); } all_global &= c.source == CandidateSource::ParamEnv(true); }
        if all_global { return Ok(match self.try_merge_candidates(&candidates) { Some((r,_)) => (r,Some(TraitGoalProvenVia::ParamEnv)), None => (self.flounder(&candidates)?,None) }); }
        candidates.retain(|c| c.source != CandidateSource::ParamEnv(true) && !(object && matches!(c.source, CandidateSource::Impl(_))));
        let mut all_builtin = true; for c in &candidates { all_builtin &= matches!(c.source, CandidateSource::BuiltinImpl(_)); }
        if candidates.len() == 1 { return Ok((candidates.remove(0).result, Some(TraitGoalProvenVia::Misc))); }
        if all_builtin { if let Some((r,_)) = self.try_merge_candidates(&candidates) { return Ok((r,Some(TraitGoalProvenVia::Misc))); } }
        Ok((self.flounder(&candidates)?, None))
    }
    pub(super) fn compute_trait_goal(&mut self, env: ParamEnvId, goal: TraitClause) -> QueryResultOrRerunNonErased {
        self.compute_trait_goal_with_source(&env, &goal).map(|r| r.0)
    }
    pub(super) fn compute_trait_goal_with_source(&mut self, env: &ParamEnvId, goal: &TraitClause) -> Result<(canonical::Canonical<Response>, Option<TraitGoalProvenVia>), NoSolutionOrRerunNonErased> {
        let (candidates, failed) = self.assemble_and_evaluate_candidates(env, GoalKind::Trait(goal.clone()), AssembleCandidatesFrom::All)?;
        self.merge_trait_candidates(&goal.trait_ref, candidates, failed)
    }
}
// assembly/mod.rs:1382-1515: stop at the first parameter, unresolved inference,
// failed normalization or recursion overflow, including lifetime-only clauses.
impl EvalCtxt<'_, '_> {
    fn characterize_param_env_assumption(&mut self, env: &ParamEnvId, predicate: &Predicate) -> Result<(bool, Certainty), NoSolutionOrRerunNonErased> {
        if predicate.has_bound_vars(&self.delegate.tcx.tys) { return Ok((false, Certainty::Yes)); }
        let tys = &self.delegate.tcx.tys;
        let mut visitor = FindParamInClause { ecx: self, env, result: None, recursion_depth: 0, universes: Vec::new() };
        fold::fold_predicate(tys, predicate, &mut visitor);
        match visitor.result { Some(result) => Ok((false, result?)), None => Ok((true, Certainty::Yes)) }
    }
}
struct FindParamInClause<'a, 'b, 'tcx> { ecx: &'a mut EvalCtxt<'b, 'tcx>, env: &'a ParamEnvId, result: Option<Result<Certainty, NoSolutionOrRerunNonErased>>, recursion_depth: usize, universes: Vec<Option<crate::types::UniverseIndex>> }
impl crate::types::fold::TypeFolder for FindParamInClause<'_, '_, '_> {
    fn fold_ty(&mut self, tys: &crate::types::Types, ty: TyId, depth: u32) -> TyId {
        if self.result.is_some() { return ty; }
        self.universes.resize(depth as usize, None);
        let ty = super::placeholder::BoundVarReplacer::new(self.ecx.delegate, &mut self.universes).fold_ty(tys, ty, 0);
        let ty = match self.ecx.structurally_normalize_term(self.env, Term::Ty(ty)) {
            Ok(Term::Ty(ty)) => ty,
            Err(error) => { self.result = Some(Err(error)); return ty; }
            _ => unreachable!(),
        };
        match tys.kind(ty) {
            TyKind::Placeholder(p) if p.universe == crate::types::UniverseIndex::ROOT => self.result = Some(Ok(Certainty::Yes)),
            TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) => self.result = Some(Ok(Certainty::AMBIGUOUS)),
            _ if tys.flags(ty).intersects(crate::types::TypeFlags::HAS_PLACEHOLDER | crate::types::TypeFlags::HAS_INFER | crate::types::TypeFlags::HAS_TY_PROJECTION | crate::types::TypeFlags::HAS_TY_OPAQUE) => {
                self.recursion_depth += 1;
                if self.recursion_depth > self.ecx.search_graph.recursion_limit() { self.result = Some(Ok(Certainty::overflow(true))); }
                else { crate::types::fold::super_fold_ty(tys, ty, self, depth); }
                self.recursion_depth -= 1;
            }
            _ => {}
        }
        ty
    }
    fn fold_const(&mut self, tys: &crate::types::Types, ct: Const, depth: u32) -> Const {
        if self.result.is_some() { return ct; }
        self.universes.resize(depth as usize, None);
        let ct = super::placeholder::BoundVarReplacer::new(self.ecx.delegate, &mut self.universes).fold_const(tys, ct, 0);
        let ct = match self.ecx.structurally_normalize_term(self.env, Term::Const(ct)) {
            Ok(Term::Const(ct)) => ct,
            Err(error) => { self.result = Some(Err(error)); return ct; }
            _ => unreachable!(),
        };
        match tys.const_kind(ct) {
            crate::consts::ConstKind::Placeholder(p) if p.universe == crate::types::UniverseIndex::ROOT => self.result = Some(Ok(Certainty::Yes)),
            crate::consts::ConstKind::Infer(_) => self.result = Some(Ok(Certainty::AMBIGUOUS)),
            _ => { crate::types::fold::super_fold_const(tys, ct, self, depth); }
        }
        ct
    }
    fn fold_region(&mut self, tys: &crate::types::Types, r: crate::types::RegionId, _: u32) -> crate::types::RegionId {
        use crate::types::RegionKind::*;
        if self.result.is_some() { return r; }
        let r = crate::infer::resolve::DeepVariableResolver::new(self.ecx.delegate).fold_region(tys, r, 0);
        match tys.region_kind(r) {
            RePlaceholder { universe, .. } if universe == crate::types::UniverseIndex::ROOT => self.result = Some(Ok(Certainty::Yes)),
            ReVar(_) => self.result = Some(Ok(Certainty::Yes)),
            ReStatic | ReError | ReBound(..) | RePlaceholder { .. } => {},
            ReErased | ReEarlyParam(_) | ReLateParam(..) => unreachable!("noncanonical region in param-env clause"),
        }
        r
    }
}
