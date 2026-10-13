// ported from N:rustc_hir_typeck/src/callee.rs @8d1a764
//! Call receiver adjustments belong to the generic body, not its instances.
use super::*;
use crate::{borrowck::tyx::Tx, lang_items::LangItem, upvar::ClosureKind};

#[derive(Clone, Copy)]
pub struct CallTrait {
    pub trait_def: DefId,
    pub tupled_args: TyId,
    pub self_ty: TyId,
    pub derefs: usize,
    pub from_param: bool,
    /// FnOnce consumes a non-Copy generic receiver even if its instance is Copy.
    pub by_move: bool,
}

fn trait_order(tcx: &Tcx, td: DefId) -> Option<u8> {
    Some(match tcx.lang_items.by_def.get(&td)? {
        LangItem::Fn => 0, LangItem::FnMut => 1, LangItem::FnOnce => 2,
        LangItem::AsyncFn => 3, LangItem::AsyncFnMut => 4, LangItem::AsyncFnOnce => 5,
        _ => return None,
    })
}

impl Fcx<'_> {
    pub(super) fn record_call_trait(&mut self, e: ExprId, td: DefId, t: TyId, derefs: usize, tupled_args: TyId) {
        let from_param = matches!(self.tcx.tys.kind(t), TyKind::Param(_));
        let tx = Tx::new(self.prog, self.tcx, self.cx.typing_env, self.cx.state);
        let by_move = from_param && !tx.is_copy(t);
        self.b.call_traits.insert(e.0, CallTrait { trait_def: td, self_ty: t, tupled_args, derefs, from_param, by_move });
    }
}

impl Body {
    /// One source of receiver semantics for expr-use/upvars and borrowck. Concrete
    /// closures are resolved after their owner's capture analysis has completed.
    pub(crate) fn call_kind(&self, tx: &Tx<'_>, e: ExprId, t: TyId) -> Option<ClosureKind> {
        if let Some(selection) = self.call_traits.get(&e.0) {
            return trait_order(tx.tcx, selection.trait_def).map(|order| match order % 3 {
                0 => ClosureKind::Fn, 1 => ClosureKind::FnMut, _ => ClosureKind::FnOnce,
            });
        }
        match tx.kind(t) {
            TyKind::Closure(file, ce, args) => crate::upvar::closure_kind(tx.tcx, self, file, ce, args),
            TyKind::FnDef(..) | TyKind::FnPtr(..) => Some(ClosureKind::FnOnce),
            _ => None,
        }
    }
}

/// rustc_hir_typeck/src/callee.rs:1214-1278. Closure kind is inferred by
/// capture analysis before selecting and confirming the overloaded call.
pub(super) struct DeferredCallResolution {
    pub call_expr: ExprId,
    pub closure_ty: TyId,
    pub derefs: usize,
    pub tuple: TyId,
    pub output: TyId,
}
impl DeferredCallResolution {
    pub(super) fn resolve(self, fcx: &mut Fcx<'_>) {
        if !fcx.b.errors.is_empty() { return; }
        let ct = fcx.infcx.deeply_resolve_ignoring_regions(self.closure_ty);
        let Some((td, tuple, _, output)) = fcx.try_overloaded_call_traits(ct, None) else {
            fcx.err(fcx.ast().expr(self.call_expr).lo, "closure does not implement a call trait".into());
            return;
        };
        // callee.rs:1267-1278: demand equality; do not discard a mismatch.
        let lo = fcx.ast().expr(self.call_expr).lo;
        if !fcx.eq(tuple, self.tuple) || !fcx.eq(output, self.output) {
            fcx.err(lo, "E0308: closure call signature mismatch".into());
        }
        fcx.record_call_trait(self.call_expr, td, ct, self.derefs, tuple);
    }
}
