// ported from N:rustc_trait_selection/src/solve/fulfill.rs @8d1a764
// ported from N:rustc_trait_selection/src/traits/engine.rs @8d1a764
use super::*;
use crate::infer::InferCtxt;
use crate::program::Program;

/// rustc_middle/src/traits/mod.rs ObligationCause: diagnostics retain the
/// initiating expression and its analysis owner across fulfillment retries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObligationCause { pub span: u32, pub body_id: crate::program::DefId }
impl Default for ObligationCause {
    fn default() -> Self { Self { span: 0, body_id: crate::program::NO_DEF } }
}
#[derive(Clone, Debug)]
pub struct PredicateObligation { pub goal: Goal, pub recursion_depth: usize, pub cause: ObligationCause }
#[derive(Clone, Debug)]
pub enum NextSolverError { TrueError(PredicateObligation), Ambiguity(PredicateObligation), Overflow(PredicateObligation) }
#[derive(Default)]
struct ObligationStorage { pending: Vec<(PredicateObligation, Option<GoalStalledOn>)> }
pub struct FulfillmentCtxt { obligations: ObligationStorage, usable_in_snapshot: usize }
impl FulfillmentCtxt {
    pub fn new(infcx: &InferCtxt<'_>) -> Self { Self { obligations: Default::default(), usable_in_snapshot: infcx.num_open_snapshots() } }
    pub fn register_predicate_obligation(&mut self, infcx: &InferCtxt<'_>, obligation: PredicateObligation) {
        assert_eq!(self.usable_in_snapshot, infcx.num_open_snapshots());
        self.obligations.pending.push((obligation, None));
    }
    pub fn has_pending_obligations(&self) -> bool { !self.obligations.pending.is_empty() }
    pub fn pending_obligations(&self) -> Vec<PredicateObligation> { self.obligations.pending.iter().map(|(o,_)| o.clone()).collect() }
    pub fn try_evaluate_obligations(&mut self, infcx: &InferCtxt<'_>, prog: &Program) -> Vec<NextSolverError> {
        assert_eq!(self.usable_in_snapshot, infcx.num_open_snapshots());
        let mut errors = Vec::new();
        loop {
            let mut any_changed = false;
            self.obligations.pending.retain_mut(|(obligation, stalled_on)| {
                let result = match infcx.evaluate_root_goal(prog, obligation.goal.clone(), stalled_on.as_ref()) {
                    Ok(result) => result,
                    Err(NoSolution) => { errors.push(NextSolverError::TrueError(obligation.clone())); return false; }
                };
                obligation.goal = result.goal;
                *stalled_on = result.stalled_on;
                if result.has_changed {
                    if obligation.recursion_depth >= infcx.recursion_limit(prog) {
                        errors.push(NextSolverError::Overflow(obligation.clone())); return false;
                    }
                    obligation.recursion_depth += 1; any_changed = true;
                }
                !result.certainty.is_yes()
            });
            if !any_changed { break; }
        }
        errors
    }
    pub fn evaluate_obligations_error_on_ambiguity(&mut self, infcx: &InferCtxt<'_>, prog: &Program) -> Vec<NextSolverError> {
        let mut errors = self.try_evaluate_obligations(infcx, prog);
        for (obligation, _) in std::mem::take(&mut self.obligations.pending) { errors.push(NextSolverError::Ambiguity(obligation)); }
        errors
    }
}
pub struct ObligationCtxt<'a, 'tcx> { pub infcx: &'a InferCtxt<'tcx>, pub prog: &'a Program, pub engine: FulfillmentCtxt, pub cause: ObligationCause }
impl<'a, 'tcx> ObligationCtxt<'a, 'tcx> {
    pub fn new(infcx: &'a InferCtxt<'tcx>, prog: &'a Program) -> Self { Self { infcx, prog, engine: FulfillmentCtxt::new(infcx), cause: infcx.cause } }
    pub fn register_obligation(&mut self, goal: Goal) { self.engine.register_predicate_obligation(self.infcx, PredicateObligation { goal, recursion_depth: 0, cause: self.cause }); }
    pub fn register_obligations(&mut self, goals: Vec<Goal>) { for goal in goals { self.register_obligation(goal); } }
    pub fn eq(&mut self, env: &ParamEnvId, a: TyId, b: TyId) -> Result<(), NoSolution> {
        let goals = self.infcx.at(self.prog, env.clone()).eq(a, b).map_err(|_| NoSolution)?;
        self.register_obligations(goals); Ok(())
    }
    pub fn normalize_ty(&mut self, env: &ParamEnvId, ty: TyId) -> TyId {
        let (ty, goals) = self.infcx.normalize_ty(self.prog, env, ty); self.register_obligations(goals); ty
    }
    pub fn normalize_predicate(&mut self, env: &ParamEnvId, p: &Predicate) -> Predicate {
        let (p, goals) = self.infcx.normalize_predicate(self.prog, env, p); self.register_obligations(goals); p
    }
    pub fn deeply_normalize_ty(&mut self, env: &ParamEnvId, ty: TyId) -> Result<TyId, NoSolution> {
        let ty = self.normalize_ty(env, ty);
        self.evaluate_obligations_error_on_ambiguity()?;
        Ok(self.infcx.deeply_resolve_ignoring_regions(ty))
    }
    pub fn deeply_normalize_predicate(&mut self, env: &ParamEnvId, p: &Predicate) -> Result<Predicate, NoSolution> {
        let p = self.normalize_predicate(env, p);
        self.evaluate_obligations_error_on_ambiguity()?;
        let mut resolver = crate::infer::resolve::DeepResolverIgnoringRegions::new(self.infcx);
        Ok(fold::fold_predicate(&self.infcx.tcx.tys, &p, &mut resolver))
    }
    pub fn evaluate_obligations_error_on_ambiguity(&mut self) -> Result<(), NoSolution> {
        if self.engine.evaluate_obligations_error_on_ambiguity(self.infcx, self.prog).is_empty() { Ok(()) } else { Err(NoSolution) }
    }
    /// rustc_trait_selection/src/traits/engine.rs (ObligationCtxt::try_evaluate_obligations).
    pub fn try_evaluate_obligations(&mut self) -> Vec<NextSolverError> { self.engine.try_evaluate_obligations(self.infcx, self.prog) }
}
