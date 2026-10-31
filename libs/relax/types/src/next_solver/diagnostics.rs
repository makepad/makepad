use super::*;
use super::inspect::{InspectGoal, ProbeKind};
use crate::infer::InferCtxt;
use crate::program::Program;
use crate::types::TyKind;

fn trait_self(goal: &Goal) -> Option<(DefId, TyId)> {
    let PredicateKind::Clause(ClauseKind::Trait(p)) = &goal.predicate.kind else { return None };
    if p.polarity != ClausePolarity::Positive { return None; }
    let GenericArg::Type(ty) = *p.trait_ref.args.first()? else { return None };
    Some((p.trait_ref.def_id, ty))
}

/// rustc_trait_selection/src/solve/fulfill/derive_errors.rs::find_best_leaf_obligation: follow failed nested goals of a
/// matching candidate. Use the solver's impl obligations, including Send -> Sync for &T.
fn failure_chain(infcx: &InferCtxt<'_>, prog: &Program, goal: Goal, depth: usize) -> Vec<Goal> {
    crate::stack::ensure_sufficient_stack(|| {
        let goal = fold::fold_goal(infcx.tcx, &goal, &mut crate::infer::resolve::DeepVariableResolver::new(infcx));
        if depth >= infcx.recursion_limit(prog) { return vec![goal]; }
        if let Some((_, ty)) = trait_self(&goal) {
            if let Ok((normalized, nested)) = infcx.normalize_ty_with_universes(prog, &goal.param_env, ty) {
                if normalized != ty && nested.into_iter().all(|g| infcx.evaluate_root_goal(prog, g, None).is_ok_and(|r| r.certainty.is_yes())) {
                    let mut next = goal.clone();
                    if let PredicateKind::Clause(ClauseKind::Trait(p)) = &mut next.predicate.kind { p.trait_ref.args[0] = GenericArg::Type(normalized); }
                    let mut chain = vec![goal];
                    chain.extend(failure_chain(infcx, prog, next, depth + 1));
                    return chain;
                }
            }
        }
        let tree = infcx.evaluate_root_goal_for_diagnostics(prog, goal.clone());
        let inspected = InspectGoal::new(infcx, prog, tree);
        let mut chain = vec![goal];
        if inspected.result.is_ok() { return chain; }
        let mut candidates = inspected.candidates();
        candidates.retain(|c| c.result.is_err() && c.shallow_certainty.is_yes() && c.has_final_state() && matches!(c.kind, ProbeKind::TraitCandidate { .. }));
        // Multiple matching impls do not identify a unique cause.
        if candidates.len() != 1 { return chain; }
        for nested in candidates[0].instantiate_nested_goals() {
            if infcx.evaluate_root_goal(prog, nested.clone(), None).is_err() {
                if nested != chain[0] { chain.extend(failure_chain(infcx, prog, nested, depth + 1)); }
                break;
            }
        }
        chain
    })
}

/// rustc_trait_selection/src/error_reporting/traits/suggestions.rs::{maybe_note_obligation_cause_for_async_await,
/// note_obligation_cause_for_async_await}: retain the leaf, its containing types, and the innermost coroutine's source.
pub(crate) fn auto_trait_failure_notes(infcx: &InferCtxt<'_>, prog: &Program, goal: &Goal, body: Option<&crate::typeck::Body>) -> String {
    let Some((trait_def, _)) = trait_self(goal) else { return String::new() };
    if !super::eval_ctxt::trait_is_auto(prog, trait_def) { return String::new(); }
    infcx.probe(|_| {
        let tcx = infcx.tcx;
        let chain = failure_chain(infcx, prog, goal.clone(), 0);
        let types: Vec<_> = chain.iter().filter_map(trait_self).collect();
        let Some(&(leaf_trait, leaf)) = types.last() else { return String::new() };
        let describe = |ty| {
            if tcx.is_coroutine(ty) {
                if let TyKind::Closure(_, _, args) = tcx.tys.kind(ty) {
                    if let TyKind::FnDef(def, ..) = tcx.tys.kind(args.parent_args) { return format!("future from `{}`", prog.def_path(*def)); }
                }
            }
            crate::typeck::ty_to_string(false, prog, tcx, infcx.deeply_resolve_ignoring_regions(ty))
        };
        let mut notes = format!("\nnote: `{}` does not implement `{}`", describe(leaf), prog.name(leaf_trait));
        for &(tr, ty) in types.iter().rev().skip(1) {
            notes.push_str(&format!("\nnote: required for `{}` to implement `{}`", describe(ty), prog.name(tr)));
        }
        if let Some(body) = body {
            for (index, &(tr, ty)) in types.iter().enumerate().rev() {
                if !tcx.is_coroutine(ty) { continue; }
                let targets: Vec<_> = types[index + 1..].iter().map(|&(_, t)| t).collect();
                if let Some((source, await_span, name, witness)) = crate::borrowck::driver::coroutine_obligation_source(prog, tcx, body, ty, &targets) {
                    let position = |s: crate::mir::Span| {
                        let (line, col) = prog.files[s.file as usize].line_col(s.lo);
                        format!("{}:{line}:{col}", prog.file_paths[s.file as usize])
                    };
                    if let Some(await_span) = await_span {
                        notes.push_str(&format!("\nnote: future is not `{}` as {name} of type `{}` at {} is used across an await at {}", prog.name(tr), describe(witness), position(source), position(await_span)));
                    } else {
                        notes.push_str(&format!("\nnote: future is not `{}` as it captures {name} of type `{}` at {}", prog.name(tr), describe(witness), position(source)));
                    }
                    break;
                }
            }
        }
        notes
    })
}
