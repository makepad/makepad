// ported from N:rustc_borrowck/src/region_infer/reverse_sccs.rs @8d1a764
use super::ConstraintSccs;
use crate::borrowck::universal_regions::UniversalRegions;

pub struct ReverseSccGraph { graph: Vec<Vec<usize>>, scc_regions: Vec<std::ops::Range<usize>>, universal_regions: Vec<usize> }
impl ReverseSccGraph {
    pub fn compute(sccs: &ConstraintSccs, universal_regions: &UniversalRegions) -> Self {
        let graph = sccs.reverse();
        let mut pairs: Vec<_> = universal_regions.universal_regions_iter().map(|r| (sccs.scc(r), r)).collect();
        pairs.sort();
        let universal_regions = pairs.iter().map(|p| p.1).collect();
        let mut scc_regions = vec![0..0; graph.len()];
        let mut start = 0;
        for chunk in pairs.chunk_by(|a,b| a.0 == b.0) {
            scc_regions[chunk[0].0] = start..start + chunk.len(); start += chunk.len();
        }
        Self { graph, scc_regions, universal_regions }
    }
    pub fn upper_bounds(&self, scc0: usize) -> Vec<usize> {
        let mut stack = vec![scc0];
        let mut seen = vec![false; self.graph.len()];
        let mut regions = vec![];
        while let Some(s) = stack.pop() {
            if seen[s] { continue; } seen[s] = true;
            regions.extend_from_slice(&self.universal_regions[self.scc_regions[s].clone()]);
            stack.extend(self.graph[s].iter().copied());
        }
        regions
    }
}
