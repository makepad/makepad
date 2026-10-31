// ported from N:rustc_infer/src/infer/at.rs @8d1a764
use super::InferCtxt;
use super::relate::{TypeError, TypeRelation, solver_relating::SolverRelating};
use crate::next_solver::Goal;
use crate::next_solver::ParamEnvId;
use crate::types::{TyId, variance::V};

pub struct At<'a, 'tcx> { prog: &'a crate::program::Program, infcx: &'a InferCtxt<'tcx>, param_env: ParamEnvId }

impl<'tcx> InferCtxt<'tcx> {
    pub fn at<'a>(&'a self, prog: &'a crate::program::Program, param_env: ParamEnvId) -> At<'a, 'tcx> { At { prog, infcx: self, param_env } }
}
impl At<'_, '_> {
    /// rustc_infer/src/infer/at.rs:282-295: the caller owns the snapshot.
    pub fn lub(&self, a: TyId, b: TyId) -> Result<(TyId, Vec<Goal>), TypeError> {
        let mut relation = super::relate::lattice::LatticeOp::new(self.infcx, self.prog, self.param_env, super::relate::lattice::LatticeOpKind::Lub);
        let ty = relation.tys(a, b)?;
        Ok((ty, relation.goals))
    }
    pub fn eq(&self, a: TyId, b: TyId) -> Result<Vec<Goal>, TypeError> { self.relate(a, V::Inv, b) }
    pub fn sub(&self, a: TyId, b: TyId) -> Result<Vec<Goal>, TypeError> { self.relate(a, V::Co, b) }
    pub fn relate(&self, a: TyId, variance: V, b: TyId) -> Result<Vec<Goal>, TypeError> {
        // rustc_infer/src/infer/at.rs:131-230: callers own rollback boundaries.
        let mut relation = SolverRelating::new(self.infcx, self.prog, variance, self.param_env);
        relation.tys(a, b)?;
        Ok(relation.goals)
    }
}
