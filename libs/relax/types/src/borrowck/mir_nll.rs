// ported from N:rustc_borrowck/src/nll.rs @8d1a764 (compute_regions)
use super::constraints::OutlivesConstraintSet;
use super::dataflow::RegionScope;
use super::handle_placeholders::compute_sccs_applying_placeholder_outlives_constraints;
use super::region_infer::{RegionDefinition, RegionInferenceContext, TypeTest};
use super::region_infer::values::LivenessValues;
use super::type_check::free_region_relations::UniversalRegionRelations;
use crate::mir::{Body, Location, Region};
use crate::program::Program;
use crate::tcx::Tcx;

/// Dense MIR location numbering, including every terminator. Shared with C-NLL5.
#[derive(Clone)]
pub struct LocationMap { offsets: Vec<usize> }
impl LocationMap {
    pub fn new(body: &Body) -> Self {
        let mut offsets = vec![0];
        for block in &body.basic_blocks { offsets.push(offsets.last().unwrap() + block.statements.len() + 1); }
        Self { offsets }
    }
    pub fn num_points(&self) -> usize { *self.offsets.last().unwrap() }
    pub fn point(&self, at: Location) -> usize {
        let b = at.block.0 as usize;
        let point = self.offsets[b] + at.statement_index;
        assert!(point < self.offsets[b + 1]);
        point
    }
    pub fn location(&self, point: usize) -> Location {
        assert!(point < self.num_points());
        let block = self.offsets.partition_point(|&offset| offset <= point) - 1;
        Location { block: crate::mir::BasicBlock(block as u32), statement_index: point - self.offsets[block] }
    }
    pub fn successors(&self, body: &Body, point: usize) -> Vec<usize> {
        use crate::mir::visit::Visitor;
        let at = self.location(point);
        let block = &body.basic_blocks[at.block.0 as usize];
        if at.statement_index < block.statements.len() { return vec![point + 1]; }
        struct Targets<'a> { map: &'a LocationMap, points: Vec<usize> }
        impl Visitor for Targets<'_> {
            fn visit_target(&mut self, block: &crate::mir::BasicBlock, _: bool, _: Location) {
                self.points.push(self.map.point(Location { block: *block, statement_index: 0 }));
            }
        }
        let mut targets = Targets { map: self, points: vec![] };
        targets.visit_terminator(block.terminator.as_ref().unwrap(), at);
        targets.points
    }
    pub fn predecessors(&self, body: &Body) -> Vec<Vec<usize>> {
        let mut result = vec![vec![]; self.num_points()];
        for p in 0..self.num_points() { for q in self.successors(body, p) { result[q].push(p); } }
        result
    }
    pub fn matches(&self, body: &Body) -> bool { self.offsets == Self::new(body).offsets }
}
/// C-NLL5 supplies renumbered type-check/liveness output; no BIR producer is used.
pub struct RegionInput {
    pub polonius: super::polonius::PoloniusContext,
    /// Prior type-check/normalization failure taints the root even with no region errors.
    pub tainted_by_errors: bool,
    pub definitions: Vec<RegionDefinition>,
    pub outlives: OutlivesConstraintSet,
    pub relations: UniversalRegionRelations,
    pub liveness: LivenessValues,
    pub type_tests: Vec<TypeTest>,
}
pub struct MirRegionResult {
    pub polonius: super::polonius::PoloniusContext,
    pub nested: bool,
    pub tainted_by_errors: bool,
    pub regioncx: RegionInferenceContext,
    pub locations: LocationMap,
    pub type_tests: Vec<TypeTest>,
}
impl RegionScope for MirRegionResult {
    fn is_loan_live_at(&self, loan: usize, at: Location) -> Option<bool> {
        self.regioncx.liveness_constraints().live_loans().map(|loans| loans.contains(self.locations.point(at), loan))
    }
    fn contains_point(&self, region: Region, at: Location) -> bool {
        self.regioncx.region_contains_point(region.0 as usize, self.locations.point(at))
    }
}
pub fn compute_regions(tcx: &Tcx, prog: &Program, body: &Body, input: RegionInput, nested: bool) -> MirRegionResult {
    let locations = LocationMap::new(body);
    let lowered = compute_sccs_applying_placeholder_outlives_constraints(
        input.definitions, input.outlives, input.relations.universal_regions.fr_static);
    let regioncx = RegionInferenceContext::solve(tcx, prog, lowered, input.relations, input.liveness,
        locations.num_points(), &input.type_tests, nested);
    MirRegionResult { polonius: input.polonius, nested, tainted_by_errors: input.tainted_by_errors, regioncx, locations, type_tests: input.type_tests }
}
