// ported from N:rustc_type_ir/src/ty_kind/closure.rs @8d1a764
use super::{TyId, TyKind, Types};
use crate::upvar::ClosureKind;

/// Concrete ClosureArgsParts. The parent FnDef carries its complete generic args
/// and identifies the typeck root; nested closures share it (EXCEPTION(N4)).
/// ty_kind/closure.rs:118-126 stores a RustCall tupled signature in nightly.
/// Relax stores untupled inputs; Instance::resolve_closure tuples them at the
/// call-trait boundary, with binders retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClosureArgs {
    pub parent_args: TyId,
    pub closure_kind_ty: TyId,
    pub closure_sig_as_fn_ptr_ty: TyId,
    pub tupled_upvars_ty: TyId,
}
impl ClosureArgs {
    pub fn kind_ty(self) -> TyId { self.closure_kind_ty }
    pub fn tupled_upvars_ty(self) -> TyId { self.tupled_upvars_ty }
    pub fn sig_as_fn_ptr_ty(self) -> TyId { self.closure_sig_as_fn_ptr_ty }
}
impl Types {
    // rustc_middle/src/ty/sty.rs:1902-1930.
    pub fn from_closure_kind(&self, kind: ClosureKind) -> TyId {
        self.prim(match kind { ClosureKind::Fn => crate::program::Prim::I8,
            ClosureKind::FnMut => crate::program::Prim::I16, ClosureKind::FnOnce => crate::program::Prim::I32 })
    }
    pub fn to_opt_closure_kind(&self, ty: TyId) -> Option<ClosureKind> {
        match self.kind(ty) {
            TyKind::Int(crate::program::Prim::I8) => Some(ClosureKind::Fn),
            TyKind::Int(crate::program::Prim::I16) => Some(ClosureKind::FnMut),
            TyKind::Int(crate::program::Prim::I32) => Some(ClosureKind::FnOnce),
            TyKind::Infer(_) | TyKind::Bound(..) | TyKind::Placeholder(_) | TyKind::Param(_) => None,
            TyKind::Error => Some(ClosureKind::Fn),
            _ => panic!("invalid closure kind"),
        }
    }
}
