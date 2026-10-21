// ported from N:rustc_middle/src/ty/instance.rs @8d1a764
// ported from N:rustc_middle/src/ty/normalize_erasing_regions.rs @8d1a764
use super::*;

/// Share identity codegen bodies, including empty argument lists. Substituted
/// instances retain the fallible PostAnalysis fold; identity consumers normalize
/// individual reads (rustc_codegen_ssa/src/mir/mod.rs:129-139).
pub(super) fn try_instantiate_mir_and_normalize_erasing_regions(prog: &Program, tcx: &Tcx, body: std::sync::Arc<Body>, args: &[TyId]) -> Result<std::sync::Arc<Body>, crate::next_solver::NoSolution> {
    if *body.args == *args { return Ok(body); }
    try_normalize_body(prog, tcx, crate::tcx::TypingEnv::fully_monomorphized(prog), &body, args).map(std::sync::Arc::new)
}

/// Analysis consumers normalize in the environment of the substituted arguments
/// (rustc_middle/src/ty/normalize_erasing_regions.rs), even for identity
/// substitutions. Unresolved projections still fail at this fallible boundary.
pub(super) fn try_normalize_body(prog: &Program, tcx: &Tcx, env: crate::tcx::TypingEnv, body: &Body, args: &[TyId]) -> Result<Body, crate::next_solver::NoSolution> {
    static COUNTS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *COUNTS.get_or_init(|| std::env::var_os("MAKEPAD_RUN_BODY_COUNTS").is_some()) { eprintln!("body-count instantiate-clone"); }
    // a body is instantiated with exactly its generics (a `fn main<T>()`, E0131, has no instance)
    if args.len() != tcx.generic_indices_of(prog, body.def).types_consts.len() { return Err(crate::next_solver::NoSolution); }
    let mut b = body.clone();
    let root_errors = tcx.dcx.clone();
    let checkpoint = root_errors.checkpoint();
    let errors = &root_errors;
    // Const-param operands read this argument list. It is instantiated data,
    // and must cross the same normalization boundary as MIR operand constants.
    b.args = args.iter().map(|&ty| tcx.try_normalize_erasing_regions(prog, env, ty)).collect::<Result<ThinVec<_>, _>>()?;
    // The body now uses the caller's parameters, including consts nested in
    // types. Keep their clauses for subsequent borrowck type operations.
    b.param_env = (env.param_env != crate::tcx::ParamEnvId::EMPTY).then_some(env.param_env);
    let indices = tcx.generic_indices_of(prog, body.def);
    let regions = tcx.tys.intern_region_args(&vec![crate::types::RegionId::ERASED; indices.lifetimes.len()]);
    let full = tcx.complete_args(prog, body.def, args, regions);
    let mut folded = HashMap::default();
    let mut s = |ty: TyId| {
        if let Some(&ty) = folded.get(&ty) { return Ok(ty); }
        let instantiated = crate::types::fold::EarlyBinder::bind(&tcx.tys, ty).instantiate(&tcx.tys, &indices, &full);
        let result = tcx.try_normalize_erasing_regions(prog, env, instantiated)?;
        folded.insert(ty, result);
        Ok(result)
    };
    for ty in &mut b.expr_ty { *ty = s(*ty)?; }
    for ty in &mut b.pat_ty { *ty = s(*ty)?; }
    b.ret = s(b.ret)?;
    for ty in b.hidden_types.values_mut() { *ty = s(*ty)?; }
    let tys = &tcx.tys;
    let sl = |l: &mut crate::types::TyList, s: &mut dyn FnMut(TyId) -> Result<TyId, crate::next_solver::NoSolution>| -> Result<(), crate::next_solver::NoSolution> {
        if l.is_empty() { return Ok(()); }
        let v = tys.list(*l).iter().map(|&t| s(t)).collect::<Result<Vec<_>, _>>()?;
        *l = tys.mk_list(&v);
        Ok(())
    };
    for args in b.res_args.values_mut() { sl(args, &mut s)?; }
    for method in b.methods.values_mut() { sl(&mut method.args, &mut s)?; }
    for method in b.binops.values_mut() { sl(&mut method.args, &mut s)?; }
    for adjust in b.adjust_payloads_mut() {
        adjust.target = s(adjust.target)?;
        match &mut adjust.kind {
            adjustments::Adjust::Deref(Some((_, args))) => sl(args, &mut s)?,
            adjustments::Adjust::Coerce(Coerce::ToDyn(a, d) | Coerce::Upcast(a, d)) => { *a = s(*a)?; *d = s(*d)?; }
            _ => {}
        }
    }
    // The canonical annotations are the generic body's: they have no instantiated reading
    // (regions are erased here), and nothing reads them from an instance.
    b.user_provided_types = SideTable::new();
    b.user_provided_sigs = SideTable::new();
    for (_, args) in b.ov_index.values_mut() { sl(args, &mut s)?; }
    for (_, args) in b.mutable_index.values_mut() { sl(args, &mut s)?; }
    for (_, args) in b.std_index.values_mut() { sl(args, &mut s)?; }
    for (_, args) in b.for_next.values_mut() { sl(args, &mut s)?; }
    for (_, args, ty) in b.for_into.values_mut() { sl(args, &mut s)?; *ty = s(*ty)?; }
    for (_, args, ty) in b.try_branch.values_mut() { sl(args, &mut s)?; *ty = s(*ty)?; }
    for (_, args) in b.try_from_residual.values_mut() { sl(args, &mut s)?; }
    for local in &mut b.locals { local.ty = s(local.ty)?; }
    for sig in b.closure_sigs.values_mut() {
        sig.bound = s(sig.bound)?; sig.liberated = s(sig.liberated)?;
        if let Some((a, b)) = &mut sig.annotation_eq { *a = s(*a)?; *b = s(*b)?; }
    }
    for call in b.call_traits.values_mut() { call.self_ty = s(call.self_ty)?; call.tupled_args = s(call.tupled_args)?; }
    for ty in b.inline_const_types.values_mut() { *ty = s(*ty)?; }
    for captures in b.min_captures.values_mut() {
        for capture in &mut captures.captures { instantiate_place(&mut capture.place, &mut s)?; }
        for (place, _, _) in &mut captures.fake_reads { instantiate_place(place, &mut s)?; }
        if let Some(coroutine) = &mut captures.coroutine {
            for capture in &mut coroutine.by_move { instantiate_place(&mut capture.place, &mut s)?; }
            for capture in &mut coroutine.child_captures { for (_, ty) in &mut capture.projections { *ty = s(*ty)?; } }
        }
    }
    if !b.errors.is_empty() { errors.note_reported_error(); }
    let (reported, warnings) = errors.take_since(checkpoint);
    for error in reported {
        let owner = if error.cause.body_id == crate::program::NO_DEF { body.def } else { error.cause.body_id };
        b.errors.push(prog.record_error(prog.def(owner).file, error.cause.span, "typeck", error.message));
    }
    for warning in warnings { eprintln!("warning: {}", warning.message); }
    Ok(b)
}

fn instantiate_place(place: &mut crate::expr_use::Place, s: &mut dyn FnMut(TyId) -> Result<TyId, crate::next_solver::NoSolution>) -> Result<(), crate::next_solver::NoSolution> {
    place.root_ty = s(place.root_ty)?;
    for (_, ty) in &mut place.projections { *ty = s(*ty)?; }
    Ok(())
}
