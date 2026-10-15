// ported from N:rustc_next_trait_solver/src/placeholder.rs @8d1a764
use crate::infer::InferCtxt;
use crate::types::{Types, TyId, TyKind, RegionId, RegionKind, UniverseIndex, BoundVar, BoundTy, PlaceholderType};
use crate::types::fold::{TypeFolder, super_fold_ty, super_fold_const};
use crate::consts::{Const, ConstKind, BoundConst, PlaceholderConst};
use std::collections::HashMap;

pub struct BoundVarReplacer<'a, 'tcx> {
    pub infcx: &'a InferCtxt<'tcx>, pub universe_indices: &'a mut Vec<Option<UniverseIndex>>,
    pub mapped_regions: HashMap<(UniverseIndex, BoundVar), BoundVar>,
    pub mapped_types: HashMap<PlaceholderType, BoundTy>, pub mapped_consts: HashMap<PlaceholderConst, BoundConst>,
}
impl<'a, 'tcx> BoundVarReplacer<'a, 'tcx> {
    pub fn new(infcx: &'a InferCtxt<'tcx>, universe_indices: &'a mut Vec<Option<UniverseIndex>>) -> Self {
        Self { infcx, universe_indices, mapped_regions: HashMap::new(), mapped_types: HashMap::new(), mapped_consts: HashMap::new() }
    }
    fn universe_for(&mut self, debruijn: u32, depth: u32) -> UniverseIndex {
        let index = (self.universe_indices.len() + depth as usize).checked_sub(debruijn as usize + 1).expect("escaping bound variable beyond normalization binders");
        for i in 0..=index { if self.universe_indices[i].is_none() { self.universe_indices[i] = Some(self.infcx.create_next_universe()); } }
        self.universe_indices[index].unwrap()
    }
}
impl TypeFolder for BoundVarReplacer<'_, '_> {
    fn fold_region(&mut self, tys: &Types, r: RegionId, depth: u32) -> RegionId {
        if let RegionKind::ReBound(d, var) = tys.region_kind(r) {
            if d >= depth { let universe = self.universe_for(d, depth); self.mapped_regions.insert((universe, var), var); return tys.intern_region(RegionKind::RePlaceholder { universe, var }); }
        }
        r
    }
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        if let TyKind::Bound(d, bound) = *tys.kind(ty) {
            if d >= depth { let universe = self.universe_for(d, depth); let p = PlaceholderType { universe, bound: bound.var }; self.mapped_types.insert(p, bound); return tys.intern(TyKind::Placeholder(p)); }
        }
        super_fold_ty(tys, ty, self, depth)
    }
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: u32) -> Const {
        if let ConstKind::Bound(d, bound) = tys.const_kind(ct) {
            if d >= depth { let universe = self.universe_for(d, depth); let p = PlaceholderConst { universe, bound }; self.mapped_consts.insert(p, bound); return tys.intern_const(ConstKind::Placeholder(p)); }
        }
        super_fold_const(tys, ct, self, depth)
    }
}
pub struct PlaceholderReplacer<'a, 'tcx> { pub replacer: BoundVarReplacer<'a, 'tcx> }
impl PlaceholderReplacer<'_, '_> {
    fn index(&self, universe: UniverseIndex, depth: u32) -> u32 {
        let index = self.replacer.universe_indices.iter().position(|&u| u == Some(universe)).expect("placeholder universe");
        (self.replacer.universe_indices.len() - index - 1) as u32 + depth
    }
}
impl TypeFolder for PlaceholderReplacer<'_, '_> {
    fn fold_region(&mut self, tys: &Types, r: RegionId, depth: u32) -> RegionId {
        let r = if let RegionKind::ReVar(vid) = tys.region_kind(r) { self.replacer.infcx.inner.borrow_mut().unwrap_region_constraints().shallow_resolve_region_var(tys, vid) } else { r };
        if let RegionKind::RePlaceholder { universe, var } = tys.region_kind(r) {
            if let Some(&bound) = self.replacer.mapped_regions.get(&(universe, var)) { return tys.intern_region(RegionKind::ReBound(self.index(universe, depth), bound)); }
        }
        r
    }
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        let ty = self.replacer.infcx.shallow_resolve(ty);
        if let TyKind::Placeholder(p) = tys.kind(ty) {
            if let Some(&bound) = self.replacer.mapped_types.get(p) { return tys.intern(TyKind::Bound(self.index(p.universe, depth), bound)); }
        }
        super_fold_ty(tys, ty, self, depth)
    }
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: u32) -> Const {
        let ct = self.replacer.infcx.shallow_resolve_const(ct);
        if let ConstKind::Placeholder(p) = tys.const_kind(ct) {
            if let Some(&bound) = self.replacer.mapped_consts.get(&p) { return tys.intern_const(ConstKind::Bound(self.index(p.universe, depth), bound)); }
        }
        super_fold_const(tys, ct, self, depth)
    }
}
