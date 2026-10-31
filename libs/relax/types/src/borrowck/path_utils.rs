// ported from N:rustc_borrowck/src/path_utils.rs @8d1a764
use super::borrow_set::{BorrowData, BorrowIndex, BorrowSet, TwoPhaseActivation};
use super::places_conflict::{borrow_conflicts_with_place, AccessDepth, PlaceConflictBias};
use crate::mir::{BasicBlock, Body, Location, Place, PlaceElem};
use crate::tcx::Tcx;
use std::ops::ControlFlow;

pub fn each_borrow_involving_path(
    tcx: &Tcx,
    body: &Body,
    access_place: (AccessDepth, &Place),
    borrow_set: &BorrowSet,
    is_candidate: impl Fn(BorrowIndex) -> bool,
    mut op: impl FnMut(BorrowIndex, &BorrowData) -> ControlFlow<()>,
) {
    let (access, place) = access_place;
    let Some(borrows) = borrow_set.borrows_on_local(place.local) else {
        return;
    };
    for &i in borrows {
        if !is_candidate(i) {
            continue;
        }
        let borrowed = &borrow_set[i];
        if borrow_conflicts_with_place(tcx, body, &borrowed.borrowed_place, borrowed.kind, place, access, PlaceConflictBias::Overlap)
            && op(i, borrowed).is_break()
        {
            return;
        }
    }
}

/// The check loop supplies its block dominators; same-block order is handled here.
pub fn is_active(block_dominates: impl Fn(BasicBlock, BasicBlock) -> bool, borrow_data: &BorrowData, location: Location) -> bool {
    let activation = match borrow_data.activation_location {
        TwoPhaseActivation::NotTwoPhase => return true,
        TwoPhaseActivation::NotActivated => return false,
        TwoPhaseActivation::ActivatedAt(loc) => loc,
    };
    let dominates = |a: Location| {
        if a.block == location.block {
            a.statement_index <= location.statement_index
        } else {
            block_dominates(a.block, location.block)
        }
    };
    if dominates(activation) {
        return true;
    }
    let reserve = borrow_data.reserve_location;
    !dominates(Location { block: reserve.block, statement_index: reserve.statement_index + 1 })
}

pub fn borrow_of_local_data(place: &Place) -> bool {
    !place.projection.iter().any(|elem| *elem == PlaceElem::Deref)
}

/// path_utils.rs `is_upvar_field_projection`: if `place` is a field of a closure/coroutine environment
/// (directly, or through the reference that a by-reference capture is), the upvar's index. This can only
/// be `self` in the current MIR, because that is the only time we directly access the fields of a closure.
pub fn is_upvar_field_projection(tcx: &Tcx, body: &Body, place: &Place) -> Option<crate::mir::FieldIdx> {
    let mut projection = &place.projection[..];
    let mut by_ref = false;
    if let Some((PlaceElem::Deref, base)) = projection.split_last() {
        projection = base;
        by_ref = true;
    }
    let (PlaceElem::Field(field, _), base) = projection.split_last()? else { return None };
    let base_ty = Place { local: place.local, projection: base.to_vec() }.ty(body, tcx)?.ty;
    (matches!(tcx.tys.kind(base_ty), crate::types::TyKind::Closure(..))
        && (!by_ref || body.upvars.get(field.0 as usize)?.is_by_ref())).then_some(*field)
}
