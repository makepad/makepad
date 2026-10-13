// ported from N:rustc_type_ir/src/relate/solver_relating.rs @8d1a764
// ported from N:rustc_type_ir/src/relate/combine.rs @8d1a764
use super::*;
use crate::consts::{ConstKind, InferConst, IsRigid};
use crate::next_solver::{PredicateKind, ClauseKind, AliasTerm, Goal, Predicate, ProjectionClause, SubtypePredicate, Term};
use crate::next_solver::ParamEnvId;
use std::collections::HashSet;

pub struct SolverRelating<'a, 'tcx> {
    pub(crate) infcx: &'a InferCtxt<'tcx>,
    prog: &'a crate::program::Program,
    param_env: ParamEnvId,
    ambient_variance: V,
    pub goals: Vec<Goal>,
    cache: HashSet<(V, TyId, TyId)>,
}

impl<'a, 'tcx> SolverRelating<'a, 'tcx> {
    pub fn new(infcx: &'a InferCtxt<'tcx>,
    prog: &'a crate::program::Program, ambient_variance: V, param_env: ParamEnvId) -> Self {
        Self { infcx, prog, param_env, ambient_variance, goals: Vec::new(), cache: HashSet::new() }
    }
    fn register_predicate(&mut self, predicate: Predicate) { self.goals.push(Goal { param_env: self.param_env, predicate }); }
}

impl<'tcx> TypeRelation<'tcx> for SolverRelating<'_, 'tcx> {
    fn tcx(&self) -> &'tcx crate::tcx::Tcx { self.infcx.tcx }
    fn prog(&self) -> &crate::program::Program { self.prog }
    fn variance(&self) -> V { self.ambient_variance }
    fn set_variance(&mut self, variance: V) { self.ambient_variance = variance; }
    fn tys(&mut self, a: TyId, b: TyId) -> Result<TyId, TypeError> {
        if a == b || self.ambient_variance == V::Bi { return Ok(a); }
        let infcx = self.infcx;
        let a = infcx.shallow_resolve(a);
        let b = infcx.shallow_resolve(b);
        if a == b || self.cache.contains(&(self.ambient_variance, a, b)) { return Ok(a); }
        let variance = self.ambient_variance;
        match (infcx.tcx.tys.kind(a), infcx.tcx.tys.kind(b)) {
            (TyKind::Infer(av), TyKind::Infer(bv)) => match self.ambient_variance {
                V::Co => self.register_predicate(Predicate::dummy(PredicateKind::Subtype(SubtypePredicate { a_is_expected: true, a, b }))),
                V::Contra => self.register_predicate(Predicate::dummy(PredicateKind::Subtype(SubtypePredicate { a_is_expected: false, a: b, b: a }))),
                V::Inv => infcx.inner.borrow_mut().type_variables().equate(*av, *bv),
                V::Bi => unreachable!(),
            },
            (TyKind::Alias(crate::consts::IsRigid::No, _), _) => {
                let var = infcx.next_ty_var();
                self.register_predicate(Predicate::dummy(PredicateKind::Clause(ClauseKind::Projection(ProjectionClause { projection_term: AliasTerm::Ty(a), term: Term::Ty(var) }))));
                self.tys(var, b)?;
            }
            (_, TyKind::Alias(crate::consts::IsRigid::No, _)) => {
                let var = infcx.next_ty_var();
                self.register_predicate(Predicate::dummy(PredicateKind::Clause(ClauseKind::Projection(ProjectionClause { projection_term: AliasTerm::Ty(b), term: Term::Ty(var) }))));
                self.tys(a, var)?;
            }
            (TyKind::Infer(vid), _) => infcx.instantiate_ty_var(self, true, *vid, variance, b)?,
            (_, TyKind::Infer(vid)) => infcx.instantiate_ty_var(self, false, *vid, xform(variance, V::Contra), a)?,
            (TyKind::IntVar(a), TyKind::IntVar(b)) => infcx.inner.borrow_mut().int_unification_table().union(*a, *b),
            (TyKind::FloatVar(a), TyKind::FloatVar(b)) => infcx.inner.borrow_mut().float_unification_table().union(*a, *b),
            (TyKind::IntVar(vid), TyKind::Int(p)) | (TyKind::Int(p), TyKind::IntVar(vid)) => {
                use crate::infer::unify_key::IntVarValue;
                let value = if matches!(p, crate::program::Prim::I8 | crate::program::Prim::I16 | crate::program::Prim::I32 | crate::program::Prim::I64 | crate::program::Prim::I128 | crate::program::Prim::Isize) { IntVarValue::IntType(*p) } else { IntVarValue::UintType(*p) };
                infcx.inner.borrow_mut().int_unification_table().union_value(*vid, value);
            }
            (TyKind::FloatVar(vid), TyKind::Float(p)) | (TyKind::Float(p), TyKind::FloatVar(vid)) =>
                infcx.inner.borrow_mut().float_unification_table().union_value(*vid, crate::infer::unify_key::FloatVarValue::Known(*p)),
            (TyKind::Error, _) | (_, TyKind::Error) => { infcx.set_tainted_by_errors(); return Ok(infcx.tcx.tys.error); }
            _ => { structurally_relate_tys(self, a, b)?; }
        }
        self.cache.insert((self.ambient_variance, a, b));
        Ok(a)
    }
    fn regions(&mut self, a: RegionId, b: RegionId) -> Result<RegionId, TypeError> {
        let mut inner = self.infcx.inner.borrow_mut();
        let mut regions = inner.unwrap_region_constraints();
        let tys = &self.infcx.tcx.tys;
        match self.ambient_variance {
            V::Co => regions.make_subregion(tys, b, a, true),
            V::Contra => regions.make_subregion(tys, a, b, true),
            V::Inv => regions.make_eqregion(tys, a, b, true),
            V::Bi => {},
        }
        Ok(a)
    }
    fn consts(&mut self, a: Const, b: Const) -> Result<Const, TypeError> {
        let infcx = self.infcx;
        if a == b { return Ok(a); }
        let a = infcx.shallow_resolve_const(a);
        let b = infcx.shallow_resolve_const(b);
        if a == b { return Ok(a); }
        match (infcx.tcx.tys.const_kind(a), infcx.tcx.tys.const_kind(b)) {
            (ConstKind::Infer(InferConst::Var(av)), ConstKind::Infer(InferConst::Var(bv))) => {
                infcx.inner.borrow_mut().const_unification_table().union(av.0, bv.0);
            }
            (ConstKind::Alias(IsRigid::No, alias), _) => self.register_predicate(Predicate::dummy(PredicateKind::Clause(ClauseKind::Projection(ProjectionClause { projection_term: AliasTerm::Const(alias), term: Term::Const(b) })))),
            (_, ConstKind::Alias(IsRigid::No, alias)) => self.register_predicate(Predicate::dummy(PredicateKind::Clause(ClauseKind::Projection(ProjectionClause { projection_term: AliasTerm::Const(alias), term: Term::Const(a) })))),
            (ConstKind::Infer(InferConst::Var(vid)), _) => infcx.instantiate_const_var(self, true, vid.0, b)?,
            (_, ConstKind::Infer(InferConst::Var(vid))) => infcx.instantiate_const_var(self, false, vid.0, a)?,
            _ => { structurally_relate_consts(self, a, b)?; }
        }
        Ok(a)
    }
    fn binders(&mut self, a: TyId, b: TyId) -> Result<TyId, TypeError> {
        if a == b { return Ok(a); }
        let infcx = self.infcx;
        if !infcx.fn_has_bound_vars(a) && !infcx.fn_has_bound_vars(b) { return super::relate_fn_sig(self, a, b); }
        match self.ambient_variance {
            V::Co => {
                let b = infcx.enter_forall_and_leak_universe(b);
                let a = infcx.instantiate_binder_with_infer(a);
                super::relate_fn_sig(self, a, b)?;
            }
            V::Contra => {
                let a = infcx.enter_forall_and_leak_universe(a);
                let b = infcx.instantiate_binder_with_infer(b);
                super::relate_fn_sig(self, a, b)?;
            }
            V::Inv => {
                let bp = infcx.enter_forall_and_leak_universe(b);
                let ai = infcx.instantiate_binder_with_infer(a);
                super::relate_fn_sig(self, ai, bp)?;
                let ap = infcx.enter_forall_and_leak_universe(a);
                let bi = infcx.instantiate_binder_with_infer(b);
                super::relate_fn_sig(self, ap, bi)?;
            }
            V::Bi => {},
        }
        Ok(a)
    }
    fn binders_args(&mut self, a: &[GenericArg], b: &[GenericArg]) -> Result<Vec<GenericArg>, TypeError> {
        if a == b || self.ambient_variance == V::Bi { return Ok(a.to_vec()); }
        let infcx = self.infcx;
        if !super::generalize::alias_has_escaping_bound_vars(&infcx.tcx.tys, a)
            && !super::generalize::alias_has_escaping_bound_vars(&infcx.tcx.tys, b) {
            return relate_args(self, a, b);
        }
        if matches!(self.ambient_variance, V::Co | V::Inv) {
            let bp = infcx.enter_forall_args_and_leak_universe(b);
            let ai = infcx.instantiate_binder_args_with_infer(a);
            relate_args(self, &ai, &bp)?;
        }
        if matches!(self.ambient_variance, V::Contra | V::Inv) {
            let ap = infcx.enter_forall_args_and_leak_universe(a);
            let bi = infcx.instantiate_binder_args_with_infer(b);
            relate_args(self, &ap, &bi)?;
        }
        Ok(a.to_vec())
    }
    fn has_unresolved_infer(&self, ty: TyId) -> bool {
        self.infcx.tcx.tys.has_infer(ty) && self.infcx.tcx.tys.has_infer(self.infcx.deeply_resolve_ignoring_regions(ty))
    }
    fn well_formed(&mut self, ty: TyId) { self.register_predicate(Predicate::dummy(PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(ty))))); }
}
