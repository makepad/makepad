// ported from N:rustc_type_ir/src/fold.rs @8d1a764
// ported from N:rustc_type_ir/src/predicate_kind.rs @8d1a764
use super::*;
use crate::types::{Types, DebruijnIndex};
use crate::types::fold::TypeFolder;

pub fn fold_arg(tys: &Types, arg: GenericArg, f: &mut dyn TypeFolder, depth: DebruijnIndex) -> GenericArg {
    match arg {
        GenericArg::Type(ty) => GenericArg::Type(f.fold_ty(tys, ty, depth)),
        GenericArg::Lifetime(region) => GenericArg::Lifetime(f.fold_region(tys, region, depth)),
        GenericArg::Const(ct) => GenericArg::Const(f.fold_const(tys, ct, depth)),
    }
}

pub fn fold_term(tys: &Types, term: Term, f: &mut dyn TypeFolder, depth: u32) -> Term {
    match term {
        Term::Ty(ty) => Term::Ty(f.fold_ty(tys, ty, depth)),
        Term::Const(ct) => Term::Const(f.fold_const(tys, ct, depth)),
    }
}

fn fold_alias(tys: &Types, alias: &AliasTerm, f: &mut dyn TypeFolder, depth: u32) -> AliasTerm {
    match alias {
        AliasTerm::Ty(ty) => AliasTerm::Ty(crate::types::fold::super_fold_ty(tys, *ty, f, depth)),
        AliasTerm::Const(alias) => {
            let mut args = Vec::with_capacity(alias.args.len());
            for &arg in &*alias.args { args.push(fold_arg(tys, arg, f, depth)); }
            AliasTerm::Const(AliasConst { kind: alias.kind, args: args.into() })
        }
    }
}

pub fn fold_predicate_kind(tys: &Types, predicate: &PredicateKind, f: &mut dyn TypeFolder, depth: u32) -> PredicateKind {
    match predicate {
        PredicateKind::Clause(clause) => PredicateKind::Clause(match clause {
            ClauseKind::Trait(clause) => {
                let mut args = Vec::with_capacity(clause.trait_ref.args.len());
                for &arg in &clause.trait_ref.args { args.push(fold_arg(tys, arg, f, depth)); }
                ClauseKind::Trait(TraitClause { trait_ref: TraitRef { def_id: clause.trait_ref.def_id, args }, polarity: clause.polarity })
            }
            ClauseKind::Projection(p) => ClauseKind::Projection(ProjectionClause { projection_term: fold_alias(tys, &p.projection_term, f, depth), term: fold_term(tys, p.term, f, depth) }),
            ClauseKind::WellFormed(term) => ClauseKind::WellFormed(fold_term(tys, *term, f, depth)),
            ClauseKind::RegionOutlives(a, b) => ClauseKind::RegionOutlives(f.fold_region(tys, *a, depth), f.fold_region(tys, *b, depth)),
            ClauseKind::TypeOutlives(ty, region) => ClauseKind::TypeOutlives(f.fold_ty(tys, *ty, depth), f.fold_region(tys, *region, depth)),
            ClauseKind::ConstEvaluatable(ct) => ClauseKind::ConstEvaluatable(f.fold_const(tys, *ct, depth)),
            ClauseKind::ConstArgHasType(ct, ty) => ClauseKind::ConstArgHasType(f.fold_const(tys, *ct, depth), f.fold_ty(tys, *ty, depth)),
        }),
        PredicateKind::Subtype(p) => PredicateKind::Subtype(SubtypePredicate { a_is_expected: p.a_is_expected, a: f.fold_ty(tys, p.a, depth), b: f.fold_ty(tys, p.b, depth) }),
        PredicateKind::Coerce(p) => PredicateKind::Coerce(CoercePredicate { a: f.fold_ty(tys, p.a, depth), b: f.fold_ty(tys, p.b, depth) }),
        PredicateKind::ConstEquate(a, b) => PredicateKind::ConstEquate(f.fold_const(tys, *a, depth), f.fold_const(tys, *b, depth)),
        PredicateKind::NormalizesTo(p) => PredicateKind::NormalizesTo(NormalizesTo { alias: fold_alias(tys, &p.alias, f, depth), term: fold_term(tys, p.term, f, depth) }),
        PredicateKind::DynCompatible(d) => PredicateKind::DynCompatible(*d),
        PredicateKind::Ambiguous => PredicateKind::Ambiguous,
    }
}

pub fn fold_predicate(tys: &Types, predicate: &Predicate, f: &mut dyn TypeFolder) -> Predicate {
    if f.fold_flags().is_some_and(|flags| predicate.flags(tys) & flags.bits() == 0) { return predicate.clone(); }
    Predicate { kind: fold_predicate_kind(tys, &predicate.kind, f, 1), bound_vars: predicate.bound_vars.clone() }
}

pub fn fold_param_env(tcx: &crate::tcx::Tcx, env: ParamEnvId, f: &mut dyn TypeFolder) -> ParamEnvId {
    if f.fold_flags().is_some_and(|flags| tcx.param_env_flags(env) & flags.bits() == 0) { return env; }
    let clauses = tcx.param_env_by_id(env).clauses.iter().map(|p| fold_predicate(&tcx.tys, p, f)).collect::<Vec<_>>();
    tcx.intern_param_env(ParamEnv { clauses: clauses.into() })
}

pub fn fold_response(tcx: &crate::tcx::Tcx, response: &Response, f: &mut dyn TypeFolder) -> Response {
    let tys = &tcx.tys;
    let mut var_values = Vec::with_capacity(response.var_values.len());
    for &arg in &response.var_values { var_values.push(fold_arg(tys, arg, f, 0)); }
    let mut external_constraints = ExternalConstraintsData::default();
    for &(constraint, visible) in &response.external_constraints.region_constraints {
        let constraint = match constraint {
            RegionConstraint::RegionOutlives(a, b) => RegionConstraint::RegionOutlives(f.fold_region(tys, a, 0), f.fold_region(tys, b, 0)),
            RegionConstraint::Eq(a, b) => RegionConstraint::Eq(f.fold_region(tys, a, 0), f.fold_region(tys, b, 0)),
            RegionConstraint::TypeOutlives(ty, region) => RegionConstraint::TypeOutlives(f.fold_ty(tys, ty, 0), f.fold_region(tys, region, 0)),
        };
        external_constraints.region_constraints.push((constraint, visible));
    }
    for (key, ty) in &response.external_constraints.opaque_types {
        let mut args = Vec::with_capacity(key.args.len());
        for &arg in &*key.args { args.push(fold_arg(tys, arg, f, 0)); }
        external_constraints.opaque_types.push((crate::infer::opaque_types::OpaqueTypeKey { def_id: key.def_id, args: args.into() }, f.fold_ty(tys, *ty, 0)));
    }
    for (source, goal) in &response.external_constraints.normalization_nested_goals {
        external_constraints.normalization_nested_goals.push((*source, fold_goal(tcx, goal, f)));
    }
    Response { certainty: response.certainty, var_values, external_constraints }
}

pub fn fold_goal(tcx: &crate::tcx::Tcx, goal: &Goal, f: &mut dyn TypeFolder) -> Goal {
    Goal { param_env: fold_param_env(tcx, goal.param_env, f), predicate: fold_predicate(&tcx.tys, &goal.predicate, f) }
}
