// ported from N:rustc_borrowck/src/polonius/mod.rs @8d1a764
mod constraints;
mod liveness;
mod liveness_constraints;

pub use liveness::DeferredRegionLiveness;
pub use liveness_constraints::record_live_region_variance;
pub(crate) use liveness_constraints::{region_vid, regions_in, record_variance, record_region_variance};
use super::borrow_set::BorrowSet;
use super::mir_nll::LocationMap;
use super::region_infer::RegionInferenceContext;
use super::region_infer::values::LivenessValues;
use crate::mir::{Body, dataflow::BitSet, move_paths::MoveData};
use crate::program::Program;
use crate::tcx::Tcx;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstraintDirection { Forward, Backward, Bidirectional }
pub type LiveRegionVariances = Vec<Option<ConstraintDirection>>;

#[derive(Clone, Debug)]
pub struct LiveLoans { num_points: usize, flat_matrix: BitSet }
impl LiveLoans {
    pub fn new(num_points: usize, num_borrows: usize) -> Self {
        Self { num_points, flat_matrix: BitSet::new_empty(num_points * num_borrows) }
    }
    pub fn insert(&mut self, point: usize, loan: usize) { self.flat_matrix.insert(point + self.num_points * loan); }
    pub fn contains(&self, point: usize, loan: usize) -> bool { self.flat_matrix.contains(point + self.num_points * loan) }
}

/// MIR typeck supplies variance for live types and defers NLL-boring locals
/// whose types contain non-universal regions. No crate/extension mode exists.
#[derive(Clone, Default)]
pub struct PoloniusContext {
    pub live_region_variances: LiveRegionVariances,
    pub deferred_liveness: DeferredRegionLiveness,
    /// Locals whose types contain only regions that outlive universal ones: NLL needs no
    /// liveness for them. Kept for diagnostics (N:polonius/mod.rs boring_nll_locals).
    pub boring_nll_locals: Vec<crate::mir::Local>,
}
impl PoloniusContext {
    pub fn compute_loan_liveness(&self, tcx: &Tcx, prog: &Program, body: &Body, moves: &MoveData,
        locations: &LocationMap, borrows: &BorrowSet, regions: &RegionInferenceContext) -> LivenessValues {
        let mut liveness = regions.liveness_constraints().clone();
        let mut live_loans = LiveLoans::new(locations.num_points(), borrows.len());
        if !borrows.is_empty() {
            let graph = constraints::LocalizedConstraintGraph::new(regions.outlives_constraints());
            let universals = &regions.universal_region_relations.universal_regions;
            let mut source = liveness::DeferredLivenessSource {
                tcx, prog, body, universals, liveness: &mut liveness,
                deferred: self.deferred_liveness.clone(), variances: self.live_region_variances.clone(),
                comp: super::type_check::liveness::LivenessComputation::new(prog, tcx, body, locations, moves),
            };
            graph.traverse(body, borrows, locations, &mut live_loans, &mut source);
        }
        liveness.record_live_loans(live_loans);
        liveness
    }
}
