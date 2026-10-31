//! Hand-built MIR over a real (tiny) program, for the pre-borrowck passes: const checking, qualifs,
//! promotion and CFG simplification. The MIR builder (S3) is not needed: bodies are written block by
//! block, naming the program's items (consts, const fns, statics, types with and without `Drop`).
use crate::mir::build::pattern::tests::{named, program};
use crate::mir::*;
use crate::program::{DefId, Program};
use crate::tcx::{Tcx, TypingEnv};
use crate::types::TyKind;

/// Items every fixture program has.
pub(crate) const PRELUDE: &str = r#"
#[lang = "sized"] pub trait Sized {}
#[lang = "drop"] pub trait Drop { fn drop(&mut self); }
#[lang = "unsafe_cell"] pub struct UnsafeCell<T>(T);
pub struct D; impl Drop for D { fn drop(&mut self) {} }
pub struct Pair(D, u8);
pub struct Cell(UnsafeCell<u8>);
pub union U { a: u8, b: D }
pub const C: u8 = 1;
pub const CELL: Cell = Cell(UnsafeCell(1));
pub const fn cf() -> u8 { 1 }
pub fn nf() -> u8 { 1 }
#[rustc_promotable] pub const fn promotable() -> u8 { 2 }
pub trait Tr { fn m(&self) -> u8; }
impl Tr for u8 { fn m(&self) -> u8 { 1 } }
pub static S: u8 = 1;
pub static mut SM: u8 = 1;
pub const fn kf() {}
pub const K: u8 = 0;
"#;

pub(crate) struct Fx {
    pub prog: Program,
    pub tcx: Tcx,
    pub env: TypingEnv,
}

impl Fx {
    pub fn new(extra: &str) -> Fx {
        let (prog, tcx) = program(&format!("{extra}\n{PRELUDE}"));
        let env = TypingEnv::fully_monomorphized(&prog);
        Fx { prog, tcx, env }
    }
    pub fn def(&self, name: &str) -> DefId {
        named(&self.prog, name)
    }
    pub fn adt(&self, name: &str) -> TyId {
        let d = self.def(name);
        self.tcx.tys.intern(TyKind::Adt(d, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY))
    }
    pub fn fn_ty(&self, name: &str) -> TyId {
        let d = self.def(name);
        self.tcx.tys.intern(TyKind::FnDef(d, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY))
    }
    pub fn u8(&self) -> TyId {
        self.tcx.tys.prim(crate::program::Prim::U8)
    }
    pub fn ref_ty(&self, mutable: bool, ty: TyId) -> TyId {
        self.tcx.tys.intern(TyKind::Ref(mutable, ty, crate::types::RegionId::ERASED))
    }
    pub fn array(&self, elem: TyId, n: u64) -> TyId {
        self.tcx.tys.intern(TyKind::Array(elem, self.tcx.tys.const_usize(n)))
    }
}

pub(crate) fn decl(ty: TyId, kind: LocalKind) -> LocalDecl {
    LocalDecl { ty, mutability: Mutability::Mut, user_ty: Vec::new(), source_info: SourceInfo::default(), kind, local_info: LocalInfo::Boring }
}

pub(crate) fn local(i: u32) -> Place {
    Place::local(Local(i))
}
pub(crate) fn copy(i: u32) -> Operand {
    Operand::Copy(local(i))
}
pub(crate) fn mv(i: u32) -> Operand {
    Operand::Move(local(i))
}
pub(crate) fn int(ty: TyId, bits: u128) -> Operand {
    let size = 1;
    Operand::Constant(Box::new(ConstOperand { span: Span::default(), user_ty: None, const_: Const::Val(ConstValue::Scalar(Scalar::Int { bits, size }), ty) }))
}
pub(crate) fn konst(c: Const) -> Operand {
    Operand::Constant(Box::new(ConstOperand { span: Span::default(), user_ty: None, const_: c }))
}
pub(crate) fn item_const(def: DefId, ty: TyId) -> Operand {
    konst(Const::Unevaluated { def, args: Vec::new(), promoted: None, ty })
}
pub(crate) fn zst(ty: TyId) -> Operand {
    konst(Const::Val(ConstValue::ZeroSized, ty))
}

pub(crate) fn assign(place: Place, rvalue: Rvalue) -> Statement {
    Statement { source_info: SourceInfo::default(), kind: StatementKind::Assign(place, rvalue) }
}
pub(crate) fn stmt(kind: StatementKind) -> Statement {
    Statement { source_info: SourceInfo::default(), kind }
}
pub(crate) fn term(kind: TerminatorKind) -> Terminator {
    Terminator { source_info: SourceInfo::default(), kind }
}
pub(crate) fn block(statements: Vec<Statement>, kind: TerminatorKind) -> BasicBlockData {
    BasicBlockData { statements, terminator: Some(term(kind)), is_cleanup: false }
}
pub(crate) fn cleanup_block(statements: Vec<Statement>, kind: TerminatorKind) -> BasicBlockData {
    BasicBlockData { statements, terminator: Some(term(kind)), is_cleanup: true }
}
pub(crate) fn call(func: Operand, args: Vec<Operand>, dest: Place, target: u32) -> TerminatorKind {
    let arg_spans = vec![Span::default(); args.len()];
    TerminatorKind::Call { func, args, destination: dest, target: Some(BasicBlock(target)), unwind: UnwindAction::Continue, call_source: CallSource::Normal, fn_span: Span::default(), arg_spans }
}
pub(crate) fn drop_(place: Place, target: u32) -> TerminatorKind {
    TerminatorKind::Drop { place, target: BasicBlock(target), unwind: UnwindAction::Continue, replace: false }
}
pub(crate) fn aggregate_adt(fx: &Fx, name: &str, ops: Vec<Operand>) -> Rvalue {
    Rvalue::Aggregate(Box::new(AggregateKind::Adt(fx.def(name), VariantIdx(0), Vec::new(), None, None)), ops)
}
pub(crate) fn borrow(kind: BorrowKind, place: Place) -> Rvalue {
    Rvalue::Ref(Region::ERASED, kind, place)
}

/// A body of kind `kind` owned by `owner`: `_0` has type `ret`, the next `args` locals are arguments,
/// the rest `locals` are temporaries.
pub(crate) fn body(owner: DefId, kind: crate::mir::BodyKind, ret: TyId, args: &[TyId], locals: &[TyId], blocks: Vec<BasicBlockData>) -> Body {
    let mut local_decls = vec![decl(ret, LocalKind::Return)];
    local_decls.extend(args.iter().map(|&ty| decl(ty, LocalKind::Arg)));
    local_decls.extend(locals.iter().map(|&ty| decl(ty, LocalKind::Temp)));
    Body {
        phase: MirPhase::Built,
        source: MirSource { owner, kind },
        basic_blocks: blocks,
        local_decls,
        source_scopes: vec![SourceScopeData { span: Span::default(), parent_scope: None }],
        span: Span::default(),
        user_type_annotations: Vec::new(),
        upvars: Vec::new(),
        allocs: AllocMap::default(),
        var_debug_info: Vec::new(),
        arg_count: args.len() as u32,
        spread_arg: None,
        promoted: Vec::new(),
        coroutine: None,
    }
}
