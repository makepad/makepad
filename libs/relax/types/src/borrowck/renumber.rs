// ported from N:rustc_borrowck/src/renumber.rs @8d1a764
use super::infer_ctxt::BorrowckInferCtxt;
use super::region_infer::NllRegionVariableOrigin;
use crate::mir::visit::MutVisitor;
use crate::mir::{Body, Location, Region};
use crate::program::Sym;
use crate::types::{RegionKind, TyId};

/// Replaces all free regions appearing in the MIR with fresh inference variables.
pub fn renumber_mir(infcx: &BorrowckInferCtxt<'_>, body: &mut Body, promoted: &mut [Body]) {
    let mut renumberer = RegionRenumberer { infcx };
    for body in promoted.iter_mut() { renumberer.visit_body(body); }
    renumberer.visit_body(body);
}

/// The fields are used only for debugging output. Relax's MIR visitor carries no
/// TyContext payload, so `TyContext` names the type position kind only.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum RegionCtxt {
    Location(Location),
    TyContext,
    Free(Sym),
    LateBound(Sym),
    Existential(Option<Sym>),
    Placeholder(Sym),
    Unknown,
}

impl RegionCtxt {
    /// Used to determine the representative of a component in the strongly connected
    /// constraint graph.
    pub fn preference_value(self) -> usize {
        match self {
            RegionCtxt::Unknown => 1,
            RegionCtxt::Existential(None) => 2,
            RegionCtxt::Existential(Some(_)) | RegionCtxt::Free(_) => 2,
            RegionCtxt::Location(_) => 3,
            RegionCtxt::TyContext => 4,
            _ => 5,
        }
    }
}

pub struct RegionRenumberer<'a, 'tcx> { pub infcx: &'a BorrowckInferCtxt<'tcx> }

impl RegionRenumberer<'_, '_> {
    /// Replaces all regions appearing in `value` with fresh inference variables.
    pub fn renumber_regions(&mut self, value: TyId, region_ctxt_fn: impl Fn() -> RegionCtxt) -> TyId {
        let origin = NllRegionVariableOrigin::Existential;
        let infcx = self.infcx;
        let tys = &infcx.tcx.tys;
        tys.map_regions(value, &|region, depth| match tys.region_kind(region) {
            RegionKind::ReBound(debruijn, _) if debruijn < depth => region,
            _ => infcx.next_nll_region_var(origin, &region_ctxt_fn),
        }, 0)
    }
}

impl MutVisitor for RegionRenumberer<'_, '_> {
    fn visit_type_region(&mut self, region: &mut crate::types::RegionId) {
        *region = self.infcx.next_nll_region_var(NllRegionVariableOrigin::Existential, || RegionCtxt::TyContext);
    }
    fn visit_type_const(&mut self, ct: &mut crate::consts::Const) {
        let ty = self.infcx.tcx.tys.intern(crate::types::TyKind::Const(*ct));
        let ty = self.renumber_regions(ty, || RegionCtxt::TyContext);
        let crate::types::TyKind::Const(value) = self.infcx.tcx.tys.kind(ty) else { unreachable!() };
        *ct = *value;
    }
    fn visit_ty(&mut self, ty: &mut TyId) {
        *ty = self.renumber_regions(*ty, || RegionCtxt::TyContext);
    }
    fn visit_region(&mut self, region: &mut Region) {
        // MIR `Ref` regions are body-local slots, not interned RegionIds: the fresh
        // variable's index is handed over explicitly.
        let r = self.infcx.next_nll_region_var(NllRegionVariableOrigin::Existential, || RegionCtxt::Unknown);
        let RegionKind::ReVar(vid) = self.infcx.tcx.tys.region_kind(r) else { unreachable!() };
        *region = Region(vid);
    }
}
