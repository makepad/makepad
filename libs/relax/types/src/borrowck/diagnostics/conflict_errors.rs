//! Moved / uninitialized values, conflicting borrows and values that do not live long enough.
//! ported from N:rustc_borrowck/src/diagnostics/conflict_errors.rs @8d1a764 (1.97.1 where
//! they differ) and N:borrowck_errors.rs. Reduced: error code, primary span, the main
//! message and labels; no suggestions and no HIR-based rewrites.

use super::explain_borrow::{explain_why_borrow_contains_point, BorrowExplanation, LaterUseKind};
use super::region_name::{RegionName, RegionNameSource};
use super::{Diag, DiagCx, UseSpans};
use crate::borrowck::borrowck_errors::{AccessKind, BorrowInfo, BorrowckError, Detail, ErrorKind, InitAction};
use crate::borrowck::constraints::ConstraintCategory;
use crate::mir::*;
use crate::types::TyKind;

fn via(msg: &str) -> String { if msg.is_empty() { String::new() } else { format!(" (via {msg})") } }

// ---------------------------------------------------------------------------------------------
// Use of moved or uninitialized values (E0382, E0381).

pub fn report_use_of_moved_or_uninitialized(cx: &DiagCx<'_>, error: &BorrowckError) -> Diag {
    let Detail::Moved { action, moved_place, used_place, location, mpi, moves } = &error.detail else { unreachable!() };
    let use_spans = cx.move_spans(moved_place, *location).or_else(|| cx.borrow_spans(error.span, *location));
    let span = use_spans.args_or_use();
    if moves.is_empty() { return report_use_of_uninitialized(cx, *mpi, used_place, moved_place, *action, span); }

    // conflict_errors.rs:117: a move of a prefix with a Box deref is not a partial move.
    let is_partial_move = moves.iter().any(|site| {
        let move_out = cx.moves.moves[site.moi];
        let moved = &cx.moves.move_paths[move_out.path].place;
        let is_box_move = moved.projection == [PlaceElem::Deref] && cx.is_box(cx.body.local_decls[moved.local.0 as usize].ty);
        !is_box_move && used_place != moved && used_place.local == moved.local && moved.projection.starts_with(&used_place.projection)
    });
    let partial_str = if is_partial_move { "partial " } else { "" };
    let partially_str = if is_partial_move { "partially " } else { "" };

    let moved_name = cx.describe_place(moved_place).map(|n| format!(": `{n}`")).unwrap_or_default();
    let mut err = Diag::new("E0382", span, format!("{} of {}moved value{}", action.as_noun(), partially_str, moved_name));

    let mut is_loop_move = false;
    let mut seen_spans: Vec<Span> = Vec::new();
    for site in moves {
        let move_out = cx.moves.moves[site.moi];
        let moved = &cx.moves.move_paths[move_out.path].place;
        let move_spans = cx.move_spans(moved, move_out.source);
        let move_span = move_spans.args_or_use();
        let is_move_msg = move_spans.for_closure() || move_spans.for_coroutine();
        let is_loop_message = *location == move_out.source || site.traversed_back_edge;
        if *location == move_out.source { is_loop_move = true; }
        if !seen_spans.contains(&move_span) {
            // explain_captures, reduced to its labels.
            let place_name = cx.describe_place(moved);
            if is_move_msg {
                let kind = cx.closure_noun(move_spans);
                err.span_label(move_span, format!("value {}moved into {kind} here", if is_partial_move { "partially " } else { "" }));
                let _ = &place_name;
            } else if is_loop_message {
                err.span_label(move_span, format!("value {}moved{} here, in previous iteration of loop", if is_partial_move { "partially " } else { "" }, ""));
            } else {
                err.span_label(move_span, format!("value {}moved here", if is_partial_move { "partially " } else { "" }));
            }
        }
        seen_spans.push(move_span);
    }
    if !is_loop_move {
        err.span_label(span, format!("value {} here after {partial_str}move", action.as_verb_in_past_tense()));
    }

    // TypeNoCopy: the type of the first moved place.
    let mpi0 = cx.moves.moves[moves[0].moi].path;
    let place = &cx.moves.move_paths[mpi0].place;
    let ty = cx.place_ty(place);
    let note_msg = match cx.describe_place(place) { Some(n) => format!("`{n}`"), None => "value".to_owned() };
    let needs_note = !matches!(cx.tcx.tys.kind(cx.place_ty(used_place)), TyKind::Closure(..));
    if needs_note {
        let ty_s = cx.ty_string(ty);
        let partial = if is_partial_move { "partial " } else { "" };
        if place.projection.is_empty() {
            let decl_span = cx.body.local_decls[place.local.0 as usize].source_info.span;
            err.span_label(decl_span, format!("move occurs because {note_msg} has type `{ty_s}`, which does not implement the `Copy` trait"));
            let _ = partial;
        } else {
            err.note(format!("move occurs because {note_msg} has type `{ty_s}`, which does not implement the `Copy` trait"));
        }
    }
    err
}

/// conflict_errors.rs report_use_of_uninitialized, without the HIR branch analysis: the init
/// statements stand in for its `ConditionVisitor` (an init that is not the use itself means
/// "possibly-uninitialized").
fn report_use_of_uninitialized(cx: &DiagCx<'_>, mpi: usize, used_place: &Place, moved_place: &Place, action: InitAction, span: Span) -> Diag {
    let move_path = &cx.moves.move_paths[mpi];
    let decl_span = cx.body.local_decls[move_path.place.local.0 as usize].source_info.span;
    let mut spans: Vec<Span> = Vec::new();
    for &init_idx in &cx.moves.init_path_map[mpi] {
        let init = cx.moves.inits[init_idx];
        let s = match init.location {
            crate::mir::move_paths::InitLocation::Argument(l) => cx.body.local_decls[l.0 as usize].source_info.span,
            crate::mir::move_paths::InitLocation::Statement(at) => cx.span_at(at),
        };
        if s != Span::default() && !spans.contains(&s) { spans.push(s); }
    }
    let contains = |outer: Span, inner: Span| outer.file == inner.file && outer.lo <= inner.lo && inner.hi <= outer.hi;
    let (name, desc) = match cx.describe_place(moved_place) {
        Some(n) => (format!("`{n}`"), format!("`{n}` ")),
        None => ("the variable".to_string(), String::new()),
    };
    let _ = name;
    let path = match cx.describe_place(used_place) { Some(n) => format!("`{n}`"), None => "value".to_string() };
    let partial = matches!(action, InitAction::PartialAssignment | InitAction::Assignment);
    let isnt_initialized = if partial { "isn't fully initialized" }
        else if !spans.iter().any(|&i| !contains(i, span)) { "isn't initialized" } else { "is possibly-uninitialized" };
    let used = action.as_general_verb_in_past_tense();
    let mut err = Diag::new("E0381", span, format!("{used} binding {desc}{isnt_initialized}"));
    if partial {
        err.help("partial initialization isn't supported, fully initialize the binding with a default value and mutate it, or use `std::mem::MaybeUninit`");
    }
    err.span_label(span, format!("{path} {used} here but it {isnt_initialized}"));
    for sp in &spans {
        let before = (sp.file, sp.lo) < (span.file, span.lo);
        let overlaps = sp.file == span.file && sp.lo < span.hi && span.lo < sp.hi;
        if before && !overlaps { err.span_label(*sp, "binding initialized here in some conditions"); }
    }
    err.span_label(decl_span, "binding declared here but left uninitialized");
    err
}

// ---------------------------------------------------------------------------------------------
// Conflicting accesses to borrowed places.

pub fn report_conflicting_access(cx: &DiagCx<'_>, error: &BorrowckError) -> Diag {
    let Detail::Conflict { place, access, location, borrow } = &error.detail else { unreachable!() };
    match error.kind {
        ErrorKind::UseWhileMutablyBorrowed => report_use_while_mutably_borrowed(cx, *location, place, borrow),
        ErrorKind::MoveWhileBorrowed => report_move_out_while_borrowed(cx, *location, place, borrow),
        ErrorKind::AssignWhileBorrowed | ErrorKind::MutateInGuard if matches!(access, AccessKind::Mutate | AccessKind::Replace) =>
            report_illegal_mutation_of_borrowed(cx, *location, place, error.span, borrow),
        _ => {
            let gen = match access { AccessKind::Borrow(k) | AccessKind::MutableBorrow(k) => *k, _ => BorrowKind::Shared };
            report_conflicting_borrow(cx, *location, place, error.span, gen, borrow)
        }
    }
}

/// conflict_errors.rs report_move_out_while_borrowed (E0505).
fn report_move_out_while_borrowed(cx: &DiagCx<'_>, location: Location, place: &Place, borrow: &BorrowInfo) -> Diag {
    let value_msg = cx.describe_any_place(place);
    let borrow_msg = cx.describe_any_place(&borrow.borrowed_place);
    let borrow_spans = cx.retrieve_borrow_spans(borrow);
    let borrow_span = borrow_spans.args_or_use();
    let move_spans = cx.move_spans(place, location);
    let span = move_spans.args_or_use();
    let mut err = Diag::new("E0505", span, format!("cannot move out of {value_msg} because it is borrowed"));
    err.span_label(span, format!("move out of {value_msg} occurs here"));
    err.span_label(borrow_span, format!("borrow of {borrow_msg} occurs here"));
    if let UseSpans::ClosureUse { is_coroutine, capture_kind_span, .. } = move_spans {
        err.span_label(capture_kind_span, if is_coroutine { "move occurs due to use in coroutine" } else { "move occurs due to use in closure" });
    }
    explain_why_borrow_contains_point(cx, location, borrow, None).add_explanation_to_diagnostic(cx, &mut err, "", Some(borrow_span));
    err
}

/// report_use_while_mutably_borrowed (E0503).
fn report_use_while_mutably_borrowed(cx: &DiagCx<'_>, location: Location, place: &Place, borrow: &BorrowInfo) -> Diag {
    let borrow_spans = cx.retrieve_borrow_spans(borrow);
    let borrow_span = borrow_spans.args_or_use();
    let use_spans = cx.move_spans(place, location);
    let span = use_spans.var_or_use();
    let desc = cx.describe_any_place(place);
    let borrow_desc = cx.describe_any_place(&borrow.borrowed_place);
    let mut err = Diag::new("E0503", span, format!("cannot use {desc} because it was mutably borrowed"));
    err.span_label(borrow_span, format!("{borrow_desc} is borrowed here"));
    err.span_label(span, format!("use of borrowed {borrow_desc}"));
    if let UseSpans::ClosureUse { is_coroutine, capture_kind_span, .. } = borrow_spans {
        err.span_label(capture_kind_span, format!("borrow occurs due to use of {desc} in {}", if is_coroutine { "coroutine" } else { "closure" }));
    }
    explain_why_borrow_contains_point(cx, location, borrow, None).add_explanation_to_diagnostic(cx, &mut err, "", None);
    err
}

/// classify_immutable_section.
fn classify_immutable_section(cx: &DiagCx<'_>, place: &Place) -> Option<&'static str> {
    let cause = cx.body.basic_blocks.iter().flat_map(|b| &b.statements).filter_map(|s| match &s.kind {
        StatementKind::FakeRead(cause, p) if p == place => Some(*cause), _ => None,
    }).last();
    match cause { Some(FakeReadCause::ForMatchGuard) => Some("match guard"), Some(FakeReadCause::ForIndex) => Some("indexing expression"), _ => None }
}

/// report_illegal_mutation_of_borrowed (E0506 / E0510).
fn report_illegal_mutation_of_borrowed(cx: &DiagCx<'_>, location: Location, place: &Place, span: Span, loan: &BorrowInfo) -> Diag {
    let loan_spans = cx.retrieve_borrow_spans(loan);
    let loan_span = loan_spans.args_or_use();
    let descr_place = cx.describe_any_place(place);
    if let BorrowKind::Fake(_) = loan.kind {
        if let Some(section) = classify_immutable_section(cx, &loan.assigned_place) {
            return cannot_mutate_in_immutable_section(span, loan_span, &descr_place, section, "assign");
        }
    }
    let mut err = Diag::new("E0506", span, format!("cannot assign to {descr_place} because it is borrowed"));
    err.span_label(loan_span, format!("{descr_place} is borrowed here"));
    err.span_label(span, format!("{descr_place} is assigned to here but it was already borrowed"));
    if let UseSpans::ClosureUse { is_coroutine, capture_kind_span, .. } = loan_spans {
        err.span_label(capture_kind_span, if is_coroutine { "borrow occurs due to use in coroutine" } else { "borrow occurs due to use in closure" });
    }
    explain_why_borrow_contains_point(cx, location, loan, None).add_explanation_to_diagnostic(cx, &mut err, "", None);
    err
}

fn cannot_mutate_in_immutable_section(mutate_span: Span, immutable_span: Span, immutable_place: &str, immutable_section: &str, action: &str) -> Diag {
    let mut err = Diag::new("E0510", mutate_span, format!("cannot {action} {immutable_place} in {immutable_section}"));
    err.span_label(mutate_span, format!("cannot {action}"));
    err.span_label(immutable_span, format!("value is immutable in {immutable_section}"));
    err
}

/// report_conflicting_borrow (E0499, E0502, E0500, E0501, E0524, E0510).
fn report_conflicting_borrow(cx: &DiagCx<'_>, location: Location, place: &Place, span: Span, gen_borrow_kind: BorrowKind, issued: &BorrowInfo) -> Diag {
    let issued_spans = cx.retrieve_borrow_spans(issued);
    let issued_span = issued_spans.args_or_use();
    let borrow_spans = cx.borrow_spans(span, location);
    let span = borrow_spans.args_or_use();
    let container_name = if issued_spans.for_coroutine() || borrow_spans.for_coroutine() { "coroutine" } else { "closure" };

    // describe_place_for_conflicting_borrow: union fields are described by the union.
    let (desc_place, msg_place, msg_borrow, union_type_name) = describe_place_for_conflicting_borrow(cx, place, &issued.borrowed_place);

    let explanation = explain_why_borrow_contains_point(cx, location, issued, None);
    let second_borrow_desc = if explanation.is_explained() { "second " } else { "" };

    use BorrowKind::*;
    use MutBorrowKind::*;
    let first_borrow_desc;
    let mut err = match (gen_borrow_kind, issued.kind) {
        (Shared | Fake(FakeBorrowKind::Deep), Mut { kind: Default | TwoPhaseBorrow }) => {
            first_borrow_desc = "mutable ";
            cannot_reborrow_already_borrowed(span, &desc_place, &msg_place, "immutable", issued_span, "it", "mutable", &msg_borrow)
        }
        (Mut { kind: Default | TwoPhaseBorrow }, Shared | Fake(FakeBorrowKind::Deep)) => {
            first_borrow_desc = "immutable ";
            cannot_reborrow_already_borrowed(span, &desc_place, &msg_place, "mutable", issued_span, "it", "immutable", &msg_borrow)
        }
        (Mut { kind: Default | TwoPhaseBorrow }, Mut { kind: Default | TwoPhaseBorrow }) => {
            first_borrow_desc = "first ";
            cannot_mutably_borrow_multiply(span, &desc_place, &msg_place, issued_span, &msg_borrow)
        }
        (Mut { kind: ClosureCapture }, Mut { kind: ClosureCapture }) => {
            first_borrow_desc = "first ";
            cannot_uniquely_borrow_by_two_closures(span, &desc_place, issued_span)
        }
        (Mut { .. }, Fake(FakeBorrowKind::Shallow)) => {
            if let Some(section) = classify_immutable_section(cx, &issued.assigned_place) {
                return cannot_mutate_in_immutable_section(span, issued_span, &desc_place, section, "mutably borrow");
            }
            first_borrow_desc = "immutable ";
            cannot_reborrow_already_borrowed(span, &desc_place, &msg_place, "mutable", issued_span, "it", "immutable", &msg_borrow)
        }
        (Mut { kind: ClosureCapture }, _) => {
            first_borrow_desc = "first ";
            cannot_uniquely_borrow_by_one_closure(span, container_name, &desc_place, "", issued_span, "it", "")
        }
        (Shared | Fake(FakeBorrowKind::Deep), Mut { kind: ClosureCapture }) => {
            first_borrow_desc = "first ";
            cannot_reborrow_already_uniquely_borrowed(span, container_name, &desc_place, "", "immutable", issued_span, "", second_borrow_desc)
        }
        (Mut { .. }, Mut { kind: ClosureCapture }) => {
            first_borrow_desc = "first ";
            cannot_reborrow_already_uniquely_borrowed(span, container_name, &desc_place, "", "mutable", issued_span, "", second_borrow_desc)
        }
        (Shared | Fake(FakeBorrowKind::Deep), Shared | Fake(_)) | (Fake(FakeBorrowKind::Shallow), _) => {
            // The checker only reports conflicts between a mutable and another borrow.
            first_borrow_desc = "";
            Diag::new("E0502", span, "conflicting borrows")
        }
    };
    if issued_spans == borrow_spans {
        if let UseSpans::ClosureUse { is_coroutine, capture_kind_span, .. } = borrow_spans {
            err.span_label(capture_kind_span, format!("borrow occurs due to use of {desc_place} in {}", if is_coroutine { "coroutine" } else { "closure" }));
        }
    } else {
        if let UseSpans::ClosureUse { is_coroutine, capture_kind_span, .. } = issued_spans {
            let bp = cx.describe_any_place(&issued.borrowed_place);
            err.span_label(capture_kind_span, format!("first borrow occurs due to use of {bp} in {}", if is_coroutine { "coroutine" } else { "closure" }));
        }
        if let UseSpans::ClosureUse { is_coroutine, capture_kind_span, .. } = borrow_spans {
            err.span_label(capture_kind_span, format!("second borrow occurs due to use of {desc_place} in {}", if is_coroutine { "coroutine" } else { "closure" }));
        }
    }
    if !union_type_name.is_empty() {
        err.note(format!("{msg_place} is a field of the union `{union_type_name}`, so it overlaps the field {msg_borrow}"));
    }
    explanation.add_explanation_to_diagnostic(cx, &mut err, first_borrow_desc, None);
    err
}

/// conflict_errors.rs describe_place_for_conflicting_borrow: (description, first message,
/// second message, union name). Fields of a union are described through the union.
fn describe_place_for_conflicting_borrow(cx: &DiagCx<'_>, first: &Place, second: &Place) -> (String, String, String, String) {
    let tys = &cx.tcx.tys;
    // The first union field projection along both places, comparing prefixes.
    let union_field = |place: &Place| -> Option<(usize, Place)> {
        for (i, elem) in place.projection.iter().enumerate() {
            if matches!(elem, PlaceElem::Field(..)) {
                let base = Place { local: place.local, projection: place.projection[..i].to_vec() };
                if let TyKind::Adt(d, ..) = tys.kind(cx.place_ty(&base)) {
                    if cx.tcx.adts[d].is_union { return Some((i, base)); }
                }
            }
        }
        None
    };
    if let (Some((i1, b1)), Some((i2, b2))) = (union_field(first), union_field(second)) {
        if b1 == b2 && first.projection.get(i1) != second.projection.get(i2) {
            let name = |p: &Place, i: usize| {
                let base = cx.describe_place(&Place { local: p.local, projection: p.projection[..i].to_vec() }).unwrap_or_default();
                let PlaceElem::Field(f, _) = p.projection[i] else { unreachable!() };
                let ty = cx.place_ty(&Place { local: p.local, projection: p.projection[..i].to_vec() });
                let fname = cx.tcx.place_field_name(cx.prog, ty, 0, f.0);
                format!("`{base}.{fname}`")
            };
            let union_ty = cx.ty_string(cx.place_ty(&b1));
            let desc = cx.describe_place(&b1).unwrap_or_default();
            return (desc, name(first, i1), name(second, i2), union_ty);
        }
    }
    (cx.describe_place(first).map(|n| format!("`{n}`")).unwrap_or_else(|| "value".into()), String::new(), String::new(), String::new())
}

// borrowck_errors.rs constructors.
fn cannot_mutably_borrow_multiply(new_loan_span: Span, desc: &str, opt_via: &str, old_loan_span: Span, old_opt_via: &str) -> Diag {
    let mut err = Diag::new("E0499", new_loan_span, format!("cannot borrow {desc}{} as mutable more than once at a time", via(opt_via)));
    if old_loan_span == new_loan_span {
        err.span_label(new_loan_span, format!("{desc}{} was mutably borrowed here in the previous iteration of the loop{opt_via}", via(opt_via)));
    } else {
        err.span_label(old_loan_span, format!("first mutable borrow occurs here{}", via(old_opt_via)));
        err.span_label(new_loan_span, format!("second mutable borrow occurs here{}", via(opt_via)));
    }
    err
}
fn cannot_uniquely_borrow_by_two_closures(new_loan_span: Span, desc: &str, old_loan_span: Span) -> Diag {
    let mut err = Diag::new("E0524", new_loan_span, format!("two closures require unique access to {desc} at the same time"));
    if old_loan_span == new_loan_span {
        err.span_label(old_loan_span, "closures are constructed here in different iterations of loop");
    } else {
        err.span_label(old_loan_span, "first closure is constructed here");
        err.span_label(new_loan_span, "second closure is constructed here");
    }
    err
}
fn cannot_uniquely_borrow_by_one_closure(new_loan_span: Span, container_name: &str, desc_new: &str, opt_via: &str, old_loan_span: Span, noun_old: &str, old_opt_via: &str) -> Diag {
    let mut err = Diag::new("E0500", new_loan_span, format!("closure requires unique access to {desc_new} but {noun_old} is already borrowed{old_opt_via}"));
    err.span_label(new_loan_span, format!("{container_name} construction occurs here{opt_via}"));
    err.span_label(old_loan_span, format!("borrow occurs here{old_opt_via}"));
    err
}
fn cannot_reborrow_already_uniquely_borrowed(new_loan_span: Span, container_name: &str, desc_new: &str, opt_via: &str, kind_new: &str, old_loan_span: Span, old_opt_via: &str, second_borrow_desc: &str) -> Diag {
    let mut err = Diag::new("E0501", new_loan_span, format!("cannot borrow {desc_new}{opt_via} as {kind_new} because previous closure requires unique access"));
    err.span_label(new_loan_span, format!("{second_borrow_desc}borrow occurs here{opt_via}"));
    err.span_label(old_loan_span, format!("{container_name} construction occurs here{old_opt_via}"));
    err
}
fn cannot_reborrow_already_borrowed(span: Span, desc_new: &str, msg_new: &str, kind_new: &str, old_span: Span, noun_old: &str, kind_old: &str, msg_old: &str) -> Diag {
    let mut err = Diag::new("E0502", span, format!("cannot borrow {desc_new}{} as {kind_new} because {noun_old} is also borrowed as {kind_old}{}", via(msg_new), via(msg_old)));
    if msg_new.is_empty() {
        err.span_label(span, format!("{kind_new} borrow occurs here"));
        err.span_label(old_span, format!("{kind_old} borrow occurs here"));
    } else {
        err.span_label(span, format!("{kind_new} borrow of {msg_new} -- which overlaps with {msg_old} -- occurs here"));
        err.span_label(old_span, format!("{kind_old} borrow occurs here{}", via(msg_old)));
    }
    err
}

// ---------------------------------------------------------------------------------------------
// Values that do not live long enough (E0597, E0716, E0713, E0712, E0515, E0373, E0521).

pub fn report_borrowed_value_does_not_live_long_enough(cx: &DiagCx<'_>, error: &BorrowckError) -> Diag {
    let Detail::Lifetime { borrow, dropped_place, location } = &error.detail else { unreachable!() };
    let drop_span = cx.span_at(*location);
    let borrowed_local = borrow.borrowed_place.local;
    let borrow_spans = cx.retrieve_borrow_spans(borrow);
    let borrow_span = borrow_spans.var_or_use_path_span();
    let proper_span = cx.body.local_decls[borrowed_local.0 as usize].source_info.span;

    if matches!(error.kind, ErrorKind::ThreadLocalEscapes) {
        let mut err = Diag::new("E0712", borrow_span, "thread-local variable borrowed past end of function");
        err.span_label(borrow_span, "thread-local variables cannot be borrowed beyond the end of the function");
        err.span_label(drop_span, "end of enclosing function is here");
        return err;
    }
    if matches!(error.kind, ErrorKind::BorrowAcrossDestructor) {
        return report_borrow_conflicts_with_destructor(cx, *location, borrow, dropped_place, drop_span);
    }

    let place_desc = cx.describe_place(&borrow.borrowed_place);
    let kind_place = place_desc.as_ref().map(|_| (AccessKind::StorageDeadOrDrop, dropped_place));
    let explanation = explain_why_borrow_contains_point(cx, *location, borrow, kind_place);

    match (place_desc, explanation) {
        // A closure that captured the local escapes through a use that is a closure capture.
        (Some(name), BorrowExplanation::UsedLater(_, LaterUseKind::ClosureCapture, var_or_use_span, _))
            if borrow_spans.for_coroutine() || borrow_spans.for_closure() => {
            let static_name = RegionName { name: format!("'{}", cx.region_counter.replace(cx.region_counter.get() + 1)), source: RegionNameSource::Static };
            report_escaping_closure_capture(cx, borrow_spans, borrow_span, &static_name, ConstraintCategory::CallArgument(None), var_or_use_span, &format!("`{name}`"), "block")
        }
        (Some(name), BorrowExplanation::MustBeValidFor { category: category @ (ConstraintCategory::Return(_) | ConstraintCategory::CallArgument(_) | ConstraintCategory::OpaqueType), from_closure: false, ref region_name, span, .. })
            if borrow_spans.for_coroutine() || borrow_spans.for_closure() =>
            report_escaping_closure_capture(cx, borrow_spans, borrow_span, region_name, category, span, &format!("`{name}`"), "function"),
        (name, BorrowExplanation::MustBeValidFor { category: ConstraintCategory::Assignment, from_closure: false,
            region_name: RegionName { source: RegionNameSource::AnonRegionFromUpvar(upvar_span, upvar_name), .. }, span, .. }) =>
            report_escaping_data(cx, borrow_span, &name, upvar_span, &upvar_name, span),
        (Some(name), explanation) => report_local_value_does_not_live_long_enough(cx, &name, borrow, drop_span, borrow_spans, explanation),
        (None, explanation) => report_temporary_value_does_not_live_long_enough(cx, borrow, drop_span, proper_span, borrow_spans, explanation),
    }
}

fn report_local_value_does_not_live_long_enough(cx: &DiagCx<'_>, name: &str, borrow: &BorrowInfo, drop_span: Span, borrow_spans: UseSpans, explanation: BorrowExplanation) -> Diag {
    let borrow_span = borrow_spans.var_or_use_path_span();
    if let BorrowExplanation::MustBeValidFor { category, span, ref opt_place_desc, from_closure: false, .. } = explanation {
        if let Err(diag) = try_report_cannot_return_reference_to_local(cx, borrow, borrow_span, span, category, opt_place_desc.as_ref()) { return diag; }
    }
    let name = format!("`{name}`");
    let mut err = Diag::new("E0597", borrow_span, format!("{name} does not live long enough"));
    // conflict_errors.rs: the declaration of the borrowed binding is labeled by the explanation.
    err.span_label(cx.body.local_decls[borrow.borrowed_place.local.0 as usize].source_info.span, format!("binding {name} declared here"));
    err.span_label(borrow_span, "borrowed value does not live long enough");
    err.span_label(drop_span, format!("{name} dropped here while still borrowed"));
    if let UseSpans::ClosureUse { is_coroutine, args_span, .. } = borrow_spans {
        err.span_label(args_span, if is_coroutine { "value captured here by coroutine" } else { "value captured here" });
    }
    explanation.add_explanation_to_diagnostic(cx, &mut err, "", Some(borrow_span));
    err
}

fn report_temporary_value_does_not_live_long_enough(cx: &DiagCx<'_>, borrow: &BorrowInfo, drop_span: Span, proper_span: Span, borrow_spans: UseSpans, explanation: BorrowExplanation) -> Diag {
    if let BorrowExplanation::MustBeValidFor { category, span, from_closure: false, .. } = explanation {
        if let Err(diag) = try_report_cannot_return_reference_to_local(cx, borrow, proper_span, span, category, None) { return diag; }
    }
    let mut err = Diag::new("E0716", proper_span, "temporary value dropped while borrowed");
    err.span_label(proper_span, "creates a temporary value which is freed while still in use");
    err.span_label(drop_span, "temporary value is freed at the end of this statement");
    if matches!(explanation, BorrowExplanation::UsedLater(..) | BorrowExplanation::UsedLaterInLoop(..) | BorrowExplanation::UsedLaterWhenDropped { .. }) {
        err.note("consider using a `let` binding to create a longer lived value");
    }
    explanation.add_explanation_to_diagnostic(cx, &mut err, "", None);
    if let UseSpans::ClosureUse { is_coroutine, args_span, .. } = borrow_spans {
        err.span_label(args_span, if is_coroutine { "value captured here by coroutine" } else { "value captured here" });
    }
    err
}

fn report_borrow_conflicts_with_destructor(cx: &DiagCx<'_>, location: Location, borrow: &BorrowInfo, place: &Place, drop_span: Span) -> Diag {
    let borrow_spans = cx.retrieve_borrow_spans(borrow);
    let borrow_span = borrow_spans.var_or_use();
    let mut err = Diag::new("E0713", borrow_span, "borrow may still be in use when destructor runs");
    let what_was_dropped = match cx.describe_place(place) { Some(n) => format!("`{n}`"), None => String::from("temporary value") };
    let ty = cx.ty_string(cx.place_ty(place));
    let label = match cx.describe_place(&borrow.borrowed_place) {
        Some(borrowed) => format!("here, drop of {what_was_dropped} needs exclusive access to `{borrowed}`, because the type `{ty}` implements the `Drop` trait"),
        None => format!("here is drop of {what_was_dropped}; whose type `{ty}` implements the `Drop` trait"),
    };
    err.span_label(drop_span, label);
    let explanation = explain_why_borrow_contains_point(cx, location, borrow, Some((AccessKind::StorageDeadOrDrop, place)));
    if matches!(explanation, BorrowExplanation::UsedLater(..) | BorrowExplanation::UsedLaterWhenDropped { .. }) {
        err.note("consider using a `let` binding to create a longer lived value");
    }
    explanation.add_explanation_to_diagnostic(cx, &mut err, "", None);
    err
}

/// try_report_cannot_return_reference_to_local (E0515).
fn try_report_cannot_return_reference_to_local(cx: &DiagCx<'_>, borrow: &BorrowInfo, borrow_span: Span, return_span: Span, category: ConstraintCategory, opt_place_desc: Option<&String>) -> Result<(), Diag> {
    let return_kind = match category { ConstraintCategory::Return(_) => "return", ConstraintCategory::Yield => "yield", _ => return Ok(()) };
    let reference_desc = if return_span == cx.span_at(borrow.reserve_location) { "reference to" } else { "value referencing" };
    let local = borrow.borrowed_place.local;
    let decl = &cx.body.local_decls[local.0 as usize];
    let (place_desc, note) = if let Some(place_desc) = opt_place_desc {
        let local_kind = if borrow.borrowed_place.projection.is_empty() {
            match decl.kind {
                LocalKind::UserVar | LocalKind::Temp => "local variable ",
                LocalKind::Arg => "function parameter ",
                LocalKind::Return => "local variable ",
            }
        } else { "local data " };
        (format!("{local_kind}`{place_desc}`"), format!("`{place_desc}` is borrowed here"))
    } else {
        match decl.kind {
            LocalKind::Arg => ("function parameter".to_string(), "function parameter borrowed here".to_string()),
            LocalKind::UserVar => ("local binding".to_string(), "local binding introduced here".to_string()),
            LocalKind::Return | LocalKind::Temp => ("temporary value".to_string(), "temporary value created here".to_string()),
        }
    };
    let mut err = Diag::new("E0515", return_span, format!("cannot {return_kind} {reference_desc} {place_desc}"));
    err.span_label(return_span, format!("{return_kind}s a {reference_desc} data owned by the current function"));
    if return_span != borrow_span { err.span_label(borrow_span, note); }
    Err(err)
}

/// report_escaping_closure_capture (E0373).
fn report_escaping_closure_capture(cx: &DiagCx<'_>, use_span: UseSpans, var_span: Span, fr_name: &RegionName, category: ConstraintCategory,
    constraint_span: Span, captured_var: &str, scope: &str) -> Diag {
    let args_span = use_span.args_or_use();
    let kind = cx.closure_noun(use_span);
    let mut err = Diag::new("E0373", args_span, format!("{kind} may outlive the current {scope}, but it borrows {captured_var}, which is owned by the current {scope}"));
    err.span_label(var_span, format!("{captured_var} is borrowed here"));
    err.span_label(args_span, format!("may outlive borrowed value {captured_var}"));
    err.help(format!("to force the {kind} to take ownership of {captured_var} (and any other referenced variables), use the `move` keyword"));
    match category {
        ConstraintCategory::Return(_) | ConstraintCategory::OpaqueType => {
            err.note(format!("{kind} is returned here"));
            err.span_label(constraint_span, format!("{kind} is returned here"));
        }
        ConstraintCategory::CallArgument(_) => {
            fr_name.highlight(&mut err);
            err.note(format!("{scope} requires argument type to outlive `{fr_name}`"));
            err.span_label(constraint_span, format!("{scope} requires argument type to outlive `{fr_name}`"));
        }
        _ => {}
    }
    let _ = cx;
    err
}

/// report_escaping_data (E0521).
fn report_escaping_data(cx: &DiagCx<'_>, borrow_span: Span, name: &Option<String>, upvar_span: Span, upvar_name: &str, escape_span: Span) -> Diag {
    let escapes_from = cx.names.def_descr;
    let mut err = Diag::new("E0521", escape_span, format!("borrowed data escapes outside of {escapes_from}"));
    err.span_label(upvar_span, format!("`{upvar_name}` declared here, outside of the {escapes_from} body"));
    err.span_label(borrow_span, format!("borrow is only valid in the {escapes_from} body"));
    match name {
        Some(name) => err.span_label(escape_span, format!("reference to `{name}` escapes the {escapes_from} body here")),
        None => err.span_label(escape_span, format!("reference escapes the {escapes_from} body here")),
    };
    err
}

/// borrowck_errors.rs cannot_borrow_across_coroutine_yield (E0626).
pub fn report_borrow_across_yield(cx: &DiagCx<'_>, error: &BorrowckError) -> Diag {
    let Detail::BorrowAcrossYield { borrow, yield_location } = &error.detail else { unreachable!() };
    let borrow_span = cx.retrieve_borrow_spans(borrow).args_or_use();
    let kind = cx.body.coroutine.as_ref().map_or("coroutine", |c| super::coroutine_noun(c.kind));
    let mut err = Diag::new("E0626", borrow_span, format!("borrow may still be in use when {kind} yields"));
    err.span_label(cx.body.source_scopes.first().map_or(borrow_span, |s| s.span), format!("within this {kind}"));
    err.span_label(cx.span_at(*yield_location), "possible yield occurs here");
    err
}
