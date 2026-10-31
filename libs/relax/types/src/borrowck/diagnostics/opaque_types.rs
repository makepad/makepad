//! The deferred opaque-type errors of a body: E0700 (a hidden type captures a lifetime the opaque type does
//! not), the opaque type used twice with different lifetimes, a non-defining use in its defining scope.
//! ported from N:rustc_borrowck/src/diagnostics/opaque_types.rs `report_opaque_type_errors` @8d1a764,
//! rustc_trait_selection `unexpected_hidden_region_diagnostic` and `NonDefiningUseReason::report`.
//! Reduced: the message, the code and the primary span; no region explanations or suggestions.

use super::{Diag, DiagCx};
use crate::borrowck::region_infer::opaque_types::{DeferredOpaqueTypeError, NonDefiningUseReason};
use crate::consts::GenericArg;
use crate::infer::opaque_types::OpaqueTypeKey;
use crate::mir::Span;
use crate::types::{AliasTyKind, TyKind};

fn opaque_string(cx: &DiagCx<'_>, key: &OpaqueTypeKey) -> String {
    cx.ty_string(cx.tcx.alias_ty_with_args(AliasTyKind::Opaque { def_id: key.def_id }, &key.args))
}

fn arg_string(cx: &DiagCx<'_>, arg: GenericArg) -> String {
    match arg {
        GenericArg::Type(t) => cx.ty_string(t),
        GenericArg::Lifetime(r) => format!("{:?}", cx.tcx.tys.region_kind(r)),
        GenericArg::Const(c) => cx.ty_string(cx.tcx.tys.intern(TyKind::Const(c))),
    }
}

pub fn report_opaque_type_errors(cx: &DiagCx<'_>, errors: &[DeferredOpaqueTypeError]) -> Vec<Diag> {
    let file = cx.prog.def(cx.owner).file;
    let at = |s: (u32, u32)| Span { file, lo: s.0, hi: s.1.max(s.0) };
    let mut out = Vec::new();
    let mut last_unexpected_hidden_region: Option<(Span, String, String)> = None;
    for error in errors {
        match error {
            DeferredOpaqueTypeError::InvalidOpaqueTypeArgs(NonDefiningUseReason::Tainted, _) => {}
            DeferredOpaqueTypeError::InvalidOpaqueTypeArgs(NonDefiningUseReason::NotAParam { opaque_type_key, param_index }, span) => {
                let span = at(*span);
                let arg = opaque_type_key.args[*param_index];
                let kind = match arg { GenericArg::Type(_) => "type", GenericArg::Lifetime(_) => "lifetime", GenericArg::Const(_) => "constant" };
                let found = match arg { GenericArg::Lifetime(r) if cx.tcx.tys.region_kind(r) == crate::types::RegionKind::ReStatic => "'static".to_string(),
                    GenericArg::Lifetime(_) => "'_".to_string(), _ => arg_string(cx, arg) };
                let mut d = Diag::new("E0792", span, format!("expected generic {kind} parameter, found `{found}`"));
                d.span_label(span, format!("this generic parameter of the opaque type is not a parameter of the defining item"));
                out.push(d);
            }
            DeferredOpaqueTypeError::InvalidOpaqueTypeArgs(NonDefiningUseReason::DuplicateParam { .. }, span) => {
                out.push(Diag::new("", at(*span), "non-defining opaque type use in defining scope"));
            }
            DeferredOpaqueTypeError::LifetimeMismatchOpaqueParam { arg, prev, span, prev_span } => {
                let span = at(*span);
                let mut d = Diag::new("", span, "opaque type used twice with different lifetimes");
                d.span_label(span, format!("lifetime `{}` used here", arg_string(cx, *arg)));
                d.span_label(at(*prev_span), format!("lifetime `{}` previously used here", arg_string(cx, *prev)));
                d.note("if all non-lifetime generic parameters are the same, but the lifetime parameters differ, it is not possible to differentiate the opaque types");
                out.push(d);
            }
            DeferredOpaqueTypeError::UnexpectedHiddenRegion { opaque_type_key, hidden_type, span, .. } => {
                let span = at(*span);
                let (opaque, hidden) = (opaque_string(cx, opaque_type_key), cx.ty_string(*hidden_type));
                // The same hidden type of the same opaque at the same place is reported once.
                if last_unexpected_hidden_region.as_ref().is_some_and(|(s, h, o)| *s == span && *h == hidden && *o == opaque) { continue; }
                last_unexpected_hidden_region = Some((span, hidden.clone(), opaque.clone()));
                let mut d = Diag::new("E0700", span, format!("hidden type for `{opaque}` captures lifetime that does not appear in bounds"));
                d.span_label(span, format!("hidden type `{hidden}` captures a lifetime that does not appear in the bounds of the opaque type"));
                out.push(d);
            }
            DeferredOpaqueTypeError::NonDefiningUseInDefiningScope { opaque_type_key, span } => {
                out.push(Diag::new("", at(*span), format!("non-defining use of `{}` in the defining scope", opaque_string(cx, opaque_type_key))));
            }
        }
    }
    out
}
