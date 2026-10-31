// ported from N:rustc_infer/src/infer/outlives/test_type_match.rs @8d1a764
use crate::infer::relate::{self, TypeRelation, TypeError};
use crate::consts::{Const, GenericArg};
use crate::tcx::Tcx;
use crate::next_solver::{Predicate, PredicateKind, ClauseKind};
use crate::types::{BoundVar, RegionId, RegionKind, TyId, TyKind};
use crate::types::variance::V;
use std::collections::HashMap;

pub fn extract_verify_if_eq(tcx: &Tcx, prog: &crate::program::Program, predicate: &Predicate, test_ty: TyId) -> Option<RegionId> {
    let PredicateKind::Clause(ClauseKind::TypeOutlives(ty, bound)) = predicate.kind else { panic!("expected type outlives clause") };
    let mut matcher = MatchAgainstHigherRankedOutlives { tcx, prog, pattern_depth: 0, map: HashMap::new(), variance: V::Co };
    matcher.tys(ty, test_ty).ok()?;
    Some(match tcx.tys.region_kind(bound) {
        RegionKind::ReBound(0, br) => matcher.map.get(&br).copied().unwrap_or_else(|| tcx.tys.intern_region(RegionKind::ReStatic)),
        _ => bound,
    })
}
pub fn can_match_erased_ty(tcx: &Tcx, prog: &crate::program::Program, predicate: &Predicate, erased: TyId) -> bool {
    let PredicateKind::Clause(ClauseKind::TypeOutlives(ty, _)) = predicate.kind else { return false; };
    let ty = tcx.tys.erase_regions(ty);
    ty == erased || MatchAgainstHigherRankedOutlives { tcx, prog, pattern_depth: 0, map: HashMap::new(), variance: V::Co }.tys(ty, erased).is_ok()
}
struct MatchAgainstHigherRankedOutlives<'a> { tcx: &'a Tcx, prog: &'a crate::program::Program, pattern_depth: u32, map: HashMap<BoundVar, RegionId>, variance: V }
impl<'a> TypeRelation<'a> for MatchAgainstHigherRankedOutlives<'a> {
    fn tcx(&self) -> &'a Tcx { self.tcx }
    fn prog(&self) -> &crate::program::Program { self.prog }
    fn variance(&self) -> V { self.variance }
    fn set_variance(&mut self, variance: V) { self.variance = if variance == V::Bi { V::Bi } else { V::Co }; }
    fn tys(&mut self, pattern: TyId, value: TyId) -> Result<TyId, TypeError> {
        if self.variance == V::Bi { return Ok(pattern); }
        if matches!(self.tcx.tys.kind(pattern), TyKind::Error | TyKind::Bound(..)) { return Err(TypeError::Mismatch); }
        if pattern == value { return Ok(pattern); }
        if let (TyKind::Alias(_, a), TyKind::Alias(_, b)) = (self.tcx.tys.kind(pattern), self.tcx.tys.kind(value)) {
            if a.kind != b.kind || self.tcx.tys.list(a.args).len() != self.tcx.tys.list(b.args).len() { return Err(TypeError::Mismatch); }
            let tcx = self.tcx;
            for (&x, &y) in tcx.tys.list(a.args).iter().zip(tcx.tys.list(b.args)) { self.tys(x, y)?; }
            let ars = self.tcx.tys.region_args(a.regions); let brs = self.tcx.tys.region_args(b.regions);
            if ars.len() != brs.len() { return Err(TypeError::Mismatch); }
            for i in 0..ars.len() { if !super::components::alias_region_is_bivariant(self.tcx, self.prog, a, i) { self.regions(ars[i], brs[i])?; } }
            return Ok(pattern);
        }
        relate::structurally_relate_tys(self, pattern, value)
    }
    fn regions(&mut self, pattern: RegionId, value: RegionId) -> Result<RegionId, TypeError> {
        if self.variance == V::Bi { return Ok(pattern); }
        if let RegionKind::ReBound(depth, var) = self.tcx.tys.region_kind(pattern) {
            if depth == self.pattern_depth {
                match self.map.get(&var) { Some(&r) if r != value => return Err(TypeError::Mismatch), Some(_) => {}, None => { self.map.insert(var, value); } }
                return Ok(value);
            }
        }
        if pattern == value { Ok(pattern) } else { Err(TypeError::Mismatch) }
    }
    fn consts(&mut self, pattern: Const, value: Const) -> Result<Const, TypeError> { relate::structurally_relate_consts(self, pattern, value) }
    fn binders(&mut self, pattern: TyId, value: TyId) -> Result<TyId, TypeError> {
        self.pattern_depth += 1; let r = relate::relate_fn_sig(self, pattern, value); self.pattern_depth -= 1; r
    }
    fn binders_args(&mut self, pattern: &[GenericArg], value: &[GenericArg]) -> Result<Vec<GenericArg>, TypeError> {
        self.pattern_depth += 1; let r = relate::relate_args(self, pattern, value); self.pattern_depth -= 1; r
    }
}
