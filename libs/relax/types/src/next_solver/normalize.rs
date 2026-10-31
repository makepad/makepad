// ported from N:rustc_next_trait_solver/src/normalize.rs @8d1a764
// ported from N:rustc_next_trait_solver/src/solve/eval_ctxt/mod.rs @8d1a764
use super::*;
use super::eval_ctxt::EvalCtxt;
use super::placeholder::{BoundVarReplacer, PlaceholderReplacer};
use crate::types::{Types, TyKind, UniverseIndex};
use crate::consts::{ConstKind, IsRigid};
use crate::types::fold::{TypeFolder, super_fold_ty, super_fold_const};

pub struct NormalizationFolder<'a, 'b, 'tcx> {
    ecx: &'a mut EvalCtxt<'b, 'tcx>, env: &'a ParamEnvId, normalization_goal_source: GoalSource, universes: Vec<Option<UniverseIndex>>, error: Option<NoSolutionOrRerunNonErased>,
}
impl EvalCtxt<'_, '_> {
    pub(super) fn normalize_ty(&mut self, env: &ParamEnvId, ty: TyId) -> Result<TyId, NoSolutionOrRerunNonErased> {
        let tys = &self.delegate.tcx.tys;
        let normalization_goal_source = GoalSource::NormalizeGoal(self.step_kind_for_source(GoalSource::TypeRelating));
        let mut folder = NormalizationFolder { ecx: self, env, normalization_goal_source, universes: Vec::new(), error: None };
        let result = folder.fold_ty(tys, ty, 0);
        match folder.error { Some(e) => Err(e), None => Ok(result) }
    }
    pub(super) fn normalize_predicate(&mut self, env: &ParamEnvId, predicate: &Predicate) -> Result<Predicate, NoSolutionOrRerunNonErased> {
        self.normalize_predicate_for_source(GoalSource::TypeRelating, env, predicate)
    }
    /// rustc ReplaceAliasWithInfer: the aliases of a goal added with `source` are normalized
    /// as if inside that goal, inheriting its step kind.
    pub(super) fn normalize_predicate_for_source(&mut self, source: GoalSource, env: &ParamEnvId, predicate: &Predicate) -> Result<Predicate, NoSolutionOrRerunNonErased> {
        // rustc_type_ir/src/inherent.rs:438-455: WF must inspect the original type.
        if matches!(predicate.kind, PredicateKind::Clause(ClauseKind::WellFormed(_))) { return Ok(predicate.clone()); }
        let tys = &self.delegate.tcx.tys;
        let normalization_goal_source = GoalSource::NormalizeGoal(self.step_kind_for_source(source));
        let mut folder = NormalizationFolder { ecx: self, env, normalization_goal_source, universes: Vec::new(), error: None };
        let result = fold::fold_predicate(tys, predicate, &mut folder);
        match folder.error { Some(e) => Err(e), None => Ok(result) }
    }
}
impl NormalizationFolder<'_, '_, '_> {
    fn normalize_alias_term(&mut self, alias: AliasTerm) -> Result<(Term, bool), NoSolutionOrRerunNonErased> {
        let term = self.ecx.next_term_infer_of_alias_kind(&alias);
        let goal = Goal { param_env: self.env.clone(), predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::Projection(ProjectionClause { projection_term: alias, term }))) };
        let evaluation = self.ecx.evaluate_goal(self.normalization_goal_source, goal.clone())?;
        if !evaluation.certainty.is_yes() { self.ecx.nested_goals.push((self.normalization_goal_source, goal, evaluation.stalled_on)); }
        let mut resolver = crate::infer::resolve::DeepResolverIgnoringRegions::new(self.ecx.delegate);
        let term = match term { Term::Ty(t) => Term::Ty(resolver.fold_ty(&self.ecx.delegate.tcx.tys, t, 0)), Term::Const(c) => Term::Const(resolver.fold_const(&self.ecx.delegate.tcx.tys, c, 0)) };
        Ok((term, !evaluation.certainty.is_yes()))
    }
}
impl TypeFolder for NormalizationFolder<'_, '_, '_> {
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        if self.error.is_some() { return ty; }
        let ty = self.ecx.delegate.shallow_resolve(ty);
        self.universes.resize(depth as usize, None);
        let ty = super_fold_ty(tys, ty, self, depth);
        self.universes.resize(depth as usize, None);
        if !matches!(tys.kind(ty), TyKind::Alias(IsRigid::No, _)) { return ty; }
        let infcx = self.ecx.delegate;
        let mut universes = std::mem::take(&mut self.universes);
        let mut replacer = BoundVarReplacer::new(infcx, &mut universes);
        let escaping = tys.has_vars_bound_at_or_above(ty, 0);
        let alias = if escaping { replacer.fold_ty(tys, ty, 0) } else { ty };
        let result = match self.normalize_alias_term(AliasTerm::Ty(alias)) {
            Ok((Term::Ty(result), ambiguous)) => {
                if escaping && ambiguous && max_universe_of_infer_vars(infcx, result).can_name(replacer.universe_indices[0].unwrap()) { ty }
                else if escaping { PlaceholderReplacer { replacer }.fold_ty(tys, result, 0) } else { result }
            }
            Ok(_) => unreachable!(), Err(e) => { self.error = Some(e); ty }
        };
        self.universes = universes; result
    }
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: u32) -> Const {
        if self.error.is_some() { return ct; }
        self.universes.resize(depth as usize, None);
        let ct = super_fold_const(tys, self.ecx.delegate.shallow_resolve_const(ct), self, depth);
        self.universes.resize(depth as usize, None);
        let ConstKind::Alias(IsRigid::No, alias) = tys.const_kind(ct) else { return ct; };
        let wrapped = tys.intern(TyKind::Const(ct));
        let escaping = tys.has_vars_bound_at_or_above(wrapped, 0);
        let infcx = self.ecx.delegate;
        let mut universes = std::mem::take(&mut self.universes);
        let mut replacer = BoundVarReplacer::new(infcx, &mut universes);
        let alias = if escaping { let mapped = replacer.fold_const(tys, ct, 0); let ConstKind::Alias(_, alias) = tys.const_kind(mapped) else { unreachable!() }; alias } else { alias };
        let result = match self.normalize_alias_term(AliasTerm::Const(alias)) {
            Ok((Term::Const(result), ambiguous)) => {
                if escaping && ambiguous && max_universe_of_infer_vars(infcx, tys.intern(TyKind::Const(result))).can_name(replacer.universe_indices[0].unwrap()) { ct }
                else if escaping { PlaceholderReplacer { replacer }.fold_const(tys, result, 0) } else { result }
            }
            Ok(_) => unreachable!(), Err(e) => { self.error = Some(e); ct }
        };
        self.universes = universes; result
    }
}
fn max_universe_of_infer_vars(infcx: &crate::infer::InferCtxt<'_>, ty: TyId) -> UniverseIndex {
    struct MaxUniverse<'a, 'tcx> { infcx: &'a crate::infer::InferCtxt<'tcx>, max: UniverseIndex }
    impl TypeFolder for MaxUniverse<'_, '_> {
        fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
            let ty = self.infcx.shallow_resolve(ty);
            if let TyKind::Infer(v) = tys.kind(ty) { if let crate::infer::type_variable::TypeVariableValue::Unknown { universe } = self.infcx.inner.borrow_mut().type_variables().probe(*v) { self.max = self.max.max(universe); } }
            super_fold_ty(tys, ty, self, depth)
        }
        fn fold_const(&mut self, tys: &Types, ct: Const, depth: u32) -> Const {
            let ct = self.infcx.shallow_resolve_const(ct);
            if let ConstKind::Infer(crate::consts::InferConst::Var(v)) = tys.const_kind(ct) {
                if let crate::infer::unify_key::ConstVariableValue::Unknown { universe } = self.infcx.inner.borrow_mut().const_unification_table().probe_value(v.0) { self.max = self.max.max(universe); }
            }
            super_fold_const(tys, ct, self, depth)
        }
    }
    let mut f = MaxUniverse { infcx, max: UniverseIndex::ROOT }; f.fold_ty(&infcx.tcx.tys, ty, 0); f.max
}

pub fn has_non_rigid_aliases(tys: &Types, ty: TyId) -> bool {
    struct Finder(bool);
    impl TypeFolder for Finder {
        fn fold_ty(&mut self, tys: &Types, t: TyId, depth: u32) -> TyId {
            self.0 |= matches!(tys.kind(t), TyKind::Alias(IsRigid::No, _));
            if !self.0 { super_fold_ty(tys, t, self, depth); } t
        }
        fn fold_const(&mut self, tys: &Types, c: Const, depth: u32) -> Const {
            self.0 |= matches!(tys.const_kind(c), ConstKind::Alias(IsRigid::No, _));
            if !self.0 { super_fold_const(tys, c, self, depth); } c
        }
    }
    let mut f = Finder(false); f.fold_ty(tys, ty, 0); f.0
}
