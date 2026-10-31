//! C-NLL6: borrow-check diagnostics on MIR results, at REDUCE level: error code, primary span,
//! rustc's main message and labels, the "borrow later used here" explanation and region names.
//! No suggestions or multipart edits.
//! ported from N:rustc_borrowck/src/diagnostics/mod.rs @8d1a764 (1.97.1 where they differ).
//!
//! Entry point: `render_errors`, which turns a `MirBorrowckResult` (structured `Detail`s chosen by
//! the checker) and the solved regions into `event::BorrowError`s for Gandalf, the console and JSON.

use super::borrow_set::BorrowSet;
use super::borrowck_errors::{BorrowInfo, BorrowckError, Detail};
use super::check::MirBorrowckResult;
use super::event::{BorrowError, Label};
use super::mir_nll::MirRegionResult;
use crate::mir::move_paths::MoveData;
use crate::mir::*;
use crate::program::{DefId, Program};
use crate::tcx::Tcx;
use crate::types::{TyId, TyKind};

pub mod conflict_errors;
pub mod explain_borrow;
pub mod move_errors;
pub mod mutability_errors;
pub mod opaque_types;
pub mod region_errors;
pub mod region_name;
#[cfg(test)]
mod tests;

/// rustc_errors::Diag at the reduced level: the primary span, the code and message, labels on
/// spans (the label on the primary span is the primary label), notes and helps.
#[derive(Clone, Debug)]
pub struct Diag {
    pub code: &'static str,
    pub message: String,
    pub span: Span,
    pub labels: Vec<(Span, String, bool)>,
    pub notes: Vec<String>,
    pub helps: Vec<String>,
    /// accepted only in relaxed Rust (field views / extended two-phase borrows)
    pub fv_only: bool,
}
impl Diag {
    pub fn new(code: &'static str, span: Span, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), span, labels: Vec::new(), notes: Vec::new(), helps: Vec::new(), fv_only: false }
    }
    pub fn span_label(&mut self, span: Span, text: impl Into<String>) -> &mut Self {
        let primary = span == self.span;
        self.labels.push((span, text.into(), primary));
        self
    }
    pub fn note(&mut self, text: impl Into<String>) -> &mut Self { self.notes.push(text.into()); self }
    pub fn help(&mut self, text: impl Into<String>) -> &mut Self { self.helps.push(text.into()); self }
}

/// Everything diagnostics read: the body and its solved analyses. MoveData is the same
/// body-owned arena the checker used; the borrow set is rebuilt deterministically.
pub struct DiagCx<'a> {
    pub tcx: &'a Tcx,
    pub prog: &'a Program,
    pub owner: DefId,
    pub body: &'a Body,
    pub moves: &'a MoveData,
    pub regions: &'a MirRegionResult,
    pub borrows: &'a BorrowSet,
    /// Universal region names, published by C-NLL5 (DESIGN-C-NLL5 §4 contract 3).
    pub names: &'a region_name::UniversalNames,
    /// region_name.rs give_region_a_name: cache and the `'1`, `'2` counter, per body.
    pub region_names: std::cell::RefCell<std::collections::HashMap<usize, region_name::RegionName>>,
    pub region_counter: std::cell::Cell<usize>,
    /// What the closure-like bodies of this body were written as (by closure expr), when the
    /// driver knows them from the nested bodies' `CoroutineInfo` (E0373 wording).
    pub coroutine_kinds: Vec<(u32, CoroutineKind)>,
}
impl<'a> DiagCx<'a> {
    pub fn new(tcx: &'a Tcx, prog: &'a Program, owner: DefId, body: &'a Body, moves: &'a MoveData, regions: &'a MirRegionResult,
        borrows: &'a BorrowSet, names: &'a region_name::UniversalNames) -> Self {
        Self { tcx, prog, owner, body, moves, regions, borrows, names, region_names: Default::default(), region_counter: std::cell::Cell::new(1), coroutine_kinds: Vec::new() }
    }
}

/// diagnostics/mod.rs UseSpans, reduced to the two shapes the primary spans depend on: a
/// closure/coroutine capture, and any other use. (FnSelfUse's receiver notes are not kept.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseSpans {
    ClosureUse { is_coroutine: bool, is_coroutine_closure: bool, expr: u32, args_span: Span, capture_kind_span: Span, path_span: Span },
    OtherUse(Span),
}
impl UseSpans {
    pub fn args_or_use(self) -> Span { match self { Self::ClosureUse { args_span, .. } => args_span, Self::OtherUse(s) => s } }
    pub fn var_or_use(self) -> Span { match self { Self::ClosureUse { capture_kind_span, .. } => capture_kind_span, Self::OtherUse(s) => s } }
    pub fn var_or_use_path_span(self) -> Span { match self { Self::ClosureUse { path_span, .. } => path_span, Self::OtherUse(s) => s } }
    pub fn for_closure(self) -> bool { matches!(self, Self::ClosureUse { is_coroutine: false, is_coroutine_closure: false, .. }) }
    pub fn for_coroutine(self) -> bool { matches!(self, Self::ClosureUse { is_coroutine: true, .. }) }
    pub fn or_else(self, other: impl FnOnce() -> Self) -> Self { match self { Self::ClosureUse { .. } => self, Self::OtherUse(_) => other() } }
}

/// Render every error of one body. Errors keep the checker's order; region errors follow
/// (rustc emits buffered region errors after the access errors, lib.rs do_mir_borrowck).
pub fn render_errors(cx: &DiagCx<'_>, result: &MirBorrowckResult) -> Vec<BorrowError> {
    let mut out = Vec::new();
    for error in &result.errors {
        out.push(capture(cx.prog, cx.owner, &render_error(cx, error)));
    }
    for diag in region_errors::report_region_errors(cx, &result.region_errors) {
        out.push(capture(cx.prog, cx.owner, &diag));
    }
    out
}

/// The relaxed-Rust acceptances of one body (`makepad check`'s "accepted only in relaxed Rust"
/// report): the conflicts rustc would reject.
pub fn render_fv_only(cx: &DiagCx<'_>, result: &MirBorrowckResult) -> Vec<BorrowError> {
    result.fv_only.iter().map(|e| capture(cx.prog, cx.owner, &render_error(cx, e))).collect()
}

/// The export to rustc of one body's field-view acceptances as typed edits.
pub fn render_fv_rewrites(cx: &DiagCx<'_>, result: &MirBorrowckResult) -> Vec<super::field_views::edit::PlannedRewrite> {
    super::field_views::edit::plan(cx.prog, &result.fv_records)
}

/// What the callees of a narrowed borrow touch: "`m` uses mutably `a` and shared `b`".
fn fv_explain(cx: &DiagCx<'_>, why: &[super::borrowck_errors::FvWho]) -> String {
    let mut out = Vec::new();
    for w in why {
        let who = match (w.callee, w.is_closure) {
            (Some(d), _) => format!("`{}`", cx.prog.name(d)),
            (None, true) => "the closure".to_string(),
            (None, false) => "the call".to_string(),
        };
        let (mut muts, mut refs) = (Vec::new(), Vec::new());
        for (q, m) in &w.parts {
            let d = cx.describe_place(q).unwrap_or_else(|| "value".to_string());
            if *m { muts.push(format!("`{d}`")); } else { refs.push(format!("`{d}`")); }
        }
        let mut s = format!("{who} uses");
        if !muts.is_empty() { s.push_str(&format!(" mutably {}", muts.join(", "))); }
        if !refs.is_empty() {
            if !muts.is_empty() { s.push_str(" and"); }
            s.push_str(&format!(" shared {}", refs.join(", ")));
        }
        if muts.is_empty() && refs.is_empty() { s.push_str(" nothing of it"); }
        out.push(s);
    }
    out.join("; ")
}

/// One access/move/mutability error with its rustc message and labels.
pub fn render_error(cx: &DiagCx<'_>, error: &BorrowckError) -> Diag {
    let mut d = render_error_plain(cx, error);
    if let Some(note) = &error.fv {
        use super::borrowck_errors::FvNote::*;
        match &**note {
            TwoPhase { reserved } => {
                d.fv_only = true;
                d.labels.push((*reserved, "reserved here for the call, not used before it".to_string(), false));
                d.note("accepted with extended two-phase borrows (relaxed Rust): the reserved borrow is only used by the call, after this");
            }
            Narrowed { why } => {
                d.fv_only = true;
                d.note(format!("accepted with field views (relaxed Rust): {}", fv_explain(cx, why)));
            }
            Remaining { why } => { d.note(format!("field views: {}", fv_explain(cx, why))); }
            Disjoint { proven } => {
                d.fv_only = true;
                d.note(if *proven { "accepted with disjoint indices (relaxed Rust): the two indices are distinct; stable rustc rejects this" }
                    else { "accepted with disjoint indices (relaxed Rust): the indices are compared when this runs and an overlap panics, as `get_disjoint_mut(..).unwrap()` does; stable rustc rejects this" });
            }
            Polonius => {
                d.fv_only = true;
                d.note("accepted under Polonius (relaxed Rust): the borrow is not used on the path through this access; stable rustc (NLL) rejects this");
            }
        }
    }
    d
}

fn render_error_plain(cx: &DiagCx<'_>, error: &BorrowckError) -> Diag {
    use super::borrowck_errors::ErrorKind::*;
    match (&error.detail, error.kind) {
        (Detail::Moved { .. }, Moved | Uninitialized) => conflict_errors::report_use_of_moved_or_uninitialized(cx, error),
        (Detail::Conflict { .. }, _) => conflict_errors::report_conflicting_access(cx, error),
        (Detail::Lifetime { .. }, _) => conflict_errors::report_borrowed_value_does_not_live_long_enough(cx, error),
        (Detail::BorrowAcrossYield { .. }, _) => conflict_errors::report_borrow_across_yield(cx, error),
        (Detail::Mutability { .. }, _) => mutability_errors::report_mutability_error(cx, error),
        (Detail::IllegalMove { .. }, _) => move_errors::report_cannot_move_from(cx, error),
        _ => Diag::new(error.code(), error.span, error.kind.note()),
    }
}

// ---------------------------------------------------------------------------------------------
// Presentation: file/line/col and source text. rustc_errors/src/emitter.rs:1491-1547.

pub fn capture(prog: &Program, owner: DefId, d: &Diag) -> BorrowError {
    let mut labels: Vec<(Span, String, bool)> = d.labels.clone();
    if !labels.iter().any(|(_, _, primary)| *primary) { labels.insert(0, (d.span, String::new(), true)); }
    let primary = snapshot(prog, d.span, String::new(), true);
    BorrowError {
        fn_path: prog.def_path(owner), owner, krate: prog.def(owner).krate,
        code: d.code, message: d.message.clone(), file: prog.file_paths[d.span.file as usize].clone(),
        line: primary.line, col: primary.col, end_line: primary.end_line, end_col: primary.end_col,
        labels: labels.into_iter().map(|(s, t, p)| snapshot(prog, s, t, p)).collect(),
        notes: d.notes.clone(), helps: d.helps.clone(), fv_only: d.fv_only,
    }
}

fn snapshot(prog: &Program, span: Span, text: String, primary: bool) -> Label {
    let file = &prog.files[span.file as usize];
    let src = &file.src[..file.src_len as usize];
    let lo = (span.lo as usize).min(src.len());
    let hi = (span.hi as usize).clamp(lo, src.len());
    let (line, col) = file.line_col(lo as u32);
    let (end_line, end_col) = if hi <= lo { (line, col + 1) } else { file.line_col(hi as u32) };
    let line_start = lo.saturating_sub(col - 1);
    let line_len = src[line_start..].iter().position(|&b| b == b'\n').unwrap_or(src.len() - line_start);
    let source = String::from_utf8_lossy(&src[line_start..line_start + line_len]).into_owned();
    Label { line: line as u32, col: col as u32, end_line: end_line as u32, end_col: end_col as u32, text, primary, source }
}

// ---------------------------------------------------------------------------------------------
// Place and type descriptions. rustc diagnostics/mod.rs: describe_place_with_options,
// append_place_to_string, describe_any_place.

impl<'a> DiagCx<'a> {
    pub fn is_box(&self, ty: TyId) -> bool { matches!(self.tcx.tys.kind(ty), TyKind::Adt(d, ..) if self.tcx.adts[d].is_box()) }
    /// "closure", "coroutine", "async block", "async closure", "gen block", ...: the
    /// noun for a closure-like aggregate (rustc report_escaping_closure_capture).
    pub fn closure_noun(&self, spans: UseSpans) -> &'static str {
        let UseSpans::ClosureUse { is_coroutine, expr, .. } = spans else { return "closure" };
        match self.coroutine_kinds.iter().find(|(e, _)| *e == expr).map(|(_, k)| *k) {
            Some(kind) => coroutine_noun(kind),
            None => if is_coroutine { "coroutine" } else { "closure" },
        }
    }
    pub fn ty_string(&self, ty: TyId) -> String { crate::typeck::ty_to_string(false, self.prog, self.tcx, ty) }

    pub fn place_ty(&self, place: &Place) -> TyId {
        place.projection.iter().fold(PlaceTy::from_ty(self.body.local_decls[place.local.0 as usize].ty),
            |ty, elem| ty.projection_ty(self.tcx, elem).expect("invalid MIR place")).ty
    }
    pub fn prefix_ty(&self, place: &Place, len: usize) -> TyId {
        self.place_ty(&Place { local: place.local, projection: place.projection[..len].to_vec() })
    }

    /// The user name of a local (var_debug_info), if it is a user variable or argument.
    pub fn local_name(&self, local: Local) -> Option<&str> {
        self.body.var_debug_info.iter()
            .find(|v| v.place.local == local && v.place.projection.is_empty())
            .map(|v| v.name.as_str())
    }

    /// `x`, `x.f`, `*x`, `v[..]`; None for temporaries. Captured upvars are named by the
    /// closure body's debug info (a place `(*_1).0` with a name).
    pub fn describe_place(&self, place: &Place) -> Option<String> {
        // A debug-info alias for the whole place (closure upvars, bindings of projections).
        if let Some(v) = self.body.var_debug_info.iter().find(|v| v.place == *place && !v.place.projection.is_empty()) {
            return Some(v.name.clone());
        }
        let mut s = self.local_name(place.local)?.to_string();
        let mut ty = self.body.local_decls[place.local.0 as usize].ty;
        let mut variant = 0u32;
        for (i, elem) in place.projection.iter().enumerate() {
            match elem {
                PlaceElem::Deref => {
                    // `(*x).f` reads as `x.f`
                    if !matches!(place.projection.get(i + 1), Some(PlaceElem::Field(..) | PlaceElem::Downcast(..))) { s = format!("*{s}"); }
                }
                PlaceElem::Field(f, _) => {
                    let name = match self.tcx.tys.kind(ty) {
                        TyKind::Adt(..) => self.tcx.place_field_name(self.prog, ty, variant, f.0),
                        _ => f.0.to_string(),
                    };
                    let name = if name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') { name } else { f.0.to_string() };
                    s = format!("{s}.{name}");
                    variant = 0;
                }
                PlaceElem::Downcast(_, v) => variant = v.0,
                PlaceElem::Index(_) | PlaceElem::ConstantIndex { .. } | PlaceElem::Subslice { .. } => s = format!("{s}[..]"),
                PlaceElem::OpaqueCast(_) | PlaceElem::Subtype(_) => {}
            }
            ty = self.prefix_ty(place, i + 1);
        }
        Some(s)
    }

    /// "`x`" or "value" (describe_any_place).
    pub fn describe_any_place(&self, place: &Place) -> String {
        match self.describe_place(place) { Some(n) => format!("`{n}`"), None => "value".into() }
    }

    /// diagnostics/mod.rs move_spans: a move into a closure/coroutine aggregate at `at` is a
    /// capture; the closure expression is the span.
    pub fn move_spans(&self, moved_place: &Place, at: Location) -> UseSpans {
        let block = &self.body.basic_blocks[at.block.0 as usize];
        if let Some(Statement { kind: StatementKind::Assign(_, Rvalue::Aggregate(kind, operands)), source_info }) = block.statements.get(at.statement_index) {
            let is_coroutine = match &**kind { AggregateKind::Closure(id, _) => Some((false, false, id.expr)), AggregateKind::CoroutineClosure(id, _) => Some((false, true, id.expr)), AggregateKind::Coroutine(id, _) => Some((true, false, id.expr)), _ => None };
            if let Some((is_coroutine, is_coroutine_closure, expr)) = is_coroutine {
                let captured = operands.iter().any(|op| matches!(op, Operand::Move(p) | Operand::Copy(p)
                    if p.local == moved_place.local && (p.projection.starts_with(&moved_place.projection) || moved_place.projection.starts_with(&p.projection))));
                if captured {
                    let s = source_info.span;
                    return UseSpans::ClosureUse { is_coroutine, is_coroutine_closure, expr, args_span: s, capture_kind_span: s, path_span: s };
                }
            }
        }
        UseSpans::OtherUse(self.span_at(at))
    }

    /// diagnostics/mod.rs borrow_spans: a borrow into a temporary that a closure/coroutine
    /// aggregate then captures. The capture spans are the borrow's own (the use inside the closure).
    pub fn borrow_spans(&self, use_span: Span, at: Location) -> UseSpans {
        let block = &self.body.basic_blocks[at.block.0 as usize];
        let target = match block.statements.get(at.statement_index) {
            Some(Statement { kind: StatementKind::Assign(place, _), .. }) if place.projection.is_empty() => place.local,
            _ => return UseSpans::OtherUse(use_span),
        };
        if self.body.local_decls[target.0 as usize].kind != LocalKind::Temp { return UseSpans::OtherUse(use_span); }
        for stmt in block.statements.iter().skip(at.statement_index + 1) {
            if let StatementKind::Assign(_, Rvalue::Aggregate(kind, operands)) = &stmt.kind {
                let is_coroutine = match &**kind { AggregateKind::Closure(id, _) => Some((false, false, id.expr)), AggregateKind::CoroutineClosure(id, _) => Some((false, true, id.expr)), AggregateKind::Coroutine(id, _) => Some((true, false, id.expr)), _ => None };
                if let Some((is_coroutine, is_coroutine_closure, expr)) = is_coroutine {
                    if operands.iter().any(|op| matches!(op, Operand::Move(p) | Operand::Copy(p) if p.local == target)) {
                        return UseSpans::ClosureUse { is_coroutine, is_coroutine_closure, expr, args_span: stmt.source_info.span, capture_kind_span: use_span, path_span: use_span };
                    }
                }
            }
        }
        UseSpans::OtherUse(use_span)
    }

    /// diagnostics/mod.rs retrieve_borrow_spans.
    pub fn retrieve_borrow_spans(&self, borrow: &BorrowInfo) -> UseSpans {
        self.borrow_spans(self.span_at(borrow.reserve_location), borrow.reserve_location)
    }

    /// The span of the statement or terminator at `at` (UseSpans::OtherUse).
    pub fn span_at(&self, at: Location) -> Span {
        let block = &self.body.basic_blocks[at.block.0 as usize];
        block.statements.get(at.statement_index)
            .map_or_else(|| block.terminator.as_ref().unwrap().source_info.span, |s| s.source_info.span)
    }
}

/// hir CoroutineKind descr: `async block`, `gen closure`, `async gen block`, `coroutine`.
pub fn coroutine_noun(kind: CoroutineKind) -> &'static str {
    use CoroutineDesugaring::*;
    use CoroutineSource::*;
    match kind {
        CoroutineKind::Desugared(Gen, Block | Fn) => "gen block",
        CoroutineKind::Desugared(Gen, Closure) => "gen closure",
        CoroutineKind::Desugared(AsyncGen, Block | Fn) => "async gen block",
        CoroutineKind::Desugared(AsyncGen, Closure) => "async gen closure",
        CoroutineKind::Desugared(Async, Block | Fn) => "async block",
        CoroutineKind::Desugared(Async, Closure) => "async closure",
        CoroutineKind::Coroutine(_) => "coroutine",
    }
}
