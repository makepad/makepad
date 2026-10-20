//! Why a borrow is still live at a point: the "3-point errors" of the NLL RFC.
//! ported from N:rustc_borrowck/src/diagnostics/{explain_borrow,find_use}.rs @8d1a764
//! (1.97.1 where they differ). Reduced: no if-let / `matches!` rewrites or closure
//! capture-path notes beyond the labels.

use super::region_name::RegionName;
use super::{Diag, DiagCx};
use crate::borrowck::borrowck_errors::{AccessKind, BorrowInfo};
use crate::borrowck::constraints::{ConstraintCategory, OutlivesConstraint, RegionVid};
use crate::borrowck::def_use::{categorize, DefUse};
use crate::mir::visit::{PlaceContext, Visitor};
use crate::mir::*;
use crate::types::{RegionKind, TyId, TyKind};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaterUseKind { TraitCapture, ClosureCapture, Call, FakeLetRead, Other }

#[derive(Clone, Debug)]
pub enum BorrowExplanation {
    UsedLater(Local, LaterUseKind, Span, Option<Span>),
    UsedLaterInLoop(LaterUseKind, Span, Option<Span>),
    UsedLaterWhenDropped { drop_loc: Location, dropped_local: Local, should_note_order: bool },
    MustBeValidFor { category: ConstraintCategory, from_closure: bool, span: Span, region_name: RegionName, opt_place_desc: Option<String>, path: Vec<OutlivesConstraint> },
    Unexplained,
}

/// region_infer/mod.rs Cause.
#[derive(Clone, Copy, Debug)]
pub enum Cause { LiveVar(Local, Location), DropVar(Local, Location) }

impl BorrowExplanation {
    pub fn is_explained(&self) -> bool { !matches!(self, BorrowExplanation::Unexplained) }

    /// explain_borrow.rs add_explanation_to_diagnostic. `borrow_desc` is "" or e.g. "immutable " / "first ".
    pub fn add_explanation_to_diagnostic(&self, cx: &DiagCx<'_>, diag: &mut Diag, borrow_desc: &str, borrow_span: Option<Span>) {
        let body = cx.body;
        let overlaps = |a: Span, b: Span| a.file == b.file && a.lo < b.hi && b.lo < a.hi;
        match self {
            BorrowExplanation::UsedLater(_dropped_local, later_use_kind, var_or_use_span, path_span) => {
                let message = match later_use_kind {
                    LaterUseKind::TraitCapture => "captured here by trait object",
                    LaterUseKind::ClosureCapture => "captured here by closure",
                    LaterUseKind::Call => "used by call",
                    LaterUseKind::FakeLetRead => "stored here",
                    LaterUseKind::Other => "used here",
                };
                if path_span.is_none_or(|p| p == *var_or_use_span) {
                    if borrow_span.is_none_or(|sp| !overlaps(sp, *var_or_use_span)) {
                        diag.span_label(*var_or_use_span, format!("{borrow_desc}borrow later {message}"));
                    }
                } else {
                    let path_span = path_span.unwrap();
                    if !borrow_span.is_some_and(|sp| overlaps(sp, *var_or_use_span)) {
                        diag.span_label(*var_or_use_span, format!("{borrow_desc}borrow later {message}"));
                        diag.span_label(path_span, "used here by closure");
                    }
                }
            }
            BorrowExplanation::UsedLaterInLoop(later_use_kind, var_or_use_span, path_span) => {
                let message = match later_use_kind {
                    LaterUseKind::TraitCapture => "borrow captured here by trait object, in later iteration of loop",
                    LaterUseKind::ClosureCapture => "borrow captured here by closure, in later iteration of loop",
                    LaterUseKind::Call => "borrow used by call, in later iteration of loop",
                    LaterUseKind::FakeLetRead => "borrow later stored here",
                    LaterUseKind::Other => "borrow used here, in later iteration of loop",
                };
                if path_span.is_none_or(|p| p == *var_or_use_span) {
                    diag.span_label(*var_or_use_span, format!("{borrow_desc}{message}"));
                } else {
                    let path_span = path_span.unwrap();
                    if borrow_span.is_none_or(|sp| !overlaps(sp, *var_or_use_span)) {
                        diag.span_label(*var_or_use_span, format!("{borrow_desc}borrow later {message}"));
                        diag.span_label(path_span, "used here by closure");
                    }
                }
            }
            BorrowExplanation::UsedLaterWhenDropped { drop_loc, dropped_local, should_note_order } => {
                let local_decl = &body.local_decls[dropped_local.0 as usize];
                let ty = local_decl.ty;
                let (dtor_desc, type_desc) = match cx.tcx.tys.kind(ty) {
                    // An ADT that implements Drop is reported by name.
                    TyKind::Adt(adt, ..) if cx.tcx.adt_destructor(*adt).is_some() && !cx.tcx.adts[adt].is_box() =>
                        ("`Drop` code", format!("type `{}`", cx.prog.def_path(*adt))),
                    TyKind::Closure(..) if cx.tcx.is_coroutine(ty) => ("destructor", "coroutine".to_owned()),
                    TyKind::Closure(..) => ("destructor", "closure".to_owned()),
                    _ => ("destructor", format!("type `{}`", cx.ty_string(local_decl.ty))),
                };
                let drop_span = cx.span_at(*drop_loc);
                match cx.local_name(*dropped_local) {
                    Some(local_name) => {
                        diag.span_label(drop_span, format!(
                            "{borrow_desc}borrow might be used here, when `{local_name}` is dropped and runs the {dtor_desc} for {type_desc}"));
                        if *should_note_order { diag.note("values in a scope are dropped in the opposite order they are defined"); }
                    }
                    None => {
                        diag.span_label(local_decl.source_info.span, format!("a temporary with access to the {borrow_desc}borrow is created here ..."));
                        diag.span_label(drop_span, format!(
                            "... and the {borrow_desc}borrow might be used here, when that temporary is dropped and runs the {dtor_desc} for {type_desc}"));
                    }
                }
            }
            BorrowExplanation::MustBeValidFor { category, span, region_name, opt_place_desc, path, .. } => {
                region_name.highlight(diag);
                if let Some(desc) = opt_place_desc {
                    diag.span_label(*span, format!("{}requires that `{desc}` is borrowed for `{region_name}`", super::region_errors::description(*category)));
                } else {
                    diag.span_label(*span, format!("{}requires that {borrow_desc}borrow lasts for `{region_name}`", super::region_errors::description(*category)));
                }
                super::region_errors::add_sized_or_copy_bound_info(cx, diag, *category, path);
                let mut preds: Vec<Span> = path.iter().filter_map(|c| match c.category { ConstraintCategory::Predicate(span, _) if span != Span::default() => Some(span), _ => None }).collect();
                preds.sort_by_key(|s| (s.file, s.lo, s.hi));
                preds.dedup();
                if !preds.is_empty() {
                    let s = if preds.len() == 1 { "" } else { "s" };
                    diag.note(format!("requirement{s} that the value outlives `{region_name}` introduced here"));
                }
            }
            BorrowExplanation::Unexplained => {}
        }
    }
}

/// explain_borrow.rs explain_why_borrow_contains_point.
pub fn explain_why_borrow_contains_point(cx: &DiagCx<'_>, location: Location, borrow: &BorrowInfo, kind_place: Option<(AccessKind, &Place)>) -> BorrowExplanation {
    let borrow_region_vid = borrow.region.0 as RegionVid;
    let mut region_sub = find_sub_region_live_at(cx, borrow_region_vid, location);
    let mut use_location = location;
    let mut use_in_later_iteration_of_loop = false;
    if region_sub == borrow_region_vid {
        // The location where the borrow is issued is the location that invalidates the
        // reference: likely a loop iteration. Use the loop terminator location instead.
        if let Some(loop_terminator_location) = find_loop_terminator_location(cx, borrow_region_vid) {
            region_sub = find_sub_region_live_at(cx, borrow_region_vid, loop_terminator_location);
            use_location = loop_terminator_location;
            use_in_later_iteration_of_loop = true;
        }
    }
    match find_use(cx, region_sub, use_location) {
        Some(Cause::LiveVar(local, at)) => {
            let span = cx.span_at(at);
            if use_in_later_iteration_of_loop {
                let (kind, var_or_use_span, path_span) = later_use_kind(cx, borrow, span, use_location);
                BorrowExplanation::UsedLaterInLoop(kind, var_or_use_span, path_span)
            } else {
                let _ = local;
                let (kind, var_or_use_span, path_span) = later_use_kind(cx, borrow, span, at);
                BorrowExplanation::UsedLater(borrow.borrowed_place.local, kind, var_or_use_span, path_span)
            }
        }
        Some(Cause::DropVar(local, at)) => {
            let mut should_note_order = false;
            if cx.local_name(local).is_some() {
                if let Some((AccessKind::StorageDeadOrDrop, place)) = kind_place {
                    if place.projection.is_empty() && cx.local_name(place.local).is_some() && local != place.local { should_note_order = true; }
                }
            }
            BorrowExplanation::UsedLaterWhenDropped { drop_loc: at, dropped_local: local, should_note_order }
        }
        None => {
            if let Some(region) = to_error_region_vid(cx, borrow_region_vid) {
                let (category, from_closure, span, region_name, path) = free_region_constraint_info(cx, borrow_region_vid, region);
                if let Some(region_name) = region_name {
                    let opt_place_desc = cx.describe_place(&borrow.borrowed_place);
                    BorrowExplanation::MustBeValidFor { category, from_closure, span, region_name, opt_place_desc, path }
                } else { BorrowExplanation::Unexplained }
            } else { BorrowExplanation::Unexplained }
        }
    }
}

fn free_region_constraint_info(cx: &DiagCx<'_>, borrow_region: RegionVid, outlived_region: RegionVid)
    -> (ConstraintCategory, bool, Span, Option<RegionName>, Vec<OutlivesConstraint>) {
    let regioncx = &cx.regions.regioncx;
    let path = regioncx.constraint_path_to(borrow_region, |r| r == outlived_region, true).map(|p| p.0).unwrap_or_default();
    let blame = regioncx.best_blame_constraint(cx.tcx, borrow_region, super::super::region_infer::NllRegionVariableOrigin::FreeRegion, outlived_region);
    let (category, from_closure, span) = match blame {
        Some(c) => (c.category, c.from_closure, c.span),
        None => (ConstraintCategory::Boring, false, Span::default()),
    };
    let name = super::region_name::give_region_a_name(cx, outlived_region);
    (category, from_closure, span, name, path)
}

/// region_errors.rs to_error_region_vid: a nameable universal region for `r`.
pub fn to_error_region_vid(cx: &DiagCx<'_>, r: RegionVid) -> Option<RegionVid> {
    let regioncx = &cx.regions.regioncx;
    let universal = &regioncx.universal_region_relations.universal_regions;
    if universal.is_universal_region(r) { return Some(r); }
    let upper_bound = approx_universal_upper_bound(cx, r);
    let scc = regioncx.constraint_sccs.scc(r);
    if regioncx.scc_values.contains_free_region(scc, upper_bound) { to_error_region_vid(cx, upper_bound) } else { None }
}

/// region_infer/mod.rs approx_universal_upper_bound. `postdom_upper_bound` is the unique
/// minimal common upper bound of the two regions in the free-region relations, 'static if none.
pub fn approx_universal_upper_bound(cx: &DiagCx<'_>, r: RegionVid) -> RegionVid {
    let regioncx = &cx.regions.regioncx;
    let relations = &regioncx.universal_region_relations;
    let universal = &relations.universal_regions;
    let static_r = universal.fr_static;
    let postdom_upper_bound = |a: RegionVid, b: RegionVid| -> RegionVid {
        let candidates: Vec<RegionVid> = universal.universal_regions_iter().filter(|&u| relations.outlives(u, a) && relations.outlives(u, b)).collect();
        let minimal: Vec<RegionVid> = candidates.iter().copied().filter(|&c| candidates.iter().all(|&d| relations.outlives(d, c))).collect();
        match minimal.first() { Some(&m) if minimal.iter().all(|&o| relations.equal(o, m)) => m, _ => static_r }
    };
    let mut lub = universal.fr_fn_body.unwrap_or(static_r);
    let scc = regioncx.constraint_sccs.scc(r);
    for ur in regioncx.scc_values.universal_regions_outlived_by(scc) {
        let new_lub = postdom_upper_bound(lub, ur);
        if ur != static_r && lub != static_r && new_lub == static_r {
            // Prefer the region with an external name (early-bound); else the lower index.
            if regioncx.definitions[ur].external_name.is_some() { lub = ur; }
            else if regioncx.definitions[lub].external_name.is_some() {}
            else { lub = lub.min(ur); }
        } else { lub = new_lub; }
    }
    lub
}

/// region_infer/mod.rs find_sub_region_live_at: some R with `fr1: R` live at `location`.
pub fn find_sub_region_live_at(cx: &DiagCx<'_>, fr1: RegionVid, location: Location) -> RegionVid {
    let regioncx = &cx.regions.regioncx;
    let point = cx.regions.locations.point(location);
    regioncx.constraint_path_to(fr1, |r| regioncx.liveness_constraints().is_live_at_point_eagerly(r, point), true)
        .map(|p| p.1).unwrap_or(fr1)
}

/// region_infer/mod.rs find_loop_terminator_location: the FalseUnwind terminator of the loop
/// in which `r` resides.
fn find_loop_terminator_location(cx: &DiagCx<'_>, r: RegionVid) -> Option<Location> {
    let regioncx = &cx.regions.regioncx;
    let scc = regioncx.constraint_sccs.scc(r);
    for point in 0..cx.regions.locations.num_points() {
        if !regioncx.scc_values.contains_point(scc, point) { continue; }
        let location = cx.regions.locations.location(point);
        if let Some(t) = &cx.body.basic_blocks[location.block.0 as usize].terminator {
            if matches!(t.kind, TerminatorKind::FalseUnwind { .. }) { return Some(location); }
        }
    }
    None
}

/// find_use.rs: the first use of a local whose type mentions `region_vid`, searching
/// forward through the region's points.
pub fn find_use(cx: &DiagCx<'_>, region_vid: RegionVid, start: Location) -> Option<Cause> {
    let regioncx = &cx.regions.regioncx;
    let body = cx.body;
    let mut queue = VecDeque::new();
    let mut visited: Vec<Location> = Vec::new();
    queue.push_back(start);
    while let Some(p) = queue.pop_front() {
        if !regioncx.region_contains_point(region_vid, cx.regions.locations.point(p)) { continue; }
        if visited.contains(&p) { continue; }
        visited.push(p);
        let block = &body.basic_blocks[p.block.0 as usize];
        let mut visitor = DefUseVisitor { cx, region_vid, result: None };
        let is_statement = p.statement_index < block.statements.len();
        if is_statement { visitor.visit_statement(&block.statements[p.statement_index], p); }
        else { visitor.visit_terminator(block.terminator.as_ref().unwrap(), p); }
        match visitor.result {
            Some(DefUseResult::Def) => {}
            Some(DefUseResult::UseLive(local)) => return Some(Cause::LiveVar(local, p)),
            Some(DefUseResult::UseDrop(local)) => return Some(Cause::DropVar(local, p)),
            None => {
                if is_statement { queue.push_back(Location { statement_index: p.statement_index + 1, ..p }); }
                else {
                    struct Targets(Vec<BasicBlock>);
                    impl Visitor for Targets { fn visit_target(&mut self, b: &BasicBlock, unwind: bool, _: Location) { if !unwind { self.0.push(*b); } } }
                    let mut t = Targets(vec![]);
                    t.visit_terminator(block.terminator.as_ref().unwrap(), p);
                    queue.extend(t.0.into_iter().map(|bb| Location { block: bb, statement_index: 0 }));
                }
            }
        }
    }
    None
}
enum DefUseResult { Def, UseLive(Local), UseDrop(Local) }
struct DefUseVisitor<'a, 'b> { cx: &'a DiagCx<'b>, region_vid: RegionVid, result: Option<DefUseResult> }
impl Visitor for DefUseVisitor<'_, '_> {
    fn visit_local(&mut self, local: &Local, context: PlaceContext, _: Location) {
        let ty = self.cx.body.local_decls[local.0 as usize].ty;
        if !mentions_region(self.cx, ty, self.region_vid) { return; }
        self.result = match categorize(context) {
            Some(DefUse::Def) => Some(DefUseResult::Def),
            Some(DefUse::Use) => Some(DefUseResult::UseLive(*local)),
            Some(DefUse::Drop) => Some(DefUseResult::UseDrop(*local)),
            None => None,
        };
    }
}
/// tcx.for_each_free_region(ty, |r| r.as_var() == region_vid).
pub fn mentions_region(cx: &DiagCx<'_>, ty: TyId, region_vid: RegionVid) -> bool {
    let tys = &cx.tcx.tys;
    let found = std::cell::Cell::new(false);
    tys.map_regions(ty, &|r, _| {
        if let RegionKind::ReVar(v) = tys.region_kind(r) { if v as usize == region_vid { found.set(true); } }
        r
    }, 0);
    found.get()
}

/// explain_borrow.rs later_use_kind, with UseSpans::OtherUse (closure capture spans arrive
/// with the MIR capture metadata).
fn later_use_kind(cx: &DiagCx<'_>, borrow: &BorrowInfo, span: Span, location: Location) -> (LaterUseKind, Span, Option<Span>) {
    let body = cx.body;
    let block = &body.basic_blocks[location.block.0 as usize];
    let function_span = |func: &Operand| -> Span {
        match func {
            Operand::Constant(c) => c.span,
            Operand::RuntimeChecks(_) => span,
            Operand::Copy(place) | Operand::Move(place) => {
                if place.projection.is_empty() && cx.local_name(place.local).is_none() { body.local_decls[place.local.0 as usize].source_info.span } else { span }
            }
        }
    };
    let kind = if let Some(Statement { kind: StatementKind::FakeRead(FakeReadCause::ForLet(_), place), .. }) = block.statements.get(location.statement_index) {
        if place.projection.is_empty() && matches!(cx.tcx.tys.kind(body.local_decls[place.local.0 as usize].ty), TyKind::Closure(..)) { LaterUseKind::ClosureCapture }
        else { LaterUseKind::FakeLetRead }
    } else if was_captured_by_trait_object(cx, borrow) {
        LaterUseKind::TraitCapture
    } else if location.statement_index == block.statements.len() {
        if let Some(Terminator { kind: TerminatorKind::Call { func, call_source: CallSource::Normal, .. }, .. }) = &block.terminator {
            return (LaterUseKind::Call, function_span(func), None);
        }
        LaterUseKind::Other
    } else { LaterUseKind::Other };
    (kind, span, None)
}

/// explain_borrow.rs was_captured_by_trait_object: a forward scan from the reserve location
/// for an unsizing cast of the borrow to a trait object.
fn was_captured_by_trait_object(cx: &DiagCx<'_>, borrow: &BorrowInfo) -> bool {
    let body = cx.body;
    let location = borrow.reserve_location;
    let block = &body.basic_blocks[location.block.0 as usize];
    let Some(Statement { kind: StatementKind::Assign(place, _), .. }) = block.statements.get(location.statement_index) else { return false };
    if !place.projection.is_empty() { return false; }
    let mut target = place.local;
    let mut queue = vec![location];
    while let Some(current) = queue.pop() {
        let block = &body.basic_blocks[current.block.0 as usize];
        if current.statement_index < block.statements.len() {
            let stmt = &block.statements[current.statement_index];
            if let StatementKind::Assign(place, rvalue) = &stmt.kind {
                let into = if place.projection.is_empty() { Some(place.local) }
                    else if place.projection == [PlaceElem::Deref] { Some(place.local) } else { None };
                let Some(into) = into else { queue.push(Location { statement_index: current.statement_index + 1, ..current }); continue; };
                match rvalue {
                    Rvalue::Use(Operand::Copy(p) | Operand::Move(p)) => {
                        if p.projection.is_empty() && p.local == target { target = into; }
                    }
                    Rvalue::Cast(CastKind::PointerCoercion(PointerCoercion::Unsize, _), Operand::Copy(p) | Operand::Move(p), ty) => {
                        if p.projection.is_empty() && p.local == target {
                            return match cx.tcx.tys.kind(*ty) {
                                TyKind::Ref(_, inner, _) => matches!(cx.tcx.tys.kind(*inner), TyKind::Dyn(..)),
                                TyKind::Adt(d, args, _) if cx.tcx.adts[d].is_box() => cx.tcx.tys.list(*args).first().is_some_and(|&a| matches!(cx.tcx.tys.kind(a), TyKind::Dyn(..))),
                                TyKind::Dyn(..) => true,
                                _ => false,
                            };
                        }
                        return false;
                    }
                    Rvalue::Cast(CastKind::PointerCoercion(PointerCoercion::Unsize, _), _, _) => return false,
                    _ => {}
                }
            }
            queue.push(Location { statement_index: current.statement_index + 1, ..current });
        } else if let Some(Terminator { kind: TerminatorKind::Call { destination, target: Some(next), args, .. }, .. }) = &block.terminator {
            if destination.projection.is_empty() {
                let found = args.iter().any(|a| matches!(a, Operand::Move(p) if p.projection.is_empty() && p.local == target));
                if found { target = destination.local; queue.push(Location { block: *next, statement_index: 0 }); }
            }
        }
    }
    false
}
