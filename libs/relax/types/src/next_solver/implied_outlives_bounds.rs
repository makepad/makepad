// ported from N:rustc_trait_selection/src/traits/implied_outlives_bounds.rs @8d1a764
// ported from N:rustc_trait_selection/src/regions.rs @8d1a764
use super::*;
use super::fulfill::ObligationCtxt;
use crate::types::TyKind;
use crate::infer::outlives::{env::{OutlivesBound, OutlivesEnvironment}, components::{self, Component}};

// ported from N:rustc_trait_selection/src/traits/outlives_bounds.rs @8d1a764
// ported from N:rustc_traits/src/implied_outlives_bounds.rs @8d1a764

/// implied_outlives_bounds.rs `compute_implied_outlives_bounds_inner` with `disable_implied_bounds_hack = false`
/// (every caller here: OutlivesEnvironment::new, and wfcheck, whose first, hack-disabled resolution only decides
/// whether it retries with the hack -- more assumed bounds never add region errors, so the retry's result is this one).
/// The query runs in the caller's context (see `implied_outlives_bounds`): "registered region obligations" are those
/// this computation registered, from `start` on.
pub fn query_compute_implied_outlives_bounds(ocx: &mut ObligationCtxt<'_, '_>, env: &ParamEnvId, ty: TyId) -> Result<Vec<OutlivesBound>, NoSolution> {
    let start = ocx.infcx.inner.borrow().region_obligations().len();
    let normalized = ocx.deeply_normalize_ty(env, ty)?;
    let mut bounds = compute_implied_outlives_bounds_inner(ocx, env, ty, normalized)?;
    // implied_outlives_bounds.rs:153-167: a type naming `bevy_ecs::ParamSet` (#109628) also assumes the outlives
    // obligations its normalization and well-formedness registered
    if crate::borrowck::implied_bounds::contains_bevy_param_set(ocx.infcx.tcx, ocx.prog, ty) {
        let registered: Vec<(TyId, crate::types::RegionId)> = ocx.infcx.inner.borrow().region_obligations()[start..].iter().map(|&(t, r, _)| (t, r)).collect();
        for (sup, sub) in registered {
            let mut parts = Vec::new(); components::push_outlives_components(ocx.infcx.tcx, ocx.prog, sup, &mut parts);
            push_component_bounds(&mut bounds, sub, parts);
        }
    }
    Ok(bounds)
}
/// implied_bounds_from_components
pub(crate) fn push_component_bounds(out: &mut Vec<OutlivesBound>, r: crate::types::RegionId, parts: Vec<Component>) {
    for part in parts { match part {
        Component::Region(s) => out.push(OutlivesBound::RegionSubRegion(r,s)),
        Component::Param(t) => out.push(OutlivesBound::RegionSubParam(r,t)),
        Component::Alias(t) => out.push(OutlivesBound::RegionSubAlias(r,t)),
        Component::Placeholder(_) | Component::EscapingAlias(_) | Component::UnresolvedInferenceVariable(_) => {},
    } }
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
                    push_component_bounds(&mut result, r, parts);
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
