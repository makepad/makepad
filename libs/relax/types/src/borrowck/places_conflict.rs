// ported from N:rustc_borrowck/src/places_conflict.rs @8d1a764
// STOP: PhantomDeref and UnwrapUnsafeBinder rules need arch's MIR variants.
use crate::mir::{Body, BorrowKind, FakeBorrowKind, MutBorrowKind, Place, PlaceElem, PlaceTy};
use crate::tcx::Tcx;
use crate::types::{TyId, TyKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceConflictBias {
    Overlap,
    NoOverlap,
}

// ported from N:rustc_borrowck/src/lib.rs @8d1a764 (check-loop vocabulary)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtificialField {
    ArrayLength,
    FakeBorrow,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessDepth {
    Shallow(Option<ArtificialField>),
    Deep,
    Drop,
}
enum Overlap {
    Arbitrary,
    EqualOrDisjoint,
    Disjoint,
}

pub fn places_conflict(tcx: &Tcx, body: &Body, borrow_place: &Place, access_place: &Place, bias: PlaceConflictBias) -> bool {
    borrow_conflicts_with_place(
        tcx,
        body,
        borrow_place,
        BorrowKind::Mut { kind: MutBorrowKind::TwoPhaseBorrow },
        access_place,
        AccessDepth::Deep,
        bias,
    )
}

pub fn borrow_conflicts_with_place(
    tcx: &Tcx,
    body: &Body,
    borrow_place: &Place,
    borrow_kind: BorrowKind,
    access_place: &Place,
    access: AccessDepth,
    bias: PlaceConflictBias,
) -> bool {
    if borrow_place.local != access_place.local {
        return false;
    }
    if borrow_place.projection.is_empty() && access_place.projection.is_empty() {
        return true;
    }
    place_components_conflict(tcx, body, borrow_place, borrow_kind, access_place, access, bias)
}

fn place_components_conflict(
    tcx: &Tcx,
    body: &Body,
    borrow_place: &Place,
    borrow_kind: BorrowKind,
    access_place: &Place,
    access: AccessDepth,
    bias: PlaceConflictBias,
) -> bool {
    use AccessDepth::*;
    use PlaceElem::*;
    assert_eq!(borrow_place.local, access_place.local);
    // Like PlaceRef::ty in nightly, type only the prefixes the rule actually needs.
    // Typing a final subslice eagerly would require evaluating an unused array length.
    let base_ty = |len| {
        borrow_place.projection[..len]
            .iter()
            .try_fold(PlaceTy::from_ty(body.local_decls[borrow_place.local.0 as usize].ty), |ty, elem| ty.projection_ty(tcx, elem))
            .expect("invalid borrow place")
            .ty
    };
    // Equal-or-disjoint projections are compared in lockstep; union overlap terminates early.
    for (i, (borrow_c, access_c)) in borrow_place.projection.iter().zip(&access_place.projection).enumerate() {
        match place_projection_conflict(tcx, || base_ty(i), borrow_c, access_c, bias) {
            Overlap::Arbitrary => return true,
            Overlap::Disjoint => return false,
            Overlap::EqualOrDisjoint => {}
        }
    }
    for (i, elem) in borrow_place.projection.iter().enumerate().skip(access_place.projection.len()) {
        match (elem, tcx.tys.kind(base_ty(i)), access) {
            (_, _, Shallow(Some(ArtificialField::ArrayLength | ArtificialField::FakeBorrow))) => return false,
            (Deref, _, Shallow(None)) => return false,
            (Deref, TyKind::Ref(false, _, _), _) => panic!("tracking borrow behind shared reference"),
            (Deref, TyKind::Ref(true, _, _), Drop) => return false,
            (Field(..), TyKind::Adt(def, _, _), Drop) if tcx.adt_destructor(*def).is_some() => return true,
            (Subtype(_), _, _) => panic!("Subtype is not supported in borrowck MIR"),
            _ => {}
        }
    }
    !(borrow_kind == BorrowKind::Fake(FakeBorrowKind::Shallow) && borrow_place.projection.len() < access_place.projection.len())
}

fn place_projection_conflict(
    tcx: &Tcx,
    base_ty: impl FnOnce() -> TyId,
    pi1_elem: &PlaceElem,
    pi2_elem: &PlaceElem,
    bias: PlaceConflictBias,
) -> Overlap {
    use Overlap::*;
    use PlaceElem::*;
    match (pi1_elem, pi2_elem) {
        (Deref, Deref) | (OpaqueCast(_), OpaqueCast(_)) => EqualOrDisjoint,
        (Field(f1, _), Field(f2, _)) => {
            if f1 == f2 {
                EqualOrDisjoint
            } else if matches!(tcx.tys.kind(base_ty()), TyKind::Adt(def, _, _) if tcx.adts[def].is_union()) {
                Arbitrary
            } else {
                Disjoint
            }
        }
        (Downcast(_, v1), Downcast(_, v2)) => {
            if v1 == v2 {
                EqualOrDisjoint
            } else {
                Disjoint
            }
        }
        (Index(_), Index(_) | ConstantIndex { .. } | Subslice { .. }) | (ConstantIndex { .. } | Subslice { .. }, Index(_)) => {
            if bias == PlaceConflictBias::Overlap {
                EqualOrDisjoint
            } else {
                Disjoint
            }
        }
        (ConstantIndex { offset: o1, from_end: e1, .. }, ConstantIndex { offset: o2, from_end: e2, .. }) if e1 == e2 => {
            if o1 == o2 {
                EqualOrDisjoint
            } else {
                Disjoint
            }
        }
        (
            ConstantIndex { offset: begin, min_length: m1, from_end: false },
            ConstantIndex { offset: end, min_length: m2, from_end: true },
        )
        | (
            ConstantIndex { offset: end, min_length: m1, from_end: true },
            ConstantIndex { offset: begin, min_length: m2, from_end: false },
        ) => {
            if *begin >= m1.max(m2) - end {
                EqualOrDisjoint
            } else {
                Disjoint
            }
        }
        (ConstantIndex { offset, from_end: false, .. }, Subslice { from, to, from_end: false })
        | (Subslice { from, to, from_end: false }, ConstantIndex { offset, from_end: false, .. }) => {
            if (from..to).contains(&offset) {
                EqualOrDisjoint
            } else {
                Disjoint
            }
        }
        (ConstantIndex { offset, from_end: false, .. }, Subslice { from, .. })
        | (Subslice { from, .. }, ConstantIndex { offset, from_end: false, .. }) => {
            if offset >= from {
                EqualOrDisjoint
            } else {
                Disjoint
            }
        }
        (ConstantIndex { offset, from_end: true, .. }, Subslice { to, from_end: true, .. })
        | (Subslice { to, from_end: true, .. }, ConstantIndex { offset, from_end: true, .. }) => {
            if offset > to {
                EqualOrDisjoint
            } else {
                Disjoint
            }
        }
        (Subslice { from: f1, to: t1, from_end: false }, Subslice { from: f2, to: t2, from_end: false }) => {
            if f2 >= t1 || f1 >= t2 {
                Disjoint
            } else {
                EqualOrDisjoint
            }
        }
        (Subslice { .. }, Subslice { .. }) => EqualOrDisjoint,
        _ => panic!("mismatched or unsupported projections: {pi1_elem:?} and {pi2_elem:?}"),
    }
}
