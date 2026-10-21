//! THIR: the typed, adjusted expression tree the MIR builder consumes. Shapes follow
//! rustc_middle/src/thir.rs (1.97.1); HirIds are AST node ids of the body's file, interned types are
//! TyIds, `region::Scope` is `region_scope::ScopeId`. It is a view over (Ast, typeck::Body): no second
//! typeck and no source text. `Cx` (cx.rs) produces it (port of rustc_mir_build/src/thir/cx).
use crate::consts::GenericArg;
use crate::mir::{BindingMode, BinOp, BorrowKind, CanonicalUserTypeAnnotation, FakeReadCause, FieldIdx, Mutability, PointerCoercion, Span, UnOp, VariantIdx};
use crate::program::{DefId, Sym};
use crate::region_scope::ScopeId;
use crate::types::TyId;
use std::sync::Arc;

pub mod cx;
pub use cx::Cx;

macro_rules! thir_ids {
    ($($name:ident),*) => {$(
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u32);
    )*};
}
thir_ids!(ArmId, BlockId, ExprId, StmtId, ParamId);

/// A body's THIR: the arenas, the parameters and the root expression (rustc `thir_body`).
#[derive(Debug, Clone)]
pub struct ThirBody {
    pub thir: Thir,
    /// The parameters in order. For a closure/coroutine body the first is the environment (`_1`: the closure
    /// type, `&closure` or `&mut closure` with an erased region, no pattern), followed by the declared
    /// arguments (coroutines: the resume argument).
    pub params: Vec<ParamId>,
    pub expr: ExprId,
    pub body_ty: BodyTy,
    /// rustc `hir_span_with_body`: the whole item, signature through body.
    pub span: Span,
    /// Span of the declared return type (the span of the return place).
    pub ret_span: Span,
    pub coroutine: Option<CoroutineThir>,
    /// The `CallSite` and `Arguments` region scopes of a fn-like body (None for constants).
    pub fn_scopes: Option<(ScopeId, ScopeId)>,
    /// The body coroutine of an `async fn`: the patterns of the parameters it matches itself (rustc lowers
    /// them as `let <pat> = __arg;` first in the body), each against the captured whole argument.
    pub prologue: Vec<(Pat, LocalVarId)>,
}
impl ThirBody {
    /// A constant-shaped body around `thir` for unit tests that drive individual lowering functions.
    #[cfg(test)]
    pub fn for_test(thir: Thir, params: Vec<ParamId>, expr: ExprId, ty: TyId) -> Self {
        ThirBody { thir, params, expr, body_ty: BodyTy::Const(ty), span: Span::default(), ret_span: Span::default(), coroutine: None, fn_scopes: None, prologue: Vec::new() }
    }
}
/// rustc `BodyTy`.
#[derive(Debug, Clone)]
pub enum BodyTy {
    Const(TyId),
    /// `output` is the (liberated, normalized) return type; `rust_call` for `extern "rust-call"` fns
    /// (closures never are).
    Fn { output: TyId, rust_call: bool },
}
/// What a coroutine body adds (rustc `CoroutineInfo::initial`).
#[derive(Debug, Clone)]
pub struct CoroutineThir {
    pub kind: crate::mir::CoroutineKind,
    pub yield_ty: TyId,
    pub resume_ty: TyId,
}

/// rustc `Thir`: the arenas of one body.
#[derive(Debug, Default, Clone)]
pub struct Thir {
    pub arms: Vec<Arm>,
    pub blocks: Vec<Block>,
    pub exprs: Vec<Expr>,
    pub stmts: Vec<Stmt>,
    pub params: Vec<Param>,
    /// The drop scope of each synthetic binding (`for`'s iterator, `?`'s residual: bindings of
    /// desugarings, with ids from `typeck::Body::locals.len()` up). User bindings are in
    /// `ScopeTree::var_scope`.
    pub var_scopes: crate::fxhash::HashMap<u32, ScopeId>,
}
impl Thir {
    pub fn alloc_arm(&mut self, v: Arm) -> ArmId { self.arms.push(v); ArmId(self.arms.len() as u32 - 1) }
    pub fn alloc_block(&mut self, v: Block) -> BlockId { self.blocks.push(v); BlockId(self.blocks.len() as u32 - 1) }
    pub fn alloc_expr(&mut self, v: Expr) -> ExprId { self.exprs.push(v); ExprId(self.exprs.len() as u32 - 1) }
    pub fn alloc_stmt(&mut self, v: Stmt) -> StmtId { self.stmts.push(v); StmtId(self.stmts.len() as u32 - 1) }
    pub fn alloc_param(&mut self, v: Param) -> ParamId { self.params.push(v); ParamId(self.params.len() as u32 - 1) }
}
macro_rules! thir_index_only {
    ($($id:ident => $field:ident: $value:ty),*) => {$(
        impl std::ops::Index<$id> for Thir {
            type Output = $value;
            fn index(&self, id: $id) -> &$value { &self.$field[id.0 as usize] }
        }
    )*};
}
thir_index_only!(ArmId => arms: Arm, BlockId => blocks: Block, ExprId => exprs: Expr, StmtId => stmts: Stmt, ParamId => params: Param);

/// The identity of a local variable binding: its typeck local index (`Body.pat_local[pat]`, the same
/// for every alternative of an or-pattern; `Body.locals[index].pat` is the primary binding's pattern id).
/// Also what `ScopeTree::var_scope` is keyed by. rustc LocalVarId(HirId).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LocalVarId(pub u32);

/// rustc `ErrorGuaranteed`: the pattern/expression is erroneous and already reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErrorGuaranteed;

#[derive(Clone, Debug)]
pub struct Param {
    pub pat: Option<Box<Pat>>,
    pub ty: TyId,
    pub ty_span: Option<Span>,
    pub self_kind: Option<crate::mir::ImplicitSelfKind>,
    /// The parameter's AST node (pattern id), None for synthesized parameters.
    pub node: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub targeted_by_break: bool,
    pub region_scope: ScopeId,
    pub span: Span,
    pub stmts: Box<[StmtId]>,
    pub expr: Option<ExprId>,
    pub safety_mode: BlockSafety,
}
#[derive(Copy, Clone, Debug)]
pub enum BlockSafety {
    Safe,
    BuiltinUnsafe,
    /// The `unsafe` block's AST block id.
    ExplicitUnsafe(u32),
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub kind: StmtKind,
}
#[derive(Clone, Debug)]
pub enum StmtKind {
    Expr { scope: ScopeId, expr: ExprId },
    Let {
        remainder_scope: ScopeId,
        init_scope: ScopeId,
        pattern: Box<Pat>,
        initializer: Option<ExprId>,
        else_block: Option<BlockId>,
        /// The `let` statement's AST node.
        node: u32,
        span: Span,
    },
}

type UserTy = Option<Box<crate::types::user_type::CanonicalUserType>>;

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: TyId,
    /// rustc `temp_scope_id`, resolved through the ScopeTree: the scope temporaries of this
    /// expression die in (None: static extension).
    pub temp_scope: Option<ScopeId>,
    /// rustc `TempLifetime::backwards_incompatible`: the scope the temporaries of this expression
    /// will die in from edition 2024 (None: no change, or the lint is off).
    pub bid: Option<ScopeId>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct AdtExpr {
    pub adt_def: DefId,
    pub variant_index: VariantIdx,
    pub args: Vec<GenericArg>,
    pub user_ty: UserTy,
    pub fields: Box<[FieldExpr]>,
    pub base: AdtExprBase,
}
#[derive(Clone, Debug)]
pub enum AdtExprBase {
    None,
    Base(FruInfo),
    DefaultFields(Box<[TyId]>),
}
#[derive(Clone, Debug)]
pub struct FieldExpr {
    pub name: FieldIdx,
    pub expr: ExprId,
}
#[derive(Clone, Debug)]
pub struct FruInfo {
    pub base: ExprId,
    pub field_types: Box<[TyId]>,
}

#[derive(Clone, Debug)]
pub struct ClosureExpr {
    pub closure: crate::mir::ClosureId,
    /// Full aggregate args (parent identity args ++ [kind, sig, tupled upvars]).
    pub args: Vec<GenericArg>,
    pub upvars: Box<[ExprId]>,
    pub movability: Option<crate::mir::Movability>,
    pub fake_reads: Vec<(ExprId, FakeReadCause, u32)>,
    /// async closure / coroutine flavour of the aggregate.
    pub kind: ClosureExprKind,
}
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClosureExprKind {
    Closure,
    Coroutine,
    CoroutineClosure,
}

#[derive(Clone, Debug)]
pub struct Arm {
    pub pattern: Box<Pat>,
    pub guard: Option<ExprId>,
    pub body: ExprId,
    /// The arm's AST node.
    pub node: u32,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LogicalOp {
    And,
    Or,
}
/// rustc_hir MatchSource (the desugarings the builder distinguishes).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MatchSource {
    Normal,
    Postfix,
    ForLoopDesugar,
    TryDesugar,
    AwaitDesugar,
    FormatArgs,
}
/// rustc mir AssignOp (`+=` and friends).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AssignOp(pub BinOp);

#[derive(Clone, Debug)]
pub enum ExprKind {
    Scope { region_scope: ScopeId, node: u32, value: ExprId },
    If { if_then_scope: ScopeId, cond: ExprId, then: ExprId, else_opt: Option<ExprId> },
    Call { ty: TyId, fun: ExprId, args: Box<[ExprId]>, from_hir_call: bool, fn_span: Span },
    ByUse { expr: ExprId, span: Span },
    Deref { arg: ExprId },
    Binary { op: BinOp, lhs: ExprId, rhs: ExprId },
    LogicalOp { op: LogicalOp, lhs: ExprId, rhs: ExprId },
    Unary { op: UnOp, arg: ExprId },
    Cast { source: ExprId },
    Use { source: ExprId },
    NeverToAny { source: ExprId },
    PointerCoercion { cast: PointerCoercion, source: ExprId, is_from_as_cast: bool },
    Loop { body: ExprId },
    Let { expr: ExprId, pat: Box<Pat> },
    Match { scrutinee: ExprId, arms: Box<[ArmId]>, match_source: MatchSource },
    Block { block: BlockId },
    Assign { lhs: ExprId, rhs: ExprId },
    AssignOp { op: AssignOp, lhs: ExprId, rhs: ExprId },
    Field { lhs: ExprId, variant_index: VariantIdx, name: FieldIdx },
    Index { lhs: ExprId, index: ExprId },
    VarRef { id: LocalVarId },
    /// A captured variable of the enclosing closure body.
    UpvarRef { var: LocalVarId },
    Borrow { borrow_kind: BorrowKind, arg: ExprId },
    RawBorrow { mutability: Mutability, arg: ExprId },
    Break { label: ScopeId, value: Option<ExprId> },
    Continue { label: ScopeId },
    Return { value: Option<ExprId> },
    ConstBlock { def: DefId, args: Vec<GenericArg> },
    Repeat { value: ExprId, count: crate::consts::Const },
    Array { fields: Box<[ExprId]> },
    Tuple { fields: Box<[ExprId]> },
    Adt(Box<AdtExpr>),
    PlaceTypeAscription { source: ExprId, user_ty: UserTy, user_ty_span: Span },
    ValueTypeAscription { source: ExprId, user_ty: UserTy, user_ty_span: Span },
    Closure(Box<ClosureExpr>),
    /// A literal; `expr` is the AST literal expression, `neg` a folded unary minus.
    Literal { expr: crate::ast::ExprId, neg: bool },
    NonHirLiteral { lit: crate::consts::ScalarInt, user_ty: UserTy },
    ZstLiteral { user_ty: UserTy },
    NamedConst { def: DefId, args: Vec<GenericArg>, user_ty: UserTy },
    ConstParam { param: crate::consts::ParamConst, def: DefId },
    StaticRef { def: DefId, ty: TyId },
    ThreadLocalRef(DefId),
    Yield { value: ExprId },
    /// Control never continues here (the end of a `panic!`-family macro: Relax's builtin macros have
    /// no callee to call). Lowered as an `Unreachable` terminator.
    Unreachable,
}

/// rustc `Pat`.
#[derive(Clone, Debug)]
pub struct Pat {
    pub ty: TyId,
    pub span: Span,
    pub extra: Option<Box<PatExtra>>,
    pub kind: PatKind,
}
#[derive(Clone, Debug, Default)]
pub struct PatExtra {
    pub expanded_const: Option<DefId>,
    pub ascriptions: Vec<Ascription>,
}
#[derive(Clone, Debug)]
pub struct Ascription {
    pub annotation: CanonicalUserTypeAnnotation,
    pub variance: crate::mir::Variance,
}
#[derive(Clone, Debug)]
pub struct FieldPat {
    pub field: FieldIdx,
    pub pattern: Pat,
}
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RangeEnd {
    Included,
    Excluded,
}
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Pinnedness {
    Not,
    Pinned,
}
#[derive(Copy, Clone, Debug)]
pub enum DerefPatBorrowMode {
    Borrow(Mutability),
    Box,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatRangeBoundary {
    Finite(crate::consts::ValTree),
    NegInfinity,
    PosInfinity,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatRange {
    pub lo: PatRangeBoundary,
    pub hi: PatRangeBoundary,
    pub end: RangeEnd,
    pub ty: TyId,
}
#[derive(Clone, Debug)]
pub enum PatKind {
    Missing,
    Wild,
    Binding {
        name: Sym,
        mode: BindingMode,
        var: LocalVarId,
        ty: TyId,
        subpattern: Option<Box<Pat>>,
        /// First occurrence among or-pattern alternatives (the one that declares the local).
        is_primary: bool,
        is_shorthand: bool,
    },
    Variant { adt_def: DefId, args: Vec<GenericArg>, variant_index: VariantIdx, subpatterns: Vec<FieldPat> },
    Leaf { subpatterns: Vec<FieldPat> },
    Deref { pin: Pinnedness, subpattern: Box<Pat> },
    DerefPattern { subpattern: Box<Pat>, borrow: DerefPatBorrowMode },
    Constant { value: crate::consts::Value },
    Range(Arc<PatRange>),
    Slice { prefix: Box<[Pat]>, slice: Option<Box<Pat>>, suffix: Box<[Pat]> },
    Array { prefix: Box<[Pat]>, slice: Option<Box<Pat>>, suffix: Box<[Pat]> },
    Or { pats: Box<[Pat]> },
    Guard { subpattern: Box<Pat>, condition: ExprId },
    Never,
    Error(ErrorGuaranteed),
}

impl Pat {
    /// rustc `Pat::simple_ident`.
    pub fn simple_ident(&self) -> Option<Sym> {
        match self.kind {
            PatKind::Binding { name, mode: BindingMode(crate::mir::ByRef::No, _), subpattern: None, .. } => Some(name),
            _ => None,
        }
    }
    /// rustc `Pat::walk`: pre-order, `it` returns whether to descend.
    pub fn walk(&self, mut it: impl FnMut(&Pat) -> bool) {
        self.walk_(&mut it)
    }
    fn walk_(&self, it: &mut impl FnMut(&Pat) -> bool) {
        if !it(self) {
            return;
        }
        for_each_immediate_subpat(self, |p| p.walk_(it));
    }
    pub fn walk_always(&self, mut it: impl FnMut(&Pat)) {
        self.walk(|p| {
            it(p);
            true
        })
    }
    /// rustc `Pat::each_binding`.
    pub fn each_binding(&self, mut f: impl FnMut(Sym, crate::mir::ByRef, TyId, Span)) {
        self.walk_always(|p| {
            if let PatKind::Binding { name, mode, ty, .. } = p.kind {
                f(name, mode.0, ty, p.span);
            }
        });
    }
    /// rustc `Pat::is_never_pattern`.
    pub fn is_never_pattern(&self) -> bool {
        let mut is_never_pattern = false;
        self.walk(|pat| match &pat.kind {
            PatKind::Never => {
                is_never_pattern = true;
                false
            }
            PatKind::Or { pats } => {
                is_never_pattern = pats.iter().all(|p| p.is_never_pattern());
                false
            }
            _ => true,
        });
        is_never_pattern
    }
}

/// rustc thir/visit.rs for_each_immediate_subpat.
pub fn for_each_immediate_subpat<'a>(pat: &'a Pat, mut callback: impl FnMut(&'a Pat)) {
    match &pat.kind {
        PatKind::Missing | PatKind::Wild | PatKind::Binding { subpattern: None, .. } | PatKind::Constant { .. } | PatKind::Range(..) | PatKind::Never | PatKind::Error(_) => {}
        PatKind::Binding { subpattern: Some(sub), .. } | PatKind::Deref { subpattern: sub, .. } | PatKind::DerefPattern { subpattern: sub, .. } | PatKind::Guard { subpattern: sub, .. } => callback(sub),
        PatKind::Variant { subpatterns, .. } | PatKind::Leaf { subpatterns } => subpatterns.iter().for_each(|field| callback(&field.pattern)),
        PatKind::Slice { prefix, slice, suffix } | PatKind::Array { prefix, slice, suffix } => {
            prefix.iter().chain(slice.as_deref()).chain(suffix.iter()).for_each(callback)
        }
        PatKind::Or { pats } => pats.iter().for_each(callback),
    }
}
