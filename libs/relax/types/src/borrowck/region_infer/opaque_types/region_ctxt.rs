// ported from N:rustc_borrowck/src/region_infer/opaque_types/region_ctxt.rs @8d1a764
use super::super::{ConstraintSccs, RegionDefinition, Representative, NllRegionVariableOrigin};
use super::super::values::{RegionValues, PlaceholderIndices};
use super::super::reverse_sccs::ReverseSccGraph;
use crate::borrowck::handle_placeholders::{compute_sccs_applying_placeholder_outlives_constraints, RegionTracker};
use crate::borrowck::constraints::OutlivesConstraintSet;
use crate::borrowck::type_check::free_region_relations::UniversalRegionRelations;

pub struct RegionCtxt<'a> {
    pub definitions: Vec<RegionDefinition>,
    pub universal_region_relations: &'a UniversalRegionRelations,
    pub constraint_sccs: ConstraintSccs,
    pub scc_annotations: Vec<RegionTracker>,
    pub rev_scc_graph: ReverseSccGraph,
    pub scc_values: RegionValues,
}
impl<'a> RegionCtxt<'a> {
    pub fn new(definitions: Vec<RegionDefinition>, relations: &'a UniversalRegionRelations, constraints: OutlivesConstraintSet) -> Self {
        let lowered = compute_sccs_applying_placeholder_outlives_constraints(definitions, constraints, relations.universal_regions.fr_static);
        let mut scc_values = RegionValues::new(lowered.constraint_sccs.num_sccs(), 0, lowered.definitions.iter().enumerate().filter(|(_,d)| d.origin == NllRegionVariableOrigin::FreeRegion).map(|(r,_)|r).collect(), PlaceholderIndices::default());
        for (r,d) in lowered.definitions.iter().enumerate() {
            if d.origin == NllRegionVariableOrigin::FreeRegion { scc_values.add_free_region(lowered.constraint_sccs.scc(r),r); }
        }
        let rev_scc_graph = ReverseSccGraph::compute(&lowered.constraint_sccs, &relations.universal_regions);
        Self { definitions: lowered.definitions, universal_region_relations: relations,
            constraint_sccs: lowered.constraint_sccs, scc_annotations: lowered.scc_annotations, rev_scc_graph, scc_values }
    }
    pub fn representative(&self, r: usize) -> Representative { self.scc_annotations[self.constraint_sccs.scc(r)].representative }
    pub fn eval_equal(&self, a: usize, b: usize) -> bool {
        let a = self.constraint_sccs.scc(a); let b = self.constraint_sccs.scc(b);
        if a == b { return true; }
        let outlives = |sup,sub| self.scc_values.universal_regions_outlived_by(sub).all(|r1| {
            self.scc_values.universal_regions_outlived_by(sup).any(|r2| self.universal_region_relations.outlives(r2,r1))
        });
        outlives(a,b) && outlives(b,a)
    }
}
