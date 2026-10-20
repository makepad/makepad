// ported from N:rustc_borrowck/src/region_infer/values.rs @8d1a764
use crate::mir::dataflow::bitset::{BitMatrix, BitSet};
use crate::types::RegionId;

#[derive(Clone, Debug)]
pub struct LivenessValues { num_points: usize, rows: Vec<Option<BitSet>>, live_loans: Option<crate::borrowck::polonius::LiveLoans>, deferred: Vec<bool> }
impl LivenessValues {
    pub fn new(regions: usize, num_points: usize) -> Self { Self { num_points, rows: vec![None; regions], live_loans: None, deferred: Vec::new() } }
    pub fn record_live_loans(&mut self, loans: crate::borrowck::polonius::LiveLoans) { self.live_loans = Some(loans); }
    pub fn live_loans(&self) -> Option<&crate::borrowck::polonius::LiveLoans> { self.live_loans.as_ref() }
    /// Rows grow with the region allocator: MIR type check creates call-site,
    /// placeholder and existential regions after the initial renumbering.
    pub fn add_point(&mut self, region: usize, point: usize) {
        if region >= self.rows.len() { self.rows.resize(region + 1, None); }
        self.rows[region].get_or_insert_with(|| BitSet::new_empty(self.num_points)).insert(point);
    }
    pub fn add_all_points(&mut self, region: usize) {
        if region >= self.rows.len() { self.rows.resize(region + 1, None); }
        self.rows[region] = Some(BitSet::new_filled(self.num_points));
    }
    /// Regions live at one or more points (N:region_infer/values.rs live_regions_unordered).
    pub fn live_regions(&self) -> impl Iterator<Item = usize> + '_ {
        self.rows.iter().enumerate().filter(|(_, r)| r.as_ref().is_some_and(|r| r.iter().next().is_some())).map(|(i, _)| i)
    }
    /// The liveness of `region` was computed on demand, for loan propagation (a local whose regions
    /// outlive a universal region is boring: rustc's NLL computes no liveness for it at all).
    pub fn mark_deferred(&mut self, region: usize) {
        if region >= self.deferred.len() { self.deferred.resize(region + 1, false); }
        self.deferred[region] = true;
    }
    /// `is_live_at_point` as the liveness constraints stand before any on-demand computation: a region of a
    /// boring local is not live anywhere (diagnostics ask which regions the type check made live).
    pub fn is_live_at_point_eagerly(&self, region: usize, point: usize) -> bool {
        !self.deferred.get(region).copied().unwrap_or(false) && self.is_live_at_point(region, point)
    }
    pub fn point_liveness(&self, region: usize) -> Option<&BitSet> { self.rows.get(region)?.as_ref() }
    pub fn is_live_at_point(&self, region: usize, point: usize) -> bool {
        self.point_liveness(region).is_some_and(|r| r.contains(point))
    }
}
#[derive(Clone, Debug, Default)]
pub struct PlaceholderIndices { indices: Vec<RegionId> }
impl PlaceholderIndices {
    pub fn insert(&mut self, placeholder: RegionId) -> usize {
        if let Some(i) = self.indices.iter().position(|p| *p == placeholder) { return i; }
        let i = self.indices.len(); self.indices.push(placeholder); i
    }
    pub fn lookup_index(&self, p: RegionId) -> usize { self.indices.iter().position(|r| *r == p).unwrap() }
    pub fn lookup_placeholder(&self, i: usize) -> RegionId { self.indices[i] }
    pub fn len(&self) -> usize { self.indices.len() }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionElement { Location(usize), RootUniversalRegion(usize), PlaceholderRegion(RegionId) }
#[derive(Clone, Debug)]
pub struct RegionValues {
    points: Vec<Option<BitSet>>,
    free_regions: BitMatrix,
    free_region_indices: Vec<usize>,
    placeholders: BitMatrix,
    placeholder_indices: PlaceholderIndices,
    num_points: usize,
}
impl RegionValues {
    pub fn new(rows: usize, num_points: usize, free_region_indices: Vec<usize>, placeholder_indices: PlaceholderIndices) -> Self {
        Self { points: vec![None; rows], free_regions: BitMatrix::new(rows, free_region_indices.len()), free_region_indices,
            placeholders: BitMatrix::new(rows, placeholder_indices.len()), placeholder_indices, num_points }
    }
    pub fn add_free_region(&mut self, row: usize, region: usize) { self.free_regions.insert(row, self.free_region_indices.binary_search(&region).unwrap()); }
    pub fn add_placeholder(&mut self, row: usize, p: RegionId) {
        self.placeholders.insert(row, self.placeholder_indices.lookup_index(p));
    }
    pub fn merge_liveness(&mut self, row: usize, points: &BitSet) {
        self.points[row].get_or_insert_with(|| BitSet::new_empty(self.num_points)).union(points);
    }
    pub fn add_region(&mut self, row: usize, from: usize) {
        if row == from { return; }
        self.free_regions.union_rows(from, row);
        self.placeholders.union_rows(from, row);
        if self.points[from].is_none() { return; }
        if self.points[row].is_none() { self.points[row] = self.points[from].clone(); return; }
        let (dst, src) = if row < from {
            let (a,b) = self.points.split_at_mut(from); (&mut a[row], &b[0])
        } else { let (a,b) = self.points.split_at_mut(row); (&mut b[0], &a[from]) };
        dst.as_mut().unwrap().union(src.as_ref().unwrap());
    }
    pub fn contains_point(&self, row: usize, point: usize) -> bool { self.points[row].as_ref().is_some_and(|p| p.contains(point)) }
    pub fn contains_points(&self, sup: usize, sub: usize) -> bool {
        self.points[sub].as_ref().is_none_or(|p| p.iter().all(|p| self.contains_point(sup, p)))
    }
    pub fn contains_free_region(&self, row: usize, r: usize) -> bool { self.free_region_indices.binary_search(&r).is_ok_and(|i| self.free_regions.contains(row, i)) }
    pub fn universal_regions_outlived_by(&self, row: usize) -> impl Iterator<Item=usize> + '_ { self.free_regions.iter(row).map(|i| self.free_region_indices[i]) }
    pub fn elements_contained_in(&self, row: usize) -> impl Iterator<Item=RegionElement> + '_ {
        self.points[row].iter().flat_map(|p| p.iter()).map(RegionElement::Location)
            .chain(self.universal_regions_outlived_by(row).map(RegionElement::RootUniversalRegion))
            .chain(self.placeholders.iter(row).map(|i| RegionElement::PlaceholderRegion(self.placeholder_indices.lookup_placeholder(i))))
    }
}
