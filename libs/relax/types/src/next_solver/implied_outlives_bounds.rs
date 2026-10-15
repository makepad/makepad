// ported from N:rustc_trait_selection/src/traits/implied_outlives_bounds.rs @8d1a764
// ported from N:rustc_trait_selection/src/regions.rs @8d1a764
use super::*;
use super::fulfill::ObligationCtxt;
use crate::types::TyKind;
use crate::infer::outlives::{env::{OutlivesBound, OutlivesEnvironment}, components::{self, Component}};

// ported from N:rustc_trait_selection/src/traits/outlives_bounds.rs @8d1a764
// ported from N:rustc_traits/src/implied_outlives_bounds.rs @8d1a764

/// implied_outlives_bounds.rs:151-199. NOT PORTED: the bevy_ecs `ParamSet` compatibility hack
/// (consider_implied_bounds_hack_for_ty, #119956) -- a crate-name special case, left to the lead.
pub fn query_compute_implied_outlives_bounds(ocx: &mut ObligationCtxt<'_, '_>, env: &ParamEnvId, ty: TyId) -> Result<Vec<OutlivesBound>, NoSolution> {
    let normalized = ocx.deeply_normalize_ty(env, ty)?;
    compute_implied_outlives_bounds_inner(ocx, env, ty, normalized)
}

/// outlives_bounds.rs:31-97 (implied_outlives_bounds): the bounds a type that is assumed WF
/// implies. Nightly runs the `implied_outlives_bounds` canonical query
/// (rustc_traits/src/implied_outlives_bounds.rs, enter_canonical_trait_query) and keeps its
/// region constraints on success. REDESIGN (same behaviour): the query runs in a commit_if_ok
/// boundary of the caller's context instead of a canonical query -- the type has no non-region
/// inference variables (asserted, as nightly), so the only effects that can reach the caller are
/// region constraints, which commit exactly when the canonical response would be instantiated.
/// A failing query yields no bounds for this type only (:58-73); bounds with placeholders are
/// dropped (:76, #109628); the query's fast path for types without lifetimes
/// (implied_outlives_bounds.rs:22-34) answers `[]`.
pub fn implied_outlives_bounds(infcx: &crate::infer::InferCtxt<'_>, prog: &crate::program::Program, env: &ParamEnvId, ty: TyId) -> Vec<OutlivesBound> {
    let tys = &infcx.tcx.tys;
    let ty = infcx.deeply_resolve_ignoring_regions(ty);
    assert!(!tys.flags(ty).intersects(crate::types::TypeFlags::HAS_TY_INFER), "implied bounds of a type with inference variables");
    match tys.kind(ty) {
        TyKind::Tuple(elems) if elems.is_empty() => return Vec::new(),
        TyKind::Never | TyKind::Str | TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_) => return Vec::new(),
        _ => {}
    }
    let Ok(mut bounds) = infcx.commit_if_ok(|_| {
        let mut ocx = ObligationCtxt::new(infcx, prog);
        let bounds = query_compute_implied_outlives_bounds(&mut ocx, env, ty)?;
        ocx.evaluate_obligations_error_on_ambiguity()?;
        Ok::<_, NoSolution>(bounds)
    }) else { return Vec::new(); };
    let placeholder = |r: crate::types::RegionId| matches!(tys.region_kind(r), crate::types::RegionKind::RePlaceholder { .. });
    bounds.retain(|bound| match *bound {
        OutlivesBound::RegionSubRegion(a, b) => !placeholder(a) && !placeholder(b),
        OutlivesBound::RegionSubParam(a, t) | OutlivesBound::RegionSubAlias(a, t) => !placeholder(a) && !tys.has_placeholders(t),
    });
    bounds
}
pub fn compute_implied_outlives_bounds_inner(ocx: &mut ObligationCtxt<'_, '_>, env: &ParamEnvId, ty: TyId, normalized: TyId) -> Result<Vec<OutlivesBound>, NoSolution> {
    let mut checked = crate::fxhash::HashSet::default();
    let mut wf_args = vec![Term::Ty(ty), Term::Ty(normalized)]; let mut result = Vec::new();
    while let Some(arg) = wf_args.pop() {
        if !checked.insert(arg) { continue; }
        let Some(goals) = wf::unnormalized_obligations(ocx.infcx, ocx.prog, env, arg)? else { continue; };
        for goal in goals {
            let p = ocx.deeply_normalize_predicate(env, &goal.predicate)?;
            if p.has_bound_vars(&ocx.infcx.tcx.tys) { continue; }
            match p.kind {
                PredicateKind::Clause(ClauseKind::WellFormed(term)) => wf_args.push(term),
                PredicateKind::Clause(ClauseKind::RegionOutlives(a,b)) => result.push(OutlivesBound::RegionSubRegion(b,a)),
                PredicateKind::Clause(ClauseKind::TypeOutlives(ty,r)) => {
                    let mut parts = Vec::new(); components::push_outlives_components(ocx.infcx.tcx, ocx.prog, ty, &mut parts);
                    for part in parts { match part {
                        Component::Region(s) => result.push(OutlivesBound::RegionSubRegion(r,s)),
                        Component::Param(t) => result.push(OutlivesBound::RegionSubParam(r,t)),
                        Component::Alias(t) => result.push(OutlivesBound::RegionSubAlias(r,t)),
                        Component::Placeholder(_) | Component::EscapingAlias(_) | Component::UnresolvedInferenceVariable(_) => {},
                    } }
                }
                _ => {}
            }
        }
    }
    Ok(result)
}
impl OutlivesEnvironment {
    /// rustc_trait_selection/src/regions.rs (OutlivesEnvironment::new): infallible; each
    /// assumed-WF type contributes whatever bounds its query yields.
    pub fn new(infcx: &crate::infer::InferCtxt<'_>, prog: &crate::program::Program, param_env: ParamEnvId, assumed_wf_tys: &[TyId]) -> Self {
        let mut known = Vec::new();
        for p in &*infcx.tcx.param_env_by_id(param_env).clauses { if matches!(p.kind, PredicateKind::Clause(ClauseKind::TypeOutlives(..))) { known.push(p.clone()); } }
        let mut implied = Vec::new();
        for &ty in assumed_wf_tys { implied.extend(implied_outlives_bounds(infcx, prog, &param_env, ty)); }
        Self::from_normalized_bounds(&infcx.tcx.tys, infcx.tcx.param_env_by_id(param_env).clone(), known, implied)
    }
}
impl crate::infer::InferCtxt<'_> {
    pub fn resolve_regions(&self, prog: &crate::program::Program, env: ParamEnvId, assumed_wf_tys: &[TyId]) -> Vec<crate::infer::lexical_region_resolve::RegionResolutionError> {
        let outlives = OutlivesEnvironment::new(self, prog, env, assumed_wf_tys);
        self.resolve_regions_with_outlives_env(prog, &outlives)
    }
}
