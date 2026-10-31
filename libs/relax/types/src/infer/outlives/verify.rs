// ported from N:rustc_infer/src/infer/outlives/verify.rs @8d1a764
// ported from N:rustc_infer/src/infer/region_constraints/mod.rs @8d1a764
use super::components::{self, Component};
use super::test_type_match;
use crate::next_solver::{Predicate, PredicateKind, ClauseKind};
use crate::program::Program;
use crate::tcx::Tcx;
use crate::types::{RegionId, RegionKind, TyId, TyKind};

#[derive(Clone, Debug)]
pub enum VerifyBound { IfEq(Predicate), OutlivedBy(RegionId), IsEmpty, AnyBound(Vec<VerifyBound>), AllBounds(Vec<VerifyBound>) }
impl VerifyBound {
    fn must_hold(&self, tys: &crate::types::Types) -> bool {
        match self {
            Self::OutlivedBy(r) => tys.region_kind(*r) == RegionKind::ReStatic,
            Self::AnyBound(bounds) => bounds.iter().any(|b| b.must_hold(tys)),
            Self::AllBounds(bounds) => bounds.iter().all(|b| b.must_hold(tys)),
            Self::IfEq(_) | Self::IsEmpty => false,
        }
    }
    fn cannot_hold(&self) -> bool {
        match self {
            Self::AnyBound(bounds) => bounds.iter().all(Self::cannot_hold),
            Self::AllBounds(bounds) => bounds.iter().any(Self::cannot_hold),
            Self::IfEq(_) | Self::IsEmpty | Self::OutlivedBy(_) => false,
        }
    }
    fn or(self, tys: &crate::types::Types, other: Self) -> Self {
        if self.must_hold(tys) || other.cannot_hold() { return self; }
        if other.must_hold(tys) || self.cannot_hold() { return other; }
        Self::AnyBound(vec![self, other])
    }
}
#[derive(Clone, Debug)]
pub struct Verify { pub ty: TyId, pub region: RegionId, pub bound: VerifyBound, pub span: u32 }
/// An alias type is rigid or not by how it was produced (Relax's solver marks the aliases it could not normalize);
/// rustc's types do not say so, and two spellings of one alias are the same type.
fn forget_rigidity(tys: &crate::types::Types, t: TyId) -> TyId {
    struct Forget;
    impl crate::types::fold::TypeFolder for Forget {
        fn fold_ty(&mut self, tys: &crate::types::Types, t: TyId, depth: u32) -> TyId {
            let t = crate::types::fold::super_fold_ty(tys, t, self, depth);
            match tys.kind(t) {
                TyKind::Alias(crate::consts::IsRigid::Yes, a) => tys.intern(TyKind::Alias(crate::consts::IsRigid::No, *a)),
                _ => t,
            }
        }
    }
    crate::types::fold::TypeFolder::fold_ty(&mut Forget, tys, t, 0)
}
pub struct VerifyBoundCx<'a> {
    pub tcx: &'a Tcx, pub prog: &'a Program,
    pub region_bound_pairs: &'a [(TyId, RegionId)], pub implicit_region_bound: Option<RegionId>, pub caller_bounds: &'a [Predicate],
}
impl VerifyBoundCx<'_> {
    pub fn param_or_placeholder_bound(&self, ty: TyId) -> VerifyBound {
        let mut bounds = Vec::new();
        for p in self.declared_generic_bounds_from_env_for_erased_ty(ty) {
            let PredicateKind::Clause(ClauseKind::TypeOutlives(_, r)) = p.kind else { unreachable!() };
            if matches!(self.tcx.tys.region_kind(r), RegionKind::ReBound(..)) { return VerifyBound::AllBounds(Vec::new()); }
            bounds.push(VerifyBound::OutlivedBy(r));
        }
        if let Some(r) = self.implicit_region_bound { bounds.push(VerifyBound::OutlivedBy(r)); }
        match bounds.len() { 0 => VerifyBound::IsEmpty, 1 => bounds.pop().unwrap(), _ => VerifyBound::AnyBound(bounds) }
    }
    pub fn approx_declared_bounds_from_env(&self, ty: TyId) -> Vec<Predicate> {
        self.declared_generic_bounds_from_env_for_erased_ty(self.tcx.tys.erase_regions(ty))
    }
    fn declared_generic_bounds_from_env_for_erased_ty(&self, erased: TyId) -> Vec<Predicate> {
        let mut bounds = Vec::new();
        for p in self.caller_bounds { if test_type_match::can_match_erased_ty(self.tcx, self.prog, p, erased) { bounds.push(p.clone()); } }
        let erased = forget_rigidity(&self.tcx.tys, erased);
        for &(ty, r) in self.region_bound_pairs {
            if forget_rigidity(&self.tcx.tys, self.tcx.tys.erase_regions(ty)) == erased { bounds.push(Predicate::dummy(PredicateKind::Clause(ClauseKind::TypeOutlives(ty, r)))); }
        }
        bounds
    }
    pub fn declared_bounds_from_definition(&self, ty: TyId) -> Vec<RegionId> {
        let TyKind::Alias(_, alias) = self.tcx.tys.kind(ty) else { unreachable!() };
        let def = alias.kind.def_id();
        let args = self.tcx.complete_args(self.prog, def, self.tcx.tys.list(alias.args), alias.regions);
        let mut out = Vec::new();
        for p in self.tcx.item_bounds(self.prog, def) {
            let p = self.tcx.instantiate_predicate(self.prog, def, &args, &p);
            if !!p.has_bound_vars(&self.tcx.tys) { continue; }
            if let PredicateKind::Clause(ClauseKind::TypeOutlives(subject, r)) = p.kind {
                if let TyKind::Alias(_, subject) = self.tcx.tys.kind(subject) {
                    if subject == alias { out.push(r); }
                }
            }
        }
        out
    }
    pub fn alias_bound(&self, ty: TyId) -> VerifyBound {
        let mut env_bounds = Vec::new();
        for p in self.approx_declared_bounds_from_env(ty) {
            let PredicateKind::Clause(ClauseKind::TypeOutlives(t, r)) = p.kind else { unreachable!() };
            // (an exact match compares the alias itself, not its interned type: rigidity is not identity)
            let exact = matches!((self.tcx.tys.kind(t), self.tcx.tys.kind(ty)), (TyKind::Alias(_, a), TyKind::Alias(_, b)) if a == b);
            if !p.has_bound_vars(&self.tcx.tys) && exact { env_bounds.push(VerifyBound::OutlivedBy(r)); }
            else { env_bounds.push(VerifyBound::IfEq(p)); }
        }
        for r in self.declared_bounds_from_definition(ty) { env_bounds.push(VerifyBound::OutlivedBy(r)); }
        let TyKind::Alias(_, alias) = self.tcx.tys.kind(ty) else { unreachable!() };
        let mut components = Vec::new();
        components::compute_alias_components_recursive(self.tcx, self.prog, alias, &mut components);
        VerifyBound::AnyBound(env_bounds).or(&self.tcx.tys, self.bound_from_components(&components))
    }
    fn bound_from_components(&self, components: &[Component]) -> VerifyBound {
        let mut bounds = Vec::new();
        for component in components {
            let bound = match component {
                Component::Region(r) => VerifyBound::OutlivedBy(*r),
                Component::Param(t) | Component::Placeholder(t) => self.param_or_placeholder_bound(*t),
                Component::Alias(t) => self.alias_bound(*t),
                Component::EscapingAlias(cs) => self.bound_from_components(cs),
                Component::UnresolvedInferenceVariable(_) => VerifyBound::AnyBound(Vec::new()),
            };
            if !bound.must_hold(&self.tcx.tys) { bounds.push(bound); }
        }
        if bounds.len() == 1 { bounds.pop().unwrap() } else { VerifyBound::AllBounds(bounds) }
    }
}
