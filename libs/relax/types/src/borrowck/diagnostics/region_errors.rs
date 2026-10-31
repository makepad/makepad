//! Lifetime errors: "lifetime may not live long enough", E0521 borrowed data escapes, E0621
//! explicit lifetime required, the FnMut-closure escape, E0309/E0310/E0311 type tests and
//! higher-ranked placeholder errors.
//! ported from N:rustc_borrowck/src/diagnostics/{region_errors,bound_region_errors}.rs
//! @8d1a764 and N:session_diagnostics.rs. Reduced: the blamed constraint's category and span
//! (`best_blame_constraint`), region names, the main labels; no outlives or `'static`
//! suggestions.

use super::explain_borrow::to_error_region_vid;
use super::region_name::{give_region_a_name, get_var_name_and_span_for_region, ExternalName, RegionName, RegionNameSource};
use super::{Diag, DiagCx};
use crate::borrowck::constraints::{AnnotationSource, ConstraintCategory, OutlivesConstraint, RegionVid, ReturnConstraint};
use crate::borrowck::region_infer::RegionErrorKind;
use crate::mir::{Local, Span};
use crate::types::{RegionKind, TyKind};

/// region_errors.rs ConstraintDescription: ends with a space; empty for categories without one.
pub fn description(category: ConstraintCategory) -> &'static str {
    match category {
        ConstraintCategory::Assignment => "assignment ",
        ConstraintCategory::Return(_) => "returning this value ",
        ConstraintCategory::Yield => "yielding this value ",
        ConstraintCategory::UseAsConst => "using this value as a constant ",
        ConstraintCategory::UseAsStatic => "using this value as a static ",
        ConstraintCategory::Cast { is_implicit_coercion: false, .. } => "cast ",
        ConstraintCategory::Cast { is_implicit_coercion: true, .. } => "coercion ",
        ConstraintCategory::CallArgument(_) => "argument ",
        ConstraintCategory::TypeAnnotation(AnnotationSource::GenericArg) => "generic argument ",
        ConstraintCategory::TypeAnnotation(_) => "type annotation ",
        ConstraintCategory::SizedBound => "proving this value is `Sized` ",
        ConstraintCategory::CopyBound => "copying this value ",
        ConstraintCategory::OpaqueType => "opaque type ",
        ConstraintCategory::ClosureUpvar(_) => "closure capture ",
        ConstraintCategory::Usage => "this usage ",
        ConstraintCategory::Predicate(..) | ConstraintCategory::Boring | ConstraintCategory::BoringNoLocation
        | ConstraintCategory::Internal | ConstraintCategory::OutlivesUnnameablePlaceholder(_) => "",
    }
}

/// region_errors.rs add_sized_or_copy_bound_info. The note that names the bound's declaration
/// is a span note in rustc; at this level only the general explanation is kept.
pub fn add_sized_or_copy_bound_info(_cx: &DiagCx<'_>, diag: &mut Diag, category: ConstraintCategory, path: &[OutlivesConstraint]) {
    if !matches!(category, ConstraintCategory::SizedBound | ConstraintCategory::CopyBound) { return; }
    if path.iter().any(|c| matches!(c.category, ConstraintCategory::Predicate(..))) {
        diag.note(if category == ConstraintCategory::SizedBound { "required by a `Sized` bound" } else { "required by a `Copy` bound" });
    }
}

struct ErrorConstraintInfo { fr: RegionVid, outlived_fr: RegionVid, category: ConstraintCategory, span: Span }

pub fn report_region_errors(cx: &DiagCx<'_>, errors: &[RegionErrorKind]) -> Vec<Diag> {
    let mut out = Vec::new();
    for error in errors {
        match error {
            RegionErrorKind::TypeTest { test } => {
                if let Some(d) = report_type_test(cx, *test) { out.push(d); }
            }
            RegionErrorKind::PlaceholderOutlivesIllegalRegion { longer_fr, illegally_outlived_r } => {
                // (rustc's buffered errors are deduplicated: the same mismatch is reached once per placeholder)
                let d = report_erroneous_rvid_reaches_placeholder(cx, *longer_fr, *illegally_outlived_r);
                if !out.iter().any(|o: &Diag| o.code == d.code && o.message == d.message && o.span == d.span && o.notes == d.notes) { out.push(d); }
            }
            RegionErrorKind::RegionError { longer_fr, shorter_fr, is_reported } => {
                // Only the first error of each longer region is reported.
                if *is_reported { out.push(report_region_error(cx, *longer_fr, *shorter_fr)); }
            }
        }
    }
    out
}

fn blame(cx: &DiagCx<'_>, from: RegionVid, to: RegionVid) -> (ConstraintCategory, Span, Vec<OutlivesConstraint>, bool) {
    let regioncx = &cx.regions.regioncx;
    let path = regioncx.constraint_path_to(from, |r| r == to, true).map(|p| p.0).unwrap_or_default();
    match regioncx.best_blame_constraint(cx.tcx, from, regioncx.definitions[from].origin, to) {
        Some(c) => (c.category, c.span, path, c.from_closure),
        None => (ConstraintCategory::Boring, Span::default(), path, false),
    }
}

/// report_region_error.
fn report_region_error(cx: &DiagCx<'_>, fr: RegionVid, outlived_fr: RegionVid) -> Diag {
    let (category, span, path, _from_closure) = blame(cx, fr, outlived_fr);
    let universal = &cx.regions.regioncx.universal_region_relations.universal_regions;
    // NiceRegionError::try_report_from_nll: E0621 for a named region required by an anonymous one.
    if let Some(d) = try_report_named_anon_conflict(cx, fr, outlived_fr, span) { return d; }
    let fr_is_local = universal.is_local_free_region(fr);
    let outlived_fr_is_local = universal.is_local_free_region(outlived_fr);
    let errci = ErrorConstraintInfo { fr, outlived_fr, category, span };
    let mut diag = match (category, fr_is_local, outlived_fr_is_local) {
        (ConstraintCategory::Return(kind), true, false) if is_closure_fn_mut(cx, fr) => report_fnmut_error(cx, &errci, kind),
        (ConstraintCategory::Assignment, true, false) | (ConstraintCategory::CallArgument(_), true, false) => report_escaping_data_error(cx, &errci),
        _ => report_general_error(cx, &errci),
    };
    add_sized_or_copy_bound_info(cx, &mut diag, category, &path);
    for c in &path {
        if let ConstraintCategory::Cast { is_raw_ptr_dyn_type_cast: true, .. } = c.category {
            diag.note("raw pointer casts of trait objects cannot extend lifetimes");
            diag.note("this was previously accepted by the compiler but was changed recently");
            diag.help("see <https://github.com/rust-lang/rust/issues/141402> for more information");
        }
    }
    diag
}

fn is_closure_fn_mut(cx: &DiagCx<'_>, fr: RegionVid) -> bool {
    cx.names.is_closure_fn_mut && matches!(cx.names.external(fr), Some(ExternalName::ClosureEnv { .. }))
}

/// report_fnmut_error: "captured variable cannot escape `FnMut` closure body".
fn report_fnmut_error(cx: &DiagCx<'_>, errci: &ErrorConstraintInfo, _kind: ReturnConstraint) -> Diag {
    let mut diag = Diag::new("", errci.span, "captured variable cannot escape `FnMut` closure body");
    diag.note("`FnMut` closures only have access to their captured variables while they are executing...");
    diag.note("...therefore, they cannot allow references to captured variables to escape");
    let returns_closure = cx.names.output.as_ref().is_some_and(|o| matches!(cx.tcx.tys.kind(o.ty), TyKind::Closure(..)));
    let label = if returns_closure { "returns a closure that contains a reference to a captured variable, which then escapes the closure body" }
        else { "returns a reference to a captured variable which escapes the closure body" };
    diag.span_label(errci.span, label);
    if let Some(name) = give_region_a_name(cx, errci.outlived_fr) {
        if let Some(fr_span) = name.span() { diag.span_label(fr_span, "inferred to be a `FnMut` closure"); }
    }
    diag
}

/// report_escaping_data_error (E0521).
fn report_escaping_data_error(cx: &DiagCx<'_>, errci: &ErrorConstraintInfo) -> Diag {
    let ErrorConstraintInfo { span, category, .. } = errci;
    let fr_name_and_span = get_var_name_and_span_for_region(cx, errci.fr);
    let outlived_fr_name_and_span = get_var_name_and_span_for_region(cx, errci.outlived_fr);
    let escapes_from = cx.names.def_descr;
    // Revert to the normal error in these cases: assignments are not "escapes" in function
    // items, nor is anything in a constant.
    if (fr_name_and_span.is_none() && outlived_fr_name_and_span.is_none())
        || (*category == ConstraintCategory::Assignment && cx.names.is_fn_def)
        || cx.names.is_const
        || (fr_name_and_span.is_none() && cx.names.is_fn_def) {
        return report_general_error(cx, errci);
    }
    let mut diag = Diag::new("E0521", *span, format!("borrowed data escapes outside of {escapes_from}"));
    if let Some((Some(outlived_fr_name), outlived_fr_span)) = &outlived_fr_name_and_span {
        diag.span_label(*outlived_fr_span, format!("`{outlived_fr_name}` declared here, outside of the {escapes_from} body"));
    }
    if let Some((Some(fr_name), fr_span)) = &fr_name_and_span {
        diag.span_label(*fr_span, format!("`{fr_name}` is a reference that is only valid in the {escapes_from} body"));
        diag.span_label(*span, format!("`{fr_name}` escapes the {escapes_from} body here"));
    } else {
        diag.span_label(*span, format!("a temporary borrow escapes the {escapes_from} body here"));
        if let Some((Some(outlived_name), _)) = &outlived_fr_name_and_span {
            diag.help(format!("`{outlived_name}` is declared outside the {escapes_from}, so any data borrowed inside the {escapes_from} cannot be stored into it"));
        }
    }
    // Only when both regions have error names (not just synthetic ones).
    if to_error_region_vid(cx, errci.fr).is_some() && to_error_region_vid(cx, errci.outlived_fr).is_some() {
        if let (Some(fr_region_name), Some(outlived_name)) = (give_region_a_name(cx, errci.fr), give_region_a_name(cx, errci.outlived_fr)) {
            fr_region_name.highlight(&mut diag);
            outlived_name.highlight(&mut diag);
            diag.span_label(*span, format!("{}requires that `{}` must outlive `{}`", description(*category), fr_region_name, outlived_name));
        }
    }
    diag
}

/// report_general_error: "lifetime may not live long enough".
fn report_general_error(cx: &DiagCx<'_>, errci: &ErrorConstraintInfo) -> Diag {
    let ErrorConstraintInfo { fr, outlived_fr, span, category } = errci;
    let universal = &cx.regions.regioncx.universal_region_relations.universal_regions;
    let mir_def_name = cx.names.def_descr;
    let mut diag = Diag::new("", *span, "lifetime may not live long enough");
    // A lifetime that cannot be named gets a dummy name instead of an ICE (rustc #118021).
    let fr_name = give_region_a_name(cx, *fr).unwrap_or(RegionName { name: "'_".into(), source: RegionNameSource::Static });
    fr_name.highlight(&mut diag);
    let outlived_name = give_region_a_name(cx, *outlived_fr).unwrap_or(RegionName { name: "'_".into(), source: RegionNameSource::Static });
    outlived_name.highlight(&mut diag);
    if matches!(category, ConstraintCategory::Return(_)) && universal.is_local_free_region(*outlived_fr) {
        diag.span_label(*span, format!("{mir_def_name} was supposed to return data with lifetime `{outlived_name}` but it is returning data with lifetime `{fr_name}`"));
    } else {
        diag.span_label(*span, format!("{}requires that `{fr_name}` must outlive `{outlived_name}`", description(*category)));
    }
    diag
}

/// nice_region_error/named_anon_conflict.rs: a named lifetime is required of the type of an
/// argument written with an anonymous lifetime (E0621).
fn try_report_named_anon_conflict(cx: &DiagCx<'_>, fr: RegionVid, outlived_fr: RegionVid, span: Span) -> Option<Diag> {
    let named_of = |r: RegionVid| match cx.names.external(r) { Some(ExternalName::Named { name, .. }) => Some(name.clone()), _ => None };
    let (named, anon) = match (named_of(outlived_fr), named_of(fr)) {
        (Some(n), None) => (n, fr),
        (None, Some(n)) => (n, outlived_fr),
        _ => return None,
    };
    if !matches!(cx.names.external(anon), Some(ExternalName::Anon)) { return None; }
    let index = cx.names.inputs.iter().position(|i| super::explain_borrow::mentions_region(cx, i.ty, anon))?;
    let param = cx.local_name(Local(index as u32 + 1))?;
    if param == "self" { return None; }
    let mut diag = Diag::new("E0621", span, format!("explicit lifetime required in the type of `{param}`"));
    diag.span_label(span, format!("lifetime `{named}` required"));
    Some(diag)
}

/// report_erroneous_rvid_reaches_placeholder: `longer_fr`, a placeholder, would have to outlive
/// `error_vid`. The universe's cause says what to report (bound_region_errors.rs
/// `UniverseInfo::report_erroneous_element`).
fn report_erroneous_rvid_reaches_placeholder(cx: &DiagCx<'_>, longer_fr: RegionVid, error_vid: RegionVid) -> Diag {
    use crate::borrowck::type_check::UniverseInfo;
    let (_, span, _, _) = blame(cx, longer_fr, error_vid);
    let universe = match cx.regions.regioncx.definitions[longer_fr].origin {
        crate::borrowck::region_infer::NllRegionVariableOrigin::Placeholder(p) => match cx.tcx.tys.region_kind(p) {
            RegionKind::RePlaceholder { universe, .. } => Some(universe),
            _ => None,
        },
        _ => None,
    };
    let info = universe.and_then(|u| cx.regions.universe_causes.iter().rev().find(|(x, _)| *x == u)).map(|(_, i)| i);
    match info {
        // RegionsPlaceholderMismatch
        Some(UniverseInfo::RelateTys { expected, found }) => {
            let mut diag = Diag::new("E0308", span, "mismatched types");
            diag.span_label(span, "one type is more general than the other");
            diag.note(format!("expected `{}`\n   found `{}`", cx.ty_string(*expected), cx.ty_string(*found)));
            diag
        }
        _ => Diag::new("", span, "higher-ranked lifetime error"),
    }
}

/// construct_generic_bound_failure (E0309, E0310, E0311) for a type test.
fn report_type_test(cx: &DiagCx<'_>, test: usize) -> Option<Diag> {
    let t = cx.regions.type_tests.get(test)?;
    let span = crate::mir::Span { file: cx.prog.def(cx.owner).file, lo: t.span.lo, hi: t.span.hi };
    let kind = cx.ty_string(t.generic_kind);
    let is_alias = matches!(cx.tcx.tys.kind(t.generic_kind), TyKind::Alias(..));
    let what = if is_alias { "associated type" } else { "parameter type" };
    let lower = to_error_region_vid(cx, t.lower_bound).and_then(|r| give_region_a_name(cx, r));
    match lower {
        Some(name) => {
            let (code, bound) = match name.source {
                RegionNameSource::Static => ("E0310", "'static".to_string()),
                _ if name.was_named() => ("E0309", name.name.clone()),
                _ => ("E0311", name.name.clone()),
            };
            let mut d = Diag::new(code, span, format!("the {what} `{kind}` may not live long enough"));
            d.span_label(span, format!("...so that the type `{kind}` will meet its required lifetime bounds"));
            if code != "E0310" { d.span_label(span, format!("the {what} `{kind}` must be valid for the lifetime `{bound}` as defined here...")); }
            let _ = RegionKind::ReStatic;
            Some(d)
        }
        None => Some(Diag::new("", span, format!("`{kind}` does not live long enough"))),
    }
}
