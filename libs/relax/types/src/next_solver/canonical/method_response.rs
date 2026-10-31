// ported from N:rustc_infer/src/infer/canonical/query_response.rs @8d1a764
// ported from N:rustc_hir_typeck/src/method/probe.rs @8d1a764
use super::*;
use crate::infer::opaque_types::{OpaqueTypeStorageEntries, ProvisionalHiddenType};

#[derive(Clone)]
pub struct MethodAutoderefSteps { pub param_env: ParamEnvId, pub self_ty: TyId, pub predefined_opaques_in_body: Vec<(OpaqueTypeKey, TyId)> }
#[derive(Clone)]
pub struct QueryResponse { pub response: Response, pub value: TyId }
fn fold_goal(tcx: &crate::tcx::Tcx, goal: &MethodAutoderefSteps, f: &mut dyn TypeFolder) -> MethodAutoderefSteps {
    let tys = &tcx.tys;
    let param_env = crate::next_solver::fold::fold_param_env(tcx, goal.param_env, f);
    let self_ty = f.fold_ty(tys, goal.self_ty, 0);
    let mut predefined_opaques_in_body = Vec::new();
    for (key, ty) in &goal.predefined_opaques_in_body {
        let mut args = Vec::new();
        for &arg in &*key.args { args.push(crate::next_solver::fold::fold_arg(tys, arg, f, 0)); }
        predefined_opaques_in_body.push((OpaqueTypeKey { def_id: key.def_id, args: args.into() }, f.fold_ty(tys, *ty, 0)));
    }
    MethodAutoderefSteps { param_env, self_ty, predefined_opaques_in_body }
}
fn fold_response(tcx: &crate::tcx::Tcx, r: &QueryResponse, f: &mut dyn TypeFolder) -> QueryResponse {
    QueryResponse { response: crate::next_solver::fold::fold_response(tcx, &r.response, f), value: f.fold_ty(&tcx.tys, r.value, 0) }
}
pub fn canonicalize_method_autoderef_goal(infcx: &InferCtxt<'_>, param_env: ParamEnvId, self_ty: TyId) -> (Canonical<MethodAutoderefSteps>, Vec<GenericArg>) {
    let opaques = infcx.inner.borrow_mut().opaque_types().iter_lookup_table().to_vec();
    let goal = MethodAutoderefSteps { param_env, self_ty, predefined_opaques_in_body: opaques.into_iter().map(|(k,v)| (k,v.ty)).collect() };
    // rustc_infer canonicalizer.rs:38-80,251-286,424: method queries keep
    // parameters rigid and canonicalize free regions as existential variables.
    // The next-solver goal canonicalizer instead turns parameters into placeholders.
    let mut f = Canonicalizer::new(infcx, CanonicalizeMode::Input { in_param_env: true });
    f.infer_query = true;
    let mut clauses = Vec::new();
    for p in &*infcx.tcx.param_env_by_id(goal.param_env).clauses { clauses.push(fold_predicate(&infcx.tcx.tys, p, &mut f)); }
    f.mode = CanonicalizeMode::Input { in_param_env: false }; f.state.cache.clear();
    let mut goal = goal; goal.param_env = ParamEnvId::EMPTY;
    let mut value = fold_goal(infcx.tcx, &goal, &mut f);
    value.param_env = infcx.tcx.intern_param_env(ParamEnv { clauses: clauses.into() });
    let (max_universe, values, var_kinds) = f.finalize();
    (Canonical { value, max_universe, var_kinds }, values)
}
pub fn instantiate_method_autoderef_goal(infcx: &InferCtxt<'_>, goal: &Canonical<MethodAutoderefSteps>) -> (MethodAutoderefSteps, Vec<GenericArg>) {
    let values = instantiate_canonical_vars(infcx, goal.max_universe, &goal.var_kinds);
    (fold_goal(infcx.tcx, &goal.value, &mut CanonicalInstantiator { values: &values }), values)
}
/// query_response.rs:83-121 deliberately ignores region constraints and pending
/// goals, but retains new opaque entries so unconstrained opaques stay identifiable.
pub fn make_query_response_ignoring_pending_obligations(infcx: &InferCtxt<'_>, values: &[GenericArg], ty: TyId, previous: OpaqueTypeStorageEntries) -> Canonical<QueryResponse> {
    let mut external_constraints = ExternalConstraintsData::default();
    for (key, hidden) in infcx.inner.borrow_mut().opaque_types().opaque_types_added_since(previous) { external_constraints.opaque_types.push((key, hidden.ty)); }
    let response = QueryResponse { response: Response { var_values: values.to_vec(), certainty: Certainty::Yes, external_constraints }, value: ty };
    let response = fold_response(infcx.tcx, &response, &mut DeepVariableResolver { infcx });
    let mut f = Canonicalizer::new(infcx, CanonicalizeMode::Response { max_input_universe: UniverseIndex::ROOT });
    f.infer_query = true;
    let value = fold_response(infcx.tcx, &response, &mut f);
    let (max_universe, _, var_kinds) = f.finalize();
    Canonical { value, max_universe, var_kinds }
}
/// probe.rs:895-907: generalize without relating the query's input assignments.
pub fn generalize_response(infcx: &InferCtxt<'_>, response: &Canonical<QueryResponse>) -> TyId {
    let values = instantiate_canonical_vars(infcx, response.max_universe, &response.var_kinds);
    CanonicalInstantiator { values: &values }.fold_ty(&infcx.tcx.tys, response.value.value, 0)
}
pub fn probe_instantiate_query_response(infcx: &InferCtxt<'_>, prog: &crate::program::Program, env: &ParamEnvId, original: &[GenericArg], response: &Canonical<QueryResponse>) -> Result<(TyId, Vec<Goal>), NoSolution> {
    let wrapper = Canonical { value: response.value.response.clone(), max_universe: response.max_universe, var_kinds: response.var_kinds.clone() };
    let values = response::compute_query_response_instantiation_values_in_universe(infcx, original, &wrapper, infcx.universe());
    let response = fold_response(infcx.tcx, &response.value, &mut CanonicalInstantiator { values: &values });
    let mut relation = crate::infer::relate::solver_relating::SolverRelating::new(infcx, prog, crate::types::variance::V::Inv, env.clone());
    crate::infer::relate::relate_args(&mut relation, original, &response.response.var_values).map_err(|_| NoSolution)?;
    let mut goals = relation.goals;
    for (key, ty) in response.response.external_constraints.opaque_types {
        let previous = infcx.inner.borrow_mut().opaque_types().register(key, ProvisionalHiddenType::new(ty));
        if let Some(previous) = previous { goals.extend(infcx.at(prog, env.clone()).eq(previous, ty).map_err(|_| NoSolution)?); }
    }
    Ok((response.value, goals))
}
