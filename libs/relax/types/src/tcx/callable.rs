//! Callable sugar is a trait reference plus an Output projection, as in
//! rustc_type_ir/src/predicate.rs:128,906.
use crate::types::AliasTyKind;
use crate::types::{AliasTy};
use super::*;

/// Declaration signature collected alongside its selection predicates.
#[derive(Clone, Debug)]
pub struct CallableSignature {
    pub param: u32,
    pub trait_def: DefId,
    pub signature: TyId,
}

impl Tcx {
    pub fn callable_trait(&self, def: DefId) -> bool {
        matches!(self.lang_items.by_def.get(&def), Some(LangItem::Fn | LangItem::FnMut | LangItem::FnOnce | LangItem::AsyncFn | LangItem::AsyncFnMut | LangItem::AsyncFnOnce))
    }

    pub fn async_callable_trait(&self, def: DefId) -> bool {
        matches!(self.lang_items.by_def.get(&def), Some(LangItem::AsyncFn | LangItem::AsyncFnMut | LangItem::AsyncFnOnce))
    }

    pub fn callable_output(&self, def: DefId) -> Option<DefId> {
        let item = match self.lang_items.by_def.get(&def) {
            Some(LangItem::AsyncFn | LangItem::AsyncFnMut | LangItem::AsyncFnOnce) => LangItem::AsyncFnOnceOutput,
            Some(LangItem::Fn | LangItem::FnMut | LangItem::FnOnce) => LangItem::FnOnceOutput,
            _ => return None,
        };
        self.lang_items.get(item)
    }

    pub fn callable_signatures(&self, prog: &crate::program::Program, item: DefId) -> Vec<CallableSignature> {
        let Some(sig) = self.fn_item_sig(prog, item) else { return vec![]; };
        sig.region_contract.callable_signatures.iter().filter_map(|bound| {
            let mut bound = bound.clone();
            if self.async_callable_trait(bound.trait_def) {
                let future = self.lang_items.get(LangItem::CallOnceFuture)?;
                let TyKind::FnPtr(ps, _, header, n) = self.tys.kind(bound.signature) else { return None; };
                let args = vec![self.tys.intern(TyKind::Param(bound.param)), self.tys.intern(TyKind::Tuple(ps.clone()))];
                let ret = self.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: future }, args: self.tys.mk_list(&args), regions: RegionArgsId::EMPTY }));
                bound.signature = self.tys.intern(TyKind::FnPtr(ps.clone(), ret, *header, *n));
            }
            Some(bound)
        }).collect()
    }

}
