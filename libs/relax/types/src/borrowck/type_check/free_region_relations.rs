// ported from N:rustc_borrowck/src/type_check/free_region_relations.rs @8d1a764
use crate::borrowck::constraints::RegionVid;
use crate::borrowck::universal_regions::UniversalRegions;
use crate::mir::dataflow::bitset::BitMatrix;

#[derive(Clone, Debug)]
pub struct UniversalRegionRelations {
    pub universal_regions: UniversalRegions,
    indices: Vec<RegionVid>,
    index: Vec<Option<usize>>,
    outlives: BitMatrix,
    inverse_outlives: BitMatrix,
    outlives_order: Vec<usize>,
    inverse_order: Vec<usize>,
}
impl UniversalRegionRelations {
    /// All of `known` is explicit (inserted before the universal regions).
    pub fn create(universal_regions: UniversalRegions, num_regions: usize, known: &[(RegionVid, RegionVid)]) -> Self {
        Self::create_with(universal_regions, num_regions, known, known.len())
    }
    /// `known[..explicit]` are the param-env's outlives clauses; the rest are implied bounds.
    pub fn create_with(universal_regions: UniversalRegions, num_regions: usize, known: &[(RegionVid, RegionVid)], explicit: usize) -> Self {
        let indices: Vec<_> = universal_regions.universal_regions_iter().collect();
        let mut index = vec![None; num_regions];
        for (i,&r) in indices.iter().enumerate() { index[r] = Some(i); }
        let at = |r: RegionVid| index[r].expect("relation on non-universal region");
        let n = indices.len();
        // Insertion order is the elements' index order (`minimal_scc_representative` picks the lowest),
        // as rustc's build: the param-env's outlives clauses, then every universal region (reflexive,
        // outlived by 'static, outliving the fn body), then the implied bounds.
        let mut edges = vec![];
        let known_edge = |edges: &mut Vec<(usize, usize)>, &(a, b): &(RegionVid, RegionVid)| if let (Some(a), Some(b)) = (index[a], index[b]) { edges.push((a, b)); };
        for k in &known[..explicit.min(known.len())] { known_edge(&mut edges, k); }
        for &fr in &indices {
            edges.push((at(fr),at(fr)));
            edges.push((at(universal_regions.fr_static),at(fr)));
            if let Some(body) = universal_regions.fr_fn_body { edges.push((at(fr),at(body))); }
        }
        for k in &known[explicit.min(known.len())..] { known_edge(&mut edges, k); }
        let mut outlives_order = vec![];
        let mut inverse_order = vec![];
        for &(a,b) in &edges {
            for i in [a,b] { if !outlives_order.contains(&i) { outlives_order.push(i); } }
            for i in [b,a] { if !inverse_order.contains(&i) { inverse_order.push(i); } }
        }
        // ported from N:rustc_data_structures/src/transitive_relation.rs @8d1a764
        let mut outlives = BitMatrix::new(n,n);
        let mut changed = true;
        while changed {
            changed = false;
            for &(a,b) in &edges {
                changed |= outlives.insert(a,b);
                changed |= outlives.union_rows(b,a);
            }
        }
        let mut inverse_outlives = BitMatrix::new(n,n);
        for i in 0..n { for j in outlives.iter(i) { inverse_outlives.insert(j,i); } }
        Self { universal_regions, indices, index, outlives, inverse_outlives, outlives_order, inverse_order }
    }
    /// Variables created after the relations were built (existentials) are not universal.
    fn idx(&self, r: RegionVid) -> Option<usize> { self.index.get(r).copied().flatten() }
    pub fn outlives(&self, a: RegionVid, b: RegionVid) -> bool {
        match (self.idx(a),self.idx(b)) { (Some(a),Some(b)) => self.outlives.contains(a,b), _ => false }
    }
    pub fn equal(&self, a: RegionVid, b: RegionVid) -> bool { self.outlives(a,b) && self.outlives(b,a) }
    pub fn regions_outlived_by(&self, r: RegionVid) -> impl Iterator<Item=RegionVid> + '_ {
        self.idx(r).into_iter().flat_map(move |i| self.outlives_order.iter().copied().filter(move |&j| self.outlives.contains(i,j))).map(|i| self.indices[i])
    }
    pub fn non_local_upper_bounds(&self, r: RegionVid) -> Vec<RegionVid> { self.non_local_bounds(&self.inverse_outlives, &self.inverse_order, r) }
    pub fn non_local_lower_bounds(&self, r: RegionVid) -> Vec<RegionVid> { self.non_local_bounds(&self.outlives, &self.outlives_order, r) }
    fn non_local_bounds(&self, relation: &BitMatrix, order: &[usize], r: RegionVid) -> Vec<RegionVid> {
        let representative = |r| order.iter().copied().find(|&s| relation.contains(r,s) && relation.contains(s,r)).unwrap_or(r);
        let mut queue = vec![representative(self.idx(r).unwrap())];
        let mut external_parents = vec![];
        while let Some(fr) = queue.pop() {
            if !self.universal_regions.is_local_free_region(self.indices[fr]) { external_parents.push(self.indices[fr]); continue; }
            // Parents of the SCC in the transitive relation's reduction.
            let candidates: Vec<_> = order.iter().copied().filter(|s| relation.contains(fr,*s) && !relation.contains(*s,fr) && representative(*s) == *s).collect();
            for &p in &candidates {
                if !candidates.iter().any(|&q| q != p && relation.contains(q,p)) { queue.push(p); }
            }
        }
        external_parents
    }
}
