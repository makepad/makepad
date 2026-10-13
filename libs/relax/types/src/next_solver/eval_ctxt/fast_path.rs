// ported from N:rustc_next_trait_solver/src/solve/eval_ctxt/fast_path.rs @8d1a764
// ported from N:rustc_trait_selection/src/solve/delegate.rs @8d1a764
// ported from N:rustc_middle/src/ty/sty.rs @8d1a764
use super::*;
use crate::types::{Types, TyKind};
use crate::types::fold::{TypeFolder, super_fold_ty, super_fold_const};
use crate::consts::ConstKind;
use crate::lang_items::LangItem;

pub fn rerunning_stalled_goal_may_make_progress(infcx: &InferCtxt<'_>, stalled: Option<&GoalStalledOn>) -> bool {
    let Some(stalled) = stalled else { return true; };
    for &arg in &stalled.stalled_vars {
        let changed = match arg {
            GenericArg::Type(t) => infcx.shallow_resolve(t) != t,
            GenericArg::Const(c) => infcx.shallow_resolve_const(c) != c,
            GenericArg::Lifetime(_) => unreachable!(),
        };
        if changed { return true; }
    }
    for &root in &stalled.sub_roots { if infcx.inner.borrow_mut().type_variables().sub_unification_table_root_var(root) != root { return true; } }
    if let GoalStalledOnOpaques::Yes { num_opaques_in_storage, previously_succeeded_in_erased } = &stalled.opaques {
        let mut inner = infcx.inner.borrow_mut();
        let storage = inner.opaque_types();
        if storage.num_entries().needs_reevaluation(*num_opaques_in_storage) {
            let mut opaques = Vec::new(); for (key, ty) in storage.iter_lookup_table() { opaques.push((key.clone(), ty.ty)); }
            drop(inner);
            if let SucceededInErased::Yes { accessed_opaques } = previously_succeeded_in_erased {
                if typing_mode::should_rerun_after_erased_canonicalization(infcx.tcx, &accessed_opaques.rerun, infcx.typing_mode, &opaques) == typing_mode::RerunDecision::No { return false; }
            }
            return true;
        }
    }
    false
}

pub(super) fn build_stalled_on(infcx: &InferCtxt<'_>, originals: &[GenericArg], maybe: MaybeInfo, opaques: GoalStalledOnOpaques) -> GoalStalledOn {
    let mut result = GoalStalledOn { stalled_vars: Vec::new(), sub_roots: Vec::new(), stalled_maybe_info: maybe, opaques };
    for &arg in originals {
        match arg {
            GenericArg::Type(t) => match infcx.tcx.tys.kind(t) {
                TyKind::Infer(v) => { result.sub_roots.push(infcx.inner.borrow_mut().type_variables().sub_unification_table_root_var(*v)); result.stalled_vars.push(arg); }
                TyKind::IntVar(_) | TyKind::FloatVar(_) => result.stalled_vars.push(arg),
                _ => {},
            },
            GenericArg::Const(c) if matches!(infcx.tcx.tys.const_kind(c), ConstKind::Infer(_)) => result.stalled_vars.push(arg),
            _ => {},
        }
    }
    result
}

pub fn compute_goal_fast_path(infcx: &InferCtxt<'_>, prog: &crate::program::Program, goal: &Goal) -> Option<GoalEvaluation> {
    let tys = &infcx.tcx.tys;
    let mut stalled = None;
    let bound = !goal.predicate.bound_vars.is_empty();
    match &goal.predicate.kind {
        PredicateKind::Clause(ClauseKind::Trait(p)) => {
            let GenericArg::Type(ty) = p.trait_ref.args[0] else { unreachable!() };
            let ty = infcx.shallow_resolve(ty);
            if matches!(tys.kind(ty), TyKind::Infer(_)) && !matches!(infcx.typing_mode, crate::tcx::TypingMode::ErasedNotCoherence) && infcx.inner.borrow_mut().opaque_types().is_empty() {
                stalled = Some(build_stalled_on(infcx, &[GenericArg::Type(ty)], MaybeInfo::AMBIGUOUS, GoalStalledOnOpaques::Yes { num_opaques_in_storage: 0, previously_succeeded_in_erased: SucceededInErased::No }));
            } else if p.polarity == ClausePolarity::Positive {
                let lang = infcx.tcx.lang_items.by_def.get(&p.trait_ref.def_id).copied();
                let ty = infcx.deeply_resolve_ignoring_regions(ty);
                match lang {
                    Some(LangItem::Sized | LangItem::MetaSized) => {
                        let meta = lang == Some(LangItem::MetaSized);
                        let mut holds = has_trivial_sizedness(infcx, prog, ty, meta);
                        if !holds && matches!(tys.kind(ty), TyKind::Param(_) | TyKind::Placeholder(_)) {
                            for clause in &*infcx.tcx.param_env_by_id(goal.param_env).clauses {
                                if let PredicateKind::Clause(ClauseKind::Trait(c)) = &clause.kind {
                                    holds |= c.polarity == ClausePolarity::Positive && c.trait_ref.args[0] == GenericArg::Type(ty) &&
                                        (c.trait_ref.def_id == p.trait_ref.def_id || (meta && infcx.tcx.lang_items.get(LangItem::Sized) == Some(c.trait_ref.def_id)));
                                }
                            }
                        }
                        if !holds { return None; }
                    }
                    Some(LangItem::Copy | LangItem::Clone) if !tys.has_infer(ty) && !tys.flags(ty).intersects(crate::types::TypeFlags::HAS_FREE_REGIONS) && trivially_copy(tys, ty) => {},
                    _ => return None,
                }
            } else { return None; }
        }
        PredicateKind::DynCompatible(d) if infcx.tcx.dyn_compatibility_violations(prog, *d).is_empty() => {},
        PredicateKind::Clause(ClauseKind::RegionOutlives(a,b)) if !bound => { infcx.inner.borrow_mut().unwrap_region_constraints().make_subregion(tys, *b, *a, false); }
        PredicateKind::Clause(ClauseKind::TypeOutlives(ty,r)) if !bound => {
            let ty = infcx.deeply_resolve_ignoring_regions(*ty);
            if normalize::has_non_rigid_aliases(tys, ty) { return None; }
            let mut collector = CollectNonRegionInfer { vars: Vec::new(), visited: crate::fxhash::HashSet::default() };
            collector.fold_ty(tys, ty, 0);
            if collector.vars.is_empty() { infcx.register_ty_outlives(ty, *r); }
            else { stalled = Some(build_stalled_on(infcx, &collector.vars, MaybeInfo::AMBIGUOUS, GoalStalledOnOpaques::No)); }
        }
        PredicateKind::Subtype(SubtypePredicate { a,b,.. }) | PredicateKind::Coerce(CoercePredicate { a,b }) if !bound => {
            let a = infcx.shallow_resolve(*a); let b = infcx.shallow_resolve(*b);
            let (TyKind::Infer(av), TyKind::Infer(bv)) = (tys.kind(a),tys.kind(b)) else { return None; };
            infcx.inner.borrow_mut().type_variables().sub_unify(*av,*bv);
            stalled = Some(build_stalled_on(infcx, &[GenericArg::Type(a),GenericArg::Type(b)], MaybeInfo::AMBIGUOUS, GoalStalledOnOpaques::No));
        }
        PredicateKind::Clause(ClauseKind::ConstArgHasType(c,_)) if !bound => {
            let c = infcx.shallow_resolve_const(*c); if !matches!(tys.const_kind(c),ConstKind::Infer(_)) { return None; }
            stalled = Some(build_stalled_on(infcx, &[GenericArg::Const(c)], MaybeInfo::AMBIGUOUS, GoalStalledOnOpaques::No));
        }
        PredicateKind::Clause(ClauseKind::WellFormed(term)) if !bound => {
            let arg = match term { Term::Ty(t) => GenericArg::Type(infcx.shallow_resolve(*t)), Term::Const(c) => GenericArg::Const(infcx.shallow_resolve_const(*c)) };
            let mut vars = Vec::new();
            match arg {
                GenericArg::Type(t) if trivially_wf(infcx, prog, t) => {},
                GenericArg::Type(t) if matches!(tys.kind(t),TyKind::Infer(_)) => vars.push(arg),
                GenericArg::Const(c) if matches!(tys.const_kind(c),ConstKind::Infer(_)) => vars.push(arg),
                _ => return None,
            }
            if !vars.is_empty() { stalled = Some(build_stalled_on(infcx,&vars,MaybeInfo::AMBIGUOUS,GoalStalledOnOpaques::No)); }
        }
        _ => return None,
    }
    Some(GoalEvaluation { goal: goal.clone(), certainty: if stalled.is_some() { Certainty::AMBIGUOUS } else { Certainty::Yes }, has_changed: false, stalled_on: stalled })
}
struct CollectNonRegionInfer { vars: Vec<GenericArg>, visited: crate::fxhash::HashSet<TyId> }
impl TypeFolder for CollectNonRegionInfer {
    fn fold_ty(&mut self, tys: &Types, t: TyId, d: u32) -> TyId {
        if !self.visited.insert(t) { return t; }
        if matches!(tys.kind(t),TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_)) { self.vars.push(GenericArg::Type(t)); t }
        else { super_fold_ty(tys,t,self,d) }
    }
    fn fold_const(&mut self, tys: &Types, c: Const, d: u32) -> Const {
        if matches!(tys.const_kind(c),ConstKind::Infer(_)) { self.vars.push(GenericArg::Const(c)); c }
        else { super_fold_const(tys,c,self,d) }
    }
}
fn has_trivial_sizedness(infcx: &InferCtxt<'_>, prog: &crate::program::Program, ty: TyId, meta: bool) -> bool {
    let tcx = infcx.tcx;
    match tcx.tys.kind(ty) {
        TyKind::Foreign(_) | TyKind::Alias(..) | TyKind::Param(_) | TyKind::Placeholder(_) | TyKind::Bound(..) | TyKind::Infer(_) => false,
        TyKind::Str | TyKind::Slice(_) | TyKind::Dyn(..) => meta,
        TyKind::Tuple(ts) => tcx.tys.list(*ts).last().is_none_or(|t| has_trivial_sizedness(infcx,prog,*t,meta)),
        TyKind::Adt(def,args,rs) => {
            let adt = &tcx.adts[def];
            if adt.is_enum || adt.is_union { return true; }
            let kind = if meta { super::super::sizedness::SizedTraitKind::MetaSized } else { super::super::sizedness::SizedTraitKind::Sized };
            match tcx.adt_sizedness_constraint(prog, *def, kind) {
                Ok(None) => true,
                Ok(Some(tail)) => {
                    let full = tcx.complete_args(prog, *def, tcx.tys.list(*args), *rs);
                    has_trivial_sizedness(infcx, prog, tcx.instantiate_ty(prog, *def, &full, tail), meta)
                }
                Err(_) => false,
            }
        }
        _ => true,
    }
}
fn trivially_copy(tys: &Types, t: TyId) -> bool {
    match tys.kind(t) {
        TyKind::Bool | TyKind::Char | TyKind::Never | TyKind::Int(_) | TyKind::Float(_) | TyKind::FnDef(..) | TyKind::Ptr(..) | TyKind::Ref(false,..) => true,
        TyKind::Array(t,_) => trivially_copy(tys,*t),
        TyKind::Tuple(ts) if tys.list(*ts).len() <= 3 => { for &t in tys.list(*ts) { if !trivially_copy(tys,t) { return false; } } true }
        _ => false,
    }
}
fn trivially_wf(infcx: &InferCtxt<'_>, prog: &crate::program::Program, t: TyId) -> bool {
    let tys = &infcx.tcx.tys;
    match tys.kind(t) {
        TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_) | TyKind::Str | TyKind::Never | TyKind::Param(_) | TyKind::Placeholder(_) | TyKind::Bound(..) | TyKind::IntVar(_) | TyKind::FloatVar(_) => true,
        TyKind::Slice(t) => trivially_wf(infcx,prog,*t) && has_trivial_sizedness(infcx,prog,*t,false),
        TyKind::Ptr(_,t) => trivially_wf(infcx,prog,*t),
        TyKind::FnPtr(ts,r,..) => { for &t in tys.list(*ts) { if !trivially_wf(infcx,prog,t) { return false; } } trivially_wf(infcx,prog,*r) }
        TyKind::Ref(_,t,_) if !tys.flags(*t).intersects(crate::types::TypeFlags::HAS_FREE_REGIONS) && !tys.has_infer(*t) && !tys.flags(*t).intersects(crate::types::TypeFlags::HAS_PARAM | crate::types::TypeFlags::HAS_PLACEHOLDER) => trivially_wf(infcx,prog,*t),
        _ => false,
    }
}
