//! Shared body IR, following rustc_middle/src/mir/mod.rs:206 (Body).
//! M0 defines the representation only; producers and consumers migrate in later steps.

pub mod dataflow;
pub mod drop_flag_effects;
pub mod move_paths;
pub mod build;
pub mod transform;
pub mod shim;
mod place_ty;
pub mod pretty;
pub mod syntax;
pub mod validate;
pub mod visit;
pub use syntax::*;
pub use place_ty::PlaceTy;

use crate::program::{DefId, Sym};
use crate::types::TyId;

macro_rules! index {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);
    };
}
index!(Local);
index!(BasicBlock);
index!(SourceScope);
index!(PromotedId);
index!(UserTypeAnnotationIndex);
index!(VariantIdx);
index!(FieldIdx);
index!(AllocId);

/// Pointer permissions, not allocation permissions. Immutable provenance is always shared.
/// rustc_middle/src/mir/interpret/pointer.rs:82,118.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CtfeProvenance {
    pub alloc: AllocId,
    immutable: bool,
    shared_ref: bool,
}
impl CtfeProvenance {
    pub fn new(alloc: AllocId) -> Self {
        Self {
            alloc,
            immutable: false,
            shared_ref: false,
        }
    }
    pub fn immutable(self) -> bool {
        self.immutable
    }
    pub fn shared_ref(self) -> bool {
        self.shared_ref
    }
    pub fn as_shared_ref(self) -> Self {
        Self {
            shared_ref: true,
            ..self
        }
    }
    pub fn as_immutable(self) -> Self {
        Self {
            immutable: true,
            shared_ref: true,
            ..self
        }
    }
}

/// Temporary region slot for A1; unlike TyId it is not an interned type.
/// rustc_middle/src/mir/syntax.rs:1382 carries ty::Region on Ref.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Region(pub u32);
impl Region {
    pub const ERASED: Region = Region(u32::MAX);
}

/// File-qualified byte offsets, preserving BIR's lo/hi and body file identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Span {
    pub file: u32,
    pub lo: u32,
    pub hi: u32,
}

/// rustc_middle/src/mir/mod.rs:842.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceInfo {
    pub span: Span,
    pub scope: SourceScope,
}

/// rustc_middle/src/mir/mod.rs:1478; lexical scope identity survives lowering.
#[derive(Clone, Debug)]
pub struct SourceScopeData {
    pub span: Span,
    pub parent_scope: Option<SourceScope>,
}

/// rustc_middle/src/mir/mod.rs:118. Relax closures currently have expression identities.
#[derive(Clone, Debug)]
pub struct MirSource {
    pub owner: DefId,
    pub kind: BodyKind,
}
#[derive(Clone, Debug)]
pub enum BodyKind {
    Fn,
    Closure(u32),
    Const,
    Static,
    InlineConst,
    AnonConst,
    Promoted(DefId, PromotedId),
    Shim(ShimKind),
}
/// rustc_middle/src/ty/instance.rs:62; instance arguments remain on the owning query.
#[derive(Clone, Debug)]
pub enum ShimKind {
    FnPtr,
    ClosureOnce,
    DropGlue(TyId),
    Clone(TyId),
    Virtual(u32),
    Reify,
}

/// rustc_middle/src/mir/mod.rs:146. This slot is present before coroutine lowering.
#[derive(Clone, Debug)]
pub struct CoroutineInfo {
    pub yield_ty: TyId,
    pub resume_ty: TyId,
    pub kind: CoroutineKind,
}
/// rustc_hir/src/hir.rs Movability (`static` coroutines are Static). Only movable coroutines
/// reject references into their own frame across a suspension (rustc_borrowck check_movable_place);
/// read it through `CoroutineKind::movability`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Movability {
    Movable,
    Static,
}
/// rustc_hir/src/hir.rs:2308 CoroutineKind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoroutineKind {
    /// A coroutine from a desugaring (async/gen block, or the body of an async/gen fn or closure).
    Desugared(CoroutineDesugaring, CoroutineSource),
    /// A coroutine literal created via a `yield` inside a closure.
    Coroutine(Movability),
}
impl CoroutineKind {
    /// hir.rs:2317.
    pub fn movability(self) -> Movability {
        match self {
            CoroutineKind::Desugared(CoroutineDesugaring::Async | CoroutineDesugaring::AsyncGen, _) => Movability::Static,
            CoroutineKind::Desugared(CoroutineDesugaring::Gen, _) => Movability::Movable,
            CoroutineKind::Coroutine(movability) => movability,
        }
    }
}
/// hir.rs:2381.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoroutineDesugaring {
    Async,
    Gen,
    AsyncGen,
}
/// hir.rs:2358: what the desugared coroutine was written as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoroutineSource {
    /// An explicit `async`/`gen` block.
    Block,
    /// An explicit `async`/`gen` closure.
    Closure,
    /// The block generated as the body of an async/gen fn.
    Fn,
}

/// rustc_middle/src/mir/mod.rs:967. Region skeletons belong to borrowck, not local declarations.
#[derive(Clone, Debug)]
pub struct LocalDecl {
    pub ty: TyId,
    pub mutability: Mutability,
    pub user_ty: Vec<UserTypeProjection>,
    pub source_info: SourceInfo,
    pub kind: LocalKind,
    pub local_info: LocalInfo,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalKind {
    Return,
    Arg,
    UserVar,
    Temp,
}
/// Diagnostic facts from rustc_middle/src/mir/mod.rs:1074 (LocalInfo), not borrowck constraints.
#[derive(Clone, Debug, Default)]
pub enum LocalInfo {
    /// A user-defined local variable or function parameter (mod.rs:1080).
    User(BindingForm),
    /// A temporary that references the static (mod.rs:1082).
    StaticRef { def_id: DefId, is_thread_local: bool },
    /// A temporary that references the const (mod.rs:1084).
    ConstRef { def_id: DefId },
    /// A temporary created for an aggregate operand (mod.rs:1087).
    AggregateTemp,
    /// A temporary created for a subexpression of a block's tail expression (mod.rs:1090).
    BlockTailTemp(BlockTailInfo),
    /// A temporary created during evaluation of an `if` predicate, subject to Edition 2024
    /// temporary lifetime rules (mod.rs:1093). `if_then` is the `if` expression's id in the
    /// body's file (rustc: HirId).
    IfThenRescopeTemp { if_then: u32 },
    /// A transparent alias for the place CopyForDeref copied from (mod.rs:1099).
    DerefTemp,
    /// A temporary created for borrow checking (mod.rs:1101).
    FakeBorrow,
    /// A local without anything interesting about it.
    #[default]
    Boring,
}
/// rustc_middle/src/mir/mod.rs:924. Solely diagnostics for the current crate.
#[derive(Clone, Debug)]
pub enum BindingForm {
    /// A non-`self` binding, or a `self` with an explicit type.
    Var(VarBindingForm),
    /// `self`/`&self`/`&mut self` where the type is implicit.
    ImplicitSelf(ImplicitSelfKind),
    /// The reference used in a guard expression to ensure immutability.
    RefForGuard(Local),
}
/// rustc_middle/src/mir/mod.rs:899.
#[derive(Clone, Debug)]
pub struct VarBindingForm {
    /// `x`, `mut x`, `ref x`, `ref mut x`, `mut ref x` or `mut ref mut x`.
    pub binding_mode: BindingMode,
    /// The span of the explicit type of the binding, if any.
    pub opt_ty_info: Option<Span>,
    /// Place of the RHS of the `=` or the `match` subject; None for `let PATTERN;`;
    /// Some((None, ..)) for `let [mut] x = ...` (the RHS is not a place expression).
    pub opt_match_place: Option<(Option<Place>, Span)>,
    /// The span of the pattern in which this variable was bound.
    pub pat_span: Span,
    /// A binding can be introduced several times by or-patterns (mod.rs:934).
    pub introductions: Vec<VarBindingIntroduction>,
}
/// rustc_middle/src/mir/mod.rs:934.
#[derive(Clone, Copy, Debug)]
pub struct VarBindingIntroduction {
    pub span: Span,
    /// `Foo { x }` instead of `Foo { x: x }`.
    pub is_shorthand: bool,
}
/// rustc_ast_ir BindingMode / ByRef.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingMode(pub ByRef, pub Mutability);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ByRef {
    No,
    Yes(Mutability),
}
/// rustc_hir/src/hir.rs:4201.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImplicitSelfKind {
    Imm,
    Mut,
    RefImm,
    RefMut,
    None,
}
/// rustc_middle/src/mir/mod.rs:1131.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockTailInfo {
    /// Whether the tail's value is ignored by the enclosing statement context.
    pub tail_result_is_ignored: bool,
    /// The span of the tail expression.
    pub span: Span,
}
impl VarBindingForm {
    /// A plain `let x = ...` of an immutable by-value binding with no extra facts.
    pub fn plain(binding_mode: BindingMode, pat_span: Span) -> Self {
        Self { binding_mode, opt_ty_info: None, opt_match_place: None, pat_span, introductions: Vec::new() }
    }
}
impl LocalInfo {
    pub fn user_var(binding_mode: BindingMode, opt_match_place: Option<(Option<Place>, Span)>, pat_span: Span) -> Self {
        LocalInfo::User(BindingForm::Var(VarBindingForm { binding_mode, opt_ty_info: None, opt_match_place, pat_span, introductions: Vec::new() }))
    }
}
impl LocalDecl {
    /// rustc_middle/src/mir/mod.rs:1117: a binding that can itself be made mutable by adding `mut`.
    pub fn can_be_made_mutable(&self) -> bool {
        matches!(
            &self.local_info,
            LocalInfo::User(BindingForm::Var(VarBindingForm { binding_mode: BindingMode(ByRef::No, _), .. }) | BindingForm::ImplicitSelf(ImplicitSelfKind::Imm))
        )
    }
    /// rustc_middle/src/mir/mod.rs:1130: definitely not a `ref` / `ref mut` binding.
    pub fn is_nonref_binding(&self) -> bool {
        matches!(
            &self.local_info,
            LocalInfo::User(BindingForm::Var(VarBindingForm { binding_mode: BindingMode(ByRef::No, _), .. }) | BindingForm::ImplicitSelf(_))
        )
    }
    /// rustc_middle/src/mir/mod.rs:1142: a user variable or argument (not a temporary).
    pub fn is_user_variable(&self) -> bool {
        matches!(&self.local_info, LocalInfo::User(_))
    }
    /// rustc_middle/src/mir/mod.rs:1155: the reference a guard uses for a binding.
    pub fn is_ref_for_guard(&self) -> bool {
        matches!(&self.local_info, LocalInfo::User(BindingForm::RefForGuard(_)))
    }
    /// rustc_middle/src/mir/mod.rs:1160: a temporary that references a static.
    pub fn is_ref_to_static(&self) -> bool {
        matches!(&self.local_info, LocalInfo::StaticRef { .. })
    }
    /// rustc_middle/src/mir/mod.rs:1166.
    pub fn is_ref_to_thread_local(&self) -> bool {
        matches!(&self.local_info, LocalInfo::StaticRef { is_thread_local: true, .. })
    }
}

/// rustc_middle/src/mir/mod.rs:1536 ProjectionKind (= ProjectionElem<(), ()>): the projections of
/// a user type carry neither an index local nor an inferred type; those are place facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UserTypeProjectionElem {
    Deref,
    Field(FieldIdx),
    Index,
    ConstantIndex { offset: u64, min_length: u64, from_end: bool },
    Subslice { from: u64, to: u64, from_end: bool },
    Downcast(Option<Sym>, VariantIdx),
    OpaqueCast,
    Subtype,
}
/// rustc_middle/src/mir/mod.rs:1563. `base` indexes `Body::user_type_annotations`.
#[derive(Clone, Debug)]
pub struct UserTypeProjection {
    pub base: UserTypeAnnotationIndex,
    pub projection: Vec<UserTypeProjectionElem>,
}
/// rustc_middle/src/ty/typeck_results.rs:732. The canonical user-written type of a body
/// annotation and the type the body inferred for it (both with regions renumbered by borrowck).
#[derive(Clone, Debug)]
pub struct CanonicalUserTypeAnnotation {
    pub user_ty: Box<crate::types::user_type::CanonicalUserType>,
    pub span: Span,
    pub inferred_ty: TyId,
}

impl CanonicalUserTypeAnnotation {
    /// A plain `Ty(ty)` annotation with no canonical variables.
    #[cfg(test)]
    pub fn for_test(ty: TyId, span: Span) -> Self {
        use crate::types::user_type::{UserType, UserTypeKind};
        Self {
            user_ty: Box::new(crate::next_solver::canonical::Canonical { value: UserType::new(UserTypeKind::Ty(ty)), max_universe: crate::types::UniverseIndex::ROOT, var_kinds: Vec::new() }),
            span,
            inferred_ty: ty,
        }
    }
}

/// rustc_middle/src/ty/closure.rs UpvarCapture / ty::BorrowKind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpvarCapture {
    ByValue,
    ByRef(UpvarBorrowKind),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum UpvarBorrowKind {
    Immutable,
    UniqueImmutable,
    Mutable,
}
/// rustc_middle/src/ty/closure.rs CapturedPlace (minimum capture, flattened): one entry per
/// upvar field of the closure/coroutine type, in field order. The place is a path in the
/// enclosing body: the root variable and Deref/Field projections.
#[derive(Clone, Debug)]
pub struct CapturedPlace {
    /// The root variable's name (rustc: the ident of `var_hir_id`).
    pub name: String,
    /// The root variable's binding id in the enclosing body (expr_use::Place::root).
    pub var: u32,
    pub root_ty: TyId,
    pub projection: Vec<PlaceElem>,
    /// The type of the captured place (the upvar field type is a reference to it when by ref).
    pub ty: TyId,
    pub capture: UpvarCapture,
    /// Whether the captured place can be mutated through (CapturedPlace::mutability).
    pub mutability: Mutability,
    pub path_span: Span,
    pub kind_span: Span,
}
impl CapturedPlace {
    /// rustc_middle/src/ty/closure.rs is_by_ref.
    pub fn is_by_ref(&self) -> bool {
        matches!(self.capture, UpvarCapture::ByRef(_))
    }
}
#[derive(Clone, Debug)]
pub struct VarDebugInfo {
    pub name: String,
    pub source_info: SourceInfo,
    pub place: Place,
}

/// rustc_middle/src/mir/mod.rs:206. Arguments occupy _1..=arg_count, return is _0.
#[derive(Clone, Debug)]
pub struct Body {
    pub phase: MirPhase,
    pub source: MirSource,
    pub basic_blocks: Vec<BasicBlockData>,
    pub local_decls: Vec<LocalDecl>,
    pub source_scopes: Vec<SourceScopeData>,
    /// The span of the whole body (rustc_middle Body::span).
    pub span: Span,
    pub user_type_annotations: Vec<CanonicalUserTypeAnnotation>,
    /// Captured places of a closure/coroutine body, in upvar field order (empty otherwise).
    pub upvars: Vec<CapturedPlace>,
    /// Global allocations named by this body's constants (rustc: the tcx alloc map);
    /// AllocIds are local to the body.
    pub allocs: AllocMap,
    pub var_debug_info: Vec<VarDebugInfo>,
    pub arg_count: u32,
    pub spread_arg: Option<Local>,
    pub promoted: Vec<Body>,
    pub coroutine: Option<CoroutineInfo>,
}

/// rustc_middle/src/mir/mod.rs:1319. An unfinished builder block may lack a terminator;
/// validation requires all blocks to be finished.
#[derive(Clone, Debug)]
pub struct BasicBlockData {
    pub statements: Vec<Statement>,
    pub terminator: Option<Terminator>,
    pub is_cleanup: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Location {
    pub block: BasicBlock,
    pub statement_index: usize,
}

/// rustc_middle/src/mir/interpret/allocation.rs:95. AllocId is local to the interpreter's
/// allocation table; provenance offsets mark pointer starts, not host addresses.
#[derive(Clone, Debug)]
pub struct Allocation {
    pub bytes: Vec<u8>,
    pub align: u64,
    pub init_mask: Vec<bool>,
    pub deallocated: bool,
    pub provenance: Vec<(u64, CtfeProvenance)>,
    /// Byte fragments retain provenance across partial pointer copies (allocation.rs:95).
    pub provenance_fragments: Vec<(u64, CtfeProvenance, u8)>,
    pub mutability: Mutability,
}
/// rustc_middle/src/mir/interpret/mod.rs GlobalAlloc. AllocIds name entries of one AllocMap.
#[derive(Clone, Debug)]
pub enum GlobalAlloc {
    Memory(Allocation),
    /// Function provenance is stable across interpreter instances sharing this table.
    Function { def: DefId, args: Vec<TyId> },
    /// The address of a static item (identity only; no memory).
    Static(DefId),
}
#[derive(Clone, Debug, Default)]
pub struct AllocMap {
    pub entries: Vec<GlobalAlloc>,
}
impl AllocMap {
    fn push(&mut self, alloc: GlobalAlloc) -> AllocId {
        let id = AllocId(u32::try_from(self.entries.len()).expect("too many MIR allocations"));
        self.entries.push(alloc);
        id
    }
    pub fn insert(&mut self, allocation: Allocation) -> AllocId {
        self.push(GlobalAlloc::Memory(allocation))
    }
    pub fn insert_function(&mut self, def: DefId, args: Vec<TyId>) -> AllocId {
        self.push(GlobalAlloc::Function { def, args })
    }
    /// rustc `reserve_and_set_static_alloc`: one id per static, so a pointer to the static
    /// has the same provenance everywhere in the body.
    pub fn static_alloc(&mut self, def: DefId) -> AllocId {
        if let Some(i) = self.entries.iter().position(|e| matches!(e, GlobalAlloc::Static(d) if *d == def)) {
            return AllocId(i as u32);
        }
        self.push(GlobalAlloc::Static(def))
    }
    /// The memory behind `id`; functions and statics have none.
    pub fn get(&self, id: AllocId) -> Option<&Allocation> {
        match self.entries.get(id.0 as usize)? {
            GlobalAlloc::Memory(allocation) => Some(allocation),
            _ => None,
        }
    }
    pub fn global(&self, id: AllocId) -> Option<&GlobalAlloc> {
        self.entries.get(id.0 as usize)
    }
}
impl Const {
    /// rustc_middle/src/mir/consts.rs ConstOperand::check_static_ptr: the static a pointer
    /// constant points to (offset 0, `GlobalAlloc::Static`).
    pub fn check_static_ptr(&self, allocs: &AllocMap) -> Option<DefId> {
        match self {
            Const::Val(ConstValue::Scalar(Scalar::Ptr { provenance, offset: 0 }), _) => match allocs.global(provenance.alloc)? {
                GlobalAlloc::Static(def) => Some(*def),
                _ => None,
            },
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
