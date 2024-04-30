use {
    crate::{
        aliased_box::AliasableBox,
        code,
        code::{BinOpInfo, BlockType, InstrVisitor, LoadInfo, MemArg, StoreInfo, UnOpInfo},
        decode::DecodeError,
        exec,
        exec::ThreadedInstr,
        extern_ref::ExternRef,
        func::{CodeSlot, CompiledCode, Func, FuncEntity, FuncType, UncompiledCode},
        func_ref::FuncRef,
        instance::Instance,
        ref_::RefType,
        stack::StackSlot,
        store::Store,
        val::{UnguardedVal, ValType},
    },
    std::{mem, ops::Deref},
};

#[derive(Clone, Debug)]
pub(crate) struct Compiler {
    label_idxs: Vec<u32>,
    locals: Vec<Local>,
    blocks: Vec<Block>,
    opds: Vec<Opd>,
    fixup_idxs: Vec<usize>,
}

impl Compiler {
    pub(crate) fn new() -> Self {
        Self {
            label_idxs: Vec::new(),
            locals: Vec::new(),
            blocks: Vec::new(),
            opds: Vec::new(),
            fixup_idxs: Vec::new(),
        }
    }

    pub(crate) fn compile(
        &mut self,
        store: &mut Store,
        func: Func,
        instance: &Instance,
        code: &UncompiledCode,
    ) -> CompiledCode {
        use crate::decode::Decoder;

        self.locals.clear();
        self.blocks.clear();
        self.opds.clear();
        self.fixup_idxs.clear();
        let type_ = func.type_(store);
        let locals = &mut self.locals;
        for type_ in type_
            .params()
            .iter()
            .copied()
            .chain(code.locals.iter().copied())
        {
            locals.push(Local {
                type_,
                first_opd_idx: None,
            });
        }
        let local_count = locals.len() - type_.params().len();
        let mut compile = Compile {
            store,
            type_: type_.clone(),
            instance,
            locals,
            blocks: &mut self.blocks,
            opds: &mut self.opds,
            fixup_idxs: &mut self.fixup_idxs,
            first_param_result_stack_idx: -(type_.callee_stack_slot_count() as isize),
            first_temp_stack_idx: local_count,
            max_stack_height: local_count,
            regs: [None; 2],
            code: Vec::new(),
        };
        compile.push_block(
            BlockKind::Block,
            FuncType::new([], type_.results().iter().copied()),
        );
        compile.emit(exec::enter as ThreadedInstr);
        compile.emit(func.to_unguarded(store.id()));
        compile.emit(
            compile
                .instance
                .mem(0)
                .map(|mem| mem.to_unguarded(store.id())),
        );
        let mut decoder = Decoder::new(&code.expr);
        while !compile.blocks.is_empty() {
            code::decode_instr(&mut decoder, &mut self.label_idxs, &mut compile).unwrap();
        }
        for (result_idx, result_type) in type_.clone().results().iter().copied().enumerate().rev() {
            compile.emit(copy_stack(result_type));
            compile.emit_stack_offset(compile.temp_stack_idx(result_idx));
            compile.emit_stack_offset(compile.param_result_stack_idx(result_idx));
        }
        compile.emit(exec::return_ as ThreadedInstr);
        compile.opds.clear();
        let max_stack_slot_count = compile.max_stack_height;
        let mut code: AliasableBox<[CodeSlot]> = AliasableBox::from_box(Box::from(compile.code));
        for fixup_idx in compile.fixup_idxs.drain(..) {
            code[fixup_idx] += code.as_ptr() as usize;
        }
        self.locals.clear();
        self.opds.clear();
        CompiledCode {
            max_stack_slot_count,
            local_count,
            slots: code,
        }
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct Compile<'a> {
    store: &'a Store,
    type_: FuncType,
    instance: &'a Instance,
    locals: &'a mut [Local],
    blocks: &'a mut Vec<Block>,
    opds: &'a mut Vec<Opd>,
    fixup_idxs: &'a mut Vec<usize>,
    first_param_result_stack_idx: isize,
    first_temp_stack_idx: usize,
    max_stack_height: usize,
    regs: [Option<usize>; 2],
    code: Vec<CodeSlot>,
}

impl<'a> Compile<'a> {
    fn resolve_block_type(&self, type_: BlockType) -> FuncType {
        match type_ {
            BlockType::TypeIdx(idx) => self
                .store
                .resolve_type(self.instance.type_(idx).unwrap())
                .clone(),
            BlockType::ValType(val_type) => FuncType::from_val_type(val_type),
        }
    }

    /// Constants
    
    fn copy_const_to_opd(&mut self, opd_depth: usize) {
        let opd_idx = self.opds.len() - 1 - opd_depth;
        self.emit(copy_imm_to_stack(self.opds[opd_idx].type_));
        self.emit_val(self.opds[opd_idx].val.unwrap());
        self.emit_stack_offset(self.temp_stack_idx(opd_idx));
        self.opd_mut(opd_depth).val = None;
    }

    /// Locals

    fn push_opd_onto_local(&mut self, local_idx: usize) {
        let opd_idx = self.opds.len() - 1;
        debug_assert!(self.opds[opd_idx].local_idx.is_none());
        self.opds[opd_idx].local_idx = Some(local_idx);
        self.opds[opd_idx].next_opd_idx = self.locals[local_idx].first_opd_idx;
        if let Some(first_opd_idx) = self.locals[local_idx].first_opd_idx {
            self.opds[first_opd_idx].prev_opd_idx = Some(opd_idx);
        }
        self.locals[local_idx].first_opd_idx = Some(opd_idx);
    }

    fn pop_opd_from_local(&mut self, opd_idx: usize) {
        let local_idx = self.opds[opd_idx].local_idx.unwrap();
        if let Some(prev_opd_idx) = self.opds[opd_idx].prev_opd_idx {
            self.opds[prev_opd_idx].next_opd_idx = self.opds[opd_idx].next_opd_idx;
        } else {
            self.locals[local_idx].first_opd_idx = self.opds[opd_idx].next_opd_idx;
        }
        if let Some(next_opd_idx) = self.opds[opd_idx].next_opd_idx {
            self.opds[next_opd_idx].prev_opd_idx = self.opds[opd_idx].prev_opd_idx;
        }
        self.opds[opd_idx].local_idx = None;
        self.opds[opd_idx].prev_opd_idx = None;
        self.opds[opd_idx].next_opd_idx = None;
    }

    fn copy_local_to_opd(&mut self, opd_idx: usize) {
        let local_idx = self.opds[opd_idx].local_idx.unwrap();
        self.emit(copy_stack(self.locals[local_idx].type_));
        self.emit_stack_offset(self.local_stack_idx(local_idx));
        self.emit_stack_offset(self.temp_stack_idx(opd_idx));
        self.pop_opd_from_local(opd_idx);
    }

    fn copy_local_to_all_opds(&mut self, local_idx: usize) {
        while let Some(opd_idx) = self.locals[local_idx].first_opd_idx {
            self.copy_local_to_opd(opd_idx);
            self.locals[local_idx].first_opd_idx = self.opds[opd_idx].next_opd_idx;
            self.opds[opd_idx].local_idx = None;
        }
    }

    /// Blocks

    /// Returns a reference to the block with the given index.
    fn block(&self, idx: usize) -> &Block {
        &self.blocks[self.blocks.len() - 1 - idx]
    }

    /// Returns a mutable reference to the block with the given index
    fn block_mut(&mut self, idx: usize) -> &mut Block {
        let len = self.blocks.len();
        &mut self.blocks[len - 1 - idx]
    }

    /// Marks the current block as unreachable.
    fn set_unreachable(&mut self) {
        while self.opds.len() > self.block(0).height {
            self.pop_opd();
        }
        self.block_mut(0).is_unreachable = true;
    }

    /// Pushes the hole with the given index onto the block with the given index.
    fn push_hole_onto_block(&mut self, block_idx: usize, hole_idx: usize) {
        self.code[hole_idx] = self.block(block_idx).first_hole_idx.unwrap_or(usize::MAX);
        self.block_mut(block_idx).first_hole_idx = Some(hole_idx);
    }

    /// Pops a hole from the block with the given index.
    fn pop_hole_from_block(&mut self, block_idx: usize) -> Option<usize> {
        if let Some(hole_idx) = self.block(block_idx).first_hole_idx {
            self.block_mut(block_idx).first_hole_idx = if self.code[hole_idx] == usize::MAX {
                None
            } else {
                Some(self.code[hole_idx])
            };
            Some(hole_idx)
        } else {
            None
        }
    }

    /// Pushes a block with the given kind and type on stack.
    fn push_block(&mut self, kind: BlockKind, type_: FuncType) {
        self.blocks.push(Block {
            kind,
            type_,
            is_unreachable: false,
            height: self.opds.len(),
            first_code_idx: self.code.len(),
            first_hole_idx: None,
            else_hole_idx: None,
        });
        for input_type in self.block(0).type_.clone().params().iter().copied() {
            self.push_opd(input_type);
        }
    }

    /// Pops a block from the stack.
    fn pop_block(&mut self) -> Block {
        while self.opds.len() > self.block(0).height {
            self.pop_opd();
        }
        self.blocks.pop().unwrap()
    }

    // Operands

    fn opd(&self, depth: usize) -> &Opd {
        &self.opds[self.opds.len() - 1 - depth]
    }

    fn opd_mut(&mut self, depth: usize) -> &mut Opd {
        let len = self.opds.len();
        &mut self.opds[len - 1 - depth]
    }

    fn ensure_opd_not_const(&mut self, opd_depth: usize) {
        if self.opd(opd_depth).is_const() {
            self.copy_const_to_opd(opd_depth);
        }
    }
    
    fn ensure_opd_not_in_local(&mut self, opd_depth: usize) {
        if self.opd(opd_depth).is_in_local() {
            self.copy_local_to_opd(self.opds.len() - 1 - opd_depth);
        }
    }

    fn ensure_opd_not_in_reg(&mut self, opd_depth: usize) {
        if self.opd(opd_depth).is_in_reg {
            self.copy_reg_to_opd(self.opd(opd_depth).type_.reg_idx());
        }
    }

    /// Pushes an operand of the given type on the stack.
    fn push_opd(&mut self, type_: impl Into<ValType>) {
        self.opds.push(Opd {
            type_: type_.into(),
            val: None,
            local_idx: None,
            prev_opd_idx: None,
            next_opd_idx: None,
            is_in_reg: false,
        });
        let stack_height = self.first_temp_stack_idx as usize + (self.opds.len() - 1);
        self.max_stack_height = self.max_stack_height.max(stack_height);
    }

    fn push_opd_and_emit_stack_offset(&mut self, type_: impl Into<ValType>) {
        self.push_opd(type_);
        self.emit_stack_offset(self.opd_stack_idx(0));
    }

    fn push_opd_and_alloc_reg(&mut self, type_: impl Into<ValType>) {
        self.push_opd(type_);
        self.alloc_reg();
    }

    /// Pops an operand from the stack.
    fn pop_opd(&mut self) -> ValType {
        if self.opd(0).is_in_reg {
            self.dealloc_reg(self.opd(0).type_.reg_idx());
        }
        let opd_idx = self.opds.len() - 1;
        if let Some(local_idx) = self.opds[opd_idx].local_idx {
            self.locals[local_idx].first_opd_idx = self.opds[opd_idx].next_opd_idx;
        }
        self.opds.pop().unwrap().type_
    }

    fn pop_opd_and_emit(&mut self) {
        self.emit_opd(0);
        self.pop_opd();
    }

    /// Stack

    /// Returns the stack index of the parameter/result with the given index.
    fn param_result_stack_idx(&self, param_result_idx: usize) -> isize {
        self.first_param_result_stack_idx + param_result_idx as isize
    }

    /// Returns the stack index of the local with the given index.
    fn local_stack_idx(&self, local_idx: usize) -> isize {
        if local_idx < self.type_.params().len() {
            self.param_result_stack_idx(local_idx)
        } else {
            (local_idx - self.type_.params().len()) as isize
        }
    }

    /// Returns the stack index of the temporary with the given index.
    fn temp_stack_idx(&self, temp_idx: usize) -> isize {
        (self.first_temp_stack_idx + temp_idx) as isize
    }

    /// Returns the stack index of the operand at the given depth.
    fn opd_stack_idx(&self, opd_depth: usize) -> isize {
        let opd_idx = self.opds.len() - 1 - opd_depth;
        if let Some(local_idx) = self.opds[opd_idx].local_idx {
            self.local_stack_idx(local_idx)
        } else {
            self.temp_stack_idx(opd_idx)
        }
    }

    /// Registers

    /// Returns `true` if the register with the given index is used.
    fn is_reg_used(&self, reg_idx: usize) -> bool {
        self.regs[reg_idx].is_some()
    }

    /// Allocates a register to the top operand.
    fn alloc_reg(&mut self) {
        debug_assert!(!self.opd(0).is_in_reg);
        let reg_idx = self.opd(0).type_.reg_idx();
        debug_assert!(!self.is_reg_used(reg_idx));
        let opd_idx = self.opds.len() - 1;
        self.opds[opd_idx].is_in_reg = true;
        self.regs[reg_idx] = Some(opd_idx);
    }

    /// Deallocates the register with the given index.
    fn dealloc_reg(&mut self, reg_idx: usize) {
        let opd_idx = self.regs[reg_idx].unwrap();
        self.opds[opd_idx].is_in_reg = false;
        self.regs[reg_idx] = None;
    }

    fn copy_reg_to_opd(&mut self, reg_idx: usize) {
        let opd_idx = self.regs[reg_idx].unwrap();
        let opd_type = self.opds[opd_idx].type_;
        self.emit(copy_reg_to_stack(opd_type));
        self.emit_stack_offset(self.temp_stack_idx(opd_idx));
        self.dealloc_reg(reg_idx);
    }

    fn copy_all_regs_to_opd(&mut self) {
        for reg_idx in 0..self.regs.len() {
            if self.is_reg_used(reg_idx) {
                self.copy_reg_to_opd(reg_idx);
            }
        }
    }

    fn resolve_label_vals(&mut self, label_idx: usize) {
        for (label_val_idx, label_type) in self
            .block(label_idx)
            .label_types()
            .iter()
            .copied()
            .enumerate()
            .rev()
        {
            self.emit(copy_stack(label_type));
            self.emit_stack_offset(self.opd_stack_idx(0));
            self.pop_opd();
            self.emit_stack_offset(
                self.temp_stack_idx(self.block(label_idx).height + label_val_idx),
            );
        }
    }

    /// Emitting

    fn emit<T>(&mut self, val: T)
    where
        T: Copy,
    {
        debug_assert!(mem::size_of::<T>() <= mem::size_of::<CodeSlot>());
        self.code.push(CodeSlot::default());
        unsafe { *(self.code.last_mut().unwrap() as *mut _ as *mut T) = val };
    }

    fn emit_opd(&mut self, opd_depth: usize) {
        match self.opd(opd_depth).kind() {
            OpdKind::Stack => {
                self.emit_stack_offset(self.opd_stack_idx(opd_depth))
            }
            OpdKind::Reg => {},
            OpdKind::Imm => {
                self.emit_val(self.opd(opd_depth).val.unwrap());
            }
        }
    }

    fn emit_val(&mut self, val: UnguardedVal) {
        match val {
            UnguardedVal::I32(val) => self.emit(val),
            UnguardedVal::I64(val) => self.emit(val),
            UnguardedVal::F32(val) => self.emit(val),
            UnguardedVal::F64(val) => self.emit(val),
            UnguardedVal::FuncRef(val) => self.emit(val),
            UnguardedVal::ExternRef(val) => self.emit(val),
        }
    }

    fn emit_stack_offset(&mut self, stack_idx: isize) {
        self.emit(stack_idx * mem::size_of::<StackSlot>() as isize);
    }

    fn emit_label(&mut self, block_idx: usize) {
        match self.block(block_idx).kind {
            BlockKind::Block => {
                let hole_idx = self.emit_hole();
                self.push_hole_onto_block(block_idx, hole_idx);
            }
            BlockKind::Loop => {
                self.emit_code_offset(self.block(block_idx).first_code_idx);
            }
        }
    }

    fn emit_hole(&mut self) -> usize {
        let hole_idx = self.code.len();
        self.code.push(0);
        hole_idx
    }

    fn patch_hole(&mut self, hole_idx: usize) {
        self.fixup_idxs.push(hole_idx);
        self.code[hole_idx] = self.code.len() * mem::size_of::<usize>();
    }

    fn emit_code_offset(&mut self, code_idx: usize) {
        self.fixup_idxs.push(self.code.len());
        self.emit(code_idx * mem::size_of::<CodeSlot>());
    }
}

impl<'a> InstrVisitor for Compile<'a> {
    type Error = DecodeError;

    // Control instructions
    fn visit_nop(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_unreachable(&mut self) -> Result<(), Self::Error> {
        self.emit(exec::unreachable as ThreadedInstr);
        self.set_unreachable();
        Ok(())
    }

    fn visit_block(&mut self, type_: BlockType) -> Result<(), Self::Error> {
        let type_ = self.resolve_block_type(type_);
        if !self.block(0).is_unreachable {
            for opd_depth in 0..type_.params().len() {
                self.ensure_opd_not_const(opd_depth);
                self.ensure_opd_not_in_local(opd_depth);
                self.ensure_opd_not_in_reg(opd_depth);
            }
            for _ in 0..type_.params().len() {
                self.pop_opd();
            }
        }
        self.push_block(BlockKind::Block, type_);
        Ok(())
    }

    fn visit_loop(&mut self, type_: BlockType) -> Result<(), Self::Error> {
        let type_ = self.resolve_block_type(type_);
        if !self.block(0).is_unreachable {
            for opd_depth in 0..type_.params().len() {
                self.ensure_opd_not_const(opd_depth);
                self.ensure_opd_not_in_local(opd_depth);
                self.ensure_opd_not_in_reg(opd_depth);
            }
            for _ in 0..type_.params().len() {
                self.pop_opd();
            }
        }
        self.push_block(BlockKind::Loop, type_);
        Ok(())
    }

    fn visit_if(&mut self, type_: BlockType) -> Result<(), Self::Error> {
        let type_ = self.resolve_block_type(type_);
        let else_hole_idx = if !self.block(0).is_unreachable {
            self.ensure_opd_not_const(0);
            for opd_depth in 1..type_.params().len() + 1 {
                self.ensure_opd_not_const(opd_depth);
                self.ensure_opd_not_in_local(opd_depth);
                self.ensure_opd_not_in_reg(opd_depth);
            }
            self.emit(br_if_z(self.opd(0).kind()));
            self.pop_opd_and_emit();
            let else_hole_idx = self.emit_hole();
            for _ in 0..type_.params().len() {
                self.pop_opd();
            }
            Some(else_hole_idx)
        } else {
            None
        };
        self.push_block(BlockKind::Block, type_);
        self.block_mut(0).else_hole_idx = else_hole_idx;
        Ok(())
    }

    fn visit_else(&mut self) -> Result<(), Self::Error> {
        if !self.block(0).is_unreachable {
            for opd_depth in 0..self.block(0).type_.results().len() {
                self.ensure_opd_not_const(opd_depth);
                self.ensure_opd_not_in_local(opd_depth);
                self.ensure_opd_not_in_reg(opd_depth);
            }
            self.emit(exec::br as ThreadedInstr);
            let hole_idx = self.emit_hole();
            self.push_hole_onto_block(0, hole_idx);
        }
        if let Some(else_hole_idx) = self.block_mut(0).else_hole_idx.take() {
            self.patch_hole(else_hole_idx);
        }
        let block = self.pop_block();
        self.push_block(BlockKind::Block, block.type_);
        self.block_mut(0).first_hole_idx = block.first_hole_idx;
        Ok(())
    }

    fn visit_end(&mut self) -> Result<(), Self::Error> {
        if !self.block(0).is_unreachable {
            for opd_depth in 0..self.block(0).type_.results().len() {
                self.ensure_opd_not_const(opd_depth);
                self.ensure_opd_not_in_local(opd_depth);
                self.ensure_opd_not_in_reg(opd_depth);
            }
        }
        if let Some(else_hole_idx) = self.block_mut(0).else_hole_idx.take() {
            self.patch_hole(else_hole_idx);
        }
        while let Some(hole_idx) = self.pop_hole_from_block(0) {
            self.patch_hole(hole_idx);
        }
        let block = self.pop_block();
        for result_type in block.type_.results().iter().copied() {
            self.push_opd(result_type);
        }
        Ok(())
    }

    fn visit_br(&mut self, label_idx: u32) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let label_idx = label_idx as usize;
        for opd_depth in 0..self.block(label_idx).label_types().len() {
            self.ensure_opd_not_const(opd_depth);
            self.ensure_opd_not_in_local(opd_depth);
            self.ensure_opd_not_in_reg(opd_depth);
        }
        self.resolve_label_vals(label_idx);
        self.emit(exec::br as ThreadedInstr);
        self.emit_label(label_idx);
        self.set_unreachable();
        Ok(())
    }

    fn visit_br_if(&mut self, label_idx: u32) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let label_idx = label_idx as usize;
        self.ensure_opd_not_const(0);
        for opd_depth in 1..self.block(label_idx).label_types().len() + 1 {
            self.ensure_opd_not_const(opd_depth);
            self.ensure_opd_not_in_local(opd_depth);
            self.ensure_opd_not_in_reg(opd_depth);
        }
        if self.block(label_idx).label_types().is_empty() {
            self.emit(br_if_nz(self.opd(0).kind()));
            self.pop_opd_and_emit();
            self.emit_label(label_idx);
        } else {
            self.emit(br_if_z(self.opd(0).kind()));
            self.pop_opd_and_emit();
            let hole_idx = self.emit_hole();
            self.resolve_label_vals(label_idx);
            self.emit(exec::br as ThreadedInstr);
            self.emit_label(label_idx);
            self.patch_hole(hole_idx);
        }
        for label_type in self.block(label_idx).label_types().iter().copied() {
            self.push_opd(label_type);
        }
        Ok(())
    }

    fn visit_br_table(
        &mut self,
        label_idxs: &[u32],
        default_label_idx: u32,
    ) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let default_label_idx = default_label_idx as usize;
        self.ensure_opd_not_const(0);
        for opd_depth in 1..self.block(default_label_idx).label_types().len() + 1 {
            self.ensure_opd_not_const(opd_depth);
            self.ensure_opd_not_in_local(opd_depth);
            self.ensure_opd_not_in_reg(opd_depth);
        }
        if self.block(default_label_idx).label_types().is_empty() {
            self.emit(br_table(self.opd(0).kind()));
            self.pop_opd_and_emit();
            self.emit(label_idxs.len() as u32);
            for label_idx in label_idxs.iter().copied() {
                let label_idx = label_idx as usize;
                self.emit_label(label_idx);
                for label_type in self.block(0).label_types().iter().copied() {
                    self.push_opd(label_type);
                }
            }
            self.emit_label(default_label_idx);
        } else {
            self.emit(br_table(self.opd(0).kind()));
            self.pop_opd_and_emit();
            self.emit(label_idxs.len() as u32);
            let mut hole_idxs = Vec::new();
            for _ in 0..label_idxs.len() {
                let hole_idx = self.emit_hole();
                hole_idxs.push(hole_idx);
            }
            let default_hole_idx = self.emit_hole();
            for (label_idx, hole_idx) in label_idxs.iter().copied().zip(hole_idxs) {
                let label_idx = label_idx as usize;
                self.patch_hole(hole_idx);
                self.resolve_label_vals(label_idx);
                self.emit(exec::br as ThreadedInstr);
                self.emit_label(label_idx);
                for label_type in self.block(label_idx).label_types().iter().copied() {
                    self.push_opd(label_type);
                }
            }
            self.patch_hole(default_hole_idx);
            self.resolve_label_vals(default_label_idx);
            self.emit(exec::br as ThreadedInstr);
            self.emit_label(default_label_idx);
        }
        self.set_unreachable();
        Ok(())
    }

    fn visit_return(&mut self) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        for (result_idx, result_type) in self
            .type_
            .clone()
            .results()
            .iter()
            .copied()
            .enumerate()
            .rev()
        {
            self.ensure_opd_not_const(0);
            self.emit(if self.opd(0).is_in_reg {
                copy_reg_to_stack(result_type)
            } else {
                copy_stack(result_type)
            });
            self.pop_opd_and_emit();
            self.emit_stack_offset(self.param_result_stack_idx(result_idx));
        }
        self.emit(exec::return_ as ThreadedInstr);
        self.set_unreachable();
        Ok(())
    }

    fn visit_call(&mut self, func_idx: u32) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let func = self.instance.func(func_idx).unwrap();
        let type_ = func.type_(&self.store).clone();
        for opd_depth in 0..type_.params().len() {
            self.ensure_opd_not_const(opd_depth);
            self.ensure_opd_not_in_local(opd_depth);
        }
        self.copy_all_regs_to_opd();
        self.emit(match func.0.as_ref(&self.store) {
            FuncEntity::Wasm(_) => exec::compile as ThreadedInstr,
            FuncEntity::Host(_) => exec::call_host as ThreadedInstr,
        });
        for _ in 0..type_.params().len() {
            self.pop_opd();
        }
        self.emit(func.0.to_unguarded(self.store.id()));
        let first_callee_stack_idx = self.first_temp_stack_idx + self.opds.len();
        let last_callee_stack_idx = first_callee_stack_idx + type_.callee_stack_slot_count();
        self.max_stack_height = self.max_stack_height.max(last_callee_stack_idx);
        self.emit_stack_offset(last_callee_stack_idx as isize);
        if let FuncEntity::Host(_) = func.0.as_ref(&self.store) {
            self.emit(
                self.instance
                    .mem(0)
                    .map(|mem| mem.0.to_unguarded(self.store.id())),
            );
        }
        for result_type in type_.results().iter().copied() {
            self.push_opd(result_type);
        }
        Ok(())
    }

    fn visit_call_indirect(&mut self, table_idx: u32, type_idx: u32) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let table = self.instance.table(table_idx).unwrap();
        let interned_type = self.instance.type_(type_idx).unwrap();
        let type_ = self.store.resolve_type(interned_type).clone();
        self.ensure_opd_not_const(0);
        for opd_depth in 1..type_.params().len() + 1 {
            self.ensure_opd_not_const(opd_depth);
            self.ensure_opd_not_in_local(opd_depth);
        }
        self.copy_all_regs_to_opd();
        self.emit(exec::call_indirect as ThreadedInstr);
        self.emit_stack_offset(self.opd_stack_idx(0));
        self.pop_opd();
        for _ in 0..type_.params().len() {
            self.pop_opd();
        }
        self.emit(table.0.to_unguarded(self.store.id()));
        self.emit(interned_type.to_unguarded(self.store.id()));
        let first_callee_stack_idx = self.first_temp_stack_idx + self.opds.len();
        let last_callee_stack_idx = first_callee_stack_idx + type_.callee_stack_slot_count();
        self.max_stack_height = self.max_stack_height.max(last_callee_stack_idx as usize);
        self.emit_stack_offset(last_callee_stack_idx as isize);
        self.emit(
            self.instance
                .mem(0)
                .map(|mem| mem.0.to_unguarded(self.store.id())),
        );
        for result_type in type_.results().iter().copied() {
            self.push_opd(result_type);
        }
        Ok(())
    }

    // Reference instructions
    fn visit_ref_null(&mut self, type_: RefType) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        self.emit(ref_null(type_));
        match type_ {
            RefType::FuncRef => self.emit(FuncRef::null().to_unguarded(self.store.id())),
            RefType::ExternRef => self.emit(ExternRef::null().to_unguarded(self.store.id())),
        };
        self.push_opd_and_emit_stack_offset(type_);
        Ok(())
    }

    fn visit_ref_is_null(&mut self) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        self.ensure_opd_not_const(0);
        self.emit(ref_is_null(self.opd(0).type_.to_ref().unwrap(), self.opd(0).kind()));
        self.pop_opd_and_emit();
        self.push_opd_and_alloc_reg(ValType::I32);
        Ok(())
    }

    fn visit_ref_func(&mut self, func_idx: u32) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        self.emit(exec::copy_imm_to_stack_func_ref as ThreadedInstr);
        self.emit(
            self.instance
                .func(func_idx)
                .unwrap()
                .to_unguarded(self.store.id()),
        );
        self.push_opd_and_emit_stack_offset(ValType::FuncRef);
        Ok(())
    }

    // Parametric instructions
    fn visit_drop(&mut self) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        self.pop_opd();
        Ok(())
    }

    fn visit_select(&mut self, type_: Option<ValType>) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let type_ = type_.unwrap_or_else(|| self.opd(1).type_);
        for opd_depth in 0..3 {
            self.ensure_opd_not_const(opd_depth);
        }
        // If this operation has an output, and the output register is used, then we need to save
        // the output register, unless one of its inputs is in the output register. Otherwise, the
        // operation will overwrite the output register while it's already used.
        let output_reg_idx = type_.reg_idx();
        if self.is_reg_used(output_reg_idx)
            && !self.opd(2).is_in_given_reg(output_reg_idx)
            && !self.opd(1).is_in_given_reg(output_reg_idx)
            && !self.opd(0).is_in_given_reg(output_reg_idx)
        {
            self.copy_reg_to_opd(output_reg_idx);
        }
        self.emit(select(
            type_,
            self.opd(2).kind(),
            self.opd(1).kind(),
            self.opd(0).kind(),
        ));
        self.pop_opd_and_emit();
        self.pop_opd_and_emit();
        self.pop_opd_and_emit();
        self.push_opd_and_alloc_reg(type_);
        Ok(())
    }

    // Variable instructions
    fn visit_local_get(&mut self, local_idx: u32) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let local_idx = local_idx as usize;
        self.push_opd(self.locals[local_idx].type_);
        self.push_opd_onto_local(local_idx);
        Ok(())
    }

    fn visit_local_set(&mut self, local_idx: u32) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let local_idx = local_idx as usize;
        let local_type = self.locals[local_idx].type_;
        self.copy_local_to_all_opds(local_idx);
        self.emit(match self.opd(0).kind() {
            OpdKind::Stack => copy_stack(local_type),
            OpdKind::Reg => copy_reg_to_stack(local_type),
            OpdKind::Imm => copy_imm_to_stack(local_type),
        });
        self.pop_opd_and_emit();
        self.emit_stack_offset(self.local_stack_idx(local_idx));
        Ok(())
    }

    fn visit_local_tee(&mut self, local_idx: u32) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let local_idx = local_idx as usize;
        let local_type = self.locals[local_idx].type_;
        self.ensure_opd_not_const(0);
        self.copy_local_to_all_opds(local_idx);
        self.emit(if self.opd(0).is_in_reg {
            copy_reg_to_stack(local_type)
        } else {
            copy_stack(local_type)
        });
        if !self.opd(0).is_in_reg {
            self.emit_stack_offset(self.opd_stack_idx(0));
        }
        self.emit_stack_offset(self.local_stack_idx(local_idx));
        Ok(())
    }

    fn visit_global_get(&mut self, global_idx: u32) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let global = self.instance.global(global_idx).unwrap();
        let val_type = global.type_(&self.store).val;
        self.emit(global_get(val_type));
        self.emit(global.to_unguarded(self.store.id()));
        self.push_opd_and_emit_stack_offset(val_type);
        Ok(())
    }

    fn visit_global_set(&mut self, global_idx: u32) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let global = self.instance.global(global_idx).unwrap();
        let val_type = global.type_(&self.store).val;
        self.ensure_opd_not_const(0);
        self.emit(global_set(val_type, self.opd(0).kind()));
        self.pop_opd_and_emit();
        self.emit(global.to_unguarded(self.store.id()));
        Ok(())
    }

    // Table instructions
    fn visit_table_get(&mut self, table_idx: u32) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let table = self.instance.table(table_idx).unwrap();
        let elem_type = table.type_(&self.store).elem;
        self.ensure_opd_not_const(0);
        self.emit(table_get(elem_type, self.opd(0).kind()));
        self.pop_opd_and_emit();
        self.emit(table.to_unguarded(self.store.id()));
        self.push_opd_and_emit_stack_offset(elem_type);
        Ok(())
    }

    fn visit_table_set(&mut self, table_idx: u32) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let table = self.instance.table(table_idx).unwrap();
        let elem_type = table.type_(&self.store).elem;
        for opd_depth in 0..2 {
            self.ensure_opd_not_const(opd_depth);
        }
        self.emit(table_set(elem_type, self.opd(1).kind(), self.opd(0).kind()));
        self.pop_opd_and_emit();
        self.pop_opd_and_emit();
        self.emit(table.to_unguarded(self.store.id()));
        Ok(())
    }

    fn visit_table_size(&mut self, table_idx: u32) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let table = self.instance.table(table_idx).unwrap();
        let elem_type = table.type_(&self.store).elem;
        self.emit(table_size(elem_type));
        self.emit(table.to_unguarded(self.store.id()));
        self.push_opd_and_emit_stack_offset(ValType::I32);
        Ok(())
    }

    fn visit_table_grow(&mut self, table_idx: u32) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let table = self.instance.table(table_idx).unwrap();
        let elem_type = table.type_(&self.store).elem;
        for opd_depth in 0..2 {
            self.ensure_opd_not_const(opd_depth);
            self.ensure_opd_not_in_reg(opd_depth);
        }
        self.emit(table_grow(elem_type));
        self.pop_opd_and_emit();
        self.pop_opd_and_emit();
        self.emit(table.to_unguarded(self.store.id()));
        self.push_opd_and_emit_stack_offset(ValType::I32);
        Ok(())
    }

    fn visit_table_fill(&mut self, table_idx: u32) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let table = self.instance.table(table_idx).unwrap();
        let elem_type = table.type_(&self.store).elem;
        for opd_depth in 0..3 {
            self.ensure_opd_not_const(opd_depth);
            self.ensure_opd_not_in_reg(opd_depth);
        }
        self.emit(table_fill(elem_type));
        self.pop_opd_and_emit();
        self.pop_opd_and_emit();
        self.pop_opd_and_emit();
        self.emit(table.to_unguarded(self.store.id()));
        Ok(())
    }

    fn visit_table_copy(
        &mut self,
        dst_table_idx: u32,
        src_table_idx: u32,
    ) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let dst_table = self.instance.table(dst_table_idx).unwrap();
        let src_table = self.instance.table(src_table_idx).unwrap();
        let elem_type = dst_table.type_(&self.store).elem;
        for opd_depth in 0..3 {
            self.ensure_opd_not_const(opd_depth);
            self.ensure_opd_not_in_reg(opd_depth);
        }
        self.emit(table_copy(elem_type));
        self.pop_opd_and_emit();
        self.pop_opd_and_emit();
        self.pop_opd_and_emit();
        self.emit(dst_table.to_unguarded(self.store.id()));
        self.emit(src_table.to_unguarded(self.store.id()));
        Ok(())
    }

    fn visit_table_init(
        &mut self,
        dst_table_idx: u32,
        src_elem_idx: u32,
    ) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let dst_table = self.instance.table(dst_table_idx).unwrap();
        let src_elem = self.instance.elem(src_elem_idx).unwrap();
        let elem_type = dst_table.type_(&self.store).elem;
        for opd_depth in 0..3 {
            self.ensure_opd_not_const(opd_depth);
            self.ensure_opd_not_in_reg(opd_depth);
        }
        self.emit(table_init(elem_type));
        self.pop_opd_and_emit();
        self.pop_opd_and_emit();
        self.pop_opd_and_emit();
        self.emit(dst_table.0.to_unguarded(self.store.id()));
        self.emit(src_elem.0.to_unguarded(self.store.id()));
        Ok(())
    }

    fn visit_elem_drop(&mut self, elem_idx: u32) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let elem = self.instance.elem(elem_idx).unwrap();
        let elem_type = elem.type_(&self.store);
        self.emit(elem_drop(elem_type));
        self.emit(elem.to_unguarded(self.store.id()));
        Ok(())
    }

    // Memory instructions
    fn visit_load(&mut self, arg: MemArg, info: LoadInfo) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        self.visit_un_op(info.op)?;
        self.emit(arg.offset);
        Ok(())
    }

    fn visit_store(&mut self, arg: MemArg, info: StoreInfo) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        self.visit_bin_op(info.op)?;
        self.emit(arg.offset);
        Ok(())
    }

    fn visit_memory_size(&mut self) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let mem = self.instance.mem(0).unwrap();
        self.emit(exec::memory_size as ThreadedInstr);
        self.emit(mem.to_unguarded(self.store.id()));
        self.push_opd_and_emit_stack_offset(ValType::I32);
        Ok(())
    }

    fn visit_memory_grow(&mut self) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let mem = self.instance.mem(0).unwrap();
        self.ensure_opd_not_const(0);
        self.ensure_opd_not_in_reg(0);
        self.emit(exec::memory_grow as ThreadedInstr);
        self.pop_opd_and_emit();
        self.emit(mem.to_unguarded(self.store.id()));
        self.push_opd_and_emit_stack_offset(ValType::I32);
        Ok(())
    }

    fn visit_memory_fill(&mut self) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let mem = self.instance.mem(0).unwrap();
        for opd_depth in 0..3 {
            self.ensure_opd_not_const(opd_depth);
            self.ensure_opd_not_in_reg(opd_depth);
        }
        self.emit(exec::memory_fill as ThreadedInstr);
        self.pop_opd_and_emit();
        self.pop_opd_and_emit();
        self.pop_opd_and_emit();
        self.emit(mem.to_unguarded(self.store.id()));
        Ok(())
    }

    fn visit_memory_copy(&mut self) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let mem = self.instance.mem(0).unwrap();
        for opd_depth in 0..3 {
            self.ensure_opd_not_const(opd_depth);
            self.ensure_opd_not_in_reg(opd_depth);
        }
        self.emit(exec::memory_copy as ThreadedInstr);
        self.pop_opd_and_emit();
        self.pop_opd_and_emit();
        self.pop_opd_and_emit();
        self.emit(mem.to_unguarded(self.store.id()));
        Ok(())
    }

    fn visit_memory_init(&mut self, data_idx: u32) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let dst_mem = self.instance.mem(0).unwrap();
        let src_data = self.instance.data(data_idx).unwrap();
        for opd_depth in 0..3 {
            self.ensure_opd_not_const(opd_depth);
            self.ensure_opd_not_in_reg(opd_depth);
        }
        self.emit(exec::memory_init as ThreadedInstr);
        self.pop_opd_and_emit();
        self.pop_opd_and_emit();
        self.pop_opd_and_emit();
        self.emit(dst_mem.to_unguarded(self.store.id()));
        self.emit(src_data.to_unguarded(self.store.id()));
        Ok(())
    }

    fn visit_data_drop(&mut self, data_idx: u32) -> Result<(), Self::Error> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        let data = self.instance.data(data_idx).unwrap();
        self.emit(exec::data_drop as ThreadedInstr);
        self.emit(data.to_unguarded(self.store.id()));
        Ok(())
    }

    // Numeric instructions
    fn visit_i32_const(&mut self, val: i32) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        self.push_opd(ValType::I32);
        self.opd_mut(0).val = Some(UnguardedVal::I32(val));
        Ok(())
    }

    fn visit_i64_const(&mut self, val: i64) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        self.push_opd(ValType::I64);
        self.opd_mut(0).val = Some(UnguardedVal::I64(val));
        Ok(())
    }

    fn visit_f32_const(&mut self, val: f32) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        self.push_opd(ValType::F32);
        self.opd_mut(0).val = Some(UnguardedVal::F32(val));
        Ok(())
    }

    fn visit_f64_const(&mut self, val: f64) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        self.push_opd(ValType::F64);
        self.opd_mut(0).val = Some(UnguardedVal::F64(val));
        Ok(())
    }

    fn visit_un_op(&mut self, info: UnOpInfo) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        if self.opd(0).is_const() && info.instr_i == exec::unreachable {
            self.ensure_opd_not_const(0);
        }
        // If this operation has an output, and the output register is used, then we need to save
        // the output register, unless one of its inputs is in the output register. Otherwise, the
        // operation will overwrite the output register while it's already used.
        if let Some(output_type) = info.output_type {
            let output_reg_idx = output_type.reg_idx();
            if self.is_reg_used(output_reg_idx) && !self.opd(0).is_in_given_reg(output_reg_idx) {
                self.copy_reg_to_opd(output_reg_idx);
            }
        }
        self.emit(match self.opd(0).kind() {
            OpdKind::Stack => info.instr_s,
            OpdKind::Reg => info.instr_r,
            OpdKind::Imm => info.instr_i,
        });
        self.pop_opd_and_emit();
        if let Some(output_type) = info.output_type {
            self.push_opd_and_alloc_reg(output_type);
        }
        Ok(())
    }

    fn visit_bin_op(&mut self, info: BinOpInfo) -> Result<(), DecodeError> {
        if self.block(0).is_unreachable {
            return Ok(());
        }
        if self.opd(0).is_const() && self.opd(1).is_const() && info.instr_ii == exec::unreachable {
            self.ensure_opd_not_const(0);
        }
        // If this operation has an output, and the output register is used, then we need to save
        // the output register, unless one of its inputs is in the output register. Otherwise, the
        // operation will overwrite the output register while it's already used.
        if let Some(output_type) = info.output_type {
            let output_reg_idx = output_type.reg_idx();
            if self.is_reg_used(output_reg_idx)
                && !self.opd(1).is_in_given_reg(output_reg_idx)
                && !self.opd(0).is_in_given_reg(output_reg_idx)
            {
                self.copy_reg_to_opd(output_reg_idx);
            }
        }
        self.emit(match (self.opd(1).kind(), self.opd(0).kind()) {
            (OpdKind::Stack, OpdKind::Stack) => info.instr_ss,
            (OpdKind::Reg, OpdKind::Stack) => info.instr_rs,
            (OpdKind::Imm, OpdKind::Stack) => info.instr_is,
            (OpdKind::Stack, OpdKind::Reg) => info.instr_sr,
            (OpdKind::Reg, OpdKind::Reg) => info.instr_rr,
            (OpdKind::Imm, OpdKind::Reg) => info.instr_ir,
            (OpdKind::Stack, OpdKind::Imm) => info.instr_si,
            (OpdKind::Reg, OpdKind::Imm) => info.instr_ri,
            (OpdKind::Imm, OpdKind::Imm) => info.instr_ii,
        });
        match (self.opd(1).kind(), self.opd(0).kind()) {
            (OpdKind::Stack, OpdKind::Imm) if info.instr_is == info.instr_si => {
                self.emit_opd(1);
                self.emit_opd(0);
                self.pop_opd();
                self.pop_opd();
            }
            _ => {
                self.pop_opd_and_emit();
                self.pop_opd_and_emit();
            }
        }
        if let Some(output_type) = info.output_type {
            self.push_opd_and_alloc_reg(output_type);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct Local {
    type_: ValType,
    first_opd_idx: Option<usize>,
}

#[derive(Clone, Debug)]
struct Block {
    kind: BlockKind,
    type_: FuncType,
    is_unreachable: bool,
    height: usize,
    first_code_idx: usize,
    else_hole_idx: Option<usize>,
    first_hole_idx: Option<usize>,
}

impl Block {
    fn label_types(&self) -> LabelTypes {
        LabelTypes {
            kind: self.kind,
            type_: self.type_.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum BlockKind {
    Block,
    Loop,
}

#[derive(Clone, Debug)]
struct LabelTypes {
    kind: BlockKind,
    type_: FuncType,
}

impl Deref for LabelTypes {
    type Target = [ValType];

    fn deref(&self) -> &Self::Target {
        match self.kind {
            BlockKind::Block => self.type_.results(),
            BlockKind::Loop => self.type_.params(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Opd {
    type_: ValType,
    val: Option<UnguardedVal>,
    local_idx: Option<usize>,
    prev_opd_idx: Option<usize>,
    next_opd_idx: Option<usize>,
    is_in_reg: bool,
}

impl Opd {
    fn is_const(&self) -> bool {
        self.val.is_some()
    }

    fn is_in_local(&self) -> bool {
        self.local_idx.is_some()
    }

    fn is_in_given_reg(&self, reg_idx: usize) -> bool {
        self.is_in_reg && self.type_.reg_idx() == reg_idx
    }

    fn kind(&self) -> OpdKind {
        if self.is_const() {
            OpdKind::Imm
        } else if self.is_in_reg {
            OpdKind::Reg
        } else {
            OpdKind::Stack
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum OpdKind {
    Stack,
    Reg,
    Imm,
}

fn br_if_z(kind: OpdKind) -> ThreadedInstr {
    match kind {
        OpdKind::Stack => exec::br_if_z_s,
        OpdKind::Reg => exec::br_if_z_r,
        OpdKind::Imm => exec::unreachable,
    }
}

fn br_if_nz(kind: OpdKind) -> ThreadedInstr {
    match kind {
        OpdKind::Stack => exec::br_if_nz_s,
        OpdKind::Reg => exec::br_if_nz_r,
        OpdKind::Imm => exec::unreachable,
    }
}

fn br_table(kind: OpdKind) -> ThreadedInstr {
    match kind {
        OpdKind::Stack => exec::br_table_s,
        OpdKind::Reg => exec::br_table_r,
        OpdKind::Imm => exec::unreachable,
    }
}

fn ref_null(type_: RefType) -> ThreadedInstr {
    match type_ {
        RefType::FuncRef => exec::copy_imm_to_stack_func_ref,
        RefType::ExternRef => exec::copy_imm_to_stack_extern_ref,
    }
}

fn ref_is_null(type_: RefType, kind: OpdKind) -> ThreadedInstr {
    match (type_, kind) {
        (RefType::FuncRef, OpdKind::Stack) => exec::ref_is_null_func_ref_s,
        (RefType::FuncRef, OpdKind::Reg) => exec::ref_is_null_func_ref_r,

        (RefType::ExternRef, OpdKind::Stack) => exec::ref_is_null_extern_ref_s,
        (RefType::ExternRef, OpdKind::Reg) => exec::ref_is_null_extern_ref_r,

        (_, OpdKind::Imm) => exec::unreachable,
    }
}

fn select(type_: ValType, kind_0: OpdKind, kind_1: OpdKind, kind_2: OpdKind) -> ThreadedInstr {
    match (type_, kind_0, kind_1, kind_2) {
        (ValType::I32, OpdKind::Stack, OpdKind::Stack, OpdKind::Stack) => exec::select_i32_sss,
        (ValType::I32, OpdKind::Reg, OpdKind::Stack, OpdKind::Stack) => exec::select_i32_rss,
        (ValType::I32, OpdKind::Imm, OpdKind::Stack, OpdKind::Stack) => exec::select_i32_iss,
        (ValType::I32, OpdKind::Stack, OpdKind::Reg, OpdKind::Stack) => exec::select_i32_srs,
        (ValType::I32, OpdKind::Imm, OpdKind::Reg, OpdKind::Stack) => exec::select_i32_irs,
        (ValType::I32, OpdKind::Stack, OpdKind::Imm, OpdKind::Stack) => exec::select_i32_sis,
        (ValType::I32, OpdKind::Reg, OpdKind::Imm, OpdKind::Stack) => exec::select_i32_ris,
        (ValType::I32, OpdKind::Imm, OpdKind::Imm, OpdKind::Stack) => exec::select_i32_iis,
        (ValType::I32, OpdKind::Stack, OpdKind::Stack, OpdKind::Reg) => exec::select_i32_ssr,
        (ValType::I32, OpdKind::Imm, OpdKind::Stack, OpdKind::Reg) => exec::select_i32_isr,
        (ValType::I32, OpdKind::Stack, OpdKind::Imm, OpdKind::Reg) => exec::select_i32_sir,
        (ValType::I32, OpdKind::Imm, OpdKind::Imm, OpdKind::Reg) => exec::select_i32_iir,
        (ValType::I32, OpdKind::Reg, _, OpdKind::Reg) => exec::unreachable,
        (ValType::I32, _, OpdKind::Reg, OpdKind::Reg) => exec::unreachable,

        (ValType::I64, OpdKind::Stack, OpdKind::Stack, OpdKind::Stack) => exec::select_i64_sss,
        (ValType::I64, OpdKind::Reg, OpdKind::Stack, OpdKind::Stack) => exec::select_i64_rss,
        (ValType::I64, OpdKind::Imm, OpdKind::Stack, OpdKind::Stack) => exec::select_i64_iss,
        (ValType::I64, OpdKind::Stack, OpdKind::Reg, OpdKind::Stack) => exec::select_i64_srs,
        (ValType::I64, OpdKind::Imm, OpdKind::Reg, OpdKind::Stack) => exec::select_i64_irs,
        (ValType::I64, OpdKind::Stack, OpdKind::Imm, OpdKind::Stack) => exec::select_i64_sis,
        (ValType::I64, OpdKind::Reg, OpdKind::Imm, OpdKind::Stack) => exec::select_i64_ris,
        (ValType::I64, OpdKind::Imm, OpdKind::Imm, OpdKind::Stack) => exec::select_i64_iis,
        (ValType::I64, OpdKind::Stack, OpdKind::Stack, OpdKind::Reg) => exec::select_i64_ssr,
        (ValType::I64, OpdKind::Imm, OpdKind::Stack, OpdKind::Reg) => exec::select_i64_isr,
        (ValType::I64, OpdKind::Stack, OpdKind::Imm, OpdKind::Reg) => exec::select_i64_sir,
        (ValType::I64, OpdKind::Imm, OpdKind::Imm, OpdKind::Reg) => exec::select_i64_iir,
        (ValType::I64, OpdKind::Reg, _, OpdKind::Reg) => exec::unreachable,
        (ValType::I64, _, OpdKind::Reg, OpdKind::Reg) => exec::unreachable,

        (ValType::F32, OpdKind::Stack, OpdKind::Stack, OpdKind::Stack) => exec::select_f32_sss,
        (ValType::F32, OpdKind::Reg, OpdKind::Stack, OpdKind::Stack) => exec::select_f32_rss,
        (ValType::F32, OpdKind::Imm, OpdKind::Stack, OpdKind::Stack) => exec::select_f32_iss,
        (ValType::F32, OpdKind::Stack, OpdKind::Reg, OpdKind::Stack) => exec::select_f32_srs,
        (ValType::F32, OpdKind::Imm, OpdKind::Reg, OpdKind::Stack) => exec::select_f32_irs,
        (ValType::F32, OpdKind::Stack, OpdKind::Imm, OpdKind::Stack) => exec::select_f32_sis,
        (ValType::F32, OpdKind::Reg, OpdKind::Imm, OpdKind::Stack) => exec::select_f32_ris,
        (ValType::F32, OpdKind::Imm, OpdKind::Imm, OpdKind::Stack) => exec::select_f32_iis,
        (ValType::F32, OpdKind::Stack, OpdKind::Stack, OpdKind::Reg) => exec::select_f32_ssr,
        (ValType::F32, OpdKind::Imm, OpdKind::Stack, OpdKind::Reg) => exec::select_f32_isr,
        (ValType::F32, OpdKind::Stack, OpdKind::Imm, OpdKind::Reg) => exec::select_f32_sir,
        (ValType::F32, OpdKind::Imm, OpdKind::Imm, OpdKind::Reg) => exec::select_f32_iir,
        (ValType::F32, OpdKind::Reg, OpdKind::Stack, OpdKind::Reg) => exec::select_f32_rsr,
        (ValType::F32, OpdKind::Stack, OpdKind::Reg, OpdKind::Reg) => exec::select_f32_srr,
        (ValType::F32, OpdKind::Imm, OpdKind::Reg, OpdKind::Reg) => exec::select_f32_irr,
        (ValType::F32, OpdKind::Reg, OpdKind::Imm, OpdKind::Reg) => exec::select_f32_rir,

        (ValType::F64, OpdKind::Stack, OpdKind::Stack, OpdKind::Stack) => exec::select_f64_sss,
        (ValType::F64, OpdKind::Reg, OpdKind::Stack, OpdKind::Stack) => exec::select_f64_rss,
        (ValType::F64, OpdKind::Imm, OpdKind::Stack, OpdKind::Stack) => exec::select_f64_iss,
        (ValType::F64, OpdKind::Stack, OpdKind::Reg, OpdKind::Stack) => exec::select_f64_srs,
        (ValType::F64, OpdKind::Imm, OpdKind::Reg, OpdKind::Stack) => exec::select_f64_irs,
        (ValType::F64, OpdKind::Stack, OpdKind::Imm, OpdKind::Stack) => exec::select_f64_sis,
        (ValType::F64, OpdKind::Reg, OpdKind::Imm, OpdKind::Stack) => exec::select_f64_ris,
        (ValType::F64, OpdKind::Imm, OpdKind::Imm, OpdKind::Stack) => exec::select_f64_iis,
        (ValType::F64, OpdKind::Stack, OpdKind::Stack, OpdKind::Reg) => exec::select_f64_ssr,
        (ValType::F64, OpdKind::Imm, OpdKind::Stack, OpdKind::Reg) => exec::select_f64_isr,
        (ValType::F64, OpdKind::Stack, OpdKind::Imm, OpdKind::Reg) => exec::select_f64_sir,
        (ValType::F64, OpdKind::Imm, OpdKind::Imm, OpdKind::Reg) => exec::select_f64_iir,
        (ValType::F64, OpdKind::Reg, OpdKind::Stack, OpdKind::Reg) => exec::select_f64_rsr,
        (ValType::F64, OpdKind::Stack, OpdKind::Reg, OpdKind::Reg) => exec::select_f64_srr,
        (ValType::F64, OpdKind::Imm, OpdKind::Reg, OpdKind::Reg) => exec::select_f64_irr,
        (ValType::F64, OpdKind::Reg, OpdKind::Imm, OpdKind::Reg) => exec::select_f64_rir,

        (ValType::FuncRef, OpdKind::Stack, OpdKind::Stack, OpdKind::Stack) => {
            exec::select_func_ref_sss
        }
        (ValType::FuncRef, OpdKind::Reg, OpdKind::Stack, OpdKind::Stack) => {
            exec::select_func_ref_rss
        }
        (ValType::FuncRef, OpdKind::Imm, OpdKind::Stack, OpdKind::Stack) => {
            exec::select_func_ref_iss
        }
        (ValType::FuncRef, OpdKind::Stack, OpdKind::Reg, OpdKind::Stack) => {
            exec::select_func_ref_srs
        }
        (ValType::FuncRef, OpdKind::Imm, OpdKind::Reg, OpdKind::Stack) => exec::select_func_ref_irs,
        (ValType::FuncRef, OpdKind::Stack, OpdKind::Imm, OpdKind::Stack) => {
            exec::select_func_ref_sis
        }
        (ValType::FuncRef, OpdKind::Reg, OpdKind::Imm, OpdKind::Stack) => exec::select_func_ref_ris,
        (ValType::FuncRef, OpdKind::Imm, OpdKind::Imm, OpdKind::Stack) => exec::select_func_ref_iis,
        (ValType::FuncRef, OpdKind::Stack, OpdKind::Stack, OpdKind::Reg) => {
            exec::select_func_ref_ssr
        }
        (ValType::FuncRef, OpdKind::Imm, OpdKind::Stack, OpdKind::Reg) => exec::select_func_ref_isr,
        (ValType::FuncRef, OpdKind::Stack, OpdKind::Imm, OpdKind::Reg) => exec::select_func_ref_sir,
        (ValType::FuncRef, OpdKind::Imm, OpdKind::Imm, OpdKind::Reg) => exec::select_func_ref_iir,
        (ValType::FuncRef, OpdKind::Reg, _, OpdKind::Reg) => exec::unreachable,
        (ValType::FuncRef, _, OpdKind::Reg, OpdKind::Reg) => exec::unreachable,

        (ValType::ExternRef, OpdKind::Stack, OpdKind::Stack, OpdKind::Stack) => {
            exec::select_extern_ref_sss
        }
        (ValType::ExternRef, OpdKind::Reg, OpdKind::Stack, OpdKind::Stack) => {
            exec::select_extern_ref_rss
        }
        (ValType::ExternRef, OpdKind::Imm, OpdKind::Stack, OpdKind::Stack) => {
            exec::select_extern_ref_iss
        }
        (ValType::ExternRef, OpdKind::Stack, OpdKind::Reg, OpdKind::Stack) => {
            exec::select_extern_ref_srs
        }
        (ValType::ExternRef, OpdKind::Imm, OpdKind::Reg, OpdKind::Stack) => {
            exec::select_extern_ref_irs
        }
        (ValType::ExternRef, OpdKind::Stack, OpdKind::Imm, OpdKind::Stack) => {
            exec::select_extern_ref_sis
        }
        (ValType::ExternRef, OpdKind::Reg, OpdKind::Imm, OpdKind::Stack) => {
            exec::select_extern_ref_ris
        }
        (ValType::ExternRef, OpdKind::Imm, OpdKind::Imm, OpdKind::Stack) => {
            exec::select_extern_ref_iis
        }
        (ValType::ExternRef, OpdKind::Stack, OpdKind::Stack, OpdKind::Reg) => {
            exec::select_extern_ref_ssr
        }
        (ValType::ExternRef, OpdKind::Imm, OpdKind::Stack, OpdKind::Reg) => {
            exec::select_extern_ref_isr
        }
        (ValType::ExternRef, OpdKind::Stack, OpdKind::Imm, OpdKind::Reg) => {
            exec::select_extern_ref_sir
        }
        (ValType::ExternRef, OpdKind::Imm, OpdKind::Imm, OpdKind::Reg) => {
            exec::select_extern_ref_iir
        }
        (ValType::ExternRef, OpdKind::Reg, _, OpdKind::Reg) => exec::unreachable,
        (ValType::ExternRef, _, OpdKind::Reg, OpdKind::Reg) => exec::unreachable,

        (_, OpdKind::Reg, OpdKind::Reg, _) => exec::unreachable,
        (_, _, _, OpdKind::Imm) => exec::unreachable,
    }
}

fn global_get(type_: ValType) -> ThreadedInstr {
    match type_ {
        ValType::I32 => exec::global_get_i32,
        ValType::I64 => exec::global_get_i64,
        ValType::F32 => exec::global_get_f32,
        ValType::F64 => exec::global_get_f64,
        ValType::FuncRef => exec::global_get_func_ref,
        ValType::ExternRef => exec::global_get_extern_ref,
    }
}

fn global_set(type_: ValType, kind: OpdKind) -> ThreadedInstr {
    match (type_, kind) {
        (ValType::I32, OpdKind::Stack) => exec::global_set_i32_s,
        (ValType::I32, OpdKind::Reg) => exec::global_set_i32_r,
        (ValType::I32, OpdKind::Imm) => exec::global_set_i32_i,
        (ValType::I64, OpdKind::Stack) => exec::global_set_i64_s,
        (ValType::I64, OpdKind::Reg) => exec::global_set_i64_r,
        (ValType::I64, OpdKind::Imm) => exec::global_set_i64_i,
        (ValType::F32, OpdKind::Stack) => exec::global_set_f32_s,
        (ValType::F32, OpdKind::Reg) => exec::global_set_f32_r,
        (ValType::F32, OpdKind::Imm) => exec::global_set_f32_i,
        (ValType::F64, OpdKind::Stack) => exec::global_set_f64_s,
        (ValType::F64, OpdKind::Reg) => exec::global_set_f64_r,
        (ValType::F64, OpdKind::Imm) => exec::global_set_f64_i,
        (ValType::FuncRef, OpdKind::Stack) => exec::global_set_func_ref_s,
        (ValType::FuncRef, OpdKind::Reg) => exec::global_set_func_ref_r,
        (ValType::FuncRef, OpdKind::Imm) => exec::global_set_func_ref_i,
        (ValType::ExternRef, OpdKind::Stack) => exec::global_set_extern_ref_s,
        (ValType::ExternRef, OpdKind::Reg) => exec::global_set_extern_ref_r,
        (ValType::ExternRef, OpdKind::Imm) => exec::global_set_extern_ref_i,
    }
}

fn table_get(type_: RefType, kind: OpdKind) -> ThreadedInstr {
    match (type_, kind) {
        (RefType::FuncRef, OpdKind::Stack) => exec::table_get_func_ref_s,
        (RefType::FuncRef, OpdKind::Reg) => exec::table_get_func_ref_r,
        (RefType::FuncRef, OpdKind::Imm) => exec::table_get_func_ref_i,

        (RefType::ExternRef, OpdKind::Stack) => exec::table_get_extern_ref_s,
        (RefType::ExternRef, OpdKind::Reg) => exec::table_get_extern_ref_r,
        (RefType::ExternRef, OpdKind::Imm) => exec::table_get_extern_ref_i,
    }
}

fn table_set(type_: RefType, kind_0: OpdKind, kind_1: OpdKind) -> ThreadedInstr {
    match (type_, kind_0, kind_1) {
        (RefType::FuncRef, OpdKind::Stack, OpdKind::Stack) => exec::table_set_func_ref_ss,
        (RefType::FuncRef, OpdKind::Reg, OpdKind::Stack) => exec::table_set_func_ref_rs,
        (RefType::FuncRef, OpdKind::Imm, OpdKind::Stack) => exec::table_set_func_ref_is,
        (RefType::FuncRef, OpdKind::Imm, OpdKind::Reg) => exec::table_set_func_ref_ir,
        (RefType::FuncRef, OpdKind::Imm, OpdKind::Imm) => exec::table_set_func_ref_ii,
        (RefType::FuncRef, OpdKind::Stack, OpdKind::Reg) => exec::table_set_func_ref_sr,
        (RefType::FuncRef, OpdKind::Stack, OpdKind::Imm) => exec::table_set_func_ref_si,
        (RefType::FuncRef, OpdKind::Reg, OpdKind::Imm) => exec::table_set_func_ref_ri,
        (RefType::FuncRef, OpdKind::Reg, OpdKind::Reg) => exec::unreachable,

        (RefType::ExternRef, OpdKind::Stack, OpdKind::Stack) => exec::table_set_extern_ref_ss,
        (RefType::ExternRef, OpdKind::Reg, OpdKind::Stack) => exec::table_set_extern_ref_rs,
        (RefType::ExternRef, OpdKind::Imm, OpdKind::Stack) => exec::table_set_extern_ref_is,
        (RefType::ExternRef, OpdKind::Imm, OpdKind::Reg) => exec::table_set_extern_ref_ir,
        (RefType::ExternRef, OpdKind::Imm, OpdKind::Imm) => exec::table_set_extern_ref_ii,
        (RefType::ExternRef, OpdKind::Stack, OpdKind::Reg) => exec::table_set_extern_ref_sr,
        (RefType::ExternRef, OpdKind::Stack, OpdKind::Imm) => exec::table_set_extern_ref_si,
        (RefType::ExternRef, OpdKind::Reg, OpdKind::Imm) => exec::table_set_extern_ref_ri,
        (RefType::ExternRef, OpdKind::Reg, OpdKind::Reg) => exec::unreachable,
    }
}

fn table_size(type_: RefType) -> ThreadedInstr {
    match type_ {
        RefType::FuncRef => exec::table_size_func_ref,
        RefType::ExternRef => exec::table_size_extern_ref,
    }
}

fn table_grow(type_: RefType) -> ThreadedInstr {
    match type_ {
        RefType::FuncRef => exec::table_grow_func_ref,
        RefType::ExternRef => exec::table_grow_extern_ref,
    }
}

fn table_fill(type_: RefType) -> ThreadedInstr {
    match type_ {
        RefType::FuncRef => exec::table_fill_func_ref,
        RefType::ExternRef => exec::table_fill_extern_ref,
    }
}

fn table_copy(type_: RefType) -> ThreadedInstr {
    match type_ {
        RefType::FuncRef => exec::table_copy_func_ref,
        RefType::ExternRef => exec::table_copy_extern_ref,
    }
}

fn table_init(type_: RefType) -> ThreadedInstr {
    match type_ {
        RefType::FuncRef => exec::table_init_func_ref,
        RefType::ExternRef => exec::table_init_extern_ref,
    }
}

fn elem_drop(type_: RefType) -> ThreadedInstr {
    match type_ {
        RefType::FuncRef => exec::elem_drop_func_ref,
        RefType::ExternRef => exec::elem_drop_extern_ref,
    }
}

fn copy_imm_to_stack(type_: ValType) -> ThreadedInstr {
    match type_ {
        ValType::I32 => exec::copy_imm_to_stack_i32,
        ValType::I64 => exec::copy_imm_to_stack_i64,
        ValType::F32 => exec::copy_imm_to_stack_f32,
        ValType::F64 => exec::copy_imm_to_stack_f64,
        ValType::FuncRef => exec::copy_imm_to_stack_func_ref,
        ValType::ExternRef => exec::copy_imm_to_stack_extern_ref,
    }
}

fn copy_stack(type_: ValType) -> ThreadedInstr {
    match type_.into() {
        ValType::I32 => exec::copy_stack_i32,
        ValType::I64 => exec::copy_stack_i64,
        ValType::F32 => exec::copy_stack_f32,
        ValType::F64 => exec::copy_stack_f64,
        ValType::FuncRef => exec::copy_stack_func_ref,
        ValType::ExternRef => exec::copy_stack_extern_ref,
    }
}

fn copy_reg_to_stack(type_: ValType) -> ThreadedInstr {
    match type_ {
        ValType::I32 => exec::copy_reg_to_stack_i32,
        ValType::I64 => exec::copy_reg_to_stack_i64,
        ValType::F32 => exec::copy_reg_to_stack_f32,
        ValType::F64 => exec::copy_reg_to_stack_f64,
        ValType::FuncRef => exec::copy_reg_to_stack_func_ref,
        ValType::ExternRef => exec::copy_reg_to_stack_extern_ref,
    }
}
