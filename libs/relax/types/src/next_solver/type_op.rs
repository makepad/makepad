// ported from N:rustc_trait_selection/src/traits/query/type_op/mod.rs @8d1a764
// ported from N:rustc_trait_selection/src/traits/query/type_op/custom.rs @8d1a764
// ported from N:rustc_trait_selection/src/traits/query/type_op/normalize.rs @8d1a764
// ported from N:rustc_trait_selection/src/traits/query/type_op/prove_predicate.rs @8d1a764
// ported from N:rustc_traits/src/type_op.rs @8d1a764 (type_op_normalize, type_op_prove_predicate)
// ported from N:rustc_infer/src/infer/canonical/query_response.rs @8d1a764 (make_query_region_constraints)
//
// DEBLOAT: the canonical-query wrapper (canonicalize_query / enter_canonical_trait_query /
// instantiate_nll_query_response_and_region_obligations) is the old solver's caching layer.
// With the next solver its provider runs the same ObligationCtxt operation, so each op is
// performed directly in the caller's inference context under scrape_region_constraints.
// Region assumptions (-Zhigher-ranked-assumptions) and error_info (diagnostics) are cut.
use super::*;
use super::fulfill::ObligationCtxt;
use crate::borrowck::constraints::ConstraintCategory;
use crate::infer::InferCtxt;
use crate::infer::region_constraints::{ConstraintKind, RegionConstraintData};
use crate::program::Program;
use crate::tcx::outlives::OutlivesPredicate;

/// rustc_middle ty::RegionConstraint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionConstraint { Outlives(OutlivesPredicate), Eq(RegionId, RegionId) }
impl RegionConstraint {
    pub fn iter_outlives(self) -> impl Iterator<Item = OutlivesPredicate> {
        let (first, second) = match self {
            Self::Outlives(p) => (p, None),
            Self::Eq(a, b) => (OutlivesPredicate::Region(a, b), Some(OutlivesPredicate::Region(b, a))),
        };
        std::iter::once(first).chain(second)
    }
}
/// rustc_middle infer/canonical.rs QueryRegionConstraint(s).
#[derive(Clone, Copy, Debug)]
pub struct QueryRegionConstraint { pub constraint: RegionConstraint, pub category: ConstraintCategory, pub visible_for_leak_check: bool }
#[derive(Clone, Debug, Default)]
pub struct QueryRegionConstraints { pub constraints: Vec<QueryRegionConstraint> }
impl QueryRegionConstraints {
    pub fn is_empty(&self) -> bool { self.constraints.is_empty() }
}

pub struct TypeOpOutput<R> { pub output: R, pub constraints: Option<QueryRegionConstraints> }

pub trait TypeOp: Sized {
    type Output;
    fn fully_perform(self, infcx: &InferCtxt<'_>, prog: &Program) -> Result<TypeOpOutput<Self::Output>, NoSolution>;
}

#[derive(Clone, Debug)]
pub struct ParamEnvAnd<Q> { pub param_env: ParamEnvId, pub value: Q }
#[derive(Clone, Copy, Debug)]
pub struct Normalize { pub value: TyId }
#[derive(Clone, Debug)]
pub struct ProvePredicate { pub predicate: Predicate }

/// normalize.rs:14 try_fast_path; rustc_traits type_op.rs:41 type_op_normalize with the
/// next solver's deep normalization (at.query_normalize).
impl TypeOp for ParamEnvAnd<Normalize> {
    type Output = TyId;
    fn fully_perform(self, infcx: &InferCtxt<'_>, prog: &Program) -> Result<TypeOpOutput<TyId>, NoSolution> {
        let ty = self.value.value;
        let tys = &infcx.tcx.tys;
        if !tys.may_project(ty) && !tys.has_opaque(ty) { return Ok(TypeOpOutput { output: ty, constraints: None }); }
        scrape_region_constraints(infcx, prog, |ocx| ocx.deeply_normalize_ty(&self.param_env, ty))
    }
}
/// prove_predicate.rs:10 (the WF fast path; sizedness has no Relax fast path here) and
/// type_op_prove_predicate_with_cause.
impl TypeOp for ParamEnvAnd<ProvePredicate> {
    type Output = ();
    fn fully_perform(self, infcx: &InferCtxt<'_>, prog: &Program) -> Result<TypeOpOutput<()>, NoSolution> {
        scrape_region_constraints(infcx, prog, |ocx| {
            ocx.register_obligation(Goal { param_env: self.param_env.clone(), predicate: self.value.predicate.clone() });
            Ok(())
        })
    }
}

/// custom.rs:16.
pub struct CustomTypeOp<F> { closure: F }
impl<F> CustomTypeOp<F> { pub fn new(closure: F) -> Self { Self { closure } } }
impl<R, F: FnOnce(&mut ObligationCtxt<'_, '_>) -> Result<R, NoSolution>> TypeOp for CustomTypeOp<F> {
    type Output = R;
    fn fully_perform(self, infcx: &InferCtxt<'_>, prog: &Program) -> Result<TypeOpOutput<R>, NoSolution> {
        scrape_region_constraints(infcx, prog, self.closure)
    }
}

/// custom.rs:68-150: run `op`, prove what it registered, then scrape the region
/// constraints and type-outlives obligations it produced.
pub fn scrape_region_constraints<R>(infcx: &InferCtxt<'_>, prog: &Program,
    op: impl FnOnce(&mut ObligationCtxt<'_, '_>) -> Result<R, NoSolution>) -> Result<TypeOpOutput<R>, NoSolution> {
    let pre_obligations = infcx.take_registered_region_obligations();
    assert!(pre_obligations.is_empty(), "scrape_region_constraints: incoming region obligations = {pre_obligations:?}");
    let value = infcx.commit_if_ok(|_| {
        let mut ocx = ObligationCtxt::new(infcx, prog);
        let value = op(&mut ocx)?;
        ocx.evaluate_obligations_error_on_ambiguity()?;
        Ok(value)
    })?;
    let region_obligations = infcx.take_registered_region_obligations();
    let region_constraint_data = infcx.take_and_reset_region_constraints();
    let region_constraints = make_query_region_constraints(region_obligations, &region_constraint_data);
    let constraints = if region_constraints.is_empty() { None } else { Some(region_constraints) };
    Ok(TypeOpOutput { output: value, constraints })
}

/// query_response.rs:618: sub (<=) constraints become outlives (>=).
pub fn make_query_region_constraints(outlives_obligations: Vec<(TyId, RegionId)>, region_constraints: &RegionConstraintData) -> QueryRegionConstraints {
    assert!(region_constraints.verifys.is_empty());
    let mut constraints = Vec::with_capacity(region_constraints.constraints.len() + outlives_obligations.len());
    for c in &region_constraints.constraints {
        let constraint = match c.kind {
            ConstraintKind::VarSubVar | ConstraintKind::RegSubVar | ConstraintKind::VarSubReg | ConstraintKind::RegSubReg =>
                RegionConstraint::Outlives(OutlivesPredicate::Region(c.sup, c.sub)),
            ConstraintKind::VarEqVar | ConstraintKind::VarEqReg | ConstraintKind::RegEqReg => RegionConstraint::Eq(c.sup, c.sub),
        };
        constraints.push(QueryRegionConstraint { constraint, category: ConstraintCategory::Boring, visible_for_leak_check: c.visible_for_leak_check });
    }
    for (sup_type, sub_region) in outlives_obligations {
        // We don't do leak checks for type outlives.
        constraints.push(QueryRegionConstraint { constraint: RegionConstraint::Outlives(OutlivesPredicate::Type(sup_type, sub_region)),
            category: ConstraintCategory::Boring, visible_for_leak_check: false });
    }
    QueryRegionConstraints { constraints }
}
