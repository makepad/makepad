// ported from N:rustc_borrowck/src/handle_placeholders.rs @8d1a764
use super::constraints::{ConstraintCategory, OutlivesConstraint, OutlivesConstraintSet, RegionVid};
use super::region_infer::{ConstraintSccs, NllRegionVariableOrigin, RegionDefinition, Representative};
use crate::types::UniverseIndex;

pub struct LoweredConstraints {
    pub constraint_sccs: ConstraintSccs,
    pub definitions: Vec<RegionDefinition>,
    pub scc_annotations: Vec<RegionTracker>,
    pub outlives_constraints: OutlivesConstraintSet,
}
pub struct SccAnnotations<'d> {
    pub scc_to_annotation: Vec<RegionTracker>,
    definitions: &'d [RegionDefinition],
}
impl<'d> SccAnnotations<'d> {
    pub fn init(definitions: &'d [RegionDefinition]) -> Self {
        Self { scc_to_annotation: vec![], definitions }
    }
    pub fn new(&self, element: RegionVid) -> RegionTracker { RegionTracker::new(element, &self.definitions[element]) }
    pub fn annotate_scc(&mut self, scc: usize, annotation: RegionTracker) {
        assert_eq!(scc, self.scc_to_annotation.len());
        self.scc_to_annotation.push(annotation);
    }
}
#[derive(Copy, Debug, Clone, PartialEq, Eq)]
struct PlaceholderReachability {
    max_universe: (UniverseIndex, RegionVid),
    min_placeholder: RegionVid,
    max_placeholder: RegionVid,
}

impl PlaceholderReachability {
    fn merge(&mut self, other: &Self) {
        self.max_universe = self.max_universe.max(other.max_universe);
        self.min_placeholder = self.min_placeholder.min(other.min_placeholder);
        self.max_placeholder = self.max_placeholder.max(other.max_placeholder);
    }
}
#[derive(Copy, Debug, Clone)]
pub struct RegionTracker {
    reachable_placeholders: Option<PlaceholderReachability>,
    max_nameable_universe: (UniverseIndex, RegionVid),
    pub representative: Representative,
}

impl RegionTracker {
    pub fn new(rvid: RegionVid, definition: &RegionDefinition) -> Self {
        let reachable_placeholders =
            if matches!(definition.origin, NllRegionVariableOrigin::Placeholder(_)) {
                Some(PlaceholderReachability {
                    max_universe: (definition.universe, rvid),
                    min_placeholder: rvid,
                    max_placeholder: rvid,
                })
            } else {
                None
            };

        Self {
            reachable_placeholders,
            max_nameable_universe: (definition.universe, rvid),
            representative: Representative::new(rvid, definition),
        }
    }
    pub fn max_nameable_universe(self) -> UniverseIndex {
        self.max_nameable_universe.0
    }

    pub fn max_placeholder_universe_reached(self) -> UniverseIndex {
        self.reachable_placeholders.map(|pls| pls.max_universe.0).unwrap_or(UniverseIndex::ROOT)
    }
    fn placeholders_can_be_named_by(&self, from: UniverseIndex) -> bool {
        self.reachable_placeholders.is_none_or(|pls| from.can_name(pls.max_universe.0))
    }
    pub fn can_name_all_placeholders(&self, other: Self) -> bool {
        self.max_nameable_universe().can_name(other.max_nameable_universe())
            || other.placeholders_can_be_named_by(self.max_nameable_universe.0)
    }
    fn unnameable_placeholder(&self) -> Option<(UniverseIndex, RegionVid)> {
        self.reachable_placeholders
            .filter(|pls| !self.max_nameable_universe().can_name(pls.max_universe.0))
            .map(|pls| pls.max_universe)
    }
}

impl RegionTracker {
    pub fn update_scc(&mut self, other: &Self) {

        self.representative = self.representative.min(other.representative);
        self.update_reachable(other);
    }

    pub fn update_reachable(&mut self, other: &Self) {
        self.max_nameable_universe = self.max_nameable_universe.min(other.max_nameable_universe);
        match (self.reachable_placeholders.as_mut(), other.reachable_placeholders.as_ref()) {
            (None, None) | (Some(_), None) => (),
            (None, Some(theirs)) => self.reachable_placeholders = Some(*theirs),
            (Some(ours), Some(theirs)) => ours.merge(theirs),
        };
    }
}


pub fn compute_sccs_applying_placeholder_outlives_constraints(
    definitions: Vec<RegionDefinition>, mut outlives_constraints: OutlivesConstraintSet, fr_static: RegionVid,
) -> LoweredConstraints {
    let compute = |constraints: &OutlivesConstraintSet| {
        let mut annotations = SccAnnotations::init(&definitions);
        let graph = constraints.graph(definitions.len()).region_graph(constraints, fr_static);
        let sccs = ConstraintSccs::new_with_annotation(&graph, &mut annotations);
        (sccs, annotations.scc_to_annotation)
    };
    let (mut constraint_sccs, mut scc_annotations) = compute(&outlives_constraints);
    if definitions.iter().any(|d| matches!(d.origin, NllRegionVariableOrigin::Placeholder(_)))
        && rewrite_placeholder_outlives(&constraint_sccs, &scc_annotations, fr_static, &mut outlives_constraints) {
        (constraint_sccs, scc_annotations) = compute(&outlives_constraints);
    }
    LoweredConstraints { constraint_sccs, definitions, scc_annotations, outlives_constraints }
}
pub fn rewrite_placeholder_outlives(sccs: &ConstraintSccs, annotations: &[RegionTracker],
    fr_static: RegionVid, constraints: &mut OutlivesConstraintSet) -> bool {
    let mut added_constraints = false;
    for scc in sccs.all_sccs() {
        if scc == sccs.scc(fr_static) { continue; }
        let annotation = annotations[scc];
        let Some((_, max_u_rvid)) = annotation.unnameable_placeholder() else { continue; };
        let blame_to = if annotation.representative.rvid() == max_u_rvid {
            annotation.max_nameable_universe.1
        } else { max_u_rvid };
        constraints.push(OutlivesConstraint {
            sup: annotation.representative.rvid(), sub: fr_static, location: None,
            span: Default::default(), category: ConstraintCategory::OutlivesUnnameablePlaceholder(blame_to), from_closure: false,
        });
        added_constraints = true;
    }
    added_constraints
}
