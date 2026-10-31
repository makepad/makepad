//! Check the bodies of `const`s, `static`s and `const fn`s for illegal operations:
//! rustc_const_eval/src/check_consts/{mod,check,ops}.rs (1.97.1), carrying the semantics of today's
//! BIR checker (borrowck/check_consts.rs, rc-consts review fixes included):
//! E0015 (calls, closures, fn pointers, trait methods/operators), E0493 (live drops), E0492/E0764
//! (escaping borrows, secondary), E0625 (thread locals), pointer-to-int casts and pointer
//! comparisons.
//!
//! Relax-specific policy (see qualifs.rs for the qualif half):
//! - `#![feature(const_trait_impl)]` allows every trait method call (Relax has no `const trait`
//!   machinery to judge the call with); without it a trait method call is never const.
//! - `#![feature(const_destruct)]` / `const_precise_live_drops` allow every drop.
//! - Callees of Relax's own std are judged by `const fn`, like any other (the std const audit of
//!   rc-consts2 made its markings mirror the real library's).
//! - No stability checking (`staged_api`, `rustc_const_unstable`), no intrinsic stability, no
//!   `-Zunleash-the-miri-inside-of-you`, no coroutines (a body with coroutine locals is `Blocked`).
use super::qualifs::{in_any_value_of_ty, Blocked, ConstQualifs, ConstQualifsProvider};
use crate::mir::*;
use crate::program::{DefId, Program};
use crate::tcx::{Tcx, TypingEnv};
use std::cell::RefCell;

mod check;
mod ops;
#[cfg(test)]
pub(crate) mod tests;

pub use check::Checker;
pub use ops::{ConstError, Importance};

/// rustc_hir `ConstContext`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstContext {
    ConstFn,
    /// A `static` (`true`: `static mut`).
    Static(bool),
    Const { inline: bool },
}

impl ConstContext {
    /// hir::ConstContext's Display (`constants`, `statics`, `constant functions`).
    pub fn plural(self) -> &'static str {
        match self {
            ConstContext::ConstFn => "constant functions",
            ConstContext::Static(_) => "statics",
            ConstContext::Const { .. } => "constants",
        }
    }
}

/// `#![feature(..)]` gates that change what a const context may do.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConstFeatures {
    /// `const_trait_impl`: trait method calls may be const.
    pub const_traits: bool,
    /// `const_destruct` / `const_precise_live_drops`: drops may be const.
    pub const_drops: bool,
}

/// Information about the item currently being const-checked, as well as a reference to the global
/// context.
pub struct ConstCx<'a> {
    pub body: &'a Body,
    pub prog: &'a Program,
    pub tcx: &'a Tcx,
    pub env: TypingEnv,
    pub const_kind: Option<ConstContext>,
    pub features: ConstFeatures,
    pub provider: &'a dyn ConstQualifsProvider,
    blocked: RefCell<Option<Blocked>>,
}

impl<'a> ConstCx<'a> {
    pub fn new(prog: &'a Program, tcx: &'a Tcx, env: TypingEnv, body: &'a Body, provider: &'a dyn ConstQualifsProvider) -> Self {
        let owner = body.source.owner;
        let krate = &prog.crates[prog.def(owner).krate as usize];
        let features = ConstFeatures {
            const_traits: krate.features(prog).has("const_trait_impl"),
            const_drops: krate.features(prog).has("const_destruct") || krate.features(prog).has("const_precise_live_drops"),
        };
        let const_kind = Self::body_const_context(prog, tcx, body);
        ConstCx { body, prog, tcx, env, const_kind, features, provider, blocked: RefCell::new(None) }
    }

    /// rustc `hir_body_const_context`: the const context a body is checked in, if any. Closure
    /// bodies are not const contexts (a closure is never `const`).
    pub fn body_const_context(prog: &Program, tcx: &Tcx, body: &Body) -> Option<ConstContext> {
        let owner = body.source.owner;
        // `#[rust(expr)]` field initial values are generated consts holding arbitrary run-time
        // expressions (front derive.rs default_consts, marked `#[relax_live_default]`): not constants
        // of the program
        if is_live_default(prog, owner) {
            return None;
        }
        match body.source.kind {
            BodyKind::Const | BodyKind::AnonConst => Some(ConstContext::Const { inline: false }),
            BodyKind::InlineConst => Some(ConstContext::Const { inline: true }),
            BodyKind::Static => {
                let d = prog.def(owner);
                let ast = &prog.files[d.file as usize].ast;
                match &ast.item(d.item).kind {
                    crate::ast::ItemKind::Static(mutable, _, _) => Some(ConstContext::Static(*mutable)),
                    _ => Some(ConstContext::Static(false)),
                }
            }
            BodyKind::Fn => {
                if tcx.fn_sig(prog, owner)?.is_const {
                    Some(ConstContext::ConstFn)
                } else {
                    None
                }
            }
            BodyKind::Closure(_) | BodyKind::Promoted(..) | BodyKind::Shim(_) => None,
        }
    }

    /// Returns the kind of const context this `Item` represents (`const`, `static`, etc.).
    ///
    /// Panics if this `Item` is not const.
    pub fn const_kind(&self) -> ConstContext {
        self.const_kind.expect("`const_kind` must not be called on a non-const fn")
    }

    /// Records a question a provider could not answer; the checker returns `Blocked` at the end.
    pub(crate) fn note_blocked(&self, blocked: Blocked) {
        let mut slot = self.blocked.borrow_mut();
        if slot.is_none() {
            *slot = Some(blocked);
        }
    }

    pub(crate) fn take_blocked(&self) -> Option<Blocked> {
        self.blocked.borrow_mut().take()
    }
}

fn is_live_default(prog: &Program, d: DefId) -> bool {
    let def = prog.def(d);
    let ast = &prog.files[def.file as usize].ast;
    let src = prog.src(def.file);
    ast.list(ast.item(def.item).attrs).iter().any(|a| crate::cfg::tok_str(src, a.toks.lo as usize) == b"relax_live_default")
}

/// The result of const-checking one body.
#[derive(Debug)]
pub struct ConstCheck {
    /// The qualifs of the return place (rustc `mir_const_qualif`).
    pub qualifs: ConstQualifs,
    /// rustc's primary errors; the secondary ones (escaping borrows) only when no primary error was
    /// found in the same body.
    pub errors: Vec<ConstError>,
}

/// rustc `mir_const_qualif`: const-check `body` (a `const`, `static`, `const fn`, inline or anonymous
/// const) and return the qualifs of its return place and the errors. A body that is not a const
/// context has nothing to check.
pub fn mir_const_qualif(cx: &ConstCx<'_>) -> Result<ConstCheck, Blocked> {
    super::qualifs::check_body_types(cx)?;
    if cx.const_kind.is_none() {
        let ret = cx.body.local_decls[0].ty;
        return Ok(ConstCheck { qualifs: in_any_value_of_ty(cx, ret, false), errors: Vec::new() });
    }
    let ret = cx.body.local_decls[0].ty;
    if cx.tcx.tys.flags(ret).intersects(crate::types::TypeFlags::HAS_ERROR) {
        // It's possible to reach here without an error being emitted (#121103).
        return Ok(ConstCheck { qualifs: ConstQualifs { tainted_by_errors: true, ..Default::default() }, errors: Vec::new() });
    }
    let mut checker = Checker::new(cx);
    checker.check_body();
    let qualifs = checker.qualifs_in_return_place();
    let errors = checker.finish();
    if let Some(blocked) = cx.take_blocked() {
        return Err(blocked);
    }
    Ok(ConstCheck { qualifs, errors })
}
