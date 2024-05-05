use {
    crate::{
        code,
        code::{
            BinOpInfo, BlockType, InstrVisitor, LoadInfo, MemArg, StoreInfo, UnOpInfo,
            UncompiledCode,
        },
        decode::DecodeError,
        func::FuncType,
        global::Mut,
        module::ModuleBuilder,
        ref_::RefType,
        val::ValType,
    },
    std::{mem, ops::Deref},
};

#[derive(Clone, Debug)]
pub(crate) struct Validator {
    label_idxs: Vec<u32>,
    locals: Vec<ValType>,
    blocks: Vec<Block>,
    opds: Vec<OpdType>,
    aux_opds: Vec<OpdType>,
}

impl Validator {
    pub(crate) fn new() -> Validator {
        Validator {
            label_idxs: Vec::new(),
            locals: Vec::new(),
            blocks: Vec::new(),
            opds: Vec::new(),
            aux_opds: Vec::new(),
        }
    }

    pub(crate) fn validate(
        &mut self,
        type_: &FuncType,
        module: &ModuleBuilder,
        code: &UncompiledCode,
    ) -> Result<(), DecodeError> {
        use crate::decode::Decoder;

        self.label_idxs.clear();
        self.locals.clear();
        self.blocks.clear();
        self.opds.clear();
        let mut validation = Validation {
            module,
            locals: &mut self.locals,
            blocks: &mut self.blocks,
            opds: &mut self.opds,
            aux_opds: &mut self.aux_opds,
        };
        validation.locals.extend(type_.params().iter().copied());
        validation.locals.extend(code.locals.iter().copied());
        validation.push_block(
            BlockKind::Block,
            FuncType::new([], type_.results().iter().copied()),
        );
        let mut decoder = Decoder::new(&code.expr);
        while !validation.blocks.is_empty() {
            code::decode_instr(&mut decoder, &mut self.label_idxs, &mut validation)?;
        }
        Ok(())
    }
}

impl Default for Validator {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct Validation<'a> {
    module: &'a ModuleBuilder,
    locals: &'a mut Vec<ValType>,
    blocks: &'a mut Vec<Block>,
    opds: &'a mut Vec<OpdType>,
    aux_opds: &'a mut Vec<OpdType>,
}

impl<'a> Validation<'a> {
    fn resolve_block_type(&self, type_: BlockType) -> Result<FuncType, DecodeError> {
        match type_ {
            BlockType::TypeIdx(idx) => self.module.type_(idx).cloned(),
            BlockType::ValType(val_type) => Ok(FuncType::from_val_type(val_type)),
        }
    }

    fn local(&self, idx: u32) -> Result<ValType, DecodeError> {
        self.locals
            .get(idx as usize)
            .copied()
            .ok_or_else(|| DecodeError::new("unknown local"))
    }

    fn label(&self, idx: u32) -> Result<(), DecodeError> {
        let idx = usize::try_from(idx).unwrap();
        if idx >= self.blocks.len() {
            return Err(DecodeError::new("unknown label"));
        }
        Ok(())
    }

    fn block(&self, idx: u32) -> &Block {
        let idx = usize::try_from(idx).unwrap();
        &self.blocks[self.blocks.len() - 1 - idx]
    }

    fn push_block(&mut self, kind: BlockKind, type_: FuncType) {
        self.blocks.push(Block {
            kind,
            type_,
            is_unreachable: false,
            height: self.opds.len(),
        });
        for start_type in self.block(0).type_.clone().params().iter().copied() {
            self.push_opd(start_type);
        }
    }

    fn pop_block(&mut self) -> Result<Block, DecodeError> {
        for end_type in self.block(0).type_.clone().results().iter().rev().copied() {
            self.pop_opd()?.check(end_type)?;
        }
        if self.opds.len() != self.block(0).height {
            return Err(DecodeError::new("type mismatch"));
        }
        Ok(self.blocks.pop().unwrap())
    }

    fn set_unreachable(&mut self) {
        self.opds.truncate(self.block(0).height);
        self.blocks.last_mut().unwrap().is_unreachable = true;
    }

    fn push_opd(&mut self, type_: impl Into<OpdType>) {
        let type_ = type_.into();
        self.opds.push(type_);
    }

    fn pop_opd(&mut self) -> Result<OpdType, DecodeError> {
        if self.opds.len() == self.block(0).height {
            if !self.block(0).is_unreachable {
                return Err(DecodeError::new("type mismatch"));
            }
            Ok(OpdType::Unknown)
        } else {
            Ok(self.opds.pop().unwrap())
        }
    }
}

impl<'a> InstrVisitor for Validation<'a> {
    type Error = DecodeError;

    // Control instructions
    fn visit_nop(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn visit_unreachable(&mut self) -> Result<(), Self::Error> {
        self.set_unreachable();
        Ok(())
    }

    fn visit_block(&mut self, type_: BlockType) -> Result<(), Self::Error> {
        let type_ = self.resolve_block_type(type_)?;
        for start_type in type_.params().iter().rev().copied() {
            self.pop_opd()?.check(start_type)?;
        }
        self.push_block(BlockKind::Block, type_);
        Ok(())
    }

    fn visit_loop(&mut self, type_: BlockType) -> Result<(), Self::Error> {
        let type_ = self.resolve_block_type(type_)?;
        for start_type in type_.params().iter().rev().copied() {
            self.pop_opd()?.check(start_type)?;
        }
        self.push_block(BlockKind::Loop, type_);
        Ok(())
    }

    fn visit_if(&mut self, type_: BlockType) -> Result<(), Self::Error> {
        let type_ = self.resolve_block_type(type_)?;
        self.pop_opd()?.check(ValType::I32)?;
        for start_type in type_.params().iter().rev().copied() {
            self.pop_opd()?.check(start_type)?;
        }
        self.push_block(BlockKind::If, type_);
        Ok(())
    }

    fn visit_else(&mut self) -> Result<(), Self::Error> {
        let block = self.pop_block()?;
        if block.kind != BlockKind::If {
            return Err(DecodeError::new("unexpected else opcode"));
        }
        self.push_block(BlockKind::Else, block.type_);
        Ok(())
    }

    fn visit_end(&mut self) -> Result<(), Self::Error> {
        let block = self.pop_block()?;
        let block = if block.kind == BlockKind::If {
            self.push_block(BlockKind::Else, block.type_);
            self.pop_block()?
        } else {
            block
        };
        for end_type in block.type_.results().iter().copied() {
            self.push_opd(end_type);
        }
        Ok(())
    }

    fn visit_br(&mut self, label_idx: u32) -> Result<(), Self::Error> {
        self.label(label_idx)?;
        for label_type in self.block(label_idx).label_types().iter().rev().copied() {
            self.pop_opd()?.check(label_type)?;
        }
        self.set_unreachable();
        Ok(())
    }

    fn visit_br_if(&mut self, label_idx: u32) -> Result<(), Self::Error> {
        self.pop_opd()?.check(ValType::I32)?;
        self.label(label_idx)?;
        for &label_type in self.block(label_idx).label_types().iter().rev() {
            self.pop_opd()?.check(label_type)?;
        }
        for &label_type in self.block(label_idx).label_types().iter() {
            self.push_opd(label_type);
        }
        Ok(())
    }

    fn visit_br_table(
        &mut self,
        label_idxs: &[u32],
        default_label_idx: u32,
    ) -> Result<(), Self::Error> {
        self.pop_opd()?.check(ValType::I32)?;
        self.label(default_label_idx)?;
        let arity = self.block(default_label_idx).label_types().len();
        for label_idx in label_idxs.iter().copied() {
            self.label(label_idx)?;
            if self.block(label_idx).label_types().len() != arity {
                return Err(DecodeError::new("arity mismatch"));
            }
            let mut aux_opds = mem::take(self.aux_opds);
            for label_type in self.block(label_idx).label_types().iter().rev().copied() {
                let opd = self.pop_opd()?;
                opd.check(label_type)?;
                aux_opds.push(opd);
            }
            while let Some(opd) = aux_opds.pop() {
                self.push_opd(opd);
            }
            *self.aux_opds = aux_opds;
        }
        for label_type in self
            .block(default_label_idx)
            .label_types()
            .iter()
            .rev()
            .copied()
        {
            self.pop_opd()?.check(label_type)?;
        }
        self.set_unreachable();
        Ok(())
    }

    fn visit_return(&mut self) -> Result<(), Self::Error> {
        self.visit_br(self.blocks.len() as u32 - 1)
    }

    fn visit_call(&mut self, func_idx: u32) -> Result<(), Self::Error> {
        let type_ = self.module.func(func_idx)?;
        for param_type in type_.params().iter().rev().copied() {
            self.pop_opd()?.check(param_type)?;
        }
        for result_type in type_.results().iter().copied() {
            self.push_opd(result_type);
        }
        Ok(())
    }

    fn visit_call_indirect(&mut self, table_idx: u32, type_idx: u32) -> Result<(), Self::Error> {
        let table_type = self.module.table(table_idx)?;
        if table_type.elem != RefType::FuncRef {
            return Err(DecodeError::new("type mismatch"));
        }
        let type_ = self.module.type_(type_idx)?;
        self.pop_opd()?.check(ValType::I32)?;
        for param_type in type_.params().iter().rev().copied() {
            self.pop_opd()?.check(param_type)?;
        }
        for result_type in type_.results().iter().copied() {
            self.push_opd(result_type);
        }
        Ok(())
    }

    // Reference instructions
    fn visit_ref_null(&mut self, type_: RefType) -> Result<(), Self::Error> {
        self.push_opd(type_);
        Ok(())
    }

    fn visit_ref_is_null(&mut self) -> Result<(), Self::Error> {
        if !self.pop_opd()?.is_ref() {
            return Err(DecodeError::new("type mismatch"));
        };
        self.push_opd(ValType::I32);
        Ok(())
    }

    fn visit_ref_func(&mut self, func_idx: u32) -> Result<(), Self::Error> {
        self.module.ref_(func_idx)?;
        self.push_opd(ValType::FuncRef);
        Ok(())
    }

    // Parametric instructions
    fn visit_drop(&mut self) -> Result<(), Self::Error> {
        self.pop_opd()?;
        Ok(())
    }

    fn visit_select(&mut self, type_: Option<ValType>) -> Result<(), Self::Error> {
        if let Some(type_) = type_ {
            self.pop_opd()?.check(ValType::I32)?;
            self.pop_opd()?.check(type_)?;
            self.pop_opd()?.check(type_)?;
            self.push_opd(type_);
        } else {
            self.pop_opd()?.check(ValType::I32)?;
            let input_type_1 = self.pop_opd()?;
            let input_type_0 = self.pop_opd()?;
            if !(input_type_0.is_num() && input_type_1.is_num()) {
                return Err(DecodeError::new("type mismatch"));
            }
            if let OpdType::ValType(input_type_1) = input_type_1 {
                input_type_0.check(input_type_1)?;
            }
            self.push_opd(if input_type_0.is_unknown() {
                input_type_1
            } else {
                input_type_0
            });
        }
        Ok(())
    }

    // Variable instructions
    fn visit_local_get(&mut self, local_idx: u32) -> Result<(), Self::Error> {
        let type_ = self.local(local_idx)?;
        self.push_opd(type_);
        Ok(())
    }

    fn visit_local_set(&mut self, local_idx: u32) -> Result<(), Self::Error> {
        let type_ = self.local(local_idx)?;
        self.pop_opd()?.check(type_)?;
        Ok(())
    }

    fn visit_local_tee(&mut self, local_idx: u32) -> Result<(), Self::Error> {
        let type_ = self.local(local_idx)?;
        self.pop_opd()?.check(type_)?;
        self.push_opd(type_);
        Ok(())
    }

    fn visit_global_get(&mut self, global_idx: u32) -> Result<(), Self::Error> {
        let type_ = self.module.global(global_idx)?;
        self.push_opd(type_.val);
        Ok(())
    }

    fn visit_global_set(&mut self, global_idx: u32) -> Result<(), Self::Error> {
        let type_ = self.module.global(global_idx)?;
        if type_.mut_ != Mut::Var {
            return Err(DecodeError::new("type mismatch"));
        }
        self.pop_opd()?.check(type_.val)?;
        Ok(())
    }

    // Table instructions
    fn visit_table_get(&mut self, table_idx: u32) -> Result<(), Self::Error> {
        let type_ = self.module.table(table_idx)?;
        self.pop_opd()?.check(ValType::I32)?;
        self.push_opd(type_.elem);
        Ok(())
    }

    fn visit_table_set(&mut self, table_idx: u32) -> Result<(), Self::Error> {
        let type_ = self.module.table(table_idx)?;
        self.pop_opd()?.check(type_.elem)?;
        self.pop_opd()?.check(ValType::I32)?;
        Ok(())
    }

    fn visit_table_size(&mut self, table_idx: u32) -> Result<(), Self::Error> {
        self.module.table(table_idx)?;
        self.push_opd(ValType::I32);
        Ok(())
    }

    fn visit_table_grow(&mut self, table_idx: u32) -> Result<(), Self::Error> {
        let type_ = self.module.table(table_idx)?;
        self.pop_opd()?.check(ValType::I32)?;
        self.pop_opd()?.check(type_.elem)?;
        self.push_opd(ValType::I32);
        Ok(())
    }

    fn visit_table_fill(&mut self, table_idx: u32) -> Result<(), Self::Error> {
        let type_ = self.module.table(table_idx)?;
        self.pop_opd()?.check(ValType::I32)?;
        self.pop_opd()?.check(type_.elem)?;
        self.pop_opd()?.check(ValType::I32)?;
        Ok(())
    }

    fn visit_table_copy(
        &mut self,
        dst_table_idx: u32,
        src_table_idx: u32,
    ) -> Result<(), Self::Error> {
        let dst_type = self.module.table(dst_table_idx)?;
        let src_type = self.module.table(src_table_idx)?;
        if dst_type.elem != src_type.elem {
            return Err(DecodeError::new("type mismatch"));
        }
        self.pop_opd()?.check(ValType::I32)?;
        self.pop_opd()?.check(ValType::I32)?;
        self.pop_opd()?.check(ValType::I32)?;
        Ok(())
    }

    fn visit_table_init(&mut self, table_idx: u32, elem_idx: u32) -> Result<(), Self::Error> {
        let dst_type = self.module.table(table_idx)?;
        let src_type = self.module.elem(elem_idx)?;
        if dst_type.elem != src_type {
            return Err(DecodeError::new("type mismatch"));
        }
        self.pop_opd()?.check(ValType::I32)?;
        self.pop_opd()?.check(ValType::I32)?;
        self.pop_opd()?.check(ValType::I32)?;
        Ok(())
    }

    fn visit_elem_drop(&mut self, elem_idx: u32) -> Result<(), Self::Error> {
        self.module.elem(elem_idx)?;
        Ok(())
    }

    // Memory instructions
    fn visit_load(&mut self, arg: MemArg, info: LoadInfo) -> Result<(), Self::Error> {
        if arg.align > info.max_align {
            return Err(DecodeError::new("alignment too large"));
        }
        self.module.memory(0)?;
        self.visit_un_op(info.op)
    }

    fn visit_store(&mut self, arg: MemArg, info: StoreInfo) -> Result<(), Self::Error> {
        if arg.align > info.max_align {
            return Err(DecodeError::new("alignment too large"));
        }
        self.module.memory(0)?;
        self.visit_bin_op(info.op)
    }

    fn visit_memory_size(&mut self) -> Result<(), Self::Error> {
        self.module.memory(0)?;
        self.push_opd(ValType::I32);
        Ok(())
    }

    fn visit_memory_grow(&mut self) -> Result<(), Self::Error> {
        self.module.memory(0)?;
        self.pop_opd()?.check(ValType::I32)?;
        self.push_opd(ValType::I32);
        Ok(())
    }

    fn visit_memory_fill(&mut self) -> Result<(), Self::Error> {
        self.module.memory(0)?;
        self.pop_opd()?.check(ValType::I32)?;
        self.pop_opd()?.check(ValType::I32)?;
        self.pop_opd()?.check(ValType::I32)?;
        Ok(())
    }

    fn visit_memory_copy(&mut self) -> Result<(), Self::Error> {
        self.module.memory(0)?;
        self.pop_opd()?.check(ValType::I32)?;
        self.pop_opd()?.check(ValType::I32)?;
        self.pop_opd()?.check(ValType::I32)?;
        Ok(())
    }

    fn visit_memory_init(&mut self, data_idx: u32) -> Result<(), Self::Error> {
        self.module.memory(0)?;
        self.module.data(data_idx)?;
        self.pop_opd()?.check(ValType::I32)?;
        self.pop_opd()?.check(ValType::I32)?;
        self.pop_opd()?.check(ValType::I32)?;
        Ok(())
    }

    fn visit_data_drop(&mut self, data_idx: u32) -> Result<(), Self::Error> {
        self.module.data(data_idx)?;
        Ok(())
    }

    // Numeric instructions
    fn visit_i32_const(&mut self, _val: i32) -> Result<(), Self::Error> {
        self.push_opd(ValType::I32);
        Ok(())
    }

    fn visit_i64_const(&mut self, _val: i64) -> Result<(), Self::Error> {
        self.push_opd(ValType::I64);
        Ok(())
    }

    fn visit_f32_const(&mut self, _val: f32) -> Result<(), Self::Error> {
        self.push_opd(ValType::F32);
        Ok(())
    }

    fn visit_f64_const(&mut self, _val: f64) -> Result<(), Self::Error> {
        self.push_opd(ValType::F64);
        Ok(())
    }

    fn visit_un_op(&mut self, info: UnOpInfo) -> Result<(), Self::Error> {
        self.pop_opd()?.check(info.input_type)?;
        if let Some(output_type) = info.output_type {
            self.push_opd(output_type);
        }
        Ok(())
    }

    fn visit_bin_op(&mut self, info: BinOpInfo) -> Result<(), Self::Error> {
        self.pop_opd()?.check(info.input_type_1)?;
        self.pop_opd()?.check(info.input_type_0)?;
        if let Some(output_type) = info.output_type {
            self.push_opd(output_type);
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct Block {
    kind: BlockKind,
    type_: FuncType,
    is_unreachable: bool,
    height: usize,
}

impl Block {
    fn label_types(&self) -> LabelTypes {
        LabelTypes {
            kind: self.kind,
            type_: self.type_.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BlockKind {
    Block,
    Loop,
    If,
    Else,
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
            BlockKind::Block | BlockKind::If | BlockKind::Else => self.type_.results(),
            BlockKind::Loop => self.type_.params(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum OpdType {
    ValType(ValType),
    Unknown,
}

impl OpdType {
    fn is_num(self) -> bool {
        match self {
            OpdType::ValType(type_) => type_.is_num(),
            _ => true,
        }
    }

    fn is_ref(self) -> bool {
        match self {
            OpdType::ValType(type_) => type_.is_ref(),
            _ => true,
        }
    }

    fn is_unknown(self) -> bool {
        match self {
            OpdType::Unknown => true,
            _ => false,
        }
    }

    fn check(self, expected_type: impl Into<ValType>) -> Result<(), DecodeError> {
        let expected_type = expected_type.into();
        match self {
            OpdType::ValType(actual_type) if actual_type != expected_type => {
                Err(DecodeError::new("type mismatch"))
            }
            _ => Ok(()),
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

#[derive(Debug)]
struct Unknown;
