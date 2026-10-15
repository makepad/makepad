// ported from N:rustc_middle/src/ty/inhabitedness/mod.rs @8d1a764
// ported from N:rustc_middle/src/ty/inhabitedness/inhabited_predicate.rs @8d1a764
use crate::borrowck::infer_ctxt::BorrowckInferCtxt;
use crate::consts::Const;
use crate::types::{TyId, TyKind, AliasTyKind};

// Only apply_ignore_module is required by borrowck. NotInModule evaluates to
// false there; opaque revelation is None (inhabited_predicate.rs:74-80).
pub(crate) enum InhabitedPredicate {
    False, True, ConstIsZero(Const), GenericType(TyId),
    And(Vec<InhabitedPredicate>), Or(Vec<InhabitedPredicate>),
}
impl InhabitedPredicate {
    pub(crate) fn apply_ignore_module(self, cx: &BorrowckInferCtxt<'_>, stack: &mut Vec<TyId>) -> bool {
        match self {
            Self::False => false, Self::True => true,
            Self::ConstIsZero(c) => c.try_to_target_usize(&cx.tcx.tys).is_none_or(|n| n == 0),
            Self::And(ps) => { for p in ps { if !p.apply_ignore_module(cx, stack) { return false; } } true }
            Self::Or(ps) => { for p in ps { if p.apply_ignore_module(cx, stack) { return true; } } false }
            Self::GenericType(t) => {
                let env = crate::tcx::TypingEnv { limit: cx.infcx.recursion_limit, mode: cx.typing_mode, param_env: cx.param_env_id() };
                let normalized = cx.tcx.try_normalize_erasing_regions(cx.prog, env, t).unwrap_or(t);
                let pred = inhabited_predicate(cx, normalized, &mut Vec::new());
                if matches!(pred, Self::GenericType(_)) || stack.contains(&t) { return true; }
                stack.push(t); let result = pred.apply_ignore_module(cx, stack); stack.pop(); result
            }
        }
    }
}
pub(crate) fn inhabited_predicate(cx: &BorrowckInferCtxt<'_>, ty: TyId, stack: &mut Vec<TyId>) -> InhabitedPredicate {
    use InhabitedPredicate as P;
    if stack.contains(&ty) { return P::True; } // Invalid recursive storage is diagnosed by representability.
    let tys = &cx.tcx.tys;
    match tys.kind(ty) {
        TyKind::Never => P::False,
        TyKind::Param(_) => P::GenericType(ty),
        TyKind::Alias(_, a) => if matches!(a.kind, AliasTyKind::Opaque { .. }) { P::True } else { P::GenericType(ty) },
        TyKind::Adt(def, args, rs) => {
            let adt = &cx.tcx.adts[def];
            if adt.is_union() || (adt.is_variant_list_non_exhaustive() && cx.prog.def(*def).krate != cx.prog.def(cx.root_def_id).krate) { return P::True; }
            let args = cx.tcx.complete_args(cx.prog, *def, tys.list(*args), *rs);
            stack.push(ty);
            let mut variants = Vec::new();
            for variant in &adt.variants {
                let mut fields = Vec::new();
                for field in &variant.fields { fields.push(inhabited_predicate(cx, cx.tcx.instantiate_ty(cx.prog, *def, &args, field.ty), stack)); }
                variants.push(P::And(fields));
            }
            stack.pop(); P::Or(variants)
        }
        TyKind::Tuple(ts) => { let mut ps = Vec::new(); for &t in tys.list(*ts) { ps.push(inhabited_predicate(cx, t, stack)); } P::And(ps) }
        TyKind::Array(t, n) => match n.try_to_target_usize(tys) {
            Some(0) => P::True,
            Some(_) => inhabited_predicate(cx, *t, stack),
            None => P::Or(vec![inhabited_predicate(cx, *t, stack), P::ConstIsZero(*n)]),
        },
        _ => P::True,
    }
}
