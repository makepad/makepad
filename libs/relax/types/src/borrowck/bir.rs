//! BIR: the borrow checker's MIR-like body (rustc_middle::mir at the level borrowck needs).
//! Built from a typed `typeck::Body` by build.rs; one BIR per fn body and one per closure.

use crate::types::TyId;

pub type Local = u32;
pub type Bb = u32;
/// region variable
pub type Rv = u32;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Proj {
    Deref,
    /// field index (in the variant selected by a preceding Downcast, else variant 0) and the
    /// field's type
    Field(u32, TyId),
    /// runtime index (the index value lives in a local)
    Index(Local),
    /// `[a, b, x, ..]` / `[.., x]` patterns: (offset, from_end, min_length)
    ConstIndex(u32, bool, u32),
    /// `[a, rest @ .., b]`: (from, to, from_end) -- arrays: absolute `to` (from_end false),
    /// slices: `to` counted from the end
    Subslice(u32, u32, bool),
    Downcast(u32),
}

#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct Place {
    pub local: Local,
    pub proj: Vec<Proj>,
}

impl Place {
    pub fn local(l: Local) -> Place {
        Place { local: l, proj: Vec::new() }
    }
    pub fn project(&self, p: Proj) -> Place {
        let mut q = self.clone();
        q.proj.push(p);
        q
    }
    pub fn deref(&self) -> Place {
        self.project(Proj::Deref)
    }
    pub fn is_prefix_of(&self, other: &Place) -> bool {
        self.local == other.local && self.proj.len() <= other.proj.len() && other.proj[..self.proj.len()] == self.proj[..]
    }
}

#[derive(Clone, Debug)]
pub enum Operand {
    Copy(Place),
    Move(Place),
    Const(Box<Constant>),
}

/// Semantic constant input to MIR-style qualification and promotion. Keep identities
/// unevaluated: in particular promotion must not evaluate an inline const while
/// checking its typeck root (rustc promote_consts.rs:691).
#[derive(Clone, Debug)]
pub struct Constant {
    pub ty: TyId,
    pub kind: ConstantKind,
}

#[derive(Clone, Debug)]
pub enum ConstantKind {
    Scalar(u128),
    /// IEEE bits; the type distinguishes f32 from f64.
    Float(u64),
    Bytes(std::sync::Arc<[u8]>),
    Item { def: crate::program::DefId, args: Vec<TyId> },
    Param { owner: crate::program::DefId, index: u32, args: Vec<TyId> },
    Inline { owner: crate::program::DefId, expr: crate::ast::ExprId, args: Vec<TyId> },
    StaticPointer { def: crate::program::DefId, thread_local: bool },
    /// Index in the promoted bodies returned with this BIR by promote_candidates.
    /// Its type/regions must be checked using the parent's inference context.
    Promoted(u32),
    /// An existing BIR abstraction with no constant value (e.g. a built-in
    /// iterator's next result). This is not evidence of promotability.
    Unknown,
}

impl Operand {
    pub fn place(&self) -> Option<&Place> {
        match self {
            Operand::Copy(p) | Operand::Move(p) => Some(p),
            Operand::Const(_) => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BorrowKind {
    Shared,
    /// two_phase: an autoref'd `&mut` receiver / overloaded compound assignment operand
    Mut { two_phase: bool },
    /// rustc FakeBorrowKind::Shallow (match guards: the scrutinee's discriminant may not change)
    Shallow,
    /// rustc FakeBorrowKind::Deep (match guards on places behind references)
    Fake,
    /// a closure's unique immutable capture (rustc MutBorrowKind::ClosureCapture): a mutable
    /// borrow that does not need a `mut` binding
    Unique,

}

impl BorrowKind {
    pub fn is_mut(self) -> bool {
        matches!(self, BorrowKind::Mut { .. } | BorrowKind::Unique)
    }
}

#[derive(Clone, Debug)]
pub enum AggKind {
    Tuple,
    Array(TyId),
    /// (ADT type, variant)
    Adt(TyId, u32),
    /// closure expression (index into Bir::closures of the owner)
    Closure(u32),
}

/// How a value changes type without a call (region relations differ per kind).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastKind {
    /// numeric / fn item to pointer: no regions carried over
    Plain,
    /// Exposing a raw/fn pointer's address is forbidden during const evaluation.
    PointerExposeProvenance,
    /// Integer address to raw pointer; unlike exposing provenance, promotable.
    PointerWithExposedProvenance,
    /// same type (an implicit coercion that keeps the representation: `!` to T, MutToConst of a
    /// value already reborrowed, raw pointer casts keeping the pointee)
    Same,
    /// &[T; N] -> &[T] and friends: the pointer region and the element's regions carry over
    Unsize,
    /// a pointer to T -> a pointer to `dyn Tr + 'r`: T's regions outlive 'r
    ToDyn,
    /// raw pointer to raw pointer (pointee changes): nothing carried
    PtrToPtr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Unary(crate::ast::UnOp),
    Binary(crate::ast::BinOp),
    /// A polymorphic comparison without a resolved trait method in today's BIR.
    /// Its operands are borrowed; it must not be validated as primitive arithmetic.
    TraitComparison(crate::ast::BinOp),
    /// Existing BIR pattern-test abstraction: a read, not primitive arithmetic.
    PatternTest,
}

#[derive(Clone, Debug)]
pub enum Rvalue {
    Use(Operand),
    Ref(Rv, BorrowKind, Place),
    /// `&raw const p` / `&raw mut p` / `&p as *const _` (a shallow access of p)
    RawPtr(bool, Place),
    Aggregate(AggKind, Vec<Operand>),
    Cast(CastKind, Operand, TyId),
    /// arithmetic / comparison, including explicit markers for abstract BIR tests
    Op(Operation, Vec<Operand>),
    Discriminant(Place),
    Len(Place),
    Repeat(Operand),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeReadCause {
    ForMatchedPlace,
    ForLet,
    ForGuardBinding,
    ForIndex,
}

#[derive(Clone, Debug)]
pub enum StmtKind {
    Assign(Place, Rvalue),
    StorageLive(Local),
    StorageDead(Local),
    FakeRead(FakeReadCause, Place),
    /// the scrutinee of a match / let pattern (rustc PlaceMention): a use for liveness only
    PlaceMention(Place),
    /// the scope of a two-phase borrow's value ends: nothing (kept for spans)
    Nop,
}

pub use super::constraints::Span;

#[derive(Clone, Debug)]
pub struct Statement {
    pub kind: StmtKind,
    pub span: Span,
}

/// A call's callee as borrowck sees it.
#[derive(Clone, Debug)]
pub enum Callee {
    /// a fn item instance (DefId resolved to the impl's method, generic args)
    Fn(crate::program::DefId, Vec<TyId>),
    /// through a value: fn pointer, dyn Fn*, closure (the callee operand is in args[0] for
    /// closures / dyn: the receiver)
    Indirect,
    /// overloaded deref / index (the method instance), as Fn but flagged for diagnostics
    Overloaded(crate::program::DefId, Vec<TyId>),
    /// a trait method called through `dyn Trait`: its signature relates the regions; the body
    /// (any impl's) is unknown, so field views take the whole value
    Virtual(crate::program::DefId, Vec<TyId>),
}

/// The instantiated signature of a call: region variables of the formal parameters / return
/// type, and the outlives requirements of the callee (where-clauses, WF of the instantiated
/// signature, opaque captures). Built by build.rs from the callee's AST signature.
#[derive(Clone, Debug, Default)]
pub struct CallSig {
    pub inputs: Vec<Vec<Rv>>,
    pub input_tys: Vec<TyId>,
    pub output: Vec<Rv>,
    pub output_ty: Option<TyId>,
    /// (a, b): 'a: 'b
    pub outlives: Vec<(Rv, Rv)>,
}

#[derive(Clone, Debug)]
pub enum TermKind {
    Goto(Bb),
    /// a branch on a value (the operand is read); targets in any order
    Switch(Operand, Vec<Bb>),
    Call {
        callee: Callee,
        args: Vec<Operand>,
        dest: Place,
        target: Option<Bb>,
        sig: Box<CallSig>,
        /// per argument: the source span of its expression (empty: the call's span)
        arg_spans: Vec<Span>,
    },
    /// drop of a place (no-op when the place is uninitialized: borrowck uses init state)
    Drop(Place, Bb),
    Return,
    Unreachable,
    /// a match arm's edge: `real` taken, `imaginary` only for borrowck (rustc FalseEdge)
    FalseEdge(Bb, Bb),
    /// a loop head: `real` taken, the unwind edge imaginary (rustc FalseUnwind); we model only
    /// `real`
    FalseUnwind(Bb),
}

#[derive(Clone, Debug)]
pub struct Term {
    pub kind: TermKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct BasicBlock {
    pub stmts: Vec<Statement>,
    pub term: Option<Term>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalKind {
    /// the return place `_0`
    Ret,
    /// a fn argument (`_1..`): the closure env is the first of a closure body
    Arg,
    /// a user variable (binding of a pattern)
    User,
    Temp,
}

#[derive(Clone, Debug)]
pub struct LocalDecl {
    pub ty: TyId,
    pub kind: LocalKind,
    pub mutable: bool,
    /// name for diagnostics (user variables, upvars)
    pub name: Option<String>,
    pub span: Span,
    /// region variables of the type's skeleton (rty.rs)
    pub regions: Vec<Rv>,
    /// a `ref` / default-binding-mode reference binding (diagnostics: "borrowed here")
    pub is_ref_binding: bool,
    /// rustc LocalDecl::can_be_made_mutable: a by-value user binding or owned self.
    pub can_be_made_mutable: bool,
}

/// One captured place of a closure (2021 precise capture).
#[derive(Clone, Debug)]
pub struct Capture {
    /// the place in the parent body (a parent BIR place)
    pub place: Place,
    pub ty: TyId,
    /// 0 by value, 1 shared borrow, 2 mutable borrow, 3 unique immutable borrow (a `&mut`
    /// borrow of a place behind a `&mut` the closure only writes through)
    pub kind: u8,
    pub name: String,
    pub span: Span,
    /// the captured binding is declared `mut`
    pub mutable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClosureKind {
    Fn,
    FnMut,
    FnOnce,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegionOrigin {
    /// universal: 'static
    Static,
    /// universal: named / elided lifetime of the signature, a type param's region (instance)
    Universal(String),
    /// closure body: a region of the closure's upvars / signature from the parent (requirements
    /// on these propagate to the parent)
    External,
    /// closure body: a region of a parameter (higher-ranked: placeholder)
    Placeholder,
    /// existential: local types, borrows, call instantiations
    Var,
}

#[derive(Clone)]
pub struct Bir {
    pub checked_body: Option<super::body::CheckedBody>,
    pub blocks: Vec<BasicBlock>,
    pub locals: Vec<LocalDecl>,
    /// number of argument locals (after `_0`)
    pub n_args: u32,
    pub regions: Vec<RegionOrigin>,
    /// known outlives between universal regions (where-clauses, implied bounds): (a, b): 'a: 'b
    pub known: Vec<(Rv, Rv)>,
    /// constraints recorded while building (at the point they arise): (a, b, point block, stmt)
    pub outlives: Vec<(Rv, Rv, Bb, u32)>,
    /// for a closure body: its captures and kind; the env is local 1
    pub captures: Vec<Capture>,
    pub closure_kind: Option<ClosureKind>,
    /// the closure expressions created in this body (index = AggKind::Closure)
    pub closures: Vec<u32>,
    /// what the builder could not express ("unchecked: ...")
    pub unsupported: Option<String>,
    pub file: u32,
    pub name: String,
    /// the fn instance this body is (None for closure bodies)
    pub def: Option<crate::program::DefId>,
    pub args: Vec<TyId>,
    /// a closure body: its closure expression
    pub closure_expr: Option<u32>,
    /// the module scope of the body's fn (resolving annotation paths)
    pub module: u32,
    /// rustc user_type_annotations: the written types (source, the erased annotated type)
    pub user_tys: Vec<(UserAnn, TyId)>,
    /// rustc AggregateKind::Adt(.., user_ty): an ADT aggregate's type is exactly the annotation
    /// (by statement), written on its path (`S::<'a>`) or through `Self`
    pub rvalue_user_tys: Vec<((Bb, u32), u32)>,
    /// rustc LocalDecl.user_ty: a binding's type is exactly the annotation projected to it
    /// (relate_type_and_user_type, Invariant; a `ref` binding's pointee)
    pub local_user_tys: Vec<LocalUserTy>,
    /// rustc StatementKind::AscribeUserType: the place's type is a subtype of the annotation
    /// projected to it (a `let` pattern's initializer)
    pub ascriptions: Vec<Ascription>,
    /// every `let` annotation (any pattern, reachable or not): its type must be well-formed
    /// (rustc checks annotations' WF at all points): (AST type, erased type, block, stmt index)
    pub wf_annots: Vec<(crate::ast::TyId, TyId, Span)>,
    /// ADT aggregates built by calling a constructor VALUE (`let f = S::<&'a u32>; f(x)`):
    /// the place holding it, whose slots (its fn item type's args) the aggregate's equal
    pub agg_callee: Vec<((Bb, u32), Place)>,
    /// a closure body: argument locals whose outer reference region is higher-ranked (the
    /// closure's signature comes from an `Fn(&T)` bound or an elided `&T` annotation): a
    /// placeholder that may not escape into the closure's environment
    pub hr_args: Vec<Local>,
    /// results of overloaded `*x` / `x[i]` calls: (the `&T` local, the base type, is index)
    pub ov_results: Vec<(Local, TyId, bool)>,
    /// the fn whose body this is or encloses this closure
    pub owner: Option<crate::program::DefId>,
    /// closure capture borrows: (the temp holding the borrow, the capturing use's span)
    pub capture_spans: Vec<(Local, Span)>,
    /// match-guard binding locals (rustc: references to the bindings) and their names
    pub guard_locals: Vec<(Local, String)>,
    /// the temps of index expressions' fake borrows (E0510 "in indexing expression")
    pub index_fakes: Vec<Local>,
    /// a closure whose signature comes from an `Fn*` expectation (its params' regions are
    /// higher-ranked, never the closure type's)
    pub expected_sig: bool,
    /// temps holding a static's address (`*&'static S`): the pointee has the static's declared
    /// type, elided lifetimes 'static
    pub static_temps: Vec<(Local, crate::program::DefId)>,
    /// indirect calls (by block) whose callee is a static
    pub static_callees: Vec<(Bb, crate::program::DefId)>,
    /// closure params without a written type (their regions are inferred)
    pub infer_params: Vec<Local>,
    /// statements writing a union literal's field (an initialization, not a partial assignment)
    pub union_inits: Vec<(Bb, u32)>,
    /// associated consts read through a path with a written self type / trait arguments
    pub const_uses: Vec<ConstUse>,
    /// a closure whose bound's elided output takes this parameter's higher-ranked region
    pub hr_ret: Option<Local>,
    /// pattern bindings moving out of a matched place: the scrutinee / initializer span,
    /// where rustc reports "cannot move out of" (by statement)
    pub move_sites: Vec<((Bb, u32), Span)>,
    /// the user type written on the path of a call (rustc Constant.user_ty of the callee,
    /// UserType::TypeOf): by the block the call ends
    pub call_user_tys: Vec<(Bb, PathUserTy)>,
    /// calls through a fn item VALUE (`let f = path; f(x)`, `(&path)(x)`): the place holding it,
    /// whose slots are the callee's generic arguments' regions, by the block the call ends
    pub fn_values: Vec<(Bb, Place)>,
    /// a fn item path used as a value, with the user type written on it: the assigned local's
    /// slots are related to the annotation (by statement)
    pub fn_anns: Vec<((Bb, u32), PathUserTy)>,
}

/// The generic arguments written on a path to a fn / method (rustc UserType::TypeOf): the self
/// type (`A::<'a>::f`, `<A<'a>>::f`, `<T as Tr>::f`) and the explicit type arguments.
#[derive(Clone, Default)]
pub struct PathUserTy {
    pub self_ann: Option<SelfAnn>,
    /// the trait's written lifetime arguments (`<_ as Tr<'a>>::f`)
    pub trait_lts: Vec<crate::ast::Ident>,
    /// the trait's written type arguments (`<_ as Tr<A>>::f`)
    pub trait_targs: Vec<crate::ast::TyId>,
    pub targs: Vec<crate::ast::TyId>,
}

impl PathUserTy {
    pub fn is_empty(&self) -> bool {
        self.self_ann.is_none() && self.targs.is_empty() && self.trait_targs.is_empty() && self.trait_lts.is_empty()
    }
}

/// What a user type annotation was written as.
#[derive(Clone, PartialEq)]
pub enum UserAnn {
    /// a written type (`let x: T`, `e as T`, a closure parameter)
    Ty(crate::ast::TyId),
    /// the generic arguments written on an ADT path (`S::<'a>`, `E::V::<T>`)
    Path(Vec<PathArg>),
    /// `Self` in an impl: its self type
    SelfTy,
}

/// A binding's user type: annotation `ann` (index in Bir::user_tys) projected by `projs`
/// (rustc UserTypeProjection).
#[derive(Clone)]
pub struct LocalUserTy {
    pub local: Local,
    pub ann: u32,
    pub projs: Vec<Proj>,
    pub by_ref: bool,
}

/// `AscribeUserType(place, projection, Covariant)`.
#[derive(Clone)]
pub struct Ascription {
    pub place: Place,
    pub ann: u32,
    pub projs: Vec<Proj>,
}

/// The self type written on an associated-fn path.
#[derive(Clone)]
pub enum SelfAnn {
    Ty(crate::ast::TyId),
    Args(Vec<PathArg>),
}

/// A generic argument written on a path (`S::<'a, T>`).
#[derive(Clone, PartialEq)]
pub enum PathArg {
    Lifetime(crate::ast::Ident),
    Type(crate::ast::TyId),
}

pub const STATIC: Rv = 0;

/// An associated const's value assigned to `local` at `at`: its user type comes from the
/// path (`<Foo<'a>>::C`, `A::<'a>::C`, `<() as Foo<'a>>::C`; rustc Constant.user_ty).
#[derive(Clone)]
pub struct ConstUse {
    pub at: (Bb, u32),
    pub local: Local,
    pub def: crate::program::DefId,
    pub args: Vec<crate::types::TyId>,
    pub user: PathUserTy,
    /// a const pattern's value: the matched place it is compared with (rustc PartialEq::eq
    /// of both at one type)
    pub matched: Option<Place>,
}

impl Bir {
    pub fn ty_of_local(&self, l: Local) -> TyId {
        self.locals[l as usize].ty
    }
}
