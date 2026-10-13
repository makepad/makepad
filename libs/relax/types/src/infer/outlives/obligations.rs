// ported from N:rustc_infer/src/infer/outlives/obligations.rs @8d1a764
use super::{components::{self, Component}, verify::{Verify, VerifyBound, VerifyBoundCx}};
use crate::borrowck::constraints::ConstraintCategory;
use crate::infer::InferCtxt;
use crate::program::Program;
use crate::tcx::Tcx;
use crate::types::{RegionId, TyId, TyKind, AliasTyKind};
use crate::next_solver::{Predicate, PredicateKind, ClauseKind};

/// obligations.rs:432: where `TypeOutlives` sends what it derives. Lexical region
/// resolution records into the InferCtxt; borrowck converts into NLL constraints.
pub trait TypeOutlivesDelegate {
    fn delayed_bug(&mut self, message: String);
    fn push_sub_region_constraint(&mut self, a: RegionId, b: RegionId, category: ConstraintCategory);
    fn push_verify(&mut self, ty: TyId, a: RegionId, bound: VerifyBound);
}

pub struct TypeOutlives<'a> { delegate: &'a mut dyn TypeOutlivesDelegate, tcx: &'a Tcx, verify_bound: VerifyBoundCx<'a> }
impl<'a> TypeOutlives<'a> {
    pub fn new(delegate: &'a mut dyn TypeOutlivesDelegate, tcx: &'a Tcx, prog: &'a Program, region_bound_pairs: &'a [(TyId, RegionId)],
        implicit_region_bound: Option<RegionId>, caller_bounds: &'a [Predicate]) -> Self {
        Self { delegate, tcx, verify_bound: VerifyBoundCx { tcx, prog, region_bound_pairs, implicit_region_bound, caller_bounds } }
    }
    pub fn type_must_outlive(&mut self, ty: TyId, region: RegionId, category: ConstraintCategory) {
        assert!(!self.tcx.tys.has_vars_bound_at_or_above(ty, 0));
        let mut components = Vec::new();
        components::push_outlives_components(self.tcx, self.verify_bound.prog, ty, &mut components);
        self.components_must_outlive(&components, region, category);
    }
    fn components_must_outlive(&mut self, components: &[Component], region: RegionId, category: ConstraintCategory) {
        for component in components {
            match *component {
                Component::Region(r) => self.delegate.push_sub_region_constraint(region, r, category),
                Component::Param(ty) | Component::Placeholder(ty) => {
                    let bound = self.verify_bound.param_or_placeholder_bound(ty);
                    self.delegate.push_verify(ty, region, bound);
                }
                Component::Alias(ty) => self.alias_ty_must_outlive(ty, region, category),
                Component::EscapingAlias(ref cs) => self.components_must_outlive(cs, region, category),
                Component::UnresolvedInferenceVariable(ty) => self.delegate.delayed_bug(format!("unresolved type in region resolution: {ty:?}")),
            }
        }
    }
    fn alias_ty_must_outlive(&mut self, ty: TyId, region: RegionId, category: ConstraintCategory) {
        let tcx = self.tcx;
        let TyKind::Alias(_, alias) = tcx.tys.kind(ty) else { unreachable!() };
        if alias.args.is_empty() && tcx.tys.region_args(alias.regions).is_empty() { return; }
        if tcx.tys.has_infer(ty) { self.delegate.delayed_bug("unresolved type in alias outlives".into()); }
        let trait_bounds = self.verify_bound.declared_bounds_from_definition(ty);
        let env_bounds = self.verify_bound.approx_declared_bounds_from_env(ty);
        let has_infer_regions = tcx.tys.flags(ty).contains(crate::types::TypeFlags::HAS_RE_INFER);
        if env_bounds.is_empty() && trait_bounds.is_empty() && (has_infer_regions || matches!(alias.kind, AliasTyKind::Opaque { .. })) {
            for &ty in tcx.tys.list(alias.args) { if !matches!(tcx.tys.kind(ty), TyKind::Const(_)) { self.type_must_outlive(ty, region, category); } }
            for (i, &r) in tcx.tys.region_args(alias.regions).iter().enumerate() {
                if !components::alias_region_is_bivariant(tcx, self.verify_bound.prog, alias, i) { self.delegate.push_sub_region_constraint(region, r, category); }
            }
            return;
        }
        if let Some(&unique) = trait_bounds.first() {
            let mut all = true;
            for &r in &trait_bounds[1..] { all &= r == unique; }
            for p in &env_bounds {
                let PredicateKind::Clause(ClauseKind::TypeOutlives(_, r)) = p.kind else { unreachable!() };
                all &= !p.has_bound_vars(&self.tcx.tys) && r == unique;
            }
            if all { self.delegate.push_sub_region_constraint(region, unique, category); return; }
        }
        let bound = self.verify_bound.alias_bound(ty);
        self.delegate.push_verify(ty, region, bound);
    }
}

/// obligations.rs:720.
impl TypeOutlivesDelegate for &InferCtxt<'_> {
    fn delayed_bug(&mut self, message: String) { self.errors.delayed_bug(self.cause, message); }
    fn push_sub_region_constraint(&mut self, a: RegionId, b: RegionId, _category: ConstraintCategory) {
        // We don't do leak check in lexical region resolution.
        self.inner.borrow_mut().unwrap_region_constraints().make_subregion(&self.tcx.tys, a, b, false);
    }
    fn push_verify(&mut self, ty: TyId, region: RegionId, bound: VerifyBound) {
        self.inner.borrow_mut().unwrap_region_constraints().verify_generic_bound(Verify { ty, region, bound });
    }
}
