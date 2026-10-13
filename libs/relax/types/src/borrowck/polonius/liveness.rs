// ported from N:rustc_borrowck/src/polonius/liveness.rs @8d1a764
use super::{LiveRegionVariances, record_live_region_variance};
use super::liveness_constraints::regions_in;
use crate::borrowck::{region_infer::values::LivenessValues, universal_regions::UniversalRegions,
    type_check::liveness::LivenessComputation};
use crate::fxhash::HashMap;
use crate::mir::{Body, Local};
use crate::program::Program;
use crate::tcx::Tcx;
use crate::types::TyId;

#[derive(Clone, Default)]
pub struct DeferredRegionLiveness {
    by_region: HashMap<usize, Local>,
    // Already-normalized dropck_outlives components, expressed as region vids.
    // Typeck must register their outlives constraints eagerly, even if unused.
    drop_regions_by_local: HashMap<Local, Vec<usize>>,
}
impl DeferredRegionLiveness {
    pub fn defer_local(&mut self, tcx: &Tcx, universals: &UniversalRegions,
        local: Local, local_ty: TyId, drop_regions: Vec<usize>) {
        self.drop_regions_by_local.insert(local, drop_regions);
        for region in regions_in(tcx, universals, local_ty) {
            let previous = self.by_region.insert(region, local);
            assert!(previous.is_none(), "MIR renumbering must give each deferred local its own regions");
        }
    }
    pub(super) fn ensure_deferred_liveness(&mut self, region: usize, tcx: &Tcx, prog: &Program, body: &Body,
        universals: &UniversalRegions, liveness: &mut LivenessValues,
        variances: &mut LiveRegionVariances, comp: &mut LivenessComputation<'_>) {
        let Some(local) = self.by_region.remove(&region) else { return; };
        let Some(drop_regions) = self.drop_regions_by_local.remove(&local) else { return; };
        let (use_live, drop_live) = comp.compute(local);
        let ty = body.local_decls[local.0 as usize].ty;
        // #160670: variance comes from the entire local type, including for drops.
        if !use_live.is_empty() || !drop_live.is_empty() { record_live_region_variance(tcx, prog, variances, universals, ty); }
        for r in regions_in(tcx, universals, ty) { for p in use_live.iter() { liveness.add_point(r, p); } }
        for r in drop_regions { for p in drop_live.iter() { liveness.add_point(r, p); } }
    }
}

// One concrete source replaces nightly's one-implementation trait. Rows are
// borrowed directly; no bitset copies or eagerly expanded localized CFG graph.
pub(super) struct DeferredLivenessSource<'a> {
    pub tcx: &'a Tcx, pub prog: &'a Program, pub body: &'a Body, pub universals: &'a UniversalRegions,
    pub liveness: &'a mut LivenessValues, pub deferred: DeferredRegionLiveness,
    pub variances: LiveRegionVariances, pub comp: LivenessComputation<'a>,
}
pub(super) struct RegionLiveness<'a> {
    pub direction: super::ConstraintDirection,
    points: Option<&'a crate::mir::dataflow::BitSet>,
}
impl RegionLiveness<'_> {
    pub fn is_live_at(&self, point: usize) -> bool { self.points.is_some_and(|row| row.contains(point)) }
}
impl DeferredLivenessSource<'_> {
    pub fn liveness_for_region(&mut self, region: usize) -> RegionLiveness<'_> {
        self.deferred.ensure_deferred_liveness(region, self.tcx, self.prog, self.body, self.universals,
            self.liveness, &mut self.variances, &mut self.comp);
        let direction = if self.universals.is_universal_region(region) { super::ConstraintDirection::Forward }
            else { self.variances.get(region).copied().flatten().unwrap_or(super::ConstraintDirection::Bidirectional) };
        RegionLiveness { direction, points: self.liveness.point_liveness(region) }
    }
}
