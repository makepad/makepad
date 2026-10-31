//! rustc_const_eval/src/check_consts/ops.rs: the operations that are *not allowed* in a const
//! context, as errors (code, message, span). Messages follow today's BIR checker; the code is
//! rustc's (`""`: an error without a code).
use super::{ConstContext, ConstCx};
use crate::mir::{CallSource, Span};
use crate::program::{DefId, DefKind};
use crate::types::{TyId, TyKind};

/// check_consts/ops.rs `DiagImportance`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Importance {
    /// An operation that must be removed for const-checking to pass.
    Primary,
    /// An operation that causes const-checking to fail, but is usually a side-effect of a `Primary`
    /// operation elsewhere.
    Secondary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstError {
    pub code: &'static str,
    pub message: String,
    pub span: Span,
    pub importance: Importance,
}

fn error(code: &'static str, message: String, span: Span, importance: Importance) -> ConstError {
    ConstError { code, message, span, importance }
}

fn kind(cx: &ConstCx<'_>) -> &'static str {
    cx.const_kind().plural()
}

/// A function call where the callee is a pointer.
pub(super) fn fn_call_indirect(cx: &ConstCx<'_>, span: Span) -> ConstError {
    error("E0015", format!("function pointer calls are not allowed in {}", kind(cx)), span, Importance::Primary)
}

/// A function call where the callee is not marked as `const` (or a trait method that cannot be
/// const).
pub(super) fn fn_call_non_const(cx: &ConstCx<'_>, callee: DefId, span: Span, call_source: CallSource) -> ConstError {
    let (prog, tcx) = (cx.prog, cx.tcx);
    let def = prog.def(callee);
    let parent = def.parent;
    let parent_trait = parent != crate::program::NO_DEF && prog.def(parent).kind == DefKind::Trait;
    // a call through an `Fn*` trait is a call of a closure or function value
    let fn_trait = parent_trait && [crate::lang_items::LangItem::Fn, crate::lang_items::LangItem::FnMut, crate::lang_items::LangItem::FnOnce].iter().any(|item| tcx.lang_items.get(*item) == Some(parent));
    let message = if fn_trait {
        format!("cannot call non-const closure in {}", kind(cx))
    } else if matches!(call_source, CallSource::OverloadedOperator | CallSource::MatchCmp) {
        format!("cannot call non-const operator in {}", kind(cx))
    } else {
        let descr = if tcx.fn_sig(prog, callee).is_some_and(|s| s.self_kind != 0) {
            "method"
        } else if def.kind == DefKind::AssocFn {
            "associated function"
        } else {
            "function"
        };
        format!("cannot call non-const {} `{}` in {}", descr, prog.def_path(callee), kind(cx))
    };
    error("E0015", message, span, Importance::Primary)
}

/// A call to a `panic()` lang item where the first argument is _not_ a `&str`.
pub(super) fn panic_non_str(span: Span) -> ConstError {
    error("E0015", "argument to `panic!()` in a const context must have type `&str`".to_string(), span, Importance::Primary)
}

/// A live drop of a value that needs a non-const destructor.
pub(super) fn live_drop(cx: &ConstCx<'_>, dropped_ty: TyId, span: Span) -> ConstError {
    error(
        "E0493",
        format!("destructor of `{}` cannot be evaluated at compile-time", crate::typeck::ty_to_string(false, cx.prog, cx.tcx, dropped_ty)),
        span,
        Importance::Primary,
    )
}

/// A borrow of a type that contains an `UnsafeCell` somewhere. The borrow might escape to the final
/// value of the constant, and thus we cannot allow this (for now).
pub(super) fn escaping_cell_borrow(cx: &ConstCx<'_>, span: Span) -> ConstError {
    error("E0492", format!("{} cannot refer to interior mutable data", kind(cx)), span, Importance::Secondary)
}

/// `&mut` borrows in the trailing expression of a constant which uses the "enclosing scopes rule" to
/// leak its locals into anonymous static or const items.
pub(super) fn escaping_mut_borrow(cx: &ConstCx<'_>, span: Span) -> ConstError {
    let message = match cx.const_kind() {
        ConstContext::Static(_) => "mutable borrows of temporaries that have their lifetime extended until the end of the program are not allowed".to_string(),
        _ => format!("mutable references are not allowed in the final value of {}", kind(cx)),
    };
    error("E0764", message, span, Importance::Secondary)
}

/// Comparing raw pointers for equality.
pub(super) fn raw_ptr_comparison(span: Span) -> ConstError {
    error("", "pointers cannot be reliably compared during const eval".to_string(), span, Importance::Primary)
}

/// Casting raw pointer or function pointer to an integer.
pub(super) fn raw_ptr_to_int_cast(span: Span) -> ConstError {
    error("", "pointers cannot be cast to integers during const eval".to_string(), span, Importance::Primary)
}

/// An access to a thread-local `static`.
pub(super) fn thread_local_access(span: Span) -> ConstError {
    error("E0625", "thread-local statics cannot be accessed at compile-time".to_string(), span, Importance::Primary)
}

/// Whether a type is a function pointer or a raw pointer.
pub(super) fn is_ptr_like(cx: &ConstCx<'_>, ty: TyId) -> bool {
    matches!(cx.tcx.tys.kind(ty), TyKind::Ptr(..) | TyKind::FnPtr(..))
}
