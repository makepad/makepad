//! MIR syntax: rustc_middle/src/mir/syntax.rs:40,305,692,1178,1358.
//! Owned vectors replace rustc's arena-interned lists. Generic arguments use Relax's TyIds.
use super::*;
use crate::program::Sym;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MirPhase {
    Built,
    Analysis(AnalysisPhase),
    Runtime(RuntimePhase),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AnalysisPhase {
    Initial,
    PostCleanup,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuntimePhase {
    Initial,
    PostCleanup,
    Optimized,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutability {
    Not,
    Mut,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variance {
    Covariant,
    Contravariant,
    Invariant,
    Bivariant,
}

/// rustc_middle/src/mir/syntax.rs:1178,1187. Field carries its instantiated type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Place {
    pub local: Local,
    pub projection: Vec<PlaceElem>,
}
impl Place {
    pub fn local(local: Local) -> Place {
        Place { local, projection: Vec::new() }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PlaceElem {
    Deref,
    Field(FieldIdx, TyId),
    Index(Local),
    ConstantIndex { offset: u64, min_length: u64, from_end: bool },
    Subslice { from: u64, to: u64, from_end: bool },
    Downcast(Option<Sym>, VariantIdx),
    OpaqueCast(TyId),
    Subtype(TyId),
}
#[derive(Clone, Debug)]
pub enum Operand {
    Copy(Place),
    Move(Place),
    Constant(Box<ConstOperand>),
    /// rustc_middle/src/mir/syntax.rs:1329: a session flag, not a constant (differs per crate).
    RuntimeChecks(RuntimeChecks),
}
/// rustc_middle/src/mir/syntax.rs:1574.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RuntimeChecks {
    UbChecks,
    ContractChecks,
    OverflowChecks,
}
#[derive(Clone, Debug)]
pub struct ConstOperand {
    pub span: Span,
    pub user_ty: Option<UserTypeAnnotationIndex>,
    pub const_: Const,
}
/// rustc_middle/src/mir/consts.rs:211-237. Every constant carries its MIR type, including
/// unevaluated/promoted ones, whose regions are renumbered independently of the declaration
/// (or the promoted body's return type).
#[derive(Clone, Debug)]
pub enum Const {
    /// A type-system constant (const generic parameter or valtree) used as an operand
    /// (consts.rs:219-221).
    Ty(TyId, crate::consts::Const),
    Val(ConstValue, TyId),
    Unevaluated { def: DefId, args: Vec<crate::consts::GenericArg>, promoted: Option<PromotedId>, ty: TyId },
}
impl Const {
    /// rustc_middle/src/mir/consts.rs:258-273. A valtree's inner type wins over the outer one.
    pub fn ty(&self, tys: &crate::types::Types) -> TyId {
        match *self {
            Const::Ty(ty, ct) => match tys.const_kind(ct) {
                crate::consts::ConstKind::Value(value) => value.ty,
                _ => ty,
            },
            Const::Val(_, ty) | Const::Unevaluated { ty, .. } => ty,
        }
    }
}
/// rustc_middle/src/mir/consts.rs:34; allocation identity preserves provenance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConstValue {
    Scalar(Scalar),
    ZeroSized,
    Slice { alloc: AllocId, meta: u64 },
    Indirect { alloc: AllocId, offset: u64 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scalar {
    Int { bits: u128, size: u8 },
    Ptr { provenance: CtfeProvenance, offset: u64 },
}

/// rustc_middle/src/mir/syntax.rs:155,196,246.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorrowKind {
    Shared,
    Fake(FakeBorrowKind),
    Mut { kind: MutBorrowKind },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeBorrowKind {
    Shallow,
    Deep,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutBorrowKind {
    Default,
    TwoPhaseBorrow,
    ClosureCapture,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawPtrKind {
    Const,
    Mut,
    FakeForPtrMetadata,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerCoercion {
    /// rustc_middle/src/ty/adjustment.rs:14: a fn item to a fn pointer of the given safety.
    ReifyFnPointer(crate::types::Safety),
    UnsafeFnPointer,
    /// adjustment.rs:21: a non-capturing closure to a fn pointer of the given safety.
    ClosureFnPointer(crate::types::Safety),
    MutToConstPointer,
    ArrayToPointer,
    Unsize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoercionSource {
    AsCast,
    Implicit,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastKind {
    PointerExposeProvenance,
    PointerWithExposedProvenance,
    PtrToPtr,
    FnPtrToPtr,
    PointerCoercion(PointerCoercion, CoercionSource),
    IntToInt,
    FloatToInt,
    FloatToFloat,
    IntToFloat,
    Transmute,
}
/// rustc_middle/src/mir/syntax.rs:1358 (1.97.1: no Len/NullaryOp/ShallowInitBox; bounds checks use
/// a constant length or `UnOp::PtrMetadata`, runtime flags are `Operand::RuntimeChecks`).
#[derive(Clone, Debug)]
pub enum Rvalue {
    Use(Operand),
    Repeat(Operand, Const),
    Ref(Region, BorrowKind, Place),
    ThreadLocalRef(DefId),
    RawPtr(RawPtrKind, Place),
    Cast(CastKind, Operand, TyId),
    BinaryOp(BinOp, Box<(Operand, Operand)>),
    UnaryOp(UnOp, Operand),
    Discriminant(Place),
    Aggregate(Box<AggregateKind>, Vec<Operand>),
    CopyForDeref(Place),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
    PtrMetadata,
}
/// rustc_middle/src/mir/syntax.rs:1616. Overflow forms return (T, bool); Cmp returns Ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    AddUnchecked,
    AddWithOverflow,
    Sub,
    SubUnchecked,
    SubWithOverflow,
    Mul,
    MulUnchecked,
    MulWithOverflow,
    Div,
    Rem,
    BitXor,
    BitAnd,
    BitOr,
    Shl,
    ShlUnchecked,
    Shr,
    ShrUnchecked,
    Eq,
    Lt,
    Le,
    Ne,
    Ge,
    Gt,
    Cmp,
    Offset,
}
/// Closure identity is (file, expression); its owning def is Body.source.owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClosureId {
    pub file: u32,
    pub expr: u32,
}
#[derive(Clone, Debug)]
pub enum AggregateKind {
    Array(TyId),
    Tuple,
    Adt(DefId, VariantIdx, Vec<crate::consts::GenericArg>, Option<UserTypeAnnotationIndex>, Option<FieldIdx>),
    /// The closure-like aggregates carry their full generic arguments in the layout of
    /// `closure_args` (rustc: GenericArgsRef with the synthetic tail).
    Closure(ClosureId, Vec<crate::consts::GenericArg>),
    Coroutine(ClosureId, Vec<crate::consts::GenericArg>),
    CoroutineClosure(ClosureId, Vec<crate::consts::GenericArg>),
    RawPtr(TyId, Mutability),
}

/// rustc_middle/src/mir/syntax.rs:305,515.
#[derive(Clone, Debug)]
pub struct Statement {
    pub source_info: SourceInfo,
    pub kind: StatementKind,
}
#[derive(Clone, Debug)]
pub enum StatementKind {
    Assign(Place, Rvalue),
    FakeRead(FakeReadCause, Place),
    SetDiscriminant { place: Place, variant_index: VariantIdx },
    StorageLive(Local),
    StorageDead(Local),
    PlaceMention(Place),
    AscribeUserType(Place, UserTypeProjection, Variance),
    Intrinsic(NonDivergingIntrinsic),
    ConstEvalCounter,
    Nop,
    /// rustc_middle/src/mir/syntax.rs:450: marks where `place` would be dropped under the
    /// Edition 2024 temporary rules; a lint-only no-op.
    BackwardIncompatibleDropHint { place: Place, reason: BackwardIncompatibleDropReason },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackwardIncompatibleDropReason {
    Edition2024,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeReadCause {
    ForMatchGuard,
    ForMatchedPlace(Option<ClosureId>),
    ForGuardBinding,
    ForLet(Option<ClosureId>),
    ForIndex,
}
#[derive(Clone, Debug)]
pub enum NonDivergingIntrinsic {
    Assume(Operand),
    CopyNonOverlapping { src: Operand, dst: Operand, count: Operand },
}

/// rustc_middle/src/mir/syntax.rs:692. Call signatures are derived from func's type by borrowck.
#[derive(Clone, Debug)]
pub struct Terminator {
    pub source_info: SourceInfo,
    pub kind: TerminatorKind,
}
#[derive(Clone, Debug)]
pub enum TerminatorKind {
    Goto {
        target: BasicBlock,
    },
    SwitchInt {
        discr: Operand,
        targets: SwitchTargets,
    },
    UnwindResume,
    UnwindTerminate(UnwindTerminateReason),
    Return,
    Unreachable,
    Drop {
        place: Place,
        target: BasicBlock,
        unwind: UnwindAction,
        replace: bool,
    },
    Call {
        func: Operand,
        args: Vec<Operand>,
        destination: Place,
        target: Option<BasicBlock>,
        unwind: UnwindAction,
        call_source: CallSource,
        fn_span: Span,
        arg_spans: Vec<Span>,
    },
    TailCall {
        func: Operand,
        args: Vec<Operand>,
        fn_span: Span,
        arg_spans: Vec<Span>,
    },
    Assert {
        cond: Operand,
        expected: bool,
        msg: AssertKind,
        target: BasicBlock,
        unwind: UnwindAction,
    },
    Yield {
        value: Operand,
        resume: BasicBlock,
        resume_arg: Place,
        drop: Option<BasicBlock>,
    },
    CoroutineDrop,
    FalseEdge {
        real_target: BasicBlock,
        imaginary_target: BasicBlock,
    },
    FalseUnwind {
        real_target: BasicBlock,
        unwind: UnwindAction,
    },
    InlineAsm(InlineAsmUnsupported),
}
/// Parallel arrays; unlike rustc_middle/src/mir/syntax.rs:994, otherwise is explicit.
/// The validator checks their lengths and the uniqueness of values.
#[derive(Clone, Debug)]
pub struct SwitchTargets {
    pub values: Vec<u128>,
    pub targets: Vec<BasicBlock>,
    pub otherwise: BasicBlock,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnwindAction {
    Continue,
    Unreachable,
    Terminate(UnwindTerminateReason),
    Cleanup(BasicBlock),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnwindTerminateReason {
    Abi,
    InCleanup,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallSource {
    OverloadedOperator,
    MatchCmp,
    Misc,
    Use,
    Normal,
}
#[derive(Clone, Debug)]
pub struct InlineAsmUnsupported;
/// rustc_middle/src/mir/syntax.rs:1055; operands are retained for panic diagnostics and CTFE.
#[derive(Clone, Debug)]
pub enum AssertKind {
    BoundsCheck { len: Operand, index: Operand },
    Overflow(BinOp, Operand, Operand),
    OverflowNeg(Operand),
    DivisionByZero(Operand),
    RemainderByZero(Operand),
    ResumedAfterReturn(CoroutineKind),
    ResumedAfterPanic(CoroutineKind),
    MisalignedPointerDereference { required: Operand, found: Operand },
    NullPointerDereference,
}
