// ported from N:rustc_infer/src/infer/outlives/env.rs @8d1a764
use super::super::free_regions::FreeRegionMap;
use crate::next_solver::{ParamEnv, Predicate, PredicateKind, ClauseKind};
use crate::types::{RegionId, RegionKind, TyId, Types};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlivesBound { RegionSubRegion(RegionId, RegionId), RegionSubParam(RegionId, TyId), RegionSubAlias(RegionId, TyId) }
#[derive(Clone)]
pub struct OutlivesEnvironment {
    pub param_env: ParamEnv,
    pub free_region_map: FreeRegionMap,
    pub region_bound_pairs: Vec<(TyId, RegionId)>,
    pub known_type_outlives: Vec<Predicate>,
}
impl OutlivesEnvironment {
    pub fn from_normalized_bounds(tys: &Types, param_env: ParamEnv, known_type_outlives: Vec<Predicate>, mut extra_bounds: Vec<OutlivesBound>) -> Self {
        let mut bounds = Vec::new();
        for p in &*param_env.clauses {
            if let PredicateKind::Clause(ClauseKind::RegionOutlives(a, b)) = p.kind {
                if !p.has_bound_vars(tys) { bounds.push(OutlivesBound::RegionSubRegion(b, a)); }
            }
        }
        bounds.append(&mut extra_bounds);
        let mut edges = Vec::new(); let mut pairs = Vec::new();
        for bound in bounds {
            match bound {
                OutlivesBound::RegionSubParam(r, t) | OutlivesBound::RegionSubAlias(r, t) => { if !pairs.contains(&(t, r)) { pairs.push((t, r)); } }
                OutlivesBound::RegionSubRegion(a, b) => match (tys.region_kind(a), tys.region_kind(b)) {
                    (RegionKind::ReStatic | RegionKind::ReEarlyParam(_) | RegionKind::ReLateParam(..), RegionKind::ReStatic | RegionKind::ReEarlyParam(_) | RegionKind::ReLateParam(..)) => edges.push((a, b)),
                    (RegionKind::ReError | RegionKind::ReVar(_), _) | (_, RegionKind::ReError | RegionKind::ReVar(_)) => {},
                    (a, b) => panic!("unexpected regions in implied bounds: ({a:?}, {b:?})"),
                }
            }
        }
        Self { param_env, known_type_outlives, free_region_map: FreeRegionMap::new(&edges), region_bound_pairs: pairs }
    }
}
