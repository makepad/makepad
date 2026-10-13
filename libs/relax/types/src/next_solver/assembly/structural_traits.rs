// ported from N:rustc_next_trait_solver/src/solve/assembly/structural_traits.rs @8d1a764
use super::*;
use crate::types::{TyId, TyKind};
use crate::upvar::ClosureKind;

pub fn instantiate_constituent_tys_for_auto_trait(ecx: &EvalCtxt<'_, '_>, ty: TyId) -> Result<Vec<TyId>, NoSolution> {
    let tcx = ecx.delegate.tcx;
    Ok(match tcx.tys.kind(ty) {
        TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_) | TyKind::FnDef(..) | TyKind::FnPtr(..) | TyKind::Error | TyKind::Never | TyKind::Foreign(_) => vec![],
        TyKind::Str => vec![tcx.tys.intern(TyKind::Slice(tcx.tys.u8_))],
        TyKind::Ptr(_, t) | TyKind::Ref(_, t, _) | TyKind::Array(t, _) | TyKind::Slice(t) => vec![*t],
        TyKind::Tuple(ts) => tcx.tys.list(*ts).to_vec(),
        TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: upvars, .. }) => {
            // EXCEPTION(M7): user decision 2026-10-07
            // The coroutine witness is empty until MIR witness publication.
            vec![*upvars]
        }
        TyKind::Adt(def, args, rs) => {
            let adt = &tcx.adts[def];
            if adt.is_phantom_data() { vec![tcx.tys.list(*args)[0]] } else {
                let full = tcx.complete_args(ecx.solver.prog, *def, tcx.tys.list(*args), *rs); let mut result = Vec::new();
                for v in &adt.variants { for f in &v.fields { result.push(tcx.instantiate_ty(ecx.solver.prog, *def, &full, f.ty)); } }
                result
            }
        }
        _ => return Err(NoSolution),
    })
}
pub fn instantiate_constituent_tys_for_sizedness_trait(ecx: &EvalCtxt<'_, '_>, sizedness: super::super::sizedness::SizedTraitKind, ty: TyId) -> Result<Vec<TyId>, NoSolution> {
    let tcx = ecx.delegate.tcx;
    Ok(match tcx.tys.kind(ty) {
        TyKind::Foreign(_) | TyKind::Str | TyKind::Slice(_) | TyKind::Dyn(..) | TyKind::Alias(..) | TyKind::Placeholder(_) | TyKind::Param(_) => return Err(NoSolution),
        TyKind::Tuple(ts) => match tcx.tys.list(*ts).last() { Some(&t) => vec![t], None => vec![] },
        TyKind::Adt(def, args, rs) => {
            match tcx.adt_sizedness_constraint(ecx.solver.prog, *def, sizedness).map_err(|_| NoSolution)? {
                Some(ty) => {
                    let full = tcx.complete_args(ecx.solver.prog, *def, tcx.tys.list(*args), *rs);
                    vec![tcx.instantiate_ty(ecx.solver.prog, *def, &full, ty)]
                }
                None => vec![],
            }
        }
        TyKind::Infer(_) | TyKind::Bound(..) => panic!("non-rigid sized self type"),
        _ => vec![],
    })
}
pub fn instantiate_constituent_tys_for_copy_clone_trait(ecx: &EvalCtxt<'_, '_>, ty: TyId) -> Result<Vec<TyId>, NoSolution> {
    let tcx = ecx.delegate.tcx;
    match tcx.tys.kind(ty) {
        TyKind::FnDef(..) | TyKind::FnPtr(..) | TyKind::Error => Ok(vec![]),
        TyKind::Tuple(ts) => Ok(tcx.tys.list(*ts).to_vec()),
        TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: upvars, .. }) if !tcx.is_coroutine(ty) => Ok(vec![*upvars]),
        _ => Err(NoSolution),
    }
}
pub fn extract_tupled_inputs_and_output_from_callable(ecx: &EvalCtxt<'_, '_>, ty: TyId, goal_kind: ClosureKind) -> Result<Option<TyId>, NoSolution> {
    let tcx = ecx.delegate.tcx;
    let signature = match tcx.tys.kind(ty) {
        TyKind::FnPtr(_, _, header, _) if header.is_fn_trait_compatible() => ty,
        TyKind::FnDef(def, args, rs) => {
            let sig = tcx.fn_item_sig(ecx.solver.prog, *def).ok_or(NoSolution)?;
            let d = ecx.solver.prog.def(*def); let ast = &ecx.solver.prog.files[d.file as usize].ast;
            if !sig.header.is_fn_trait_compatible() || crate::lang_items::has_attr(ecx.solver.prog, d.file, ast.list(ast.item(d.item).attrs), "target_feature") { return Err(NoSolution); }
            let full = tcx.complete_args(ecx.solver.prog, *def, tcx.tys.list(*args), *rs);
            tcx.instantiate_ty(ecx.solver.prog, *def, &full, tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&sig.params), sig.ret, sig.header, sig.bound_regions)))
        }
        TyKind::Closure(file, expr, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty, closure_sig_as_fn_ptr_ty: sig, .. }) if !tcx.is_coroutine(ty) => {
            let kind = ecx.delegate.shallow_resolve(*closure_kind_ty);
            match tcx.tys.to_opt_closure_kind(kind) {
                Some(kind) if kind > goal_kind => return Err(NoSolution),
                None if goal_kind != ClosureKind::FnOnce => return Ok(None),
                _ => {},
            }
            *sig
        }
        _ => return Err(NoSolution),
    };
    Ok(Some(signature))
}
/// structural_traits.rs:416-428.
pub struct AsyncCallableRelevantTypes { pub tupled_inputs_ty: TyId, pub output_coroutine_ty: TyId, pub coroutine_return_ty: TyId }
/// structural_traits.rs:435-582: the bound signature (its FnPtr binder) and the nested
/// predicates under that binder. Relax has no coroutine-closure type: an `async` closure is a
/// closure returning its body's coroutine, so it takes the `Closure` arm (closures returning
/// futures), as FnDef and FnPtr do.
pub fn extract_tupled_inputs_and_output_from_async_callable(ecx: &EvalCtxt<'_, '_>, ty: TyId, goal_kind: ClosureKind) -> Result<(TyId, Vec<Predicate>), NoSolution> {
    let tcx = ecx.delegate.tcx;
    let future = tcx.lang_items.get(crate::lang_items::LangItem::Future).ok_or(NoSolution)?;
    match tcx.tys.kind(ty) {
        // `FnDef` and `FnPtr` only implement `AsyncFn*` when their return type implements `Future`.
        TyKind::FnPtr(..) | TyKind::FnDef(..) => match extract_tupled_inputs_and_output_from_callable(ecx, ty, goal_kind)? {
            Some(sig) => Ok((sig, vec![bound_predicate(tcx, future, sig)])),
            None => Err(NoSolution),
        },
        TyKind::Closure(_, _, args) if !tcx.is_coroutine(ty) => {
            let mut nested = vec![bound_predicate(tcx, future, args.sig_as_fn_ptr_ty())];
            let kind_ty = ecx.delegate.shallow_resolve(args.kind_ty());
            match tcx.tys.to_opt_closure_kind(kind_ty) {
                Some(kind) if kind > goal_kind => return Err(NoSolution),
                Some(_) => {}
                None => {
                    let helper = tcx.lang_items.get(crate::lang_items::LangItem::AsyncFnKindHelper).ok_or(NoSolution)?;
                    nested.push(super::super::collect::trait_predicate(helper, vec![GenericArg::Type(kind_ty), GenericArg::Type(tcx.tys.from_closure_kind(goal_kind))]));
                }
            }
            Ok((args.sig_as_fn_ptr_ty(), nested))
        }
        _ => Err(NoSolution),
    }
}
/// `bound_sig.rebind(TraitRef::new(cx, future, [sig.output()]))`: the signature's late-bound
/// regions are the predicate's bound variables.
fn bound_predicate(tcx: &crate::tcx::Tcx, future: crate::program::DefId, bound_sig: TyId) -> Predicate {
    let TyKind::FnPtr(_, output, _, bound) = tcx.tys.kind(bound_sig) else { unreachable!() };
    let mut predicate = super::super::collect::trait_predicate(future, vec![GenericArg::Type(*output)]);
    predicate.bound_vars = vec![BoundVariableKind::Region(crate::types::BoundRegionKind::Anon); *bound as usize].into();
    predicate
}
/// The binder's contents (structural_traits.rs:499-514,571-580): the tupled inputs, the
/// returned future and `<future as Future>::Output`.
pub fn async_callable_relevant_types(ecx: &EvalCtxt<'_, '_>, sig: TyId) -> Result<AsyncCallableRelevantTypes, NoSolution> {
    let tcx = ecx.delegate.tcx;
    let TyKind::FnPtr(params, output, _, _) = tcx.tys.kind(sig) else { unreachable!() };
    let future_output = tcx.lang_items.get(crate::lang_items::LangItem::FutureOutput).ok_or(NoSolution)?;
    let coroutine_return_ty = tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, crate::types::AliasTy { kind: crate::types::AliasTyKind::Projection { def_id: future_output }, args: tcx.tys.mk_list(&[*output]), regions: crate::types::RegionArgsId::EMPTY }));
    Ok(AsyncCallableRelevantTypes { tupled_inputs_ty: tcx.tys.intern(TyKind::Tuple(params.clone())), output_coroutine_ty: *output, coroutine_return_ty })
}
