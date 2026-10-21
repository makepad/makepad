//! Illegal moves: out of borrowed content (E0507), out of an array or slice (E0508) and out
//! of a type that implements Drop (E0509).
//! ported from N:rustc_borrowck/src/diagnostics/move_errors.rs @8d1a764 and N:borrowck_errors.rs.
//! Reduced: the grouping into one error per match is done by the checker (check.rs
//! report_move_errors); this renders the message, the "cannot move out of here" label and the
//! `Copy` note, without suggestions.

use super::{Diag, DiagCx};
use crate::borrowck::borrowck_errors::{BorrowckError, Detail, ErrorKind};
use crate::mir::*;
use crate::types::TyKind;

pub fn report_cannot_move_from(cx: &DiagCx<'_>, error: &BorrowckError) -> Diag {
    let Detail::IllegalMove { place, .. } = &error.detail else { unreachable!() };
    let span = error.span;
    // The first projection that makes the move illegal, as check.rs illegal_move finds it.
    let (index, container) = illegal_projection(cx, place).unwrap_or((place.projection.len(), cx.place_ty(place)));
    let deref_target = Place { local: place.local, projection: place.projection[..(index + 1).min(place.projection.len())].to_vec() };
    let place_ty = cx.place_ty(place);
    let place_desc = cx.describe_place(place);
    let type_str = cx.ty_string(place_ty);
    let mut err = match error.kind {
        ErrorKind::MoveBorrowedContent => {
            let deref_base = Place { local: deref_target.local, projection: deref_target.projection[..deref_target.projection.len().saturating_sub(1)].to_vec() };
            let ty = cx.place_ty(&deref_target);
            match cx.tcx.tys.kind(ty) {
                TyKind::Array(..) | TyKind::Slice(_) => {
                    let name = if matches!(cx.tcx.tys.kind(ty), TyKind::Slice(_)) { "slice" } else { "array" };
                    let mut e = Diag::new("E0508", span, format!("cannot move out of type `{}`, a non-copy {name}", cx.ty_string(ty)));
                    e.span_label(span, "cannot move out of here");
                    e
                }
                _ => {
                    let source_desc = match cx.tcx.tys.kind(cx.place_ty(&deref_base)) {
                        TyKind::Ref(true, ..) => Some("mutable reference"),
                        TyKind::Ref(false, ..) => Some("shared reference"),
                        TyKind::Ptr(..) => Some("raw pointer"),
                        _ => None,
                    };
                    let what = match (&place_desc, source_desc) {
                        (Some(d), Some(s)) => format!("`{d}` which is behind a {s}"),
                        (Some(d), None) => format!("`{d}`"),
                        (None, Some(s)) => format!("a {s}"),
                        (None, None) => "value".to_string(),
                    };
                    Diag::new("E0507", span, format!("cannot move out of {what}"))
                }
            }
        }
        ErrorKind::MoveIndexed => {
            let name = if matches!(cx.tcx.tys.kind(container), TyKind::Slice(_)) { "slice" } else { "array" };
            let mut e = Diag::new("E0508", span, format!("cannot move out of type `{}`, a non-copy {name}", cx.ty_string(container)));
            e.span_label(span, "cannot move out of here");
            e
        }
        ErrorKind::MoveDropType => {
            let mut e = Diag::new("E0509", span, format!("cannot move out of type `{}`, which implements the `Drop` trait", cx.ty_string(container)));
            e.span_label(span, "cannot move out of here");
            e
        }
        _ => Diag::new(error.code(), span, error.kind.note()),
    };
    // add_move_hints, OtherIllegalMove: why the value is not copied.
    let desc = place_desc.map(|d| format!("`{d}`")).unwrap_or_else(|| "value".to_string());
    if error.kind == ErrorKind::MoveBorrowedContent || error.kind == ErrorKind::MoveIndexed || error.kind == ErrorKind::MoveDropType {
        err.span_label(span, format!("move occurs because {desc} has type `{type_str}`, which does not implement the `Copy` trait"));
    }
    err
}

/// check.rs illegal_move: the index of the projection that cannot be moved through, and the
/// type it projects from.
fn illegal_projection(cx: &DiagCx<'_>, place: &Place) -> Option<(usize, crate::types::TyId)> {
    for (i, elem) in place.projection.iter().enumerate() {
        let base_ty = cx.prefix_ty(place, i);
        let bad = match (elem, cx.tcx.tys.kind(base_ty)) {
            (PlaceElem::Deref, TyKind::Ref(..) | TyKind::Ptr(..)) => true,
            (PlaceElem::Field(..), TyKind::Adt(def, ..)) if cx.tcx.adt_destructor(*def).is_some() => true,
            (PlaceElem::Index(_), TyKind::Array(..) | TyKind::Slice(_))
            | (PlaceElem::ConstantIndex { .. } | PlaceElem::Subslice { .. }, TyKind::Slice(_)) => true,
            _ => false,
        };
        if bad { return Some((i, base_ty)); }
    }
    None
}
