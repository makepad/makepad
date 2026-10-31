// ported from N:rustc_borrowck/src/polonius/constraints.rs @8d1a764
use super::{ConstraintDirection, LiveLoans};
use crate::borrowck::{borrow_set::BorrowSet, constraints::OutlivesConstraintSet, mir_nll::LocationMap};
use crate::fxhash::{HashMap, HashSet};
use crate::mir::Body;
use super::liveness::DeferredLivenessSource;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct LocalizedNode { region: usize, point: usize }
pub(super) struct LocalizedConstraintGraph {
    edges: HashMap<LocalizedNode, Vec<usize>>,
    logical_edges: HashMap<usize, Vec<usize>>,
}
impl LocalizedConstraintGraph {
    pub fn new(outlives: &OutlivesConstraintSet) -> Self {
        let mut this = Self { edges: HashMap::default(), logical_edges: HashMap::default() };
        for c in &outlives.outlives {
            let succs = match c.location {
                Some(point) => this.edges.entry(LocalizedNode { region: c.sup, point }).or_default(),
                None => this.logical_edges.entry(c.sup).or_default(),
            };
            if !succs.contains(&c.sub) { succs.push(c.sub); }
        }
        this
    }
    pub fn traverse(&self, body: &Body, borrows: &BorrowSet, locations: &LocationMap,
        live_loans: &mut LiveLoans, liveness: &mut DeferredLivenessSource<'_>) {
        let predecessors = locations.predecessors(body);
        let mut visited = HashSet::default();
        let mut stack = Vec::new();
        for (loan, data) in borrows.iter_enumerated() {
            visited.clear(); stack.clear();
            let start = LocalizedNode { region: data.region.0 as usize, point: locations.point(data.reserve_location) };
            visited.insert(start); stack.push(start);
            while let Some(node) = stack.pop() {
                let live = liveness.liveness_for_region(node.region);
                let direction = live.direction;
                let is_live = |point| live.is_live_at(point);
                if is_live(node.point) { live_loans.insert(node.point, loan); }
                let mut successor = |next| { if visited.insert(next) { stack.push(next); } };
                for &region in self.edges.get(&node).into_iter().flatten() {
                    successor(LocalizedNode { region, point: node.point });
                }
                if matches!(direction, ConstraintDirection::Forward | ConstraintDirection::Bidirectional) {
                    for point in locations.successors(body, node.point) {
                        if is_live(point) { successor(LocalizedNode { region: node.region, point }); }
                    }
                }
                // Backward edges require liveness at the current (destination)
                // point, not at the predecessor. This permits time travel.
                if matches!(direction, ConstraintDirection::Backward | ConstraintDirection::Bidirectional) && is_live(node.point) {
                    for &point in &predecessors[node.point] { successor(LocalizedNode { region: node.region, point }); }
                }
                for &region in self.logical_edges.get(&node.region).into_iter().flatten() {
                    successor(LocalizedNode { region, point: node.point });
                }
            }
        }
    }
}
