// ported from N:rustc_trait_selection/src/traits/query/dropck_outlives.rs @8d1a764 (1.97.1 identical)
// ported from N:rustc_traits/src/dropck_outlives.rs @8d1a764 (adt_dtorck_constraint)
// ported from N:rustc_middle/src/ty/util.rs:464-525 destructor_constraints
//! The `DropckOutlives` type op: which types and regions a destructor of `dropped_ty` may touch.
//!
//! Deviations (each is a Relax data gap, not a policy):
//! * coroutines: rustc asks `mir_coroutine_witnesses` whether any interior type needs drop; borrowck
//!   computes that when it has built the coroutine's MIR (`Tcx::coroutine_witness_needs_drop`).
//! * `query_normalize` of the dtorck types is the ocx's deep normalization; a failure is a
//!   `NoSolution` for the whole op.
use super::*;
use crate::types::{TyKind, RegionKind};
use super::fulfill::ObligationCtxt;
use super::type_op::{scrape_region_constraints, ParamEnvAnd, TypeOp, TypeOpOutput};
use crate::consts::GenericArg;
use crate::infer::InferCtxt;
use crate::program::{DefId, Program};
use crate::tcx::Tcx;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DropckOutlives { pub dropped_ty: TyId }

/// rustc_middle DropckOutlivesResult.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DropckOutlivesResult { pub kinds: Vec<GenericArg>, pub overflows: Vec<TyId> }

/// rustc_middle DropckConstraint.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DropckConstraint { pub outlives: Vec<GenericArg>, pub dtorck_types: Vec<TyId>, pub overflows: Vec<TyId> }

/// dropck_outlives.rs:24 trivial_dropck_outlives.
pub fn trivial_dropck_outlives(tcx: &Tcx, ty: TyId) -> bool {
    let tys = &tcx.tys;
    match *tys.kind(ty) {
        TyKind::Bool | TyKind::Int(_) | TyKind::Float(_) | TyKind::Never | TyKind::FnDef(..) | TyKind::FnPtr(..) | TyKind::Char
        | TyKind::Ptr(..) | TyKind::Ref(..) | TyKind::Str | TyKind::Foreign(_) | TyKind::Error | TyKind::IntVar(_) | TyKind::FloatVar(_) => true,
        TyKind::Slice(t) => trivial_dropck_outlives(tcx, t),
        TyKind::Array(t, size) => match size.try_to_target_usize(tys) { Some(0) => true, _ => trivial_dropck_outlives(tcx, t) },
        TyKind::Tuple(ts) => tys.list(ts).iter().all(|&t| trivial_dropck_outlives(tcx, t)),
        TyKind::Closure(_, _, args) if !tcx.is_coroutine(ty) => trivial_dropck_outlives(tcx, args.tupled_upvars_ty),
        TyKind::Adt(def, ..) => tcx.adts.get(&def).is_some_and(|a| a.is_manually_drop()),
        _ => false,
    }
}

/// util.rs:464 destructor_constraints: the arguments of `def` that the `Drop` impl does not
/// mark `#[may_dangle]`, in terms of `def`'s own identity parameters.
fn destructor_constraints(tcx: &Tcx, prog: &Program, def: DefId, identity: &[GenericArg]) -> Vec<GenericArg> {
    let Some(dtor) = tcx.adt_destructor(def) else { return vec![] };
    let imp = prog.def(dtor.did).parent;
    let Some(&self_ty) = tcx.impl_self.get(&imp) else { return vec![] };
    let TyKind::Adt(d, args, regions) = *tcx.tys.kind(self_ty) else { return vec![] };
    if d != def { return vec![]; }
    let impl_args = tcx.complete_args(prog, def, tcx.tys.list(args), regions);
    let item = prog.def(imp);
    let ast = &prog.files[item.file as usize].ast;
    let Some(generics) = ast.kind_generics(&ast.item(item.item).kind) else { return vec![] };
    let params = ast.list(generics.params);
    let pure = |index: u32| params.get(index as usize).is_some_and(|p| crate::lang_items::has_attr(prog, item.file, ast.list(p.attrs), "may_dangle"));
    let tys = &tcx.tys;
    identity.iter().zip(&impl_args).filter(|(_, arg)| match **arg {
        GenericArg::Lifetime(r) => match tys.region_kind(r) { RegionKind::ReEarlyParam(i) => !pure(i), _ => false },
        GenericArg::Type(t) => match *tys.kind(t) { TyKind::Param(i) => !pure(type_param_decl_index(tcx, prog, imp, i)), _ => false },
        GenericArg::Const(c) => match tys.const_kind(c) { crate::consts::ConstKind::Param(p) => !pure(type_param_decl_index(tcx, prog, imp, p.index)), _ => false },
    }).map(|(item, _)| *item).collect()
}

/// A type/const parameter's compact index -> its position among the impl's declared parameters.
fn type_param_decl_index(tcx: &Tcx, prog: &Program, imp: DefId, compact: u32) -> u32 {
    tcx.generic_indices_of(prog, imp).types_consts.get(compact as usize).copied().unwrap_or(u32::MAX)
}

/// rustc_traits/src/dropck_outlives.rs:34 adt_dtorck_constraint, over the ADT's identity
/// parameters.
pub fn adt_dtorck_constraint(tcx: &Tcx, prog: &Program, def: DefId, limit: usize) -> DropckConstraint {
    if let Some(done) = tcx.dtorck_constraints.lock().unwrap().get(&(def, limit)) { return (**done).clone(); }
    let result = adt_dtorck_constraint_uncached(tcx, prog, def, limit);
    tcx.dtorck_constraints.lock().unwrap().insert((def, limit), std::sync::Arc::new(result.clone()));
    result
}

fn adt_dtorck_constraint_uncached(tcx: &Tcx, prog: &Program, def: DefId, limit: usize) -> DropckConstraint {
    // A query cycle (rustc: a cycle error): an ADT that contains itself by value has no finite size, which
    // is reported (E0072) where the item is checked; its dropck constraint is empty here.
    thread_local! { static COMPUTING: std::cell::RefCell<Vec<DefId>> = const { std::cell::RefCell::new(Vec::new()) }; }
    if COMPUTING.with(|c| c.borrow().contains(&def)) { return DropckConstraint::default(); }
    COMPUTING.with(|c| c.borrow_mut().push(def));
    struct Pop;
    impl Drop for Pop { fn drop(&mut self) { COMPUTING.with(|c| { c.borrow_mut().pop(); }); } }
    let _pop = Pop;
    let adt = &tcx.adts[&def];
    let identity = tcx.generic_indices_of(prog, def).identity_args(&tcx.tys);
    if adt.is_phantom_data() {
        let GenericArg::Type(t) = identity.iter().copied().find(|a| matches!(a, GenericArg::Type(_))).expect("PhantomData parameter") else { unreachable!() };
        return DropckConstraint { outlives: vec![], dtorck_types: vec![t], overflows: vec![] };
    }
    let mut result = DropckConstraint::default();
    // (rustc's Box owns its `T` through `Unique<T>`'s `PhantomData<T>` marker; Relax's Box holds
    // only a `NonNull<T>`, so the owned `T` is the Box's dtorck type itself)
    if adt.is_box() {
        if let Some(GenericArg::Type(t)) = identity.iter().copied().find(|a| matches!(a, GenericArg::Type(_))) { result.dtorck_types.push(t); }
    }
    for variant in &adt.variants {
        for field in &variant.fields { dtorck_constraint_for_ty_inner(tcx, prog, limit, 0, field.ty, &mut result); }
    }
    result.outlives.extend(destructor_constraints(tcx, prog, def, &identity));
    let mut seen = HashSet::new();
    result.outlives.retain(|v| seen.insert(*v));
    let mut seen = HashSet::new();
    result.dtorck_types.retain(|v| seen.insert(*v));
    result
}

/// dropck_outlives.rs:284 dtorck_constraint_for_ty_inner.
pub fn dtorck_constraint_for_ty_inner(tcx: &Tcx, prog: &Program, limit: usize, depth: usize, ty: TyId, out: &mut DropckConstraint) {
    if depth > limit { out.overflows.push(ty); return; }
    if trivial_dropck_outlives(tcx, ty) { return; }
    let tys = &tcx.tys;
    match *tys.kind(ty) {
        TyKind::Array(e, _) | TyKind::Slice(e) => dtorck_constraint_for_ty_inner(tcx, prog, limit, depth + 1, e, out),
        TyKind::Tuple(ts) => for &t in tys.list(ts) { dtorck_constraint_for_ty_inner(tcx, prog, limit, depth + 1, t, out); },
        TyKind::Closure(file, expr, args) => {
            let upvars = match *tys.kind(args.tupled_upvars_ty) { TyKind::Tuple(ts) => tys.list(ts).to_vec(), _ => vec![] };
            // rustc dtorck_constraint_for_ty_inner (Coroutine): when any type of the witness needs drop, the
            // upvars are use-live; otherwise they are only drop-live. The resume type adds nothing: its regions
            // are the coroutine's own. A coroutine whose witness is not known (its MIR was not built) has none.
            if tcx.is_coroutine(ty) && tcx.coroutine_witness_needs_drop.get(&(file, expr)).unwrap_or(false) {
                out.outlives.extend(upvars.iter().map(|&t| GenericArg::Type(t)));
            } else {
                for t in upvars { dtorck_constraint_for_ty_inner(tcx, prog, limit, depth + 1, t, out); }
            }
        }
        TyKind::Adt(def, args, regions) => {
            let c = adt_dtorck_constraint(tcx, prog, def, limit);
            let full = tcx.complete_args(prog, def, tys.list(args), regions);
            let inst = |t: TyId| tcx.instantiate_ty(prog, def, &full, t);
            let inst_arg = |a: GenericArg| match a {
                GenericArg::Type(t) => GenericArg::Type(inst(t)),
                GenericArg::Lifetime(r) => match tys.region_kind(r) {
                    RegionKind::ReEarlyParam(i) => full[i as usize],
                    _ => a,
                },
                GenericArg::Const(ct) => match tys.const_kind(ct) {
                    crate::consts::ConstKind::Param(p) => full[tcx.generic_indices_of(prog, def).types_consts[p.index as usize] as usize],
                    _ => a,
                },
            };
            out.dtorck_types.extend(c.dtorck_types.iter().map(|&t| inst(t)));
            out.outlives.extend(c.outlives.iter().map(|&a| inst_arg(a)));
            out.overflows.extend(c.overflows.iter().map(|&t| inst(t)));
        }
        TyKind::Dyn(..) => out.outlives.push(GenericArg::Type(ty)),
        TyKind::Alias(..) | TyKind::Param(_) => out.dtorck_types.push(ty),
        // unresolved types are a delayed bug in rustc; nothing to add
        _ => {}
    }
}

/// dropck_outlives.rs:116 compute_dropck_outlives_with_errors.
pub fn compute_dropck_outlives_inner(ocx: &mut ObligationCtxt<'_, '_>, env: &ParamEnvId, dropped_ty: TyId, limit: usize) -> Result<DropckOutlivesResult, NoSolution> {
    let (tcx, prog) = (ocx.infcx.tcx, ocx.prog);
    let mut result = DropckOutlivesResult::default();
    let mut ty_stack = vec![(dropped_ty, 0usize)];
    let mut ty_set = HashSet::new();
    let mut constraints = DropckConstraint::default();
    while let Some((ty, depth)) = ty_stack.pop() {
        dtorck_constraint_for_ty_inner(tcx, prog, limit, depth, ty, &mut constraints);
        result.kinds.append(&mut constraints.outlives);
        result.overflows.append(&mut constraints.overflows);
        if !result.overflows.is_empty() { break; }
        for ty in std::mem::take(&mut constraints.dtorck_types) {
            let ty = ocx.deeply_normalize_ty(env, ty)?;
            match tcx.tys.kind(ty) {
                TyKind::Param(_) => {}
                TyKind::Alias(..) => result.kinds.push(GenericArg::Type(ty)),
                _ => if ty_set.insert(ty) { ty_stack.push((ty, depth + 1)); },
            }
        }
    }
    Ok(result)
}

impl TypeOp for ParamEnvAnd<DropckOutlives> {
    type Output = DropckOutlivesResult;
    fn fully_perform(self, infcx: &InferCtxt<'_>, prog: &Program) -> Result<TypeOpOutput<DropckOutlivesResult>, NoSolution> {
        // the trivial fast path (query.rs: DropckOutlives::try_fast_path)
        if trivial_dropck_outlives(infcx.tcx, self.value.dropped_ty) {
            return Ok(TypeOpOutput { output: DropckOutlivesResult::default(), constraints: None });
        }
        let limit = infcx.recursion_limit(prog);
        scrape_region_constraints(infcx, prog, |ocx| compute_dropck_outlives_inner(ocx, &self.param_env, self.value.dropped_ty, limit))
    }
    fn input(&self) -> type_op::TypeOpInput { type_op::TypeOpInput::DropckOutlives(self.param_env, self.value.dropped_ty) }
}
