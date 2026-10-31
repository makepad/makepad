//! Assignments to and mutable borrows of places that are not mutable: E0384, E0594, E0596.
//! ported from N:rustc_borrowck/src/diagnostics/mutability_errors.rs @8d1a764 and
//! N:borrowck_errors.rs. Reduced: the message, the `reason`, the "cannot assign / borrow as
//! mutable" label and the first-assignment label; no suggestions.

use super::{Diag, DiagCx};
use crate::borrowck::borrowck_errors::{AccessKind, BorrowckError, Detail, ErrorKind};
use crate::mir::*;
use crate::types::TyKind;

pub fn report_mutability_error(cx: &DiagCx<'_>, error: &BorrowckError) -> Diag {
    let Detail::Mutability { place: access_place, the_place_err, access, location, init } = &error.detail else { unreachable!() };
    if error.kind == ErrorKind::Reassigned {
        return report_illegal_reassignment(cx, access_place, error.span, the_place_err, *init);
    }
    let span = error.span;
    let mutate = matches!(access, AccessKind::Mutate | AccessKind::Replace);
    let access_place_desc = cx.describe_any_place(access_place);

    let item_msg;
    let reason;
    match (the_place_err.projection.as_slice(), the_place_err.projection.last()) {
        ([], _) => {
            item_msg = access_place_desc;
            if access_place.projection.is_empty() {
                reason = ", as it is not declared as mutable".to_string();
            } else {
                let name = cx.local_name(the_place_err.local).unwrap_or("value");
                reason = format!(", as `{name}` is not declared as mutable");
            }
        }
        ([PlaceElem::Deref], _) if matches!(cx.body.local_decls[the_place_err.local.0 as usize].local_info, LocalInfo::StaticRef { .. }) => {
            if access_place.projection.len() == 1 {
                item_msg = format!("immutable static item {access_place_desc}");
                reason = String::new();
            } else {
                item_msg = access_place_desc;
                let LocalInfo::StaticRef { def_id, .. } = cx.body.local_decls[the_place_err.local.0 as usize].local_info else { unreachable!() };
                reason = format!(", as `{}` is an immutable static item", cx.prog.name(def_id));
            }
        }
        ([PlaceElem::Deref], _) if matches!(cx.body.local_decls[the_place_err.local.0 as usize].local_info, LocalInfo::User(BindingForm::RefForGuard(_))) => {
            item_msg = access_place_desc;
            reason = ", as it is immutable for the pattern guard".to_string();
        }
        (_, Some(PlaceElem::Deref)) => {
            let base = Place { local: the_place_err.local, projection: the_place_err.projection[..the_place_err.projection.len() - 1].to_vec() };
            let pointer_type = match cx.tcx.tys.kind(cx.place_ty(&base)) {
                TyKind::Ptr(false, _) => "a `*const` pointer".to_string(),
                TyKind::Ref(false, ..) => "a `&` reference".to_string(),
                _ => "a `&` reference".to_string(),
            };
            if let Some(desc) = cx.describe_place(access_place) {
                item_msg = format!("`{desc}`");
                reason = if mutate { format!(", which is behind {pointer_type}") } else { format!(", as it is behind {pointer_type}") };
            } else {
                item_msg = format!("data in {pointer_type}");
                reason = String::new();
            }
        }
        // A captured variable's field of the closure environment.
        (_, Some(PlaceElem::Field(..))) => {
            item_msg = access_place_desc;
            reason = ", as it is not declared as mutable".to_string();
        }
        _ => {
            item_msg = access_place_desc;
            reason = String::new();
        }
    }

    let _ = location;
    let (mut err, act) = if mutate {
        (Diag::new("E0594", span, format!("cannot assign to {item_msg}{reason}")), "assign")
    } else {
        (Diag::new("E0596", span, format!("cannot borrow {item_msg} as mutable{reason}")), "borrow as mutable")
    };
    // The label goes on the use; a binding that is the primary span (several mutable borrows
    // of one immutable binding) is labeled "not mutable".
    let decl_span = cx.body.local_decls[the_place_err.local.0 as usize].source_info.span;
    if the_place_err.projection.is_empty() && span == decl_span && !mutate {
        err.span_label(span, "not mutable");
    } else {
        err.span_label(span, format!("cannot {act}"));
    }
    err
}

/// conflict_errors.rs report_illegal_reassignment (E0384).
fn report_illegal_reassignment(cx: &DiagCx<'_>, place: &Place, span: Span, err_place: &Place, init: Option<usize>) -> Diag {
    let local = err_place.projection.is_empty().then_some(err_place.local);
    let from_arg = local.is_some_and(|l| cx.body.local_decls[l.0 as usize].kind == LocalKind::Arg);
    let assigned_span = init.map(|i| match cx.moves.inits[i].location {
        crate::mir::move_paths::InitLocation::Argument(l) => cx.body.local_decls[l.0 as usize].source_info.span,
        crate::mir::move_paths::InitLocation::Statement(at) => cx.span_at(at),
    }).unwrap_or(span);
    // If the root local is initialized immediately (everything apart from `let PATTERN;`) the
    // error refers to that local, rather than the place being assigned later.
    let (place_description, assigned_span) = match local.map(|l| &cx.body.local_decls[l.0 as usize]) {
        Some(decl) if matches!(&decl.local_info, LocalInfo::User(BindingForm::Var(VarBindingForm { opt_match_place: Some(_), .. }))) =>
            (cx.describe_any_place(err_place), decl.source_info.span),
        _ => (cx.describe_any_place(place), assigned_span),
    };
    let msg = if from_arg { "to immutable argument" } else { "twice to immutable variable" };
    let mut err = Diag::new("E0384", span, format!("cannot assign {msg} {place_description}"));
    if span != assigned_span && !from_arg {
        err.span_label(assigned_span, format!("first assignment to {place_description}"));
    }
    err.span_label(span, if from_arg { "cannot assign to immutable argument" } else { "cannot assign twice to immutable variable" });
    err
}
