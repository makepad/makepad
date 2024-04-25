use {
    crate::{
        aliased_box::AliasableBox,
        code,
        code::{BinOpInfo, BlockType, InstrVisitor, LoadInfo, MemArg, StoreInfo, UnOpInfo},
        decode::DecodeError,
        exec,
        exec::Instr,
        extern_ref::ExternRef,
        func::{CompiledCode, Func, FuncEntity, FuncType, InstrSlot, UncompiledCode},
        func_ref::FuncRef,
        instance::Instance,
        ref_::RefType,
        stack::StackSlot,
        store::Store,
        val::ValType,
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
        compile.emit(exec::enter as Instr);
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
        compile.emit(exec::return_ as Instr);
        compile.opds.clear();
        let max_stack_slot_count = compile.max_stack_height;
        let mut code: AliasableBox<[InstrSlot]> = AliasableBox::from_box(Box::from(compile.code));
        for fixup_idx in compile.fixup_idxs.drain(..) {
            code[fixup_idx] += code.as_ptr() as usize;
        }
        self.locals.clear();
        self.opds.clear();
        CompiledCode {
            max_stack_slot_count,
            local_count,
            code,
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
    code: Vec<InstrSlot>,
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

    /// Locals

    /// Returns the type of the local with the given index.
    fn local_type(&self, local_idx: usize) -> ValType {
        self.locals[local_idx as usize].type_
    }

    fn push_local_opd(&mut self, local_idx: usize) {
        let opd_idx = self.opds.len() - 1;
        self.opds[opd_idx].local_idx = Some(local_idx);
        self.opds[opd_idx].next_opd_idx = self.locals[local_idx].first_opd_idx;
        self.locals[local_idx].first_opd_idx = Some(opd_idx);
    }

    fn pop_local_opd(&mut self, local_idx: usize) -> Option<usize> {
        if let Some(opd_idx) = self.locals[local_idx].first_opd_idx {
            self.locals[local_idx].first_opd_idx = self.opds[opd_idx].next_opd_idx;
            self.opds[opd_idx].local_idx = None;
            self.opds[opd_idx].next_opd_idx = None;
            Some(opd_idx)
        } else {
            None
        }
    }

    fn save_local(&mut self, local_idx: usize) {
        while let Some(opd_idx) = self.pop_local_opd(local_idx) {
            self.emit(copy_stack(self.local_type(local_idx)));
            self.emit_stack_offset(self.local_stack_idx(local_idx));
            self.emit_stack_offset(self.temp_stack_idx(opd_idx));
        }
    }

    fn save_all_locals(&mut self) {
        for local_idx in 0..self.locals.len() {
            self.save_local(local_idx);
        }
    }

    /// Blocks

    /// Returns a reference to the block at the given depth.
    fn block(&self, depth: usize) -> &Block {
        &self.blocks[self.blocks.len() - 1 - depth]
    }

    /// Returns a mutable reference to the block at the given depth.
    fn block_mut(&mut self, depth: usize) -> &mut Block {
        let len = self.blocks.len();
        &mut self.blocks[len - 1 - depth]
    }

    /// Marks the current block as unreachable.
    fn set_unreachable(&mut self) {
        while self.opds.len() > self.block(0).first_opd_idx {
            self.pop_opd();
        }
    }

    fn push_hole(&mut self, block_idx: usize, hole_idx: usize) {
        self.code[hole_idx] = self.block(block_idx).first_hole_idx.unwrap_or(usize::MAX);
        self.block_mut(block_idx).first_hole_idx = Some(hole_idx);
    }

    fn pop_hole(&mut self, block_idx: usize) -> Option<usize> {
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
            first_opd_idx: self.opds.len(),
            first_instr_idx: self.code.len(),
            first_hole_idx: None,
            else_hole_idx: None,
        });
        for input_type in self.block(0).type_.clone().params().iter().copied() {
            self.push_opd(input_type);
        }
    }

    /// Pops a block from the stack.
    fn pop_block(&mut self) -> Block {
        for _ in 0..self.block(0).type_.results().len() {
            debug_assert!(!self.is_opd_in_reg(0));
            self.pop_opd();
        }
        self.blocks.pop().unwrap()
    }

    // Operands

    /// Returns `true` if the operand at the given depth is stored in a register.
    fn is_opd_in_reg(&self, opt_depth: usize) -> bool {
        self.opd_reg_idx(opt_depth)
            .map_or(false, |reg_idx| self.is_reg_used_by_opd(reg_idx, opt_depth))
    }

    /// Returns the type of the operand at the given depth.
    fn opd_type(&self, opd_idx: usize) -> OpdType {
        if opd_idx >= self.opds.len() - self.block(0).first_opd_idx {
            OpdType::Unknown
        } else {
            self.opds[self.opds.len() - 1 - opd_idx].type_
        }
    }

    /// Returns the stack index of the operand at the given depth.
    fn opd_stack_idx(&self, opd_depth: usize) -> Option<isize> {
        if opd_depth >= self.opds.len() - self.block(0).first_opd_idx {
            None
        } else {
            let opd_idx = self.opds.len() - 1 - opd_depth;
            if let Some(local_idx) = self.opds[opd_idx].local_idx {
                self.local_stack_idx(local_idx)
            } else {
                self.temp_stack_idx(self.opds.len() - 1 - opd_depth)
            }
        }
    }

    /// Returns the register index of the operand at the given depth.
    fn opd_reg_idx(&self, opd_idx: usize) -> Option<usize> {
        self.opd_type(opd_idx).reg_idx()
    }

    /// Pushes an operand of the given type on the stack.
    fn push_opd(&mut self, type_: impl Into<OpdType>) {
        self.opds.push(Opd {
            type_: type_.into(),
            local_idx: None,
            next_opd_idx: None,
        });
        let stack_slot_count = self.first_temp_stack_idx as usize + (self.opds.len() - 1);
        self.max_stack_height = self.max_stack_height.max(stack_slot_count);
    }

    fn push_opd_and_emit_stack_offset(&mut self, type_: impl Into<OpdType>) {
        self.push_opd(type_);
        self.emit_stack_offset(self.opd_stack_idx(0));
    }

    fn push_opd_and_alloc_reg(&mut self, type_: impl Into<OpdType>) {
        self.push_opd(type_);
        self.alloc_reg();
    }

    /// Pops an operand from the stack.
    fn pop_opd(&mut self) -> OpdType {
        if self.is_opd_in_reg(0) {
            self.dealloc_reg(self.opd_reg_idx(0).unwrap());
        }
        if self.opds.len() == self.block(0).first_opd_idx {
            OpdType::Unknown
        } else {
            if let Some(local_idx) = self.opds[self.opds.len() - 1].local_idx {
                self.pop_local_opd(local_idx);
            }
            self.opds.pop().unwrap().type_
        }
    }

    fn pop_opd_and_emit_stack_offset(&mut self) {
        if !self.is_opd_in_reg(0) {
            self.emit_stack_offset(self.opd_stack_idx(0));
        }
        self.pop_opd();
    }

    /// Stack

    /// Returns the stack index of the parameter/result with the given index.
    fn param_result_stack_idx(&self, param_result_idx: usize) -> Option<isize> {
        Some(self.first_param_result_stack_idx + param_result_idx as isize)
    }

    /// Returns the stack index of the local with the given index.
    fn local_stack_idx(&self, local_idx: usize) -> Option<isize> {
        if local_idx < self.type_.params().len() {
            self.param_result_stack_idx(local_idx)
        } else {
            Some((local_idx - self.type_.params().len()) as isize)
        }
    }

    fn temp_stack_idx(&self, temp_idx: usize) -> Option<isize> {
        Some((self.first_temp_stack_idx + temp_idx) as isize)
    }

    /// Registers

    /// Returns `true` if the register with the given index is used.
    fn is_reg_used(&self, reg_idx: usize) -> bool {
        self.regs[reg_idx].is_some()
    }

    /// Returns `true` if the register with the given index is used by the operand at the given
    /// depth.
    fn is_reg_used_by_opd(&self, reg_idx: usize, opd_depth: usize) -> bool {
        self.regs[reg_idx] == Some(self.opds.len() - 1 - opd_depth)
    }

    /// Allocates a register for the top operand.
    fn alloc_reg(&mut self) {
        if let Some(reg_idx) = self.opd_reg_idx(0) {
            debug_assert!(!self.is_reg_used(reg_idx));
            self.regs[reg_idx] = Some(self.opds.len() - 1);
        }
    }

    /// Deallocates the register with the given index.
    fn dealloc_reg(&mut self, reg_idx: usize) {
        debug_assert!(self.regs[reg_idx].is_some());
        self.regs[reg_idx] = None;
    }

    fn save_reg(&mut self, reg_idx: usize) {
        let opd_idx = self.regs[reg_idx].unwrap();
        self.emit(copy_reg_to_stack(
            self.opds[opd_idx].type_.to_val().unwrap(),
        ));
        self.emit_stack_offset(self.temp_stack_idx(opd_idx));
        self.dealloc_reg(reg_idx);
    }

    fn save_all_regs(&mut self) {
        for reg_idx in 0..self.regs.len() {
            if self.is_reg_used(reg_idx) {
                self.save_reg(reg_idx);
            }
        }
    }

    fn save_all_regs_except_top(&mut self) {
        for reg_idx in 0..self.regs.len() {
            if let Some(opd_idx) = self.regs[reg_idx] {
                if opd_idx == self.opds.len() - 1 {
                    continue;
                }
                self.save_reg(reg_idx);
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
            debug_assert!(!self.is_opd_in_reg(0));
            self.pop_opd();
            self.emit_stack_offset(
                self.temp_stack_idx(self.block(label_idx).first_opd_idx + label_val_idx),
            );
        }
    }

    /// Emitting

    fn emit<T>(&mut self, val: T)
    where
        T: Copy,
    {
        debug_assert!(mem::size_of::<T>() <= mem::size_of::<InstrSlot>());
        self.code.push(InstrSlot::default());
        unsafe { *(self.code.last_mut().unwrap() as *mut _ as *mut T) = val };
    }

    fn emit_stack_offset(&mut self, stack_idx: Option<isize>) {
        self.emit(stack_idx.map_or(isize::MIN, |stack_idx| {
            stack_idx * mem::size_of::<StackSlot>() as isize
        }));
    }

    fn emit_label(&mut self, label_idx: usize) {
        match self.block(label_idx).kind {
            BlockKind::Block => {
                let hole_idx = self.emit_hole();
                self.push_hole(label_idx, hole_idx);
            }
            BlockKind::Loop => {
                self.emit_instr_offset(self.block(label_idx).first_instr_idx);
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

    fn emit_instr_offset(&mut self, instr_idx: usize) {
        self.fixup_idxs.push(self.code.len());
        self.emit(instr_idx * mem::size_of::<InstrSlot>());
    }
}

impl<'a> InstrVisitor for Compile<'a> {
    type Error = DecodeError;

    // Control instructions
    fn visit_nop(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_unreachable(&mut self) -> Result<(), Self::Error> {
        self.emit(exec::unreachable as Instr);
        self.set_unreachable();
        Ok(())
    }

    fn visit_block(&mut self, type_: BlockType) -> Result<(), Self::Error> {
        let type_ = self.resolve_block_type(type_);
        self.save_all_locals();
        self.save_all_regs();
        for _ in 0..type_.params().len() {
            self.pop_opd();
        }
        self.push_block(BlockKind::Block, type_);
        Ok(())
    }

    fn visit_loop(&mut self, type_: BlockType) -> Result<(), Self::Error> {
        let type_ = self.resolve_block_type(type_);
        self.save_all_locals();
        self.save_all_regs();
        for _ in 0..type_.params().len() {
            self.pop_opd();
        }
        self.push_block(BlockKind::Loop, type_);
        Ok(())
    }

    fn visit_if(&mut self, type_: BlockType) -> Result<(), Self::Error> {
        let type_ = self.resolve_block_type(type_);
        self.save_all_locals();
        self.save_all_regs_except_top();
        self.emit(br_if_z(self.is_opd_in_reg(0)));
        self.pop_opd_and_emit_stack_offset();
        let else_hole_idx = self.emit_hole();
        for _ in 0..type_.params().len() {
            self.pop_opd();
        }
        self.push_block(BlockKind::Block, type_);
        self.block_mut(0).else_hole_idx = Some(else_hole_idx);
        Ok(())
    }

    fn visit_else(&mut self) -> Result<(), Self::Error> {
        self.save_all_locals();
        self.save_all_regs();
        self.emit(exec::br as Instr);
        let hole_idx = self.emit_hole();
        self.push_hole(0, hole_idx);
        let else_hole_idx = self.block_mut(0).else_hole_idx.take().unwrap();
        self.patch_hole(else_hole_idx);
        let block = self.pop_block();
        self.push_block(BlockKind::Block, block.type_);
        self.block_mut(0).first_hole_idx = block.first_hole_idx;
        Ok(())
    }

    fn visit_end(&mut self) -> Result<(), Self::Error> {
        self.save_all_locals();
        self.save_all_regs();
        if let Some(else_hole_idx) = self.block_mut(0).else_hole_idx.take() {
            self.patch_hole(else_hole_idx);
        }
        while let Some(hole_idx) = self.pop_hole(0) {
            self.patch_hole(hole_idx);
        }
        let block = self.pop_block();
        for result_type in block.type_.results().iter().copied() {
            self.push_opd(result_type);
        }
        Ok(())
    }

    fn visit_br(&mut self, label_idx: u32) -> Result<(), Self::Error> {
        let label_idx = label_idx as usize;
        self.save_all_locals();
        self.save_all_regs();
        self.resolve_label_vals(label_idx);
        self.emit(exec::br as Instr);
        self.emit_label(label_idx);
        self.set_unreachable();
        Ok(())
    }

    fn visit_br_if(&mut self, label_idx: u32) -> Result<(), Self::Error> {
        let label_idx = label_idx as usize;
        self.save_all_locals();
        self.save_all_regs_except_top();
        if self.block(label_idx).label_types().is_empty() {
            self.emit(br_if_nz(self.is_opd_in_reg(0)));
            self.pop_opd_and_emit_stack_offset();
            self.emit_label(label_idx);
        } else {
            self.emit(br_if_z(self.is_opd_in_reg(0)));
            self.pop_opd_and_emit_stack_offset();
            let hole_idx = self.emit_hole();
            self.resolve_label_vals(label_idx);
            self.emit(exec::br as Instr);
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
        self.save_all_locals();
        self.save_all_regs_except_top();
        self.emit(br_table(self.is_opd_in_reg(0)));
        self.pop_opd_and_emit_stack_offset();
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
            self.emit(exec::br as Instr);
            self.emit_label(label_idx);
            for label_type in self.block(0).label_types().iter().copied() {
                self.push_opd(label_type);
            }
        }
        let default_label_idx = default_label_idx as usize;
        self.patch_hole(default_hole_idx);
        self.resolve_label_vals(default_label_idx);
        self.emit(exec::br as Instr);
        self.emit_label(default_label_idx);
        self.set_unreachable();
        Ok(())
    }

    fn visit_return(&mut self) -> Result<(), Self::Error> {
        for (result_idx, result_type) in self
            .type_
            .clone()
            .results()
            .iter()
            .copied()
            .enumerate()
            .rev()
        {
            self.emit(if self.is_opd_in_reg(0) {
                copy_reg_to_stack(result_type)
            } else {
                copy_stack(result_type)
            });
            self.pop_opd_and_emit_stack_offset();
            self.emit_stack_offset(self.param_result_stack_idx(result_idx));
        }
        self.emit(exec::return_ as Instr);
        self.set_unreachable();
        Ok(())
    }

    fn visit_call(&mut self, func_idx: u32) -> Result<(), Self::Error> {
        let func = self.instance.func(func_idx).unwrap();
        let type_ = func.type_(&self.store).clone();
        self.save_all_locals();
        self.save_all_regs();
        self.emit(match func.0.as_ref(&self.store) {
            FuncEntity::Wasm(_) => exec::compile as Instr,
            FuncEntity::Host(_) => exec::call_host as Instr,
        });
        for _ in 0..type_.params().len() {
            debug_assert!(!self.is_opd_in_reg(0));
            self.pop_opd();
        }
        self.emit(func.0.to_unguarded(self.store.id()));
        let first_callee_stack_idx = self.first_temp_stack_idx + self.opds.len();
        let last_callee_stack_idx = first_callee_stack_idx + type_.callee_stack_slot_count();
        self.max_stack_height = self.max_stack_height.max(last_callee_stack_idx);
        self.emit_stack_offset(Some(last_callee_stack_idx as isize));
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
        let table = self.instance.table(table_idx).unwrap();
        let interned_type = self.instance.type_(type_idx).unwrap();
        let type_ = self.store.resolve_type(interned_type).clone();
        self.save_all_locals();
        self.save_all_regs();
        self.emit(exec::call_indirect as Instr);
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
        self.emit_stack_offset(Some(last_callee_stack_idx as isize));
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
        self.emit(ref_null(type_));
        match type_ {
            RefType::FuncRef => self.emit(FuncRef::null().to_unguarded(self.store.id())),
            RefType::ExternRef => self.emit(ExternRef::null().to_unguarded(self.store.id())),
        };
        self.push_opd_and_emit_stack_offset(type_);
        Ok(())
    }

    fn visit_ref_is_null(&mut self) -> Result<(), DecodeError> {
        self.emit(ref_is_null(self.opd_type(0), self.is_opd_in_reg(0)));
        self.pop_opd_and_emit_stack_offset();
        self.push_opd_and_alloc_reg(ValType::I32);
        Ok(())
    }

    fn visit_ref_func(&mut self, func_idx: u32) -> Result<(), DecodeError> {
        self.emit(exec::copy_imm_to_stack_func_ref as Instr);
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
        self.pop_opd();
        Ok(())
    }

    fn visit_select(&mut self, type_: Option<ValType>) -> Result<(), DecodeError> {
        let type_ = type_.map_or_else(|| self.opd_type(1), |type_| OpdType::ValType(type_));
        if let Some(output_reg_idx) = type_.reg_idx() {
            if self.is_reg_used(output_reg_idx) {
                self.save_reg(output_reg_idx);
            }
        }
        self.emit(select(
            type_,
            self.is_opd_in_reg(2),
            self.is_opd_in_reg(1),
            self.is_opd_in_reg(0),
        ));
        self.pop_opd_and_emit_stack_offset();
        self.pop_opd_and_emit_stack_offset();
        self.pop_opd_and_emit_stack_offset();
        self.push_opd_and_alloc_reg(type_);
        Ok(())
    }

    // Variable instructions
    fn visit_local_get(&mut self, local_idx: u32) -> Result<(), DecodeError> {
        let local_idx = local_idx as usize;
        let local_type = self.local_type(local_idx);
        self.push_opd(local_type);
        self.push_local_opd(local_idx);
        Ok(())
    }

    fn visit_local_set(&mut self, local_idx: u32) -> Result<(), DecodeError> {
        let local_idx = local_idx as usize;
        let local_type = self.local_type(local_idx);
        self.save_local(local_idx);
        self.emit(if self.is_opd_in_reg(0) {
            copy_reg_to_stack(local_type)
        } else {
            copy_stack(local_type)
        });
        self.pop_opd_and_emit_stack_offset();
        self.emit_stack_offset(self.local_stack_idx(local_idx));
        Ok(())
    }

    fn visit_local_tee(&mut self, local_idx: u32) -> Result<(), DecodeError> {
        let local_idx = local_idx as usize;
        self.save_local(local_idx);
        self.save_all_regs();
        self.emit(copy_stack(self.local_type(local_idx)));
        self.emit_stack_offset(self.opd_stack_idx(0));
        self.emit_stack_offset(self.local_stack_idx(local_idx));
        Ok(())
    }

    fn visit_global_get(&mut self, global_idx: u32) -> Result<(), DecodeError> {
        let global = self.instance.global(global_idx).unwrap();
        let val_type = global.type_(&self.store).val;
        self.emit(global_get(val_type));
        self.emit(global.to_unguarded(self.store.id()));
        self.push_opd(val_type);
        self.emit_stack_offset(self.opd_stack_idx(0));
        Ok(())
    }

    fn visit_global_set(&mut self, global_idx: u32) -> Result<(), DecodeError> {
        let global = self.instance.global(global_idx).unwrap();
        let val_type = global.type_(&self.store).val;
        self.emit(global_set(val_type, self.is_opd_in_reg(0)));
        self.pop_opd_and_emit_stack_offset();
        self.emit(global.to_unguarded(self.store.id()));
        Ok(())
    }

    // Table instructions
    fn visit_table_get(&mut self, table_idx: u32) -> Result<(), Self::Error> {
        let table = self.instance.table(table_idx).unwrap();
        let elem_type = table.type_(&self.store).elem;
        self.emit(table_get(elem_type, self.is_opd_in_reg(0)));
        self.pop_opd_and_emit_stack_offset();
        self.emit(table.to_unguarded(self.store.id()));
        self.push_opd_and_emit_stack_offset(elem_type);
        Ok(())
    }

    fn visit_table_set(&mut self, table_idx: u32) -> Result<(), Self::Error> {
        let table = self.instance.table(table_idx).unwrap();
        let elem_type = table.type_(&self.store).elem;
        self.emit(table_set(
            elem_type,
            self.is_opd_in_reg(1),
            self.is_opd_in_reg(0),
        ));
        self.emit_stack_offset(self.opd_stack_idx(1));
        self.emit_stack_offset(self.opd_stack_idx(0));
        self.pop_opd();
        self.pop_opd();
        self.emit(table.to_unguarded(self.store.id()));
        Ok(())
    }

    fn visit_table_size(&mut self, table_idx: u32) -> Result<(), Self::Error> {
        let table = self.instance.table(table_idx).unwrap();
        let elem_type = table.type_(&self.store).elem;
        self.emit(table_size(elem_type));
        self.emit(table.to_unguarded(self.store.id()));
        self.push_opd_and_emit_stack_offset(ValType::I32);
        Ok(())
    }

    fn visit_table_grow(&mut self, table_idx: u32) -> Result<(), Self::Error> {
        let table = self.instance.table(table_idx).unwrap();
        let elem_type = table.type_(&self.store).elem;
        self.save_all_regs();
        self.emit(table_grow(elem_type));
        self.pop_opd_and_emit_stack_offset();
        self.pop_opd_and_emit_stack_offset();
        self.emit(table.to_unguarded(self.store.id()));
        self.push_opd_and_emit_stack_offset(ValType::I32);
        Ok(())
    }

    fn visit_table_fill(&mut self, table_idx: u32) -> Result<(), Self::Error> {
        let table = self.instance.table(table_idx).unwrap();
        let elem_type = table.type_(&self.store).elem;
        self.save_all_regs();
        self.emit(table_fill(elem_type));
        self.pop_opd_and_emit_stack_offset();
        self.pop_opd_and_emit_stack_offset();
        self.pop_opd_and_emit_stack_offset();
        self.emit(table.to_unguarded(self.store.id()));
        Ok(())
    }

    fn visit_table_copy(
        &mut self,
        dst_table_idx: u32,
        src_table_idx: u32,
    ) -> Result<(), Self::Error> {
        let dst_table = self.instance.table(dst_table_idx).unwrap();
        let src_table = self.instance.table(src_table_idx).unwrap();
        let elem_type = dst_table.type_(&self.store).elem;
        self.save_all_regs();
        self.emit(table_copy(elem_type));
        self.pop_opd_and_emit_stack_offset();
        self.pop_opd_and_emit_stack_offset();
        self.pop_opd_and_emit_stack_offset();
        self.emit(dst_table.to_unguarded(self.store.id()));
        self.emit(src_table.to_unguarded(self.store.id()));
        Ok(())
    }

    fn visit_table_init(
        &mut self,
        dst_table_idx: u32,
        src_elem_idx: u32,
    ) -> Result<(), Self::Error> {
        let dst_table = self.instance.table(dst_table_idx).unwrap();
        let src_elem = self.instance.elem(src_elem_idx).unwrap();
        let elem_type = dst_table.type_(&self.store).elem;
        self.save_all_regs();
        self.emit(table_init(elem_type));
        self.pop_opd_and_emit_stack_offset();
        self.pop_opd_and_emit_stack_offset();
        self.pop_opd_and_emit_stack_offset();
        self.emit(dst_table.0.to_unguarded(self.store.id()));
        self.emit(src_elem.0.to_unguarded(self.store.id()));
        Ok(())
    }

    fn visit_elem_drop(&mut self, elem_idx: u32) -> Result<(), Self::Error> {
        let elem = self.instance.elem(elem_idx).unwrap();
        let elem_type = elem.type_(&self.store);
        self.emit(elem_drop(elem_type));
        self.emit(elem.to_unguarded(self.store.id()));
        Ok(())
    }

    // Memory instructions
    fn visit_load(&mut self, arg: MemArg, info: LoadInfo) -> Result<(), DecodeError> {
        self.visit_un_op(info.op)?;
        self.emit(arg.offset);
        Ok(())
    }

    fn visit_store(&mut self, arg: MemArg, info: StoreInfo) -> Result<(), DecodeError> {
        self.visit_bin_op(info.op)?;
        self.emit(arg.offset);
        Ok(())
    }

    fn visit_memory_size(&mut self) -> Result<(), Self::Error> {
        let mem = self.instance.mem(0).unwrap();
        self.emit(exec::memory_size as Instr);
        self.emit(mem.to_unguarded(self.store.id()));
        self.push_opd_and_emit_stack_offset(ValType::I32);
        Ok(())
    }

    fn visit_memory_grow(&mut self) -> Result<(), Self::Error> {
        let mem = self.instance.mem(0).unwrap();
        self.save_all_regs();
        self.emit(exec::memory_grow as Instr);
        self.pop_opd_and_emit_stack_offset();
        self.emit(mem.to_unguarded(self.store.id()));
        self.push_opd_and_emit_stack_offset(ValType::I32);
        Ok(())
    }

    fn visit_memory_fill(&mut self) -> Result<(), Self::Error> {
        let mem = self.instance.mem(0).unwrap();
        self.save_all_regs();
        self.emit(exec::memory_fill as Instr);
        self.pop_opd_and_emit_stack_offset();
        self.pop_opd_and_emit_stack_offset();
        self.pop_opd_and_emit_stack_offset();
        self.emit(mem.to_unguarded(self.store.id()));
        Ok(())
    }

    fn visit_memory_copy(&mut self) -> Result<(), Self::Error> {
        let mem = self.instance.mem(0).unwrap();
        self.save_all_regs();
        self.emit(exec::memory_copy as Instr);
        self.pop_opd_and_emit_stack_offset();
        self.pop_opd_and_emit_stack_offset();
        self.pop_opd_and_emit_stack_offset();
        self.emit(mem.to_unguarded(self.store.id()));
        Ok(())
    }

    fn visit_memory_init(&mut self, data_idx: u32) -> Result<(), Self::Error> {
        let dst_mem = self.instance.mem(0).unwrap();
        let src_data = self.instance.data(data_idx).unwrap();
        self.save_all_regs();
        self.emit(exec::memory_init as Instr);
        self.pop_opd_and_emit_stack_offset();
        self.pop_opd_and_emit_stack_offset();
        self.pop_opd_and_emit_stack_offset();
        self.emit(dst_mem.to_unguarded(self.store.id()));
        self.emit(src_data.to_unguarded(self.store.id()));
        Ok(())
    }

    fn visit_data_drop(&mut self, data_idx: u32) -> Result<(), Self::Error> {
        let data = self.instance.data(data_idx).unwrap();
        self.emit(exec::data_drop as Instr);
        self.emit(data.to_unguarded(self.store.id()));
        Ok(())
    }

    // Numeric instructions
    fn visit_i32_const(&mut self, val: i32) -> Result<(), DecodeError> {
        self.emit(exec::copy_imm_to_stack_i32 as Instr);
        self.emit(val);
        self.push_opd_and_emit_stack_offset(ValType::I32);
        Ok(())
    }

    fn visit_i64_const(&mut self, val: i64) -> Result<(), DecodeError> {
        self.emit(exec::copy_imm_to_stack_i64 as Instr);
        self.emit(val);
        self.push_opd_and_emit_stack_offset(ValType::I64);
        Ok(())
    }

    fn visit_f32_const(&mut self, val: f32) -> Result<(), DecodeError> {
        self.emit(exec::copy_imm_to_stack_f32 as Instr);
        self.emit(val);
        self.push_opd_and_emit_stack_offset(ValType::F32);
        Ok(())
    }

    fn visit_f64_const(&mut self, val: f64) -> Result<(), DecodeError> {
        self.emit(exec::copy_imm_to_stack_f64 as Instr);
        self.emit(val);
        self.push_opd_and_emit_stack_offset(ValType::F64);
        Ok(())
    }

    fn visit_un_op(&mut self, info: UnOpInfo) -> Result<(), DecodeError> {
        // If this operation has an output, and the output register is used, then we need to save
        // the output register, unless it is also used as an input register. Otherwise, the
        // operation will overwrite the output register while it's already used.
        if let Some(output_type) = info.output_type {
            let output_reg_idx = output_type.reg_idx();
            if self.is_reg_used(output_reg_idx) && !self.is_reg_used_by_opd(output_reg_idx, 0) {
                self.save_reg(output_reg_idx);
            }
        }
        self.emit(if self.is_opd_in_reg(0) {
            info.instr_r as Instr
        } else {
            info.instr_s as Instr
        });
        self.pop_opd_and_emit_stack_offset();
        if let Some(output_type) = info.output_type {
            self.push_opd_and_alloc_reg(output_type);
        }
        Ok(())
    }

    fn visit_bin_op(&mut self, info: BinOpInfo) -> Result<(), DecodeError> {
        // If this operation has an output, and the output register is used, then we need to save
        // the output register, unless it is also used as an input register. Otherwise, the
        // operation will overwrite the output register while it's already used.
        if let Some(output_type) = info.output_type {
            let output_reg_idx = output_type.reg_idx();
            if self.is_reg_used(output_reg_idx)
                && !self.is_reg_used_by_opd(output_reg_idx, 1)
                && !self.is_reg_used_by_opd(output_reg_idx, 0)
            {
                self.save_reg(output_reg_idx);
            }
        }
        self.emit(if self.is_opd_in_reg(1) {
            if self.is_opd_in_reg(0) {
                info.instr_rr as Instr
            } else {
                info.instr_rs as Instr
            }
        } else if self.is_opd_in_reg(0) {
            info.instr_sr as Instr
        } else {
            info.instr_ss as Instr
        });
        self.pop_opd_and_emit_stack_offset();
        self.pop_opd_and_emit_stack_offset();
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
    first_opd_idx: usize,
    first_instr_idx: usize,
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
    type_: OpdType,
    local_idx: Option<usize>,
    next_opd_idx: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
enum OpdType {
    ValType(ValType),
    Unknown,
}

impl OpdType {
    fn reg_idx(self) -> Option<usize> {
        match self {
            OpdType::ValType(val) => Some(val.reg_idx()),
            OpdType::Unknown => None,
        }
    }

    fn to_val(self) -> Option<ValType> {
        match self {
            OpdType::ValType(type_) => Some(type_),
            OpdType::Unknown => None,
        }
    }
}

impl From<RefType> for OpdType {
    fn from(type_: RefType) -> Self {
        OpdType::ValType(type_.into())
    }
}

impl From<ValType> for OpdType {
    fn from(type_: ValType) -> Self {
        OpdType::ValType(type_)
    }
}

impl From<Unknown> for OpdType {
    fn from(_: Unknown) -> Self {
        OpdType::Unknown
    }
}

#[derive(Clone, Copy, Debug)]
struct Unknown;

fn br_if_z(is_input_in_reg: bool) -> Instr {
    if is_input_in_reg {
        exec::br_if_z_r
    } else {
        exec::br_if_z_s
    }
}

fn br_if_nz(is_input_in_reg: bool) -> Instr {
    if is_input_in_reg {
        exec::br_if_nz_r
    } else {
        exec::br_if_nz_s
    }
}

fn br_table(is_input_in_reg: bool) -> Instr {
    if is_input_in_reg {
        exec::br_table_r
    } else {
        exec::br_table_s
    }
}

fn ref_null(type_: RefType) -> Instr {
    match type_ {
        RefType::FuncRef => exec::copy_imm_to_stack_func_ref,
        RefType::ExternRef => exec::copy_imm_to_stack_extern_ref,
    }
}

fn ref_is_null(type_: OpdType, is_input_in_reg: bool) -> Instr {
    match type_ {
        OpdType::ValType(ValType::FuncRef) => {
            if is_input_in_reg {
                exec::ref_is_null_func_ref_r
            } else {
                exec::ref_is_null_func_ref_s
            }
        }
        OpdType::ValType(ValType::ExternRef) => {
            if is_input_in_reg {
                exec::ref_is_null_extern_ref_r
            } else {
                exec::ref_is_null_extern_ref_s
            }
        }
        _ => exec::unreachable,
    }
}

fn select(
    type_: OpdType,
    is_input_0_in_reg: bool,
    is_input_1_in_reg: bool,
    is_input_2_in_reg: bool,
) -> Instr {
    match type_ {
        OpdType::ValType(ValType::I32) => {
            if is_input_2_in_reg {
                if is_input_1_in_reg || is_input_0_in_reg {
                    exec::unreachable
                } else {
                    exec::select_i32_ssr
                }
            } else if is_input_1_in_reg {
                if is_input_0_in_reg {
                    exec::unreachable
                } else {
                    exec::select_i32_srs
                }
            } else if is_input_0_in_reg {
                exec::select_i32_rss
            } else {
                exec::select_i32_sss
            }
        }
        OpdType::ValType(ValType::I64) => {
            if is_input_2_in_reg {
                if is_input_1_in_reg || is_input_0_in_reg {
                    exec::unreachable
                } else {
                    exec::select_i64_ssr
                }
            } else if is_input_1_in_reg {
                if is_input_0_in_reg {
                    exec::unreachable
                } else {
                    exec::select_i64_srs
                }
            } else if is_input_0_in_reg {
                exec::select_i64_rss
            } else {
                exec::select_i64_sss
            }
        }
        OpdType::ValType(ValType::F32) => {
            if is_input_2_in_reg {
                if is_input_1_in_reg {
                    if is_input_0_in_reg {
                        exec::unreachable
                    } else {
                        exec::select_f32_srr
                    }
                } else if is_input_0_in_reg {
                    exec::select_f32_rsr
                } else {
                    exec::select_f32_ssr
                }
            } else if is_input_1_in_reg {
                if is_input_0_in_reg {
                    exec::unreachable
                } else {
                    exec::select_f32_srs
                }
            } else if is_input_0_in_reg {
                exec::select_f32_rss
            } else {
                exec::select_f32_sss
            }
        }
        OpdType::ValType(ValType::F64) => {
            if is_input_2_in_reg {
                if is_input_1_in_reg {
                    if is_input_0_in_reg {
                        exec::unreachable
                    } else {
                        exec::select_f64_srr
                    }
                } else if is_input_0_in_reg {
                    exec::select_f64_rsr
                } else {
                    exec::select_f64_ssr
                }
            } else if is_input_1_in_reg {
                if is_input_0_in_reg {
                    exec::unreachable
                } else {
                    exec::select_f64_srs
                }
            } else if is_input_0_in_reg {
                exec::select_f64_rss
            } else {
                exec::select_f64_sss
            }
        }
        OpdType::ValType(ValType::FuncRef) => {
            if is_input_2_in_reg {
                if is_input_1_in_reg || is_input_0_in_reg {
                    exec::unreachable
                } else {
                    exec::select_func_ref_ssr
                }
            } else if is_input_1_in_reg {
                if is_input_0_in_reg {
                    exec::unreachable
                } else {
                    exec::select_func_ref_srs
                }
            } else if is_input_0_in_reg {
                exec::select_func_ref_rss
            } else {
                exec::select_func_ref_sss
            }
        }
        OpdType::ValType(ValType::ExternRef) => {
            if is_input_2_in_reg {
                if is_input_1_in_reg || is_input_0_in_reg {
                    exec::unreachable
                } else {
                    exec::select_extern_ref_ssr
                }
            } else if is_input_1_in_reg {
                if is_input_0_in_reg {
                    exec::unreachable
                } else {
                    exec::select_extern_ref_srs
                }
            } else if is_input_0_in_reg {
                exec::select_extern_ref_rss
            } else {
                exec::select_extern_ref_sss
            }
        }
        OpdType::Unknown => exec::unreachable,
    }
}

fn global_get(type_: ValType) -> Instr {
    match type_ {
        ValType::I32 => exec::global_get_i32,
        ValType::I64 => exec::global_get_i64,
        ValType::F32 => exec::global_get_f32,
        ValType::F64 => exec::global_get_f64,
        ValType::FuncRef => exec::global_get_raw_func_ref,
        ValType::ExternRef => exec::global_get_raw_extern_ref,
    }
}

fn global_set(type_: ValType, is_input_in_reg: bool) -> Instr {
    match type_ {
        ValType::I32 => {
            if is_input_in_reg {
                exec::global_set_i32_r
            } else {
                exec::global_set_i32_s
            }
        }
        ValType::I64 => {
            if is_input_in_reg {
                exec::global_set_i64_r
            } else {
                exec::global_set_i64_s
            }
        }
        ValType::F32 => {
            if is_input_in_reg {
                exec::global_set_f32_r
            } else {
                exec::global_set_f32_s
            }
        }
        ValType::F64 => {
            if is_input_in_reg {
                exec::global_set_f64_r
            } else {
                exec::global_set_f64_s
            }
        }
        ValType::FuncRef => {
            if is_input_in_reg {
                exec::global_set_func_ref_r
            } else {
                exec::global_set_func_ref_s
            }
        }
        ValType::ExternRef => {
            if is_input_in_reg {
                exec::global_set_extern_ref_r
            } else {
                exec::global_set_extern_ref_s
            }
        }
    }
}

fn table_get(type_: RefType, is_input_in_reg: bool) -> Instr {
    match type_ {
        RefType::FuncRef => {
            if is_input_in_reg {
                exec::table_get_func_ref_r
            } else {
                exec::table_get_func_ref_s
            }
        }
        RefType::ExternRef => {
            if is_input_in_reg {
                exec::table_get_extern_ref_r
            } else {
                exec::table_get_extern_ref_s
            }
        }
    }
}

fn table_set(type_: RefType, is_input_0_in_reg: bool, is_input_1_in_reg: bool) -> Instr {
    match type_ {
        RefType::FuncRef => {
            if is_input_1_in_reg {
                exec::table_set_func_ref_sr
            } else if is_input_0_in_reg {
                exec::table_set_func_ref_rs
            } else {
                exec::table_set_func_ref_ss
            }
        }
        RefType::ExternRef => {
            if is_input_1_in_reg {
                exec::table_set_extern_ref_sr
            } else if is_input_0_in_reg {
                exec::table_set_extern_ref_rs
            } else {
                exec::table_set_extern_ref_ss
            }
        }
    }
}

fn table_size(type_: RefType) -> Instr {
    match type_ {
        RefType::FuncRef => exec::table_size_func_ref,
        RefType::ExternRef => exec::table_size_extern_ref,
    }
}

fn table_grow(type_: RefType) -> Instr {
    match type_ {
        RefType::FuncRef => exec::table_grow_func_ref,
        RefType::ExternRef => exec::table_grow_extern_ref,
    }
}

fn table_fill(type_: RefType) -> Instr {
    match type_ {
        RefType::FuncRef => exec::table_fill_func_ref,
        RefType::ExternRef => exec::table_fill_extern_ref,
    }
}

fn table_copy(type_: RefType) -> Instr {
    match type_ {
        RefType::FuncRef => exec::table_copy_func_ref,
        RefType::ExternRef => exec::table_copy_extern_ref,
    }
}

fn table_init(type_: RefType) -> Instr {
    match type_ {
        RefType::FuncRef => exec::table_init_func_ref,
        RefType::ExternRef => exec::table_init_extern_ref,
    }
}

fn elem_drop(type_: RefType) -> Instr {
    match type_ {
        RefType::FuncRef => exec::elem_drop_func_ref,
        RefType::ExternRef => exec::elem_drop_extern_ref,
    }
}

fn copy_stack(type_: ValType) -> Instr {
    match type_.into() {
        ValType::I32 => exec::copy_stack_i32,
        ValType::I64 => exec::copy_stack_i64,
        ValType::F32 => exec::copy_stack_f32,
        ValType::F64 => exec::copy_stack_f64,
        ValType::FuncRef => exec::copy_stack_func_ref,
        ValType::ExternRef => exec::copy_stack_extern_ref,
    }
}

fn copy_reg_to_stack(type_: ValType) -> Instr {
    match type_ {
        ValType::I32 => exec::copy_reg_to_stack_i32,
        ValType::I64 => exec::copy_reg_to_stack_i64,
        ValType::F32 => exec::copy_reg_to_stack_f32,
        ValType::F64 => exec::copy_reg_to_stack_f64,
        ValType::FuncRef => exec::copy_reg_to_stack_func_ref,
        ValType::ExternRef => exec::copy_reg_to_stack_extern_ref,
    }
}
