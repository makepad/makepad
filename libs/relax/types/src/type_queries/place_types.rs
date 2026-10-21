//! Type queries shared by capture analysis (expr_use.rs, upvar.rs), call typing
//! (typeck/call.rs) and the MIR checker, over `Tcx` + `EvalCx` only.
//! rustc_hir_typeck's ExprUseVisitor asks its FnCtxt/InferCtxt for exactly these:
//! `shallow_resolve`, `type_is_copy_modulo_regions`, `resolve_vars_if_possible`,
//! builtin deref/field projections.

use crate::lang_items::LangItem;
use crate::next_solver::evaluate_obligation::EvaluationResult;
use crate::program::{DefId, Program};
use crate::tcx::{EvalCx, Tcx};
use crate::types::{TyId, TyKind};

/// Region variables of another inference context (typeck's, which writeback keeps) cannot
/// enter `infcx`: each is replaced by a variable of `infcx` for the operation and restored
/// in its result. A variable the operation itself created is erased (a fresh slot).
pub fn with_foreign_region_vars(infcx: &crate::infer::InferCtxt<'_>, ty: TyId, f: impl FnOnce(TyId) -> Option<TyId>) -> Option<TyId> {
    use crate::types::{RegionId, RegionKind};
    let tys = &infcx.tcx.tys;
    let map = std::cell::RefCell::new(Vec::<(RegionId, RegionId)>::new());
    let local = if tys.flags(ty).contains(crate::types::TypeFlags::HAS_RE_INFER) {
        tys.map_regions(ty, &|r, _| {
            if !matches!(tys.region_kind(r), RegionKind::ReVar(_)) { return r; }
            if let Some(&(_, l)) = map.borrow().iter().find(|(f, _)| *f == r) { return l; }
            let l = infcx.next_region_var();
            map.borrow_mut().push((r, l));
            l
        }, 0)
    } else { ty };
    let out = f(local)?;
    let map = map.into_inner();
    Some(tys.map_regions(out, &|r, _| match tys.region_kind(r) {
        RegionKind::ReVar(_) => map.iter().find(|(_, l)| *l == r).map_or(RegionId::ERASED, |(f, _)| *f),
        _ => r,
    }, 0))
}

/// A type taken out of a signature can still mention the signature's late-bound regions.
/// It is normalized under that binder (nightly normalizes the whole `PolyFnSig`): wrapped
/// as the single input of a function-pointer binder and unwrapped afterwards.
pub fn in_signature_binder(tys: &crate::types::Types, ty: TyId, f: impl FnOnce(TyId) -> Option<TyId>) -> Option<TyId> {
    if !tys.has_vars_bound_at_or_above(ty, 0) { return f(ty); }
    let bound = std::cell::Cell::new(0u32);
    let outer = std::cell::Cell::new(false);
    tys.map_regions(ty, &|r, depth| {
        if let crate::types::RegionKind::ReBound(d, v) = tys.region_kind(r) {
            if d == depth { bound.set(bound.get().max(v.0 + 1)); } else if d > depth { outer.set(true); }
        }
        r
    }, 0);
    // Escaping bound types/consts or regions of an outer binder: not a signature's own binder.
    if outer.get() || bound.get() == 0 { return None; }
    let wrapped = tys.intern(TyKind::FnPtr(tys.mk_list(&[ty]), tys.unit, crate::types::FnHeader::RUST, bound.get()));
    match tys.kind(f(wrapped)?) { TyKind::FnPtr(ps, ..) => tys.list(*ps).first().copied(), _ => None }
}

impl Tcx {
    /// `infcx.shallow_resolve` then the kind: the head of a type in the body's context.
    pub fn resolved_kind(&self, cx: &EvalCx<'_>, t: TyId) -> TyKind {
        let t = cx.infcx.map_or(t, |infcx| infcx.shallow_resolve(t));
        self.tys.kind(t).clone()
    }

    /// The pointee of a builtin deref (references, raw pointers, Box).
    /// rustc_middle/src/ty/sty.rs builtin_deref.
    pub fn builtin_deref(&self, t: TyId) -> Option<TyId> {
        match self.tys.kind(t) {
            TyKind::Ref(_, i, _) | TyKind::Ptr(_, i) => Some(*i),
            TyKind::Adt(d, a, _) if !a.is_empty() && self.adts.get(d).is_some_and(|adt| adt.is_box()) => Some(self.tys.list(*a)[0]),
            _ => None,
        }
    }

    /// Field `fi` of variant `vi` of an ADT (or tuple element `fi`, or a closure's upvar), substituted.
    pub fn place_field_ty(&self, prog: &Program, cx: &EvalCx<'_>, t: TyId, vi: u32, fi: u32) -> Option<TyId> {
        match self.tys.kind(t) {
            TyKind::Tuple(v) => self.tys.list(*v).get(fi as usize).copied(),
            TyKind::Adt(d, args, _) => {
                let adt = self.adts.get(d)?;
                adt.variants.get(vi as usize)?.fields.get(fi as usize)?;
                Some(self.field_ty(prog, cx.typing_env, *d, vi, fi, self.tys.list(*args)))
            }
            TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: up, .. }) => self.place_field_ty(prog, cx, *up, 0, fi),
            _ => None,
        }
    }

    pub fn place_n_fields(&self, t: TyId, vi: u32) -> usize {
        match self.tys.kind(t) {
            TyKind::Tuple(v) => self.tys.list(*v).len(),
            TyKind::Adt(d, _, _) => self.adts.get(d).and_then(|a| a.variants.get(vi as usize)).map_or(0, |v| v.fields.len()),
            _ => 0,
        }
    }

    pub fn place_field_name(&self, prog: &Program, t: TyId, vi: u32, fi: u32) -> String {
        match self.tys.kind(t) {
            TyKind::Adt(d, _, _) => match self.adts.get(d).and_then(|a| a.variants.get(vi as usize)).and_then(|v| v.fields.get(fi as usize)) {
                Some(f) => prog.syms.str(f.name).to_string(),
                None => fi.to_string(),
            },
            _ => fi.to_string(),
        }
    }

    pub fn is_union_ty(&self, t: TyId) -> bool {
        matches!(self.tys.kind(t), TyKind::Adt(d, _, _) if self.adts.get(d).is_some_and(|a| a.is_union))
    }

    pub fn is_packed_ty(&self, t: TyId) -> bool {
        matches!(self.tys.kind(t), TyKind::Adt(d, _, _) if self.adts.get(d).is_some_and(|a| a.repr_packed != 0))
    }

    pub fn has_drop_impl(&self, t: TyId) -> bool {
        matches!(self.tys.kind(t), TyKind::Adt(d, ..) if self.adt_destructor(*d).is_some())
    }

    /// `ty: td<args>` in the typing environment, lifetimes erased: capture analysis proves
    /// trait properties modulo regions (rustc InferCtxt::type_implements_trait with the
    /// FnCtxt's param env). With the body's inference context the goal sees its variables;
    /// without one the owner's fresh context decides.
    /// rustc_trait_selection/src/traits/query/evaluate_obligation.rs:95-114.
    pub fn evaluate_trait_modulo_regions(&self, prog: &Program, cx: &EvalCx<'_>, owner: DefId, td: DefId, ty: TyId, args: &[TyId]) -> EvaluationResult {
        let tys = &self.tys;
        let mut compact = Vec::with_capacity(args.len() + 1);
        compact.push(tys.erase_regions(ty));
        for &a in args { compact.push(tys.erase_regions(a)); }
        let lifetimes = self.generic_indices_of(prog, td).lifetimes.len();
        let regions = tys.intern_region_args(&vec![crate::types::RegionId::ERASED; lifetimes]);
        let args = self.complete_args(prog, td, &compact, regions);
        let goal = crate::next_solver::Goal { param_env: cx.typing_env.param_env, predicate: crate::next_solver::collect::trait_predicate(td, args) };
        match cx.infcx {
            Some(infcx) => infcx.evaluate_obligation(prog, goal),
            None => crate::infer::InferCtxt::for_item(self, prog, cx.typing_env.mode, owner).evaluate_obligation(prog, goal),
        }
    }

    /// rustc type_is_copy_modulo_regions: only a proven Copy goal permits a copy.
    pub fn type_is_copy_modulo_regions(&self, prog: &Program, cx: &EvalCx<'_>, owner: DefId, t: TyId) -> bool {
        match self.lang_items.get(LangItem::Copy) {
            Some(td) => self.evaluate_trait_modulo_regions(prog, cx, owner, td, t, &[]).must_apply_modulo_regions(),
            None => matches!(self.resolved_kind(cx, t), TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Ref(false, _, _) | TyKind::Ptr(..) | TyKind::FnPtr(..) | TyKind::FnDef(..)),
        }
    }

    /// The normalized shape of a type taken from the body, through the native solver's
    /// Normalize type operation (type_op/normalize.rs, rustc_traits type_op.rs:41). Its
    /// variables are resolved in the body's context first (rustc expr_use_visitor.rs:
    /// resolve_vars_if_possible in the FnCtxt's own InferCtxt); a variable the body has not
    /// inferred yet leaves the type as it is. A failed operation keeps the unnormalized type
    /// (type_check/canonical.rs:205 normalize_with_category).
    pub fn normalize_in_body(&self, prog: &Program, cx: &EvalCx<'_>, owner: DefId, ty: TyId) -> TyId {
        let tys = &self.tys;
        let ty = cx.infcx.map_or(ty, |infcx| infcx.deeply_resolve_ignoring_regions(ty));
        if !tys.may_project(ty) && !tys.has_opaque(ty) { return ty; }
        if tys.has_infer(ty) { return ty; }
        let infcx = crate::infer::InferCtxt::for_item(self, prog, cx.typing_env.mode, owner);
        let param_env = cx.typing_env.param_env;
        with_foreign_region_vars(&infcx, ty, |ty| in_signature_binder(tys, ty, |value| {
            let op = crate::next_solver::type_op::ParamEnvAnd { param_env, value: crate::next_solver::type_op::Normalize { value } };
            crate::next_solver::type_op::TypeOp::fully_perform(op, &infcx, prog).map(|out| out.output).ok()
        })).unwrap_or(ty)
    }
}
