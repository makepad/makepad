//! MIR construction from a typeck `Body`: port of rustc_mir_build (builder/ + thir/cx), S3.
//!
//! Contracts for the consumers (lanes C and D):
//! - `build_mir` returns analysis MIR (`MirPhase::Built`, `promoted` empty, S5 fills it). A body it cannot
//!   build is a `BuildError` (the body is "unchecked(reason)", counted, never clean, never an error).
//! - Region slots: the type of a closure's environment argument `_1` and the type of every ADT aggregate keep
//!   their region slots, one `ReErased` per slot (never an empty `RegionArgs`); borrowck renumbers them.
//! - A closure/coroutine aggregate's args are the parent's identity args followed by
//!   `[kind_ty, sig_as_fn_ptr_ty, tupled_upvars_ty]` (the layout `AggregateKind` documents).
//! - Closures are separate bodies: the parent only emits the `Aggregate`; nested bodies are built and checked
//!   first by the driver, all from one `BuildRoot` (one `ScopeTree` and typeck facts per typeck root).
use crate::borrowck::body::{BodyKind as OwnerKind, CheckedBody};
use crate::mir::{Body, Span};
use crate::program::Program;
use crate::region_scope::ScopeTree;
use crate::tcx::{EvalCx, Tcx, TypingEnv};
use crate::typeck;
use std::cell::RefCell;
use std::rc::Rc;

/// `unpack!(block = expr)`: run a fallible block-threading step, binding the new block.
macro_rules! unpack {
    ($x:ident = $c:expr) => {{
        let $crate::mir::build::BlockAnd(b, v) = $c?;
        $x = b;
        v
    }};
}

pub mod block;
pub mod builder;
pub mod driver;
pub mod driver_borrowck;
pub mod cfg;
pub mod expr;
pub mod matches;
mod misc;
pub mod pattern;
pub mod place;
pub mod scope;
pub mod thir;

pub use builder::{Builder, GuardFrame, GuardFrameLocal, LocalsForNode};
pub use cfg::{BlockAnd, BlockAndExtension, Cfg};
pub use place::{PlaceBase, PlaceBuilder};
pub use scope::{BreakableTarget, DropKind, Scopes};

/// The inputs every body of one typeck root shares.
pub struct BuildCx<'a> {
    pub prog: &'a Program,
    pub tcx: &'a Tcx,
    pub env: TypingEnv,
    pub eval: &'a EvalCx<'a>,
}

/// Why a body is not built. Counted per variant by the driver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnsupportedReason {
    /// `#![feature(custom_mir)]` bodies (D5).
    CustomMir,
    /// C-variadic bodies (D5).
    CVariadic,
    InlineAsm,
    /// Unstable-only rvalues the contract does not model: `Reborrow`/CoerceShared.
    Reborrow,
    UnsafeBinder,
    AsyncDrop,
    /// typeck reported errors for this body.
    TypeckTainted,
    /// A typeck fact the construct needs is missing.
    UnresolvedFact(&'static str),
    /// Not built yet in the current step (skeleton): names the construct.
    NotYetBuilt(&'static str),
    /// An invariant of the builder was violated (rustc `span_bug!`/`bug!`): the body is left unchecked.
    Internal(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildError {
    pub owner: CheckedBody,
    pub span: Span,
    pub reason: UnsupportedReason,
}
pub type BuildResult<T> = Result<T, BuildError>;

/// One typeck root: its region scope tree (computed once) and the THIR of each body built from it
/// (computed once per body, shared by every use of that body).
pub struct BuildRoot<'a> {
    pub cx: BuildCx<'a>,
    pub typeck: &'a typeck::Body,
    pub scope_tree: ScopeTree,
    thirs: RefCell<Vec<(CheckedBody, Rc<thir::ThirBody>)>>,
}

impl<'a> BuildRoot<'a> {
    pub fn new(cx: BuildCx<'a>, typeck: &'a typeck::Body) -> Result<Self, String> {
        let scope_tree = crate::region_scope::region_scope_tree(cx.prog, typeck)?;
        Ok(Self { cx, typeck, scope_tree, thirs: RefCell::new(Vec::new()) })
    }
    /// The THIR of `owner`, built on first use (rustc `thir_body`).
    pub fn thir_body(&self, owner: CheckedBody) -> BuildResult<Rc<thir::ThirBody>> {
        if let Some((_, body)) = self.thirs.borrow().iter().find(|(o, _)| *o == owner) {
            return Ok(body.clone());
        }
        let body = Rc::new(thir::Cx::new(self, owner).thir_body()?);
        self.thirs.borrow_mut().push((owner, body.clone()));
        Ok(body)
    }
}

/// Build the analysis MIR of one body owner of `root` (fn, const, static, inline/anon const, closure).
pub fn build_mir(root: &BuildRoot<'_>, owner: CheckedBody) -> BuildResult<Body> {
    let thir = root.thir_body(owner)?;
    Builder::build(root, owner, &thir)
}

impl BuildError {
    pub fn new(owner: CheckedBody, span: Span, reason: UnsupportedReason) -> Self {
        Self { owner, span, reason }
    }
}
/// The AST-level kind of owner, for diagnostics and counters.
pub fn owner_kind_name(kind: OwnerKind) -> &'static str {
    match kind {
        OwnerKind::Function => "fn",
        OwnerKind::Const => "const",
        OwnerKind::Static => "static",
        OwnerKind::AssocConst => "assoc const",
        OwnerKind::InlineConst(_) => "inline const",
        OwnerKind::AnonConst(_) => "anon const",
        OwnerKind::Closure(_) => "closure",
    }
}

#[cfg(test)]
mod tests;
