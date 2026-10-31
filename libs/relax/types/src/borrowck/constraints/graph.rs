// ported from N:rustc_borrowck/src/constraints/graph.rs @8d1a764
use super::{OutlivesConstraint, OutlivesConstraintSet, RegionVid};

/// Concrete direction replaces the nightly direction type parameter.
#[derive(Clone, Debug)]
pub struct ConstraintGraph {
    first_constraints: Vec<Option<usize>>,
    next_constraints: Vec<Option<usize>>,
    reverse: bool,
}
impl ConstraintGraph {
    pub fn new(set: &OutlivesConstraintSet, num_region_vars: usize, reverse: bool) -> Self {
        let mut first_constraints = vec![None; num_region_vars];
        let mut next_constraints = vec![None; set.outlives.len()];
        for (idx, c) in set.outlives.iter().enumerate().rev() {
            let head = &mut first_constraints[if reverse { c.sub } else { c.sup }];
            next_constraints[idx] = *head;
            *head = Some(idx);
        }
        Self { first_constraints, next_constraints, reverse }
    }
    pub fn outgoing_edges_from_graph<'a>(&'a self, region: RegionVid, set: &'a OutlivesConstraintSet)
        -> impl Iterator<Item = &'a OutlivesConstraint> {
        let mut pointer = self.first_constraints[region];
        std::iter::from_fn(move || {
            let p = pointer?;
            pointer = self.next_constraints[p];
            Some(&set.outlives[p])
        })
    }
    pub fn region_graph(&self, set: &OutlivesConstraintSet, fr_static: RegionVid) -> Vec<Vec<usize>> {
        (0..self.first_constraints.len()).map(|r| {
            if r == fr_static && !self.reverse { (0..self.first_constraints.len()).collect() }
            else { self.outgoing_edges_from_graph(r, set).map(|c| if self.reverse { c.sup } else { c.sub }).collect() }
        }).collect()
    }
}
