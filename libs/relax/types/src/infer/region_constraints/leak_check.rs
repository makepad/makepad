// ported from N:rustc_infer/src/infer/region_constraints/leak_check.rs @8d1a764
use super::*;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeakCheckError { pub placeholder: RegionId, pub other_region: RegionId }

struct MiniGraph {
    regions: Vec<RegionId>,
    sccs: Vec<u32>,
    successors: Vec<Vec<u32>>,
}

impl MiniGraph {
    fn new(constraints: &[Constraint]) -> Self {
        let mut nodes = HashMap::new();
        let mut regions = Vec::new();
        let mut edges = Vec::new();
        for constraint in constraints {
            if !constraint.visible_for_leak_check { continue; }
            let source = Self::add_node(&mut nodes, &mut regions, constraint.sup);
            let target = Self::add_node(&mut nodes, &mut regions, constraint.sub);
            edges.push((source, target));
            if matches!(constraint.kind, ConstraintKind::VarEqVar | ConstraintKind::VarEqReg | ConstraintKind::RegEqReg) {
                edges.push((target, source));
            }
        }
        let mut graph = vec![Vec::new(); regions.len()];
        for &(source, target) in &edges { graph[source as usize].push(target as usize); }
        let graph = crate::scc::Sccs::new(&graph);
        let sccs = (0..regions.len()).map(|i| graph.scc(i) as u32).collect();
        let successors = graph.all_sccs().map(|s| graph.successors(s).iter().map(|&i| i as u32).collect()).collect();
        Self { regions, sccs, successors }
    }

    fn add_node(nodes: &mut HashMap<RegionId, u32>, regions: &mut Vec<RegionId>, region: RegionId) -> u32 {
        if let Some(&node) = nodes.get(&region) { return node; }
        let node = regions.len() as u32;
        regions.push(region);
        nodes.insert(region, node);
        node
    }
}

#[derive(Clone, Copy)]
struct SccUniverse { universe: UniverseIndex, region: Option<RegionId> }

impl SccUniverse {
    fn take_min(&mut self, universe: UniverseIndex, region: RegionId) {
        if universe < self.universe || self.region.is_none() {
            self.universe = universe;
            self.region = Some(region);
        }
    }
}

struct LeakCheck<'a, 'b> {
    tys: &'a Types,
    outer_universe: UniverseIndex,
    mini_graph: MiniGraph,
    rcc: &'a mut RegionConstraintCollector<'b>,
    scc_placeholders: Vec<Option<RegionId>>,
    scc_universes: Vec<SccUniverse>,
}

impl LeakCheck<'_, '_> {
    fn assign_placeholder_values(&mut self) -> Result<(), LeakCheckError> {
        for (node, &region) in self.mini_graph.regions.iter().enumerate() {
            let scc = self.mini_graph.sccs[node] as usize;
            let universe = self.rcc.universe(self.tys, region);
            self.scc_universes[scc].take_min(universe, region);
            if let RegionKind::RePlaceholder { universe, .. } = self.tys.region_kind(region) {
                if !self.outer_universe.can_name(universe) {
                    if let Some(placeholder) = self.scc_placeholders[scc] {
                        return Err(LeakCheckError { placeholder, other_region: region });
                    }
                    self.scc_placeholders[scc] = Some(region);
                }
            }
        }
        Ok(())
    }

    fn propagate_scc_value(&mut self) -> Result<(), LeakCheckError> {
        for scc1 in 0..self.mini_graph.successors.len() {
            let mut universe = self.scc_universes[scc1];
            let mut succ_bound = None;
            for &scc2 in &self.mini_graph.successors[scc1] {
                let scc2 = scc2 as usize;
                debug_assert!(scc2 < scc1);
                let other = self.scc_universes[scc2];
                universe.take_min(other.universe, other.region.unwrap());
                if let Some(bound) = self.scc_placeholders[scc2] { succ_bound = Some(bound); }
            }
            self.scc_universes[scc1] = universe;
            if let Some(placeholder) = self.scc_placeholders[scc1] {
                let RegionKind::RePlaceholder { universe: placeholder_universe, .. } = self.tys.region_kind(placeholder) else { unreachable!() };
                if !universe.universe.can_name(placeholder_universe) {
                    return Err(LeakCheckError { placeholder, other_region: universe.region.unwrap() });
                }
                if let Some(other_region) = succ_bound {
                    return Err(LeakCheckError { placeholder, other_region });
                }
            } else {
                self.scc_placeholders[scc1] = succ_bound;
            }
        }
        Ok(())
    }
}

impl RegionConstraintCollector<'_> {
    pub fn leak_check(&mut self, tys: &Types, outer: UniverseIndex, max: UniverseIndex, snapshot: Option<&crate::infer::snapshot::CombinedSnapshot>) -> Result<(), LeakCheckError> {
        if outer == max { return Ok(()); }
        // leak_check.rs:405: verifys must not be added while testing a binder.
        assert!(!snapshot.is_some_and(|s| self.undo_log.verify_in_snapshot(&s.undo_snapshot)), "verify added during leak-check snapshot");
        let start = snapshot.map_or(0, |s| s.region_constraints_len);
        let mini_graph = MiniGraph::new(&self.storage.constraints[start..]);
        let count = mini_graph.successors.len();
        let mut check = LeakCheck {
            tys, outer_universe: outer, mini_graph, rcc: self,
            scc_placeholders: vec![None; count],
            scc_universes: vec![SccUniverse { universe: max, region: None }; count],
        };
        check.assign_placeholder_values()?;
        check.propagate_scc_value()
    }
}
