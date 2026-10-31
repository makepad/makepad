// ported from N:rustc_infer/src/infer/free_regions.rs @8d1a764
// ported from N:rustc_data_structures/src/transitive_relation.rs @8d1a764
use crate::types::{RegionId, RegionKind, Types};

// Kept audit design: rustc_infer/src/infer/free_regions.rs; dense transitive
// closure preserves the same upper-bound and postdominator ordering.
#[derive(Clone, Debug, Default)]
pub struct FreeRegionMap { elements: Vec<RegionId>, closure: Vec<Vec<bool>> }
impl FreeRegionMap {
    pub fn new(edges: &[(RegionId, RegionId)]) -> Self {
        let mut elements = Vec::new();
        for &(a, b) in edges { for r in [a, b] { if !elements.contains(&r) { elements.push(r); } } }
        let n = elements.len();
        let mut closure = vec![vec![false; n]; n];
        for &(a, b) in edges { closure[elements.iter().position(|&r| r == a).unwrap()][elements.iter().position(|&r| r == b).unwrap()] = true; }
        loop {
            let mut changed = false;
            for i in 0..n { for j in 0..n { if closure[i][j] {
                for k in 0..n { if closure[j][k] && !closure[i][k] { closure[i][k] = true; changed = true; } }
            } } }
            if !changed { break; }
        }
        Self { elements, closure }
    }
    fn contains(&self, a: RegionId, b: RegionId) -> bool {
        match (self.elements.iter().position(|&r| r == a), self.elements.iter().position(|&r| r == b)) {
            (Some(a), Some(b)) => self.closure[a][b], _ => false,
        }
    }
    pub fn sub_free_regions(&self, tys: &Types, a: RegionId, b: RegionId) -> bool {
        a == b || b == tys.intern_region(RegionKind::ReStatic) || self.contains(tys.intern_region(RegionKind::ReStatic), b) || self.contains(a, b)
    }
    fn minimal_upper_bounds(&self, a: RegionId, b: RegionId) -> Vec<RegionId> {
        let (Some(mut a), Some(mut b)) = (self.elements.iter().position(|&r| r == a), self.elements.iter().position(|&r| r == b)) else { return vec![]; };
        if a > b { std::mem::swap(&mut a, &mut b); }
        if self.closure[a][b] { return vec![self.elements[b]]; }
        if self.closure[b][a] { return vec![self.elements[a]]; }
        let mut candidates = Vec::new();
        for i in 0..self.elements.len() { if self.closure[a][i] && self.closure[b][i] { candidates.push(i); } }
        for _ in 0..2 {
            let mut i = 0;
            while i < candidates.len() {
                let mut j = i + 1;
                while j < candidates.len() {
                    if self.closure[candidates[i]][candidates[j]] { candidates.remove(j); } else { j += 1; }
                }
                i += 1;
            }
            candidates.reverse();
        }
        let mut result = Vec::new();
        for i in candidates { result.push(self.elements[i]); }
        result
    }
    pub fn lub_param_regions(&self, tys: &Types, a: RegionId, b: RegionId) -> RegionId {
        if a == b { return a; }
        let mut mubs = self.minimal_upper_bounds(a, b);
        loop {
            match mubs.len() {
                0 => return tys.intern_region(RegionKind::ReStatic), 1 => return mubs[0],
                _ => { let m = mubs.pop().unwrap(); let n = mubs.pop().unwrap(); mubs.extend(self.minimal_upper_bounds(n, m)); }
            }
        }
    }
}

pub(crate) struct RegionRelations<'a> { pub tcx: &'a crate::tcx::Tcx, pub prog: &'a crate::program::Program, pub tys: &'a Types, pub free_regions: &'a FreeRegionMap }
impl RegionRelations<'_> {
    pub fn lub_concrete_regions(&self, a: RegionId, b: RegionId) -> RegionId {
        use RegionKind::*;
        let tys = self.tys;
        match (self.tys.region_kind(a), self.tys.region_kind(b)) {
            (ReBound(..) | ReErased | ReVar(_), _) | (_, ReBound(..) | ReErased | ReVar(_)) => panic!("non-concrete region in LUB"),
            (ReError, _) => a,
            (_, ReError) => b,
            (ReStatic, _) | (_, ReStatic) => tys.intern_region(RegionKind::ReStatic),
            (ReEarlyParam(_) | ReLateParam(..), ReEarlyParam(_) | ReLateParam(..)) => self.free_regions.lub_param_regions(tys, a, b),
            (RePlaceholder { .. }, _) | (_, RePlaceholder { .. }) => if a == b { a } else { tys.intern_region(RegionKind::ReStatic) },
        }
    }
    pub fn sub_concrete_regions(&self, a: RegionId, b: RegionId) -> bool {
        if matches!(self.tys.region_kind(b), RegionKind::ReStatic | RegionKind::ReEarlyParam(_) | RegionKind::ReLateParam(..)) && self.free_regions.sub_free_regions(self.tys, self.tys.intern_region(RegionKind::ReStatic), b) { return true; }
        if matches!(self.tys.region_kind(a), RegionKind::ReStatic | RegionKind::ReEarlyParam(_) | RegionKind::ReLateParam(..)) &&
            matches!(self.tys.region_kind(b), RegionKind::ReStatic | RegionKind::ReEarlyParam(_) | RegionKind::ReLateParam(..)) {
            self.free_regions.sub_free_regions(self.tys, a, b)
        } else { self.lub_concrete_regions(a, b) == b }
    }
}
