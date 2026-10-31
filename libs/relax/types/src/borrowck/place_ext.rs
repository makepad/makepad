// ported from N:rustc_borrowck/src/place_ext.rs @8d1a764
use super::borrow_set::LocalsStateAtExit;
use crate::mir::{Body, LocalInfo, Mutability, Place, PlaceElem, PlaceTy};
use crate::tcx::Tcx;
use crate::types::TyKind;

pub fn ignore_borrow(place: &Place, tcx: &Tcx, body: &Body, state: &LocalsStateAtExit) -> bool {
    let decl = &body.local_decls[place.local.0 as usize];
    if let LocalsStateAtExit::SomeAreInvalidated { has_storage_dead_or_moved } = state {
        if !has_storage_dead_or_moved.contains(place.local.0 as usize) && decl.mutability == Mutability::Not {
            return true;
        }
    }
    for (i, elem) in place.projection.iter().enumerate() {
        if *elem == PlaceElem::Deref {
            let ty = place.projection[..i]
                .iter()
                .try_fold(PlaceTy::from_ty(decl.ty), |ty, elem| ty.projection_ty(tcx, elem))
                .expect("invalid borrow place");
            match tcx.tys.kind(ty.ty) {
                TyKind::Ref(false, _, _) if i == 0 && matches!(decl.local_info, LocalInfo::StaticRef { is_thread_local: true, .. }) => {}
                TyKind::Ptr(..) | TyKind::Ref(false, _, _) => return true,
                _ => {}
            }
        }
    }
    false
}
