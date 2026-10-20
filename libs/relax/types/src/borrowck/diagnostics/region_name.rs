//! Names for universal regions in lifetime diagnostics: `'a`, `'static`, and synthesized
//! `'1`, `'2`, ... for anonymous ones, with the source span they are highlighted at.
//! ported from N:rustc_borrowck/src/diagnostics/region_name.rs @8d1a764 (1.97.1 where they differ).
//!
//! rustc reads the signature's HIR for names and spans. Relax's universal regions carry no
//! names, so C-NLL5 publishes them as `UniversalNames` (DESIGN-C-NLL5 §4 contract 3): the
//! explicit lifetime names and declaration spans, the per-argument / output / yield / upvar
//! types with their source spans. The naming order, numbering and highlight messages are
//! rustc's.

use super::{Diag, DiagCx};
use crate::borrowck::constraints::RegionVid;
use crate::mir::Span;
use crate::types::{TyId, TyKind};

/// How the defining signature spells a universal region (`external_name` of its definition).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalName {
    Static,
    /// A named lifetime parameter; `early` for ReEarlyParam, else a named ReLateParam.
    Named { name: String, span: Span, early: bool },
    /// The region of a closure's environment (`&self` of the closure body).
    ClosureEnv { span: Span, closure_trait: &'static str },
    /// Any other late-bound region: named, if at all, from the argument/output types.
    Anon,
}

/// An argument (MIR local `_{index + 1}`) as written: its type, its span, and the `&` of an
/// outermost reference.
#[derive(Clone, Debug)]
pub struct InputInfo {
    pub ty: TyId,
    pub ty_span: Span,
    /// The `&` (with its lifetime) when the written type is a reference, else None.
    pub ref_span: Option<Span>,
}

#[derive(Clone, Debug)]
pub struct OutputInfo {
    pub ty: TyId,
    pub span: Span,
    pub ref_span: Option<Span>,
    /// " of async block" style description inserted after "return type".
    pub description: &'static str,
}

/// What a body's universal regions are called. Built by the type checker from the signature.
#[derive(Clone, Debug, Default)]
pub struct UniversalNames {
    pub regions: Vec<(RegionVid, ExternalName)>,
    pub inputs: Vec<InputInfo>,
    pub output: Option<OutputInfo>,
    /// "function", "closure", "constant", ... (tcx.def_descr of the body owner).
    pub def_descr: &'static str,
    pub is_fn_def: bool,
    pub is_const: bool,
    pub is_closure_fn_mut: bool,
}
impl UniversalNames {
    pub fn external(&self, r: RegionVid) -> Option<&ExternalName> { self.regions.iter().find(|(v, _)| *v == r).map(|(_, n)| n) }
}

#[derive(Clone, Debug)]
pub struct RegionName { pub name: String, pub source: RegionNameSource }
#[derive(Clone, Debug)]
pub enum RegionNameSource {
    NamedEarlyParamRegion(Span),
    NamedLateParamRegion(Span),
    Static,
    SynthesizedFreeEnvRegion(Span, &'static str),
    AnonRegionFromArgument(RegionNameHighlight),
    AnonRegionFromUpvar(Span, String),
    AnonRegionFromOutput(RegionNameHighlight, &'static str),
}
#[derive(Clone, Debug)]
pub enum RegionNameHighlight {
    /// The written reference type matched: label its `&`.
    MatchedHirTy(Span),
    MatchedAdtAndSegment(Span),
    CannotMatchHirTy(Span, String),
    Occluded(Span, String),
}
impl RegionName {
    pub fn was_named(&self) -> bool {
        matches!(self.source, RegionNameSource::NamedEarlyParamRegion(_) | RegionNameSource::NamedLateParamRegion(_) | RegionNameSource::Static)
    }
    pub fn span(&self) -> Option<Span> {
        use RegionNameSource::*;
        match &self.source {
            Static => None,
            NamedEarlyParamRegion(s) | NamedLateParamRegion(s) | SynthesizedFreeEnvRegion(s, _) | AnonRegionFromUpvar(s, _) => Some(*s),
            AnonRegionFromArgument(h) | AnonRegionFromOutput(h, _) => Some(match h {
                RegionNameHighlight::MatchedHirTy(s) | RegionNameHighlight::MatchedAdtAndSegment(s)
                | RegionNameHighlight::CannotMatchHirTy(s, _) | RegionNameHighlight::Occluded(s, _) => *s,
            }),
        }
    }
    /// region_name.rs highlight_region_name.
    pub fn highlight(&self, diag: &mut Diag) {
        use RegionNameHighlight::*;
        use RegionNameSource::*;
        let (span, message) = match &self.source {
            NamedEarlyParamRegion(span) | NamedLateParamRegion(span) => (*span, format!("lifetime `{self}` defined here")),
            SynthesizedFreeEnvRegion(span, closure_trait) => {
                diag.note(format!("closure implements `{closure_trait}`, so references to captured variables can't escape the closure"));
                (*span, format!("lifetime `{self}` represents this closure's body"))
            }
            AnonRegionFromArgument(CannotMatchHirTy(span, ty)) => (*span, format!("has type `{ty}`")),
            AnonRegionFromArgument(MatchedHirTy(span)) | AnonRegionFromOutput(MatchedHirTy(span), _) =>
                (*span, format!("let's call the lifetime of this reference `{self}`")),
            AnonRegionFromArgument(MatchedAdtAndSegment(span)) | AnonRegionFromOutput(MatchedAdtAndSegment(span), _) =>
                (*span, format!("let's call this `{self}`")),
            AnonRegionFromArgument(Occluded(span, ty)) => (*span, format!("lifetime `{self}` appears in the type `{ty}`")),
            AnonRegionFromOutput(Occluded(span, ty), description) => (*span, format!("return type{description} `{ty}` contains a lifetime `{self}`")),
            AnonRegionFromUpvar(span, name) => (*span, format!("lifetime `{self}` appears in the type of `{name}`")),
            AnonRegionFromOutput(CannotMatchHirTy(span, ty), description) => (*span, format!("return type{description} is {ty}")),
            Static => return,
        };
        diag.labels.push((span, message, span == diag.span));
    }
}
impl std::fmt::Display for RegionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.name) }
}

/// region_name.rs give_region_a_name: error region, arguments, upvars, output; names are
/// cached and numbered `'1`, `'2`, ... in the order they are first asked for.
pub fn give_region_a_name(cx: &DiagCx<'_>, fr: RegionVid) -> Option<RegionName> {
    assert!(cx.regions.regioncx.universal_region_relations.universal_regions.is_universal_region(fr), "region {fr} is not universal");
    if let Some(name) = cx.region_names.borrow().get(&fr) { return Some(name.clone()); }
    let name = give_name_from_error_region(cx, fr)
        .or_else(|| give_name_if_anonymous_region_appears_in_arguments(cx, fr))
        .or_else(|| give_name_if_anonymous_region_appears_in_upvars(cx, fr))
        .or_else(|| give_name_if_anonymous_region_appears_in_output(cx, fr));
    if let Some(name) = &name { cx.region_names.borrow_mut().insert(fr, name.clone()); }
    name
}

fn synthesize_region_name(cx: &DiagCx<'_>) -> String {
    let c = cx.region_counter.get();
    cx.region_counter.set(c + 1);
    format!("'{c}")
}

fn give_name_from_error_region(cx: &DiagCx<'_>, fr: RegionVid) -> Option<RegionName> {
    match cx.names.external(fr)? {
        ExternalName::Static => Some(RegionName { name: "'static".into(), source: RegionNameSource::Static }),
        ExternalName::Named { name, span, early } => Some(RegionName {
            name: name.clone(),
            source: if *early { RegionNameSource::NamedEarlyParamRegion(*span) } else { RegionNameSource::NamedLateParamRegion(*span) },
        }),
        ExternalName::ClosureEnv { span, closure_trait } =>
            Some(RegionName { name: synthesize_region_name(cx), source: RegionNameSource::SynthesizedFreeEnvRegion(*span, closure_trait) }),
        ExternalName::Anon => None,
    }
}

/// The ty mentions `fr` (as a region variable of the universal-region skeleton).
fn ty_mentions(cx: &DiagCx<'_>, ty: TyId, fr: RegionVid) -> bool { super::explain_borrow::mentions_region(cx, ty, fr) }

/// Whether the outermost reference of `ty` has region `fr`.
fn outer_ref_region_is(cx: &DiagCx<'_>, ty: TyId, fr: RegionVid) -> bool {
    use crate::types::RegionKind;
    match cx.tcx.tys.kind(ty) {
        TyKind::Ref(_, _, r) => matches!(cx.tcx.tys.region_kind(*r), RegionKind::ReVar(v) if v as usize == fr),
        _ => false,
    }
}

fn highlight_for(cx: &DiagCx<'_>, ty: TyId, ty_span: Span, ref_span: Option<Span>, fr: RegionVid, name: &str) -> RegionNameHighlight {
    if outer_ref_region_is(cx, ty, fr) {
        if let Some(s) = ref_span { return RegionNameHighlight::MatchedHirTy(s); }
    }
    let mut type_name = cx.ty_string(ty);
    if outer_ref_region_is(cx, ty, fr) {
        // The highlighted region prints as its name: `&'1 T`.
        if let Some(rest) = type_name.strip_prefix('&') { type_name = format!("&{name} {rest}"); }
    } else if matches!(cx.tcx.tys.kind(ty), TyKind::Adt(..)) {
        return RegionNameHighlight::MatchedAdtAndSegment(ty_span);
    }
    RegionNameHighlight::CannotMatchHirTy(ty_span, type_name)
}

fn give_name_if_anonymous_region_appears_in_arguments(cx: &DiagCx<'_>, fr: RegionVid) -> Option<RegionName> {
    let index = cx.names.inputs.iter().position(|i| ty_mentions(cx, i.ty, fr))?;
    let input = &cx.names.inputs[index];
    let name = synthesize_region_name(cx);
    let highlight = highlight_for(cx, input.ty, input.ty_span, input.ref_span, fr, &name);
    Some(RegionName { name, source: RegionNameSource::AnonRegionFromArgument(highlight) })
}

fn give_name_if_anonymous_region_appears_in_upvars(cx: &DiagCx<'_>, fr: RegionVid) -> Option<RegionName> {
    let upvar = cx.body.upvars.iter().find(|u| ty_mentions(cx, u.ty, fr))?;
    Some(RegionName { name: synthesize_region_name(cx), source: RegionNameSource::AnonRegionFromUpvar(upvar.path_span, upvar.name.clone()) })
}

fn give_name_if_anonymous_region_appears_in_output(cx: &DiagCx<'_>, fr: RegionVid) -> Option<RegionName> {
    let output = cx.names.output.as_ref()?;
    if !ty_mentions(cx, output.ty, fr) { return None; }
    let name = synthesize_region_name(cx);
    let highlight = highlight_for(cx, output.ty, output.span, output.ref_span, fr, &name);
    Some(RegionName { name, source: RegionNameSource::AnonRegionFromOutput(highlight, output.description) })
}

/// region_infer get_var_name_and_span_for_region: the local (argument) or upvar whose type
/// mentions `fr`, with the span of its declaration.
pub fn get_var_name_and_span_for_region(cx: &DiagCx<'_>, fr: RegionVid) -> Option<(Option<String>, Span)> {
    for (i, input) in cx.names.inputs.iter().enumerate() {
        if ty_mentions(cx, input.ty, fr) {
            let local = Local(i as u32 + 1);
            let span = cx.body.local_decls.get(local.0 as usize).map_or(input.ty_span, |d| d.source_info.span);
            return Some((cx.local_name(local).map(str::to_string), span));
        }
    }
    cx.body.upvars.iter().find(|u| ty_mentions(cx, u.ty, fr)).map(|u| (Some(u.name.clone()), u.path_span))
}
use crate::mir::Local;
