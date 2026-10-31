// ported from N:rustc_infer/src/infer/relate/lattice.rs @8d1a764
use super::*;
use crate::next_solver::{Goal, ParamEnvId};
use crate::program::Program;

#[derive(Clone, Copy)]
pub enum LatticeOpKind { Glb, Lub }

pub struct LatticeOp<'a, 'tcx> {
    infcx: &'a InferCtxt<'tcx>,
    prog: &'a Program,
    param_env: ParamEnvId,
    variance: V,
    pub goals: Vec<Goal>,
}
impl<'a, 'tcx> LatticeOp<'a, 'tcx> {
    pub fn new(infcx: &'a InferCtxt<'tcx>, prog: &'a Program, param_env: ParamEnvId, kind: LatticeOpKind) -> Self {
        Self { infcx, prog, param_env, variance: match kind { LatticeOpKind::Lub => V::Co, LatticeOpKind::Glb => V::Contra }, goals: Vec::new() }
    }
    fn invariant(&self) -> solver_relating::SolverRelating<'_, 'tcx> {
        solver_relating::SolverRelating::new(self.infcx, self.prog, V::Inv, self.param_env)
    }
    fn relate_bound(&mut self, v: TyId, a: TyId, b: TyId) -> Result<(), TypeError> {
        let at = self.infcx.at(self.prog, self.param_env);
        for bound in [a, b] {
            self.goals.extend(if self.variance == V::Co { at.sub(bound, v)? } else { at.sub(v, bound)? });
        }
        Ok(())
    }
}
impl<'tcx> TypeRelation<'tcx> for LatticeOp<'_, 'tcx> {
    fn tcx(&self) -> &'tcx crate::tcx::Tcx { self.infcx.tcx }
    fn prog(&self) -> &Program { self.prog }
    // The shared traversal composes variance. Co/Contra select LUB/GLB,
    // Inv delegates to equality and Bi does not constrain either operand.
    fn variance(&self) -> V { self.variance }
    fn set_variance(&mut self, variance: V) { self.variance = variance; }
    fn tys(&mut self, a: TyId, b: TyId) -> Result<TyId, TypeError> {
        if a == b || self.variance == V::Bi { return Ok(a); }
        if self.variance == V::Inv {
            let mut eq = self.invariant();
            let ty = eq.tys(a, b)?;
            self.goals.extend(eq.goals);
            return Ok(ty);
        }
        let a = self.infcx.shallow_resolve(a);
        let b = self.infcx.shallow_resolve(b);
        match (self.tcx().tys.kind(a), self.tcx().tys.kind(b)) {
            (TyKind::Infer(_), _) => {
                let v = self.infcx.next_ty_var();
                self.relate_bound(v, b, a)?;
                Ok(v)
            }
            (_, TyKind::Infer(_)) => {
                let v = self.infcx.next_ty_var();
                self.relate_bound(v, a, b)?;
                Ok(v)
            }
            // combine.rs super_combine_tys (next solver): an alias against anything is an
            // alias-relate goal, not a structural relation
            (TyKind::Alias(crate::consts::IsRigid::No, _), _) |
            (_, TyKind::Alias(crate::consts::IsRigid::No, _)) |
            (TyKind::IntVar(_) | TyKind::FloatVar(_) | TyKind::Error, _) |
            (_, TyKind::IntVar(_) | TyKind::FloatVar(_) | TyKind::Error) => {
                let mut eq = self.invariant();
                let ty = eq.tys(a, b)?;
                self.goals.extend(eq.goals);
                Ok(ty)
            }
            _ => structurally_relate_tys(self, a, b),
        }
    }
    fn regions(&mut self, a: RegionId, b: RegionId) -> Result<RegionId, TypeError> {
        let mut inner = self.infcx.inner.borrow_mut();
        let mut regions = inner.unwrap_region_constraints();
        let tys = &self.infcx.tcx.tys;
        // lattice.rs regions: SubregionOrigin::Subtype(trace)
        let origin = crate::infer::region_constraints::SubregionOrigin::Subtype(self.infcx.origin_span.get());
        Ok(match self.variance {
            V::Co => regions.glb_regions(tys, origin, a, b),
            V::Contra => regions.lub_regions(tys, origin, a, b),
            V::Inv => { regions.make_eqregion(tys, origin, a, b, true); a }
            V::Bi => a,
        })
    }
    fn consts(&mut self, a: Const, b: Const) -> Result<Const, TypeError> {
        let mut eq = self.invariant();
        let ct = eq.consts(a, b)?;
        self.goals.extend(eq.goals);
        Ok(ct)
    }
    fn binders(&mut self, a: TyId, b: TyId) -> Result<TyId, TypeError> {
        if a == b { return Ok(a); }
        if self.infcx.fn_has_bound_vars(a) || self.infcx.fn_has_bound_vars(b) {
            let mut eq = self.invariant();
            eq.binders(a, b)?;
            self.goals.extend(eq.goals);
            Ok(a)
        } else { relate_fn_sig(self, a, b) }
    }
    fn binders_args(&mut self, a: &[GenericArg], b: &[GenericArg]) -> Result<Vec<GenericArg>, TypeError> {
        let mut eq = self.invariant();
        let args = eq.binders_args(a, b)?;
        self.goals.extend(eq.goals);
        Ok(args)
    }
    fn has_unresolved_infer(&self, ty: TyId) -> bool {
        self.tcx().tys.has_infer(self.infcx.deeply_resolve_ignoring_regions(ty))
    }
    fn well_formed(&mut self, ty: TyId) {
        use crate::next_solver::{Predicate, PredicateKind, ClauseKind, Term};
        self.goals.push(Goal { param_env: self.param_env, predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(ty)))) });
    }
}
