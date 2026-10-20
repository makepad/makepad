//! rustc_mir_build/src/builder/cfg.rs: the control-flow graph under construction.
use crate::mir::{
    BasicBlock, BasicBlockData, FakeReadCause, Local, Operand, Place, Rvalue, SourceInfo, Span, Statement, StatementKind,
    Terminator, TerminatorKind,
};

/// rustc `BlockAnd<T>` / `BlockAndExtension`: a value together with the block control continues in.
#[must_use]
pub struct BlockAnd<T>(pub BasicBlock, pub T);
pub trait BlockAndExtension {
    fn and<T>(self, v: T) -> BlockAnd<T>;
    fn unit(self) -> BlockAnd<()>;
}
impl BlockAndExtension for BasicBlock {
    fn and<T>(self, v: T) -> BlockAnd<T> {
        BlockAnd(self, v)
    }
    fn unit(self) -> BlockAnd<()> {
        BlockAnd(self, ())
    }
}

#[derive(Default)]
pub struct Cfg {
    pub basic_blocks: Vec<BasicBlockData>,
}
impl Cfg {
    pub fn block_data(&self, block: BasicBlock) -> &BasicBlockData {
        &self.basic_blocks[block.0 as usize]
    }
    pub fn block_data_mut(&mut self, block: BasicBlock) -> &mut BasicBlockData {
        &mut self.basic_blocks[block.0 as usize]
    }
    /// cfg.rs:22 start_new_block.
    pub fn start_new_block(&mut self) -> BasicBlock {
        self.basic_blocks.push(BasicBlockData { statements: Vec::new(), terminator: None, is_cleanup: false });
        BasicBlock(self.basic_blocks.len() as u32 - 1)
    }
    /// cfg.rs:26 start_new_cleanup_block.
    pub fn start_new_cleanup_block(&mut self) -> BasicBlock {
        let bb = self.start_new_block();
        self.block_data_mut(bb).is_cleanup = true;
        bb
    }
    pub fn push(&mut self, block: BasicBlock, statement: Statement) {
        debug_assert!(self.block_data(block).terminator.is_none(), "push to terminated block {block:?}");
        self.block_data_mut(block).statements.push(statement);
    }
    pub fn push_assign(&mut self, block: BasicBlock, source_info: SourceInfo, place: Place, rvalue: Rvalue) {
        self.push(block, Statement { source_info, kind: StatementKind::Assign(place, rvalue) });
    }
    /// cfg.rs:83 push_assign_unit: `place = ()`.
    pub fn push_assign_unit(&mut self, block: BasicBlock, source_info: SourceInfo, place: Place, unit_ty: crate::types::TyId) {
        let unit = Operand::Constant(Box::new(crate::mir::ConstOperand {
            span: Span::default(),
            user_ty: None,
            const_: crate::mir::Const::Val(crate::mir::ConstValue::ZeroSized, unit_ty),
        }));
        self.push_assign(block, source_info, place, Rvalue::Use(unit));
    }
    pub fn push_fake_read(&mut self, block: BasicBlock, source_info: SourceInfo, cause: FakeReadCause, place: Place) {
        self.push(block, Statement { source_info, kind: StatementKind::FakeRead(cause, place) });
    }
    pub fn push_place_mention(&mut self, block: BasicBlock, source_info: SourceInfo, place: Place) {
        self.push(block, Statement { source_info, kind: StatementKind::PlaceMention(place) });
    }
    pub fn push_storage_live(&mut self, block: BasicBlock, source_info: SourceInfo, local: Local) {
        self.push(block, Statement { source_info, kind: StatementKind::StorageLive(local) });
    }
    /// cfg.rs:93 terminate.
    pub fn terminate(&mut self, block: BasicBlock, source_info: SourceInfo, kind: TerminatorKind) {
        debug_assert!(self.block_data(block).terminator.is_none(), "terminate already terminated block {block:?}");
        self.block_data_mut(block).terminator = Some(Terminator { source_info, kind });
    }
    pub fn goto(&mut self, origin: BasicBlock, source_info: SourceInfo, target: BasicBlock) {
        self.terminate(origin, source_info, TerminatorKind::Goto { target });
    }
}
impl Cfg {
    pub fn terminator_mut(&mut self, block: BasicBlock) -> &mut Terminator {
        self.block_data_mut(block).terminator.as_mut().expect("block without terminator")
    }
    pub fn terminator(&self, block: BasicBlock) -> &Terminator {
        self.block_data(block).terminator.as_ref().expect("block without terminator")
    }
}
impl BlockAnd<()> {
    /// rustc `BlockAnd<()>::into_block`.
    pub fn into_block(self) -> BasicBlock {
        self.0
    }
}
