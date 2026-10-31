// ported from N:rustc_infer/src/infer/resolve.rs @8d1a764
use super::InferCtxt;
use crate::consts::Const;
use crate::types::{Types, TyId, DebruijnIndex};
use crate::types::fold::{TypeFolder, super_fold_ty};
use crate::types::fold::super_fold_const;
use std::collections::HashMap;

pub struct DeepResolverIgnoringRegions<'a, 'tcx> {
    infcx: &'a InferCtxt<'tcx>,
    cache: HashMap<TyId, TyId>,
}
impl<'a, 'tcx> DeepResolverIgnoringRegions<'a, 'tcx> {
    pub fn new(infcx: &'a InferCtxt<'tcx>) -> Self { Self { infcx, cache: HashMap::new() } }
}
impl TypeFolder for DeepResolverIgnoringRegions<'_, '_> {
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: DebruijnIndex) -> Const {
        if !ct.has_non_region_infer(tys) { return ct; }
        super_fold_const(tys, self.infcx.shallow_resolve_const(ct), self, depth)
    }

    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: DebruijnIndex) -> TyId {
        if !tys.has_infer(ty) { return ty; }
        if let Some(&value) = self.cache.get(&ty) { return value; }
        let shallow = self.infcx.shallow_resolve(ty);
        let result = super_fold_ty(tys, shallow, self, depth);
        self.cache.insert(ty, result);
        result
    }
}

impl InferCtxt<'_> {
    pub fn deeply_resolve_ignoring_regions(&self, ty: TyId) -> TyId {
        // rustc_infer/src/infer/mod.rs:1452-1454: consuming an error type taints
        // the context, including values returned by a previously evaluated query.
        if self.tcx.tys.flags(ty).intersects(crate::types::TypeFlags::HAS_ERROR) { self.set_tainted_by_errors(); }
        DeepResolverIgnoringRegions::new(self).fold_ty(&self.tcx.tys, ty, 0)
    }
}

/// rustc_type_ir/src/infer_ctxt.rs:647: resolve types and unified regions.
pub struct DeepVariableResolver<'a, 'tcx> { infcx: &'a InferCtxt<'tcx> }
impl<'a, 'tcx> DeepVariableResolver<'a, 'tcx> {
    pub fn new(infcx: &'a InferCtxt<'tcx>) -> Self { Self { infcx } }
}
impl TypeFolder for DeepVariableResolver<'_, '_> {
    fn fold_flags(&self) -> Option<crate::types::TypeFlags> { Some(crate::types::TypeFlags::HAS_ANY_INFER) }
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId { if !tys.flags(ty).intersects(crate::types::TypeFlags::HAS_ANY_INFER) { return ty; } super_fold_ty(tys, self.infcx.shallow_resolve(ty), self, depth) }
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: u32) -> Const { if tys.const_flags(ct).0 & crate::types::TypeFlags::HAS_ANY_INFER.bits() == 0 { return ct; } super_fold_const(tys, self.infcx.shallow_resolve_const(ct), self, depth) }
    fn fold_region(&mut self, tys: &Types, r: crate::types::RegionId, _: u32) -> crate::types::RegionId {
        if let crate::types::RegionKind::ReVar(v) = tys.region_kind(r) { self.infcx.inner.borrow_mut().unwrap_region_constraints().shallow_resolve_region_var(tys, v) } else { r }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixupError { pub unresolved: crate::next_solver::Term }
struct FullTypeResolver<'a, 'tcx> { infcx: &'a InferCtxt<'tcx>, error: Option<FixupError> }
impl TypeFolder for FullTypeResolver<'_, '_> {
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        if self.error.is_some() { return ty; }
        let ty = self.infcx.shallow_resolve(ty);
        if matches!(tys.kind(ty), crate::types::TyKind::Infer(_) | crate::types::TyKind::IntVar(_) | crate::types::TyKind::FloatVar(_)) {
            self.error = Some(FixupError { unresolved: crate::next_solver::Term::Ty(ty) }); ty
        } else { super_fold_ty(tys, ty, self, depth) }
    }
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: u32) -> Const {
        if self.error.is_some() { return ct; }
        let ct = self.infcx.shallow_resolve_const(ct);
        if matches!(tys.const_kind(ct), crate::consts::ConstKind::Infer(_)) {
            self.error = Some(FixupError { unresolved: crate::next_solver::Term::Const(ct) }); ct
        } else { super_fold_const(tys, ct, self, depth) }
    }
    fn fold_region(&mut self, tys: &Types, r: crate::types::RegionId, _: u32) -> crate::types::RegionId {
        if matches!(tys.region_kind(r), crate::types::RegionKind::ReVar(_)) {
            self.infcx.lexical_region_resolutions.borrow().as_ref().expect("regions not resolved").resolve_region(tys, r)
        } else { r }
    }
}
impl InferCtxt<'_> {
    /// rustc_infer/src/infer/resolve.rs:76-151: unresolved types/consts are errors;
    /// lexical empty regions remain unresolved, as in LexicalRegionResolutions.
    pub fn deeply_resolve_via_region_graph(&self, ty: TyId) -> Result<TyId, FixupError> {
        let mut resolver = FullTypeResolver { infcx: self, error: None };
        let ty = resolver.fold_ty(&self.tcx.tys, ty, 0);
        match resolver.error { Some(error) => Err(error), None => Ok(ty) }
    }
}

impl InferCtxt<'_> {
    pub fn deeply_resolve_clauses_via_region_graph(&self, clauses: &[crate::next_solver::Predicate]) -> Result<Vec<crate::next_solver::Predicate>, FixupError> {
        let mut resolver = FullTypeResolver { infcx: self, error: None };
        let mut result = Vec::with_capacity(clauses.len());
        for clause in clauses { result.push(crate::next_solver::fold::fold_predicate(&self.tcx.tys, clause, &mut resolver)); }
        match resolver.error { Some(error) => Err(error), None => Ok(result) }
    }
}
