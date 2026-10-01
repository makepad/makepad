use {
    crate::{
        aliasable_box::AliasableBox,
        binary::{self, Instr},
        decode::DecodeError,
        exec::{self, ThreadedInstr},
        ref_::RefType,
        simd::V128,
        val::ValType,
    },
    std::sync::Arc,
};

#[derive(Debug)]
pub(crate) enum Code {
    Uncompiled(UncompiledCode),
    Compiling,
    Compiled(CompiledCode),
}

#[derive(Clone, Debug)]
pub(crate) struct UncompiledCode {
    pub(crate) locals: Box<[ValType]>,
    /// The decoded body, its final `End` included.
    pub(crate) body: Arc<[Instr]>,
}

#[derive(Debug)]
pub(crate) struct CompiledCode {
    pub(crate) max_stack_height: usize,
    pub(crate) local_count: usize,
    pub(crate) code: AliasableBox<[InstrSlot]>,
}

pub(crate) type InstrSlot = usize;

pub(crate) trait InstrVisitor {
    type Error;

    // Control instructions
    fn visit_nop(&mut self) -> Result<(), Self::Error>;
    fn visit_unreachable(&mut self) -> Result<(), Self::Error>;
    fn visit_block(&mut self, type_: BlockType) -> Result<(), Self::Error>;
    fn visit_loop(&mut self, type_: BlockType) -> Result<(), Self::Error>;
    fn visit_if(&mut self, type_: BlockType) -> Result<(), Self::Error>;
    fn visit_else(&mut self) -> Result<(), Self::Error>;
    fn visit_end(&mut self) -> Result<(), Self::Error>;
    fn visit_br(&mut self, label_idx: u32) -> Result<(), Self::Error>;
    fn visit_br_if(&mut self, label_idx: u32) -> Result<(), Self::Error>;
    fn visit_br_table(
        &mut self,
        label_idxs: &[u32],
        default_label_idx: u32,
    ) -> Result<(), Self::Error>;
    fn visit_return(&mut self) -> Result<(), Self::Error>;
    fn visit_call(&mut self, func_idx: u32) -> Result<(), Self::Error>;
    fn visit_call_indirect(&mut self, table_idx: u32, type_idx: u32) -> Result<(), Self::Error>;

    // Reference instructions
    fn visit_ref_null(&mut self, type_: RefType) -> Result<(), Self::Error>;
    fn visit_ref_is_null(&mut self) -> Result<(), Self::Error>;
    fn visit_ref_func(&mut self, func_idx: u32) -> Result<(), Self::Error>;

    // Parametric instructions
    fn visit_drop(&mut self) -> Result<(), Self::Error>;
    fn visit_select(&mut self, types_: Option<ValType>) -> Result<(), Self::Error>;

    // Variable instructions
    fn visit_local_get(&mut self, local_idx: u32) -> Result<(), Self::Error>;
    fn visit_local_set(&mut self, local_idx: u32) -> Result<(), Self::Error>;
    fn visit_local_tee(&mut self, local_idx: u32) -> Result<(), Self::Error>;
    fn visit_global_get(&mut self, global_idx: u32) -> Result<(), Self::Error>;
    fn visit_global_set(&mut self, global_idx: u32) -> Result<(), Self::Error>;

    // Table instructions
    fn visit_table_get(&mut self, table_idx: u32) -> Result<(), Self::Error>;
    fn visit_table_set(&mut self, table_idx: u32) -> Result<(), Self::Error>;
    fn visit_table_size(&mut self, table_idx: u32) -> Result<(), Self::Error>;
    fn visit_table_grow(&mut self, table_idx: u32) -> Result<(), Self::Error>;
    fn visit_table_fill(&mut self, table_idx: u32) -> Result<(), Self::Error>;
    fn visit_table_copy(
        &mut self,
        dst_table_idx: u32,
        src_table_idx: u32,
    ) -> Result<(), Self::Error>;
    fn visit_table_init(&mut self, table_idx: u32, elem_idx: u32) -> Result<(), Self::Error>;
    fn visit_elem_drop(&mut self, elem_idx: u32) -> Result<(), Self::Error>;

    // Memory instructions
    fn visit_load(&mut self, arg: MemArg, info: LoadInfo) -> Result<(), Self::Error>;
    fn visit_store(&mut self, arg: MemArg, info: StoreInfo) -> Result<(), Self::Error>;
    fn visit_memory_size(&mut self) -> Result<(), Self::Error>;
    fn visit_memory_grow(&mut self) -> Result<(), Self::Error>;
    fn visit_memory_fill(&mut self) -> Result<(), Self::Error>;
    fn visit_memory_copy(&mut self) -> Result<(), Self::Error>;
    fn visit_memory_init(&mut self, data_idx: u32) -> Result<(), Self::Error>;
    fn visit_data_drop(&mut self, data_idx: u32) -> Result<(), Self::Error>;

    // Numeric instructions
    fn visit_i32_const(&mut self, val: i32) -> Result<(), Self::Error>;
    fn visit_i64_const(&mut self, val: i64) -> Result<(), Self::Error>;
    fn visit_f32_const(&mut self, val: f32) -> Result<(), Self::Error>;
    fn visit_f64_const(&mut self, val: f64) -> Result<(), Self::Error>;
    fn visit_un_op(&mut self, info: UnOpInfo) -> Result<(), Self::Error>;
    fn visit_bin_op(&mut self, info: BinOpInfo) -> Result<(), Self::Error>;

    // Vector (v128) instructions.
    //
    // These have their own visitor methods (instead of reusing
    // `visit_un_op`/`visit_bin_op`) because `v128` operands are never
    // register-resident: every `v128` input is read from the stack, and
    // every `v128` output is written to a stack slot whose offset is an
    // explicit immediate in the threaded code.
    fn visit_v128_load(&mut self, arg: MemArg) -> Result<(), Self::Error>;
    fn visit_v128_store(&mut self, arg: MemArg) -> Result<(), Self::Error>;
    fn visit_v128_const(&mut self, val: V128) -> Result<(), Self::Error>;
    fn visit_i8x16_shuffle(&mut self, lanes: [u8; 16]) -> Result<(), Self::Error>;
    fn visit_f32x4_splat(&mut self) -> Result<(), Self::Error>;
    fn visit_f32x4_extract_lane(&mut self, lane: u8) -> Result<(), Self::Error>;
    fn visit_f32x4_replace_lane(&mut self, lane: u8) -> Result<(), Self::Error>;
    fn visit_v128_any_true(&mut self) -> Result<(), Self::Error>;
    fn visit_v128_bitselect(&mut self) -> Result<(), Self::Error>;
    fn visit_v128_un_op(&mut self, info: V128UnOpInfo) -> Result<(), Self::Error>;
    fn visit_v128_bin_op(&mut self, info: V128BinOpInfo) -> Result<(), Self::Error>;
    fn visit_v128_reduce_op(&mut self, info: V128ReduceOpInfo) -> Result<(), Self::Error>;
}

/// Info for a `v128 -> v128` operation. Input and output are always on the
/// stack, so there is only one instruction variant.
#[derive(Clone, Copy, Debug)]
pub(crate) struct V128UnOpInfo {
    pub(crate) _name: &'static str,
    pub(crate) instr: ThreadedInstr,
}

/// Info for a `v128 x v128 -> v128` operation. Inputs and output are always
/// on the stack, so there is only one instruction variant.
#[derive(Clone, Copy, Debug)]
pub(crate) struct V128BinOpInfo {
    pub(crate) _name: &'static str,
    pub(crate) instr: ThreadedInstr,
}

/// Info for a `v128 x v128 -> f32` reduction (e.g. the nonstandard dot
/// products). Inputs are always on the stack; the scalar result goes to
/// the float register.
#[derive(Clone, Copy, Debug)]
pub(crate) struct V128ReduceOpInfo {
    pub(crate) _name: &'static str,
    pub(crate) instr: ThreadedInstr,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum BlockType {
    TypeIdx(u32),
    ValType(Option<ValType>),
}

fn block_type(type_: binary::BlockType) -> BlockType {
    match type_ {
        binary::BlockType::Empty => BlockType::ValType(None),
        binary::BlockType::Value(val_type) => BlockType::ValType(Some(val_type)),
        binary::BlockType::Func(idx) => BlockType::TypeIdx(idx),
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MemArg {
    pub(crate) offset: u32,
}

fn mem_arg(arg: &binary::MemArg) -> MemArg {
    MemArg {
        offset: arg.offset,
    }
}

/// The `0xfd` sub-opcodes without immediates that the engine runs (see
/// `visit_simd_instr`).
const SIMD_OPS: &[u16] = &[
    19, 65, 66, 67, 68, 69, 70, 77, 78, 79, 80, 81, 82, 83, 103, 104, 105, 106, 224, 225, 227,
    228, 229, 230, 231, 232, 233, 234, 235,
];

/// Refuses an instruction the engine cannot run: tail calls, atomics, the
/// SIMD outside the `f32x4` subset, and memory indices other than 0. The
/// module was validated before, so everything else is runnable.
pub(crate) fn check_supported(instr: &Instr) -> Result<(), DecodeError> {
    let ok = match instr {
        Instr::ReturnCall(_)
        | Instr::ReturnCallIndirect { .. }
        | Instr::Atomic(..)
        | Instr::AtomicFence => false,
        Instr::Load(_, arg) | Instr::Store(_, arg) => arg.memory == 0,
        Instr::MemorySize(memory) | Instr::MemoryGrow(memory) | Instr::MemoryFill(memory) => {
            *memory == 0
        }
        Instr::MemoryInit { memory, .. } => *memory == 0,
        Instr::MemoryCopy { dst, src } => *dst == 0 && *src == 0,
        Instr::SimdMem(sub, arg) => matches!(sub, 0x00 | 0x0b) && arg.memory == 0,
        Instr::SimdMemLane(..) => false,
        Instr::SimdLane(sub, _) => matches!(sub, 0x1f | 0x20),
        Instr::Simd(sub) => SIMD_OPS.contains(sub),
        _ => true,
    };
    if !ok {
        return Err(DecodeError::new("illegal opcode"));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LoadInfo {
    pub(crate) op: UnOpInfo,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct StoreInfo {
    pub(crate) op: BinOpInfo,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UnOpInfo {
    pub(crate) _name: &'static str,
    pub(crate) output_type: Option<ValType>,
    pub(crate) instr_s: ThreadedInstr,
    pub(crate) instr_r: ThreadedInstr,
    pub(crate) instr_i: Option<ThreadedInstr>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BinOpInfo {
    pub(crate) _name: &'static str,
    pub(crate) output_type: Option<ValType>,
    pub(crate) instr_ss: ThreadedInstr,
    pub(crate) instr_rs: ThreadedInstr,
    pub(crate) instr_is: ThreadedInstr,
    pub(crate) instr_ir: ThreadedInstr,
    pub(crate) instr_ii: Option<ThreadedInstr>,
    pub(crate) instr_sr: ThreadedInstr,
    pub(crate) instr_si: ThreadedInstr,
    pub(crate) instr_ri: ThreadedInstr,
    pub(crate) instr_rr: Option<ThreadedInstr>,
}

/// Calls the visitor method for one decoded instruction. Instructions the
/// engine does not run (see `supported`) are an error.
pub(crate) fn visit_instr<V>(instr: &Instr, visitor: &mut V) -> Result<(), V::Error>
where
    V: InstrVisitor,
    V::Error: From<DecodeError>,
{
    match instr {
        Instr::Unreachable => visitor.visit_unreachable(),
        Instr::Nop => visitor.visit_nop(),
        Instr::Block(type_) => visitor.visit_block(block_type(*type_)),
        Instr::Loop(type_) => visitor.visit_loop(block_type(*type_)),
        Instr::If(type_) => visitor.visit_if(block_type(*type_)),
        Instr::Else => visitor.visit_else(),
        Instr::End => visitor.visit_end(),
        Instr::Br(label_idx) => visitor.visit_br(*label_idx),
        Instr::BrIf(label_idx) => visitor.visit_br_if(*label_idx),
        Instr::BrTable(label_idxs, default_label_idx) => {
            visitor.visit_br_table(label_idxs, *default_label_idx)
        }
        Instr::Return => visitor.visit_return(),
        Instr::Call(func_idx) => visitor.visit_call(*func_idx),
        Instr::CallIndirect { ty, table } => visitor.visit_call_indirect(*table, *ty),
        Instr::Drop => visitor.visit_drop(),
        Instr::Select => visitor.visit_select(None),
        Instr::SelectT(type_) => visitor.visit_select(Some(*type_)),
        Instr::LocalGet(idx) => visitor.visit_local_get(*idx),
        Instr::LocalSet(idx) => visitor.visit_local_set(*idx),
        Instr::LocalTee(idx) => visitor.visit_local_tee(*idx),
        Instr::GlobalGet(idx) => visitor.visit_global_get(*idx),
        Instr::GlobalSet(idx) => visitor.visit_global_set(*idx),
        Instr::TableGet(idx) => visitor.visit_table_get(*idx),
        Instr::TableSet(idx) => visitor.visit_table_set(*idx),
        Instr::Load(0x28, arg) => visitor.visit_load(
            mem_arg(arg),
            LoadInfo {
                op: UnOpInfo {
                    _name: "i32_load",
                    output_type: Some(ValType::I32),
                    instr_s: exec::i32_load_s,
                    instr_r: exec::i32_load_r,
                    instr_i: Some(exec::i32_load_i),
                },
            },
        ),
        Instr::Load(0x29, arg) => visitor.visit_load(
            mem_arg(arg),
            LoadInfo {
                op: UnOpInfo {
                    _name: "i64_load",
                    output_type: Some(ValType::I64),
                    instr_s: exec::i64_load_s,
                    instr_r: exec::i64_load_r,
                    instr_i: Some(exec::i64_load_i),
                },
            },
        ),
        Instr::Load(0x2a, arg) => visitor.visit_load(
            mem_arg(arg),
            LoadInfo {
                op: UnOpInfo {
                    _name: "f32_load",
                    output_type: Some(ValType::F32),
                    instr_s: exec::f32_load_s,
                    instr_r: exec::f32_load_r,
                    instr_i: Some(exec::f32_load_i),
                },
            },
        ),
        Instr::Load(0x2b, arg) => visitor.visit_load(
            mem_arg(arg),
            LoadInfo {
                op: UnOpInfo {
                    _name: "f64_load",
                    output_type: Some(ValType::F64),
                    instr_s: exec::f64_load_s,
                    instr_r: exec::f64_load_r,
                    instr_i: Some(exec::f64_load_i),
                },
            },
        ),
        Instr::Load(0x2c, arg) => visitor.visit_load(
            mem_arg(arg),
            LoadInfo {
                op: UnOpInfo {
                    _name: "i32_load8_s",
                    output_type: Some(ValType::I32),
                    instr_s: exec::i32_load8_s_s,
                    instr_r: exec::i32_load8_s_r,
                    instr_i: Some(exec::i32_load8_s_i),
                },
            },
        ),
        Instr::Load(0x2d, arg) => visitor.visit_load(
            mem_arg(arg),
            LoadInfo {
                op: UnOpInfo {
                    _name: "i32_load8_u",
                    output_type: Some(ValType::I32),
                    instr_s: exec::i32_load8_u_s,
                    instr_r: exec::i32_load8_u_r,
                    instr_i: Some(exec::i32_load8_u_i),
                },
            },
        ),
        Instr::Load(0x2e, arg) => visitor.visit_load(
            mem_arg(arg),
            LoadInfo {
                op: UnOpInfo {
                    _name: "i32_load16_s",
                    output_type: Some(ValType::I32),
                    instr_s: exec::i32_load16_s_s,
                    instr_r: exec::i32_load16_s_r,
                    instr_i: Some(exec::i32_load16_s_i),
                },
            },
        ),
        Instr::Load(0x2f, arg) => visitor.visit_load(
            mem_arg(arg),
            LoadInfo {
                op: UnOpInfo {
                    _name: "i32_load16_u",
                    output_type: Some(ValType::I32),
                    instr_s: exec::i32_load16_u_s,
                    instr_r: exec::i32_load16_u_r,
                    instr_i: Some(exec::i32_load16_u_i),
                },
            },
        ),
        Instr::Load(0x30, arg) => visitor.visit_load(
            mem_arg(arg),
            LoadInfo {
                op: UnOpInfo {
                    _name: "i64_load8_s",
                    output_type: Some(ValType::I64),
                    instr_s: exec::i64_load8_s_s,
                    instr_r: exec::i64_load8_s_r,
                    instr_i: Some(exec::i64_load8_s_i),
                },
            },
        ),
        Instr::Load(0x31, arg) => visitor.visit_load(
            mem_arg(arg),
            LoadInfo {
                op: UnOpInfo {
                    _name: "i64_load8_u",
                    output_type: Some(ValType::I64),
                    instr_s: exec::i64_load8_u_s,
                    instr_r: exec::i64_load8_u_r,
                    instr_i: Some(exec::i64_load8_u_i),
                },
            },
        ),
        Instr::Load(0x32, arg) => visitor.visit_load(
            mem_arg(arg),
            LoadInfo {
                op: UnOpInfo {
                    _name: "i64_load16_s",
                    output_type: Some(ValType::I64),
                    instr_s: exec::i64_load16_s_s,
                    instr_r: exec::i64_load16_s_r,
                    instr_i: Some(exec::i64_load16_s_i),
                },
            },
        ),
        Instr::Load(0x33, arg) => visitor.visit_load(
            mem_arg(arg),
            LoadInfo {
                op: UnOpInfo {
                    _name: "i64_load16_u",
                    output_type: Some(ValType::I64),
                    instr_s: exec::i64_load16_u_s,
                    instr_r: exec::i64_load16_u_r,
                    instr_i: Some(exec::i64_load16_u_i),
                },
            },
        ),
        Instr::Load(0x34, arg) => visitor.visit_load(
            mem_arg(arg),
            LoadInfo {
                op: UnOpInfo {
                    _name: "i64_load32_s",
                    output_type: Some(ValType::I64),
                    instr_s: exec::i64_load32_s_s,
                    instr_r: exec::i64_load32_s_r,
                    instr_i: Some(exec::i64_load32_s_i),
                },
            },
        ),
        Instr::Load(0x35, arg) => visitor.visit_load(
            mem_arg(arg),
            LoadInfo {
                op: UnOpInfo {
                    _name: "i64_load32_u",
                    output_type: Some(ValType::I64),
                    instr_s: exec::i64_load32_u_s,
                    instr_r: exec::i64_load32_u_r,
                    instr_i: Some(exec::i64_load32_u_i),
                },
            },
        ),
        Instr::Store(0x36, arg) => visitor.visit_store(
            mem_arg(arg),
            StoreInfo {
                op: BinOpInfo {
                    _name: "i32_store",
                    output_type: None,
                    instr_ss: exec::i32_store_ss,
                    instr_rs: exec::i32_store_rs,
                    instr_is: exec::i32_store_is,
                    instr_ir: exec::i32_store_ir,
                    instr_ii: Some(exec::i32_store_ii),
                    instr_sr: exec::i32_store_sr,
                    instr_si: exec::i32_store_si,
                    instr_ri: exec::i32_store_ri,
                    instr_rr: None,
                },
            },
        ),
        Instr::Store(0x37, arg) => visitor.visit_store(
            mem_arg(arg),
            StoreInfo {
                op: BinOpInfo {
                    _name: "i64_store",
                    output_type: None,
                    instr_ss: exec::i64_store_ss,
                    instr_rs: exec::i64_store_rs,
                    instr_is: exec::i64_store_is,
                    instr_ir: exec::i64_store_ir,
                    instr_ii: Some(exec::i64_store_ii),
                    instr_sr: exec::i64_store_sr,
                    instr_si: exec::i64_store_si,
                    instr_ri: exec::i64_store_ri,
                    instr_rr: None,
                },
            },
        ),
        Instr::Store(0x38, arg) => visitor.visit_store(
            mem_arg(arg),
            StoreInfo {
                op: BinOpInfo {
                    _name: "f32_store",
                    output_type: None,
                    instr_ss: exec::f32_store_ss,
                    instr_rs: exec::f32_store_rs,
                    instr_is: exec::f32_store_is,
                    instr_ir: exec::f32_store_ir,
                    instr_ii: Some(exec::f32_store_ii),
                    instr_sr: exec::f32_store_sr,
                    instr_si: exec::f32_store_si,
                    instr_ri: exec::f32_store_ri,
                    instr_rr: Some(exec::f32_store_rr),
                },
            },
        ),
        Instr::Store(0x39, arg) => visitor.visit_store(
            mem_arg(arg),
            StoreInfo {
                op: BinOpInfo {
                    _name: "f64_store",
                    output_type: None,
                    instr_ss: exec::f64_store_ss,
                    instr_rs: exec::f64_store_rs,
                    instr_is: exec::f64_store_is,
                    instr_ir: exec::f64_store_ir,
                    instr_ii: Some(exec::f64_store_ii),
                    instr_sr: exec::f64_store_sr,
                    instr_si: exec::f64_store_si,
                    instr_ri: exec::f64_store_ri,
                    instr_rr: Some(exec::f64_store_rr),
                },
            },
        ),
        Instr::Store(0x3a, arg) => visitor.visit_store(
            mem_arg(arg),
            StoreInfo {
                op: BinOpInfo {
                    _name: "i32_store8",
                    output_type: None,
                    instr_ss: exec::i32_store8_ss,
                    instr_rs: exec::i32_store8_rs,
                    instr_is: exec::i32_store8_is,
                    instr_ir: exec::i32_store8_ir,
                    instr_ii: Some(exec::i32_store8_ii),
                    instr_sr: exec::i32_store8_sr,
                    instr_si: exec::i32_store8_si,
                    instr_ri: exec::i32_store8_ri,
                    instr_rr: None,
                },
            },
        ),
        Instr::Store(0x3b, arg) => visitor.visit_store(
            mem_arg(arg),
            StoreInfo {
                op: BinOpInfo {
                    _name: "i32_store16",
                    output_type: None,
                    instr_ss: exec::i32_store16_ss,
                    instr_rs: exec::i32_store16_rs,
                    instr_is: exec::i32_store16_is,
                    instr_ir: exec::i32_store16_ir,
                    instr_ii: Some(exec::i32_store16_ii),
                    instr_sr: exec::i32_store16_sr,
                    instr_si: exec::i32_store16_si,
                    instr_ri: exec::i32_store16_ri,
                    instr_rr: None,
                },
            },
        ),
        Instr::Store(0x3c, arg) => visitor.visit_store(
            mem_arg(arg),
            StoreInfo {
                op: BinOpInfo {
                    _name: "i64_store8",
                    output_type: None,
                    instr_ss: exec::i64_store8_ss,
                    instr_rs: exec::i64_store8_rs,
                    instr_is: exec::i64_store8_is,
                    instr_ir: exec::i64_store8_ir,
                    instr_ii: Some(exec::i64_store8_ii),
                    instr_sr: exec::i64_store8_sr,
                    instr_si: exec::i64_store8_si,
                    instr_ri: exec::i64_store8_ri,
                    instr_rr: None,
                },
            },
        ),
        Instr::Store(0x3d, arg) => visitor.visit_store(
            mem_arg(arg),
            StoreInfo {
                op: BinOpInfo {
                    _name: "i64_store16",
                    output_type: None,
                    instr_ss: exec::i64_store16_ss,
                    instr_rs: exec::i64_store16_rs,
                    instr_is: exec::i64_store16_is,
                    instr_ir: exec::i64_store16_ir,
                    instr_ii: Some(exec::i64_store16_ii),
                    instr_sr: exec::i64_store16_sr,
                    instr_si: exec::i64_store16_si,
                    instr_ri: exec::i64_store16_ri,
                    instr_rr: None,
                },
            },
        ),
        Instr::Store(0x3e, arg) => visitor.visit_store(
            mem_arg(arg),
            StoreInfo {
                op: BinOpInfo {
                    _name: "i64_store32",
                    output_type: None,
                    instr_ss: exec::i64_store32_ss,
                    instr_rs: exec::i64_store32_rs,
                    instr_is: exec::i64_store32_is,
                    instr_ir: exec::i64_store32_ir,
                    instr_ii: Some(exec::i64_store32_ii),
                    instr_sr: exec::i64_store32_sr,
                    instr_si: exec::i64_store32_si,
                    instr_ri: exec::i64_store32_ri,
                    instr_rr: None,
                },
            },
        ),
        Instr::MemorySize(0) => visitor.visit_memory_size(),
        Instr::MemoryGrow(0) => visitor.visit_memory_grow(),
        Instr::I32Const(val) => visitor.visit_i32_const(*val),
        Instr::I64Const(val) => visitor.visit_i64_const(*val),
        Instr::F32Const(bits) => visitor.visit_f32_const(f32::from_bits(*bits)),
        Instr::F64Const(bits) => visitor.visit_f64_const(f64::from_bits(*bits)),
        Instr::Num(0x45) => visitor.visit_un_op(UnOpInfo {
            _name: "i32_eqz",
            output_type: Some(ValType::I32),
            instr_s: exec::i32_eqz_s,
            instr_r: exec::i32_eqz_r,
            instr_i: None,
        }),
        Instr::Num(0x46) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_eq",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_eq_ss,
            instr_rs: exec::i32_eq_rs,
            instr_is: exec::i32_eq_is,
            instr_ir: exec::i32_eq_ir,
            instr_ii: None,
            instr_sr: exec::i32_eq_rs,
            instr_si: exec::i32_eq_is,
            instr_ri: exec::i32_eq_ir,
            instr_rr: None,
        }),
        Instr::Num(0x47) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_ne",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_ne_ss,
            instr_rs: exec::i32_ne_rs,
            instr_is: exec::i32_ne_is,
            instr_ir: exec::i32_ne_ir,
            instr_ii: None,
            instr_sr: exec::i32_ne_rs,
            instr_si: exec::i32_ne_is,
            instr_ri: exec::i32_ne_ir,
            instr_rr: None,
        }),
        Instr::Num(0x48) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_lt_s",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_lt_s_ss,
            instr_rs: exec::i32_lt_s_rs,
            instr_is: exec::i32_lt_s_is,
            instr_ir: exec::i32_lt_s_ir,
            instr_ii: None,
            instr_sr: exec::i32_lt_s_sr,
            instr_si: exec::i32_lt_s_si,
            instr_ri: exec::i32_lt_s_ri,
            instr_rr: None,
        }),
        Instr::Num(0x49) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_lt_u",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_lt_u_ss,
            instr_rs: exec::i32_lt_u_rs,
            instr_is: exec::i32_lt_u_is,
            instr_ir: exec::i32_lt_u_ir,
            instr_ii: None,
            instr_sr: exec::i32_lt_u_sr,
            instr_si: exec::i32_lt_u_si,
            instr_ri: exec::i32_lt_u_ri,
            instr_rr: None,
        }),
        Instr::Num(0x4a) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_gt_s",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_gt_s_ss,
            instr_rs: exec::i32_gt_s_rs,
            instr_is: exec::i32_gt_s_is,
            instr_ir: exec::i32_gt_s_ir,
            instr_ii: None,
            instr_sr: exec::i32_gt_s_sr,
            instr_si: exec::i32_gt_s_si,
            instr_ri: exec::i32_gt_s_ri,
            instr_rr: None,
        }),
        Instr::Num(0x4b) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_gt_u",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_gt_u_ss,
            instr_rs: exec::i32_gt_u_rs,
            instr_is: exec::i32_gt_u_is,
            instr_ir: exec::i32_gt_u_ir,
            instr_ii: None,
            instr_sr: exec::i32_gt_u_sr,
            instr_si: exec::i32_gt_u_si,
            instr_ri: exec::i32_gt_u_ri,
            instr_rr: None,
        }),
        Instr::Num(0x4c) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_le_s",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_le_s_ss,
            instr_rs: exec::i32_le_s_rs,
            instr_is: exec::i32_le_s_is,
            instr_ir: exec::i32_le_s_ir,
            instr_ii: None,
            instr_sr: exec::i32_le_s_sr,
            instr_si: exec::i32_le_s_si,
            instr_ri: exec::i32_le_s_ri,
            instr_rr: None,
        }),
        Instr::Num(0x4d) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_le_u",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_le_u_ss,
            instr_rs: exec::i32_le_u_rs,
            instr_is: exec::i32_le_u_is,
            instr_ir: exec::i32_le_u_ir,
            instr_ii: None,
            instr_sr: exec::i32_le_u_sr,
            instr_si: exec::i32_le_u_si,
            instr_ri: exec::i32_le_u_ri,
            instr_rr: None,
        }),
        Instr::Num(0x4e) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_ge_s",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_ge_s_ss,
            instr_rs: exec::i32_ge_s_rs,
            instr_is: exec::i32_ge_s_is,
            instr_ir: exec::i32_ge_s_ir,
            instr_ii: None,
            instr_sr: exec::i32_ge_s_sr,
            instr_si: exec::i32_ge_s_si,
            instr_ri: exec::i32_ge_s_ri,
            instr_rr: None,
        }),
        Instr::Num(0x4f) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_ge_u",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_ge_u_ss,
            instr_rs: exec::i32_ge_u_rs,
            instr_is: exec::i32_ge_u_is,
            instr_ir: exec::i32_ge_u_ir,
            instr_ii: None,
            instr_sr: exec::i32_ge_u_sr,
            instr_si: exec::i32_ge_u_si,
            instr_ri: exec::i32_ge_u_ri,
            instr_rr: None,
        }),
        Instr::Num(0x50) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_eqz",
            output_type: Some(ValType::I32),
            instr_s: exec::i64_eqz_s,
            instr_r: exec::i64_eqz_r,
            instr_i: None,
        }),
        Instr::Num(0x51) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_eq",
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_eq_ss,
            instr_rs: exec::i64_eq_rs,
            instr_is: exec::i64_eq_is,
            instr_ir: exec::i64_eq_ir,
            instr_ii: None,
            instr_sr: exec::i64_eq_rs,
            instr_si: exec::i64_eq_is,
            instr_ri: exec::i64_eq_ir,
            instr_rr: None,
        }),
        Instr::Num(0x52) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_ne",
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_ne_ss,
            instr_rs: exec::i64_ne_rs,
            instr_is: exec::i64_ne_is,
            instr_ir: exec::i64_ne_ir,
            instr_ii: None,
            instr_sr: exec::i64_ne_rs,
            instr_si: exec::i64_ne_is,
            instr_ri: exec::i64_ne_ir,
            instr_rr: None,
        }),
        Instr::Num(0x53) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_lt_s",
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_lt_s_ss,
            instr_rs: exec::i64_lt_s_rs,
            instr_is: exec::i64_lt_s_is,
            instr_ir: exec::i64_lt_s_ir,
            instr_ii: None,
            instr_sr: exec::i64_lt_s_sr,
            instr_si: exec::i64_lt_s_si,
            instr_ri: exec::i64_lt_s_ri,
            instr_rr: None,
        }),
        Instr::Num(0x54) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_lt_u",
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_lt_u_ss,
            instr_rs: exec::i64_lt_u_rs,
            instr_is: exec::i64_lt_u_is,
            instr_ir: exec::i64_lt_u_ir,
            instr_ii: None,
            instr_sr: exec::i64_lt_u_sr,
            instr_si: exec::i64_lt_u_si,
            instr_ri: exec::i64_lt_u_ri,
            instr_rr: None,
        }),
        Instr::Num(0x55) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_gt_s",
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_gt_s_ss,
            instr_rs: exec::i64_gt_s_rs,
            instr_is: exec::i64_gt_s_is,
            instr_ir: exec::i64_gt_s_ir,
            instr_ii: None,
            instr_sr: exec::i64_gt_s_sr,
            instr_si: exec::i64_gt_s_si,
            instr_ri: exec::i64_gt_s_ri,
            instr_rr: None,
        }),
        Instr::Num(0x56) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_gt_u",
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_gt_u_ss,
            instr_rs: exec::i64_gt_u_rs,
            instr_is: exec::i64_gt_u_is,
            instr_ir: exec::i64_gt_u_ir,
            instr_ii: None,
            instr_sr: exec::i64_gt_u_sr,
            instr_si: exec::i64_gt_u_si,
            instr_ri: exec::i64_gt_u_ri,
            instr_rr: None,
        }),
        Instr::Num(0x57) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_le_s",
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_le_s_ss,
            instr_rs: exec::i64_le_s_rs,
            instr_is: exec::i64_le_s_is,
            instr_ir: exec::i64_le_s_ir,
            instr_ii: None,
            instr_sr: exec::i64_le_s_sr,
            instr_si: exec::i64_le_s_si,
            instr_ri: exec::i64_le_s_ri,
            instr_rr: None,
        }),
        Instr::Num(0x58) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_le_u",
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_le_u_ss,
            instr_rs: exec::i64_le_u_rs,
            instr_is: exec::i64_le_u_is,
            instr_ir: exec::i64_le_u_ir,
            instr_ii: None,
            instr_sr: exec::i64_le_u_sr,
            instr_si: exec::i64_le_u_si,
            instr_ri: exec::i64_le_u_ri,
            instr_rr: None,
        }),
        Instr::Num(0x59) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_ge_s",
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_ge_s_ss,
            instr_rs: exec::i64_ge_s_rs,
            instr_is: exec::i64_ge_s_is,
            instr_ir: exec::i64_ge_s_ir,
            instr_ii: None,
            instr_sr: exec::i64_ge_s_sr,
            instr_si: exec::i64_ge_s_si,
            instr_ri: exec::i64_ge_s_ri,
            instr_rr: None,
        }),
        Instr::Num(0x5a) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_ge_u",
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_ge_u_ss,
            instr_rs: exec::i64_ge_u_rs,
            instr_is: exec::i64_ge_u_is,
            instr_ir: exec::i64_ge_u_ir,
            instr_ii: None,
            instr_sr: exec::i64_ge_u_sr,
            instr_si: exec::i64_ge_u_si,
            instr_ri: exec::i64_ge_u_ri,
            instr_rr: None,
        }),
        Instr::Num(0x5b) => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_eq",
            output_type: Some(ValType::I32),
            instr_ss: exec::f32_eq_ss,
            instr_rs: exec::f32_eq_rs,
            instr_is: exec::f32_eq_is,
            instr_ir: exec::f32_eq_ir,
            instr_ii: None,
            instr_sr: exec::f32_eq_rs,
            instr_si: exec::f32_eq_is,
            instr_ri: exec::f32_eq_ir,
            instr_rr: None,
        }),
        Instr::Num(0x5c) => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_ne",
            output_type: Some(ValType::I32),
            instr_ss: exec::f32_ne_ss,
            instr_rs: exec::f32_ne_rs,
            instr_is: exec::f32_ne_is,
            instr_ir: exec::f32_ne_ir,
            instr_ii: None,
            instr_sr: exec::f32_ne_rs,
            instr_si: exec::f32_ne_is,
            instr_ri: exec::f32_ne_ir,
            instr_rr: None,
        }),
        Instr::Num(0x5d) => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_lt",
            output_type: Some(ValType::I32),
            instr_ss: exec::f32_lt_ss,
            instr_rs: exec::f32_lt_rs,
            instr_is: exec::f32_lt_is,
            instr_ir: exec::f32_lt_ir,
            instr_ii: None,
            instr_sr: exec::f32_lt_sr,
            instr_si: exec::f32_lt_si,
            instr_ri: exec::f32_lt_ri,
            instr_rr: None,
        }),
        Instr::Num(0x5e) => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_gt",
            output_type: Some(ValType::I32),
            instr_ss: exec::f32_gt_ss,
            instr_rs: exec::f32_gt_rs,
            instr_is: exec::f32_gt_is,
            instr_ir: exec::f32_gt_ir,
            instr_ii: None,
            instr_sr: exec::f32_gt_sr,
            instr_si: exec::f32_gt_si,
            instr_ri: exec::f32_gt_ri,
            instr_rr: None,
        }),
        Instr::Num(0x5f) => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_le",
            output_type: Some(ValType::I32),
            instr_ss: exec::f32_le_ss,
            instr_rs: exec::f32_le_rs,
            instr_is: exec::f32_le_is,
            instr_ir: exec::f32_le_ir,
            instr_ii: None,
            instr_sr: exec::f32_le_sr,
            instr_si: exec::f32_le_si,
            instr_ri: exec::f32_le_ri,
            instr_rr: None,
        }),
        Instr::Num(0x60) => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_ge",
            output_type: Some(ValType::I32),
            instr_ss: exec::f32_ge_ss,
            instr_rs: exec::f32_ge_rs,
            instr_is: exec::f32_ge_is,
            instr_ir: exec::f32_ge_ir,
            instr_ii: None,
            instr_sr: exec::f32_ge_sr,
            instr_si: exec::f32_ge_si,
            instr_ri: exec::f32_ge_ri,
            instr_rr: None,
        }),
        Instr::Num(0x61) => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_eq",
            output_type: Some(ValType::I32),
            instr_ss: exec::f64_eq_ss,
            instr_rs: exec::f64_eq_rs,
            instr_is: exec::f64_eq_is,
            instr_ir: exec::f64_eq_ir,
            instr_ii: None,
            instr_sr: exec::f64_eq_rs,
            instr_si: exec::f64_eq_is,
            instr_ri: exec::f64_eq_ir,
            instr_rr: None,
        }),
        Instr::Num(0x62) => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_ne",
            output_type: Some(ValType::I32),
            instr_ss: exec::f64_ne_ss,
            instr_rs: exec::f64_ne_rs,
            instr_is: exec::f64_ne_is,
            instr_ir: exec::f64_ne_ir,
            instr_ii: None,
            instr_sr: exec::f64_ne_rs,
            instr_si: exec::f64_ne_is,
            instr_ri: exec::f64_ne_ir,
            instr_rr: None,
        }),
        Instr::Num(0x63) => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_lt",
            output_type: Some(ValType::I32),
            instr_ss: exec::f64_lt_ss,
            instr_rs: exec::f64_lt_rs,
            instr_is: exec::f64_lt_is,
            instr_ir: exec::f64_lt_ir,
            instr_ii: None,
            instr_sr: exec::f64_lt_sr,
            instr_si: exec::f64_lt_si,
            instr_ri: exec::f64_lt_ri,
            instr_rr: None,
        }),
        Instr::Num(0x64) => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_gt",
            output_type: Some(ValType::I32),
            instr_ss: exec::f64_gt_ss,
            instr_rs: exec::f64_gt_rs,
            instr_is: exec::f64_gt_is,
            instr_ir: exec::f64_gt_ir,
            instr_ii: None,
            instr_sr: exec::f64_gt_sr,
            instr_si: exec::f64_gt_si,
            instr_ri: exec::f64_gt_ri,
            instr_rr: None,
        }),
        Instr::Num(0x65) => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_le",
            output_type: Some(ValType::I32),
            instr_ss: exec::f64_le_ss,
            instr_rs: exec::f64_le_rs,
            instr_is: exec::f64_le_is,
            instr_ir: exec::f64_le_ir,
            instr_ii: None,
            instr_sr: exec::f64_le_sr,
            instr_si: exec::f64_le_si,
            instr_ri: exec::f64_le_ri,
            instr_rr: None,
        }),
        Instr::Num(0x66) => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_ge",
            output_type: Some(ValType::I32),
            instr_ss: exec::f64_ge_ss,
            instr_rs: exec::f64_ge_rs,
            instr_is: exec::f64_ge_is,
            instr_ir: exec::f64_ge_ir,
            instr_ii: None,
            instr_sr: exec::f64_ge_sr,
            instr_si: exec::f64_ge_si,
            instr_ri: exec::f64_ge_ri,
            instr_rr: None,
        }),
        Instr::Num(0x67) => visitor.visit_un_op(UnOpInfo {
            _name: "i32_clz",
            output_type: Some(ValType::I32),
            instr_s: exec::i32_clz_s,
            instr_r: exec::i32_clz_r,
            instr_i: None,
        }),
        Instr::Num(0x68) => visitor.visit_un_op(UnOpInfo {
            _name: "i32_ctz",
            output_type: Some(ValType::I32),
            instr_s: exec::i32_ctz_s,
            instr_r: exec::i32_ctz_r,
            instr_i: None,
        }),
        Instr::Num(0x69) => visitor.visit_un_op(UnOpInfo {
            _name: "i32_popcnt",
            output_type: Some(ValType::I32),
            instr_s: exec::i32_popcnt_s,
            instr_r: exec::i32_popcnt_r,
            instr_i: None,
        }),
        Instr::Num(0x6a) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_add",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_add_ss,
            instr_rs: exec::i32_add_rs,
            instr_is: exec::i32_add_is,
            instr_ir: exec::i32_add_ir,
            instr_ii: None,
            instr_sr: exec::i32_add_rs,
            instr_si: exec::i32_add_is,
            instr_ri: exec::i32_add_ir,
            instr_rr: None,
        }),
        Instr::Num(0x6b) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_sub",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_sub_ss,
            instr_rs: exec::i32_sub_rs,
            instr_is: exec::i32_sub_is,
            instr_ir: exec::i32_sub_ir,
            instr_ii: None,
            instr_sr: exec::i32_sub_sr,
            instr_si: exec::i32_sub_si,
            instr_ri: exec::i32_sub_ri,
            instr_rr: None,
        }),
        Instr::Num(0x6c) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_mul",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_mul_ss,
            instr_rs: exec::i32_mul_rs,
            instr_is: exec::i32_mul_is,
            instr_ir: exec::i32_mul_ir,
            instr_ii: None,
            instr_sr: exec::i32_mul_rs,
            instr_si: exec::i32_mul_is,
            instr_ri: exec::i32_mul_ir,
            instr_rr: None,
        }),
        Instr::Num(0x6d) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_div_s",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_div_s_ss,
            instr_rs: exec::i32_div_s_rs,
            instr_is: exec::i32_div_s_is,
            instr_ir: exec::i32_div_s_ir,
            instr_ii: None,
            instr_sr: exec::i32_div_s_sr,
            instr_si: exec::i32_div_s_si,
            instr_ri: exec::i32_div_s_ri,
            instr_rr: None,
        }),
        Instr::Num(0x6e) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_div_u",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_div_u_ss,
            instr_rs: exec::i32_div_u_rs,
            instr_is: exec::i32_div_u_is,
            instr_ir: exec::i32_div_u_ir,
            instr_ii: None,
            instr_sr: exec::i32_div_u_sr,
            instr_si: exec::i32_div_u_si,
            instr_ri: exec::i32_div_u_ri,
            instr_rr: None,
        }),
        Instr::Num(0x6f) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_rem_s",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_rem_s_ss,
            instr_rs: exec::i32_rem_s_rs,
            instr_is: exec::i32_rem_s_is,
            instr_ir: exec::i32_rem_s_ir,
            instr_ii: None,
            instr_sr: exec::i32_rem_s_sr,
            instr_si: exec::i32_rem_s_si,
            instr_ri: exec::i32_rem_s_ri,
            instr_rr: None,
        }),
        Instr::Num(0x70) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_rem_u",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_rem_u_ss,
            instr_rs: exec::i32_rem_u_rs,
            instr_is: exec::i32_rem_u_is,
            instr_ir: exec::i32_rem_u_ir,
            instr_ii: None,
            instr_sr: exec::i32_rem_u_sr,
            instr_si: exec::i32_rem_u_si,
            instr_ri: exec::i32_rem_u_ri,
            instr_rr: None,
        }),
        Instr::Num(0x71) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_and",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_and_ss,
            instr_rs: exec::i32_and_rs,
            instr_is: exec::i32_and_is,
            instr_ir: exec::i32_and_ir,
            instr_ii: None,
            instr_sr: exec::i32_and_rs,
            instr_si: exec::i32_and_is,
            instr_ri: exec::i32_and_ir,
            instr_rr: None,
        }),
        Instr::Num(0x72) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_or",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_or_ss,
            instr_rs: exec::i32_or_rs,
            instr_is: exec::i32_or_is,
            instr_ir: exec::i32_or_ir,
            instr_ii: None,
            instr_sr: exec::i32_or_rs,
            instr_si: exec::i32_or_is,
            instr_ri: exec::i32_or_ir,
            instr_rr: None,
        }),
        Instr::Num(0x73) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_xor",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_xor_ss,
            instr_rs: exec::i32_xor_rs,
            instr_is: exec::i32_xor_is,
            instr_ir: exec::i32_xor_ir,
            instr_ii: None,
            instr_sr: exec::i32_xor_rs,
            instr_si: exec::i32_xor_is,
            instr_ri: exec::i32_xor_ir,
            instr_rr: None,
        }),
        Instr::Num(0x74) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_shl",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_shl_ss,
            instr_rs: exec::i32_shl_rs,
            instr_is: exec::i32_shl_is,
            instr_ir: exec::i32_shl_ir,
            instr_ii: None,
            instr_sr: exec::i32_shl_sr,
            instr_si: exec::i32_shl_si,
            instr_ri: exec::i32_shl_ri,
            instr_rr: None,
        }),
        Instr::Num(0x75) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_shr_s",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_shr_s_ss,
            instr_rs: exec::i32_shr_s_rs,
            instr_is: exec::i32_shr_s_is,
            instr_ir: exec::i32_shr_s_ir,
            instr_ii: None,
            instr_sr: exec::i32_shr_s_sr,
            instr_si: exec::i32_shr_s_si,
            instr_ri: exec::i32_shr_s_ri,
            instr_rr: None,
        }),
        Instr::Num(0x76) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_shr_u",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_shr_u_ss,
            instr_rs: exec::i32_shr_u_rs,
            instr_is: exec::i32_shr_u_is,
            instr_ir: exec::i32_shr_u_ir,
            instr_ii: None,
            instr_sr: exec::i32_shr_u_sr,
            instr_si: exec::i32_shr_u_si,
            instr_ri: exec::i32_shr_u_ri,
            instr_rr: None,
        }),
        Instr::Num(0x77) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_rotl",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_rotl_ss,
            instr_rs: exec::i32_rotl_rs,
            instr_is: exec::i32_rotl_is,
            instr_ir: exec::i32_rotl_ir,
            instr_ii: None,
            instr_sr: exec::i32_rotl_sr,
            instr_si: exec::i32_rotl_si,
            instr_ri: exec::i32_rotl_ri,
            instr_rr: None,
        }),
        Instr::Num(0x78) => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_rotr",
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_rotr_ss,
            instr_rs: exec::i32_rotr_rs,
            instr_is: exec::i32_rotr_is,
            instr_ir: exec::i32_rotr_ir,
            instr_ii: None,
            instr_sr: exec::i32_rotr_sr,
            instr_si: exec::i32_rotr_si,
            instr_ri: exec::i32_rotr_ri,
            instr_rr: None,
        }),
        Instr::Num(0x79) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_clz",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_clz_s,
            instr_r: exec::i64_clz_r,
            instr_i: None,
        }),
        Instr::Num(0x7a) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_ctz",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_ctz_s,
            instr_r: exec::i64_ctz_r,
            instr_i: None,
        }),
        Instr::Num(0x7b) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_popcnt",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_popcnt_s,
            instr_r: exec::i64_popcnt_r,
            instr_i: None,
        }),
        Instr::Num(0x7c) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_add",
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_add_ss,
            instr_rs: exec::i64_add_rs,
            instr_is: exec::i64_add_is,
            instr_ir: exec::i64_add_ir,
            instr_ii: None,
            instr_sr: exec::i64_add_rs,
            instr_si: exec::i64_add_is,
            instr_ri: exec::i64_add_ir,
            instr_rr: None,
        }),
        Instr::Num(0x7d) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_sub",
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_sub_ss,
            instr_rs: exec::i64_sub_rs,
            instr_is: exec::i64_sub_is,
            instr_ir: exec::i64_sub_ir,
            instr_ii: None,
            instr_sr: exec::i64_sub_sr,
            instr_si: exec::i64_sub_si,
            instr_ri: exec::i64_sub_ri,
            instr_rr: None,
        }),
        Instr::Num(0x7e) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_mul",
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_mul_ss,
            instr_rs: exec::i64_mul_rs,
            instr_is: exec::i64_mul_is,
            instr_ir: exec::i64_mul_ir,
            instr_ii: None,
            instr_sr: exec::i64_mul_rs,
            instr_si: exec::i64_mul_is,
            instr_ri: exec::i64_mul_ir,
            instr_rr: None,
        }),
        Instr::Num(0x7f) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_div_s",
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_div_s_ss,
            instr_rs: exec::i64_div_s_rs,
            instr_is: exec::i64_div_s_is,
            instr_ir: exec::i64_div_s_ir,
            instr_ii: None,
            instr_sr: exec::i64_div_s_sr,
            instr_si: exec::i64_div_s_si,
            instr_ri: exec::i64_div_s_ri,
            instr_rr: None,
        }),
        Instr::Num(0x80) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_div_u",
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_div_u_ss,
            instr_rs: exec::i64_div_u_rs,
            instr_is: exec::i64_div_u_is,
            instr_ir: exec::i64_div_u_ir,
            instr_ii: None,
            instr_sr: exec::i64_div_u_sr,
            instr_si: exec::i64_div_u_si,
            instr_ri: exec::i64_div_u_ri,
            instr_rr: None,
        }),
        Instr::Num(0x81) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_rem_s",
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_rem_s_ss,
            instr_rs: exec::i64_rem_s_rs,
            instr_is: exec::i64_rem_s_is,
            instr_ir: exec::i64_rem_s_ir,
            instr_ii: None,
            instr_sr: exec::i64_rem_s_sr,
            instr_si: exec::i64_rem_s_si,
            instr_ri: exec::i64_rem_s_ri,
            instr_rr: None,
        }),
        Instr::Num(0x82) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_rem_u",
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_rem_u_ss,
            instr_rs: exec::i64_rem_u_rs,
            instr_is: exec::i64_rem_u_is,
            instr_ir: exec::i64_rem_u_ir,
            instr_ii: None,
            instr_sr: exec::i64_rem_u_sr,
            instr_si: exec::i64_rem_u_si,
            instr_ri: exec::i64_rem_u_ri,
            instr_rr: None,
        }),
        Instr::Num(0x83) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_and",
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_and_ss,
            instr_rs: exec::i64_and_rs,
            instr_is: exec::i64_and_is,
            instr_ir: exec::i64_and_ir,
            instr_ii: None,
            instr_sr: exec::i64_and_rs,
            instr_si: exec::i64_and_is,
            instr_ri: exec::i64_and_ir,
            instr_rr: None,
        }),
        Instr::Num(0x84) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_or",
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_or_ss,
            instr_rs: exec::i64_or_rs,
            instr_is: exec::i64_or_is,
            instr_ir: exec::i64_or_ir,
            instr_ii: None,
            instr_sr: exec::i64_or_rs,
            instr_si: exec::i64_or_is,
            instr_ri: exec::i64_or_ir,
            instr_rr: None,
        }),
        Instr::Num(0x85) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_xor",
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_xor_ss,
            instr_rs: exec::i64_xor_rs,
            instr_is: exec::i64_xor_is,
            instr_ir: exec::i64_xor_ir,
            instr_ii: None,
            instr_sr: exec::i64_xor_rs,
            instr_si: exec::i64_xor_is,
            instr_ri: exec::i64_xor_ir,
            instr_rr: None,
        }),
        Instr::Num(0x86) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_shl",
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_shl_ss,
            instr_rs: exec::i64_shl_rs,
            instr_is: exec::i64_shl_is,
            instr_ir: exec::i64_shl_ir,
            instr_ii: None,
            instr_sr: exec::i64_shl_sr,
            instr_si: exec::i64_shl_si,
            instr_ri: exec::i64_shl_ri,
            instr_rr: None,
        }),
        Instr::Num(0x87) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_shr_s",
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_shr_s_ss,
            instr_rs: exec::i64_shr_s_rs,
            instr_is: exec::i64_shr_s_is,
            instr_ir: exec::i64_shr_s_ir,
            instr_ii: None,
            instr_sr: exec::i64_shr_s_sr,
            instr_si: exec::i64_shr_s_si,
            instr_ri: exec::i64_shr_s_ri,
            instr_rr: None,
        }),
        Instr::Num(0x88) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_shr_u",
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_shr_u_ss,
            instr_rs: exec::i64_shr_u_rs,
            instr_is: exec::i64_shr_u_is,
            instr_ir: exec::i64_shr_u_ir,
            instr_ii: None,
            instr_sr: exec::i64_shr_u_sr,
            instr_si: exec::i64_shr_u_si,
            instr_ri: exec::i64_shr_u_ri,
            instr_rr: None,
        }),
        Instr::Num(0x89) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_rotl",
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_rotl_ss,
            instr_rs: exec::i64_rotl_rs,
            instr_is: exec::i64_rotl_is,
            instr_ir: exec::i64_rotl_ir,
            instr_ii: None,
            instr_sr: exec::i64_rotl_sr,
            instr_si: exec::i64_rotl_si,
            instr_ri: exec::i64_rotl_ri,
            instr_rr: None,
        }),
        Instr::Num(0x8a) => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_rotr",
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_rotr_ss,
            instr_rs: exec::i64_rotr_rs,
            instr_is: exec::i64_rotr_is,
            instr_ir: exec::i64_rotr_ir,
            instr_ii: None,
            instr_sr: exec::i64_rotr_sr,
            instr_si: exec::i64_rotr_si,
            instr_ri: exec::i64_rotr_ri,
            instr_rr: None,
        }),
        Instr::Num(0x8b) => visitor.visit_un_op(UnOpInfo {
            _name: "f32_abs",
            output_type: Some(ValType::F32),
            instr_s: exec::f32_abs_s,
            instr_r: exec::f32_abs_r,
            instr_i: None,
        }),
        Instr::Num(0x8c) => visitor.visit_un_op(UnOpInfo {
            _name: "f32_neg",
            output_type: Some(ValType::F32),
            instr_s: exec::f32_neg_s,
            instr_r: exec::f32_neg_r,
            instr_i: None,
        }),
        Instr::Num(0x8d) => visitor.visit_un_op(UnOpInfo {
            _name: "f32_ceil",
            output_type: Some(ValType::F32),
            instr_s: exec::f32_ceil_s,
            instr_r: exec::f32_ceil_r,
            instr_i: None,
        }),
        Instr::Num(0x8e) => visitor.visit_un_op(UnOpInfo {
            _name: "f32_floor",
            output_type: Some(ValType::F32),
            instr_s: exec::f32_floor_s,
            instr_r: exec::f32_floor_r,
            instr_i: None,
        }),
        Instr::Num(0x8f) => visitor.visit_un_op(UnOpInfo {
            _name: "f32_trunc",
            output_type: Some(ValType::F32),
            instr_s: exec::f32_trunc_s,
            instr_r: exec::f32_trunc_r,
            instr_i: None,
        }),
        Instr::Num(0x90) => visitor.visit_un_op(UnOpInfo {
            _name: "f32_nearest",
            output_type: Some(ValType::F32),
            instr_s: exec::f32_nearest_s,
            instr_r: exec::f32_nearest_r,
            instr_i: None,
        }),
        Instr::Num(0x91) => visitor.visit_un_op(UnOpInfo {
            _name: "f32_sqrt",
            output_type: Some(ValType::F32),
            instr_s: exec::f32_sqrt_s,
            instr_r: exec::f32_sqrt_r,
            instr_i: None,
        }),
        Instr::Num(0x92) => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_add",
            output_type: Some(ValType::F32),
            instr_ss: exec::f32_add_ss,
            instr_rs: exec::f32_add_rs,
            instr_is: exec::f32_add_is,
            instr_ir: exec::f32_add_ir,
            instr_ii: None,
            instr_sr: exec::f32_add_rs,
            instr_si: exec::f32_add_is,
            instr_ri: exec::f32_add_ir,
            instr_rr: None,
        }),
        Instr::Num(0x93) => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_sub",
            output_type: Some(ValType::F32),
            instr_ss: exec::f32_sub_ss,
            instr_rs: exec::f32_sub_rs,
            instr_is: exec::f32_sub_is,
            instr_ir: exec::f32_sub_ir,
            instr_ii: None,
            instr_sr: exec::f32_sub_sr,
            instr_si: exec::f32_sub_si,
            instr_ri: exec::f32_sub_ri,
            instr_rr: None,
        }),
        Instr::Num(0x94) => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_mul",
            output_type: Some(ValType::F32),
            instr_ss: exec::f32_mul_ss,
            instr_rs: exec::f32_mul_rs,
            instr_is: exec::f32_mul_is,
            instr_ir: exec::f32_mul_ir,
            instr_ii: None,
            instr_sr: exec::f32_mul_rs,
            instr_si: exec::f32_mul_is,
            instr_ri: exec::f32_mul_ir,
            instr_rr: None,
        }),
        Instr::Num(0x95) => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_div",
            output_type: Some(ValType::F32),
            instr_ss: exec::f32_div_ss,
            instr_rs: exec::f32_div_rs,
            instr_is: exec::f32_div_is,
            instr_ir: exec::f32_div_ir,
            instr_ii: None,
            instr_sr: exec::f32_div_sr,
            instr_si: exec::f32_div_si,
            instr_ri: exec::f32_div_ri,
            instr_rr: None,
        }),
        Instr::Num(0x96) => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_min",
            output_type: Some(ValType::F32),
            instr_ss: exec::f32_min_ss,
            instr_rs: exec::f32_min_rs,
            instr_is: exec::f32_min_is,
            instr_ir: exec::f32_min_ir,
            instr_ii: None,
            instr_sr: exec::f32_min_rs,
            instr_si: exec::f32_min_is,
            instr_ri: exec::f32_min_ir,
            instr_rr: None,
        }),
        Instr::Num(0x97) => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_max",
            output_type: Some(ValType::F32),
            instr_ss: exec::f32_max_ss,
            instr_rs: exec::f32_max_rs,
            instr_is: exec::f32_max_is,
            instr_ir: exec::f32_max_ir,
            instr_ii: None,
            instr_sr: exec::f32_max_rs,
            instr_si: exec::f32_max_is,
            instr_ri: exec::f32_max_ir,
            instr_rr: None,
        }),
        Instr::Num(0x98) => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_copysign",
            output_type: Some(ValType::F32),
            instr_ss: exec::f32_copysign_ss,
            instr_rs: exec::f32_copysign_rs,
            instr_is: exec::f32_copysign_is,
            instr_ir: exec::f32_copysign_ir,
            instr_ii: None,
            instr_sr: exec::f32_copysign_sr,
            instr_si: exec::f32_copysign_si,
            instr_ri: exec::f32_copysign_ri,
            instr_rr: None,
        }),
        Instr::Num(0x99) => visitor.visit_un_op(UnOpInfo {
            _name: "f64_abs",
            output_type: Some(ValType::F64),
            instr_s: exec::f64_abs_s,
            instr_r: exec::f64_abs_r,
            instr_i: None,
        }),
        Instr::Num(0x9a) => visitor.visit_un_op(UnOpInfo {
            _name: "f64_neg",
            output_type: Some(ValType::F64),
            instr_s: exec::f64_neg_s,
            instr_r: exec::f64_neg_r,
            instr_i: None,
        }),
        Instr::Num(0x9b) => visitor.visit_un_op(UnOpInfo {
            _name: "f64_ceil",
            output_type: Some(ValType::F64),
            instr_s: exec::f64_ceil_s,
            instr_r: exec::f64_ceil_r,
            instr_i: None,
        }),
        Instr::Num(0x9c) => visitor.visit_un_op(UnOpInfo {
            _name: "f64_floor",
            output_type: Some(ValType::F64),
            instr_s: exec::f64_floor_s,
            instr_r: exec::f64_floor_r,
            instr_i: None,
        }),
        Instr::Num(0x9d) => visitor.visit_un_op(UnOpInfo {
            _name: "f64_trunc",
            output_type: Some(ValType::F64),
            instr_s: exec::f64_trunc_s,
            instr_r: exec::f64_trunc_r,
            instr_i: None,
        }),
        Instr::Num(0x9e) => visitor.visit_un_op(UnOpInfo {
            _name: "f64_nearest",
            output_type: Some(ValType::F64),
            instr_s: exec::f64_nearest_s,
            instr_r: exec::f64_nearest_r,
            instr_i: None,
        }),
        Instr::Num(0x9f) => visitor.visit_un_op(UnOpInfo {
            _name: "f64_sqrt",
            output_type: Some(ValType::F64),
            instr_s: exec::f64_sqrt_s,
            instr_r: exec::f64_sqrt_r,
            instr_i: None,
        }),
        Instr::Num(0xa0) => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_add",
            output_type: Some(ValType::F64),
            instr_ss: exec::f64_add_ss,
            instr_rs: exec::f64_add_rs,
            instr_is: exec::f64_add_is,
            instr_ir: exec::f64_add_ir,
            instr_ii: None,
            instr_sr: exec::f64_add_rs,
            instr_si: exec::f64_add_is,
            instr_ri: exec::f64_add_ir,
            instr_rr: None,
        }),
        Instr::Num(0xa1) => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_sub",
            output_type: Some(ValType::F64),
            instr_ss: exec::f64_sub_ss,
            instr_rs: exec::f64_sub_rs,
            instr_is: exec::f64_sub_is,
            instr_ir: exec::f64_sub_ir,
            instr_ii: None,
            instr_sr: exec::f64_sub_sr,
            instr_si: exec::f64_sub_si,
            instr_ri: exec::f64_sub_ri,
            instr_rr: None,
        }),
        Instr::Num(0xa2) => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_mul",
            output_type: Some(ValType::F64),
            instr_ss: exec::f64_mul_ss,
            instr_rs: exec::f64_mul_rs,
            instr_is: exec::f64_mul_is,
            instr_ir: exec::f64_mul_ir,
            instr_ii: None,
            instr_sr: exec::f64_mul_rs,
            instr_si: exec::f64_mul_is,
            instr_ri: exec::f64_mul_ir,
            instr_rr: None,
        }),
        Instr::Num(0xa3) => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_div",
            output_type: Some(ValType::F64),
            instr_ss: exec::f64_div_ss,
            instr_rs: exec::f64_div_rs,
            instr_is: exec::f64_div_is,
            instr_ir: exec::f64_div_ir,
            instr_ii: None,
            instr_sr: exec::f64_div_sr,
            instr_si: exec::f64_div_si,
            instr_ri: exec::f64_div_ri,
            instr_rr: None,
        }),
        Instr::Num(0xa4) => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_min",
            output_type: Some(ValType::F64),
            instr_ss: exec::f64_min_ss,
            instr_rs: exec::f64_min_rs,
            instr_is: exec::f64_min_is,
            instr_ir: exec::f64_min_ir,
            instr_ii: None,
            instr_sr: exec::f64_min_rs,
            instr_si: exec::f64_min_is,
            instr_ri: exec::f64_min_ir,
            instr_rr: None,
        }),
        Instr::Num(0xa5) => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_max",
            output_type: Some(ValType::F64),
            instr_ss: exec::f64_max_ss,
            instr_rs: exec::f64_max_rs,
            instr_is: exec::f64_max_is,
            instr_ir: exec::f64_max_ir,
            instr_ii: None,
            instr_sr: exec::f64_max_rs,
            instr_si: exec::f64_max_is,
            instr_ri: exec::f64_max_ir,
            instr_rr: None,
        }),
        Instr::Num(0xa6) => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_copysign",
            output_type: Some(ValType::F64),
            instr_ss: exec::f64_copysign_ss,
            instr_rs: exec::f64_copysign_rs,
            instr_is: exec::f64_copysign_is,
            instr_ir: exec::f64_copysign_ir,
            instr_ii: None,
            instr_sr: exec::f64_copysign_sr,
            instr_si: exec::f64_copysign_si,
            instr_ri: exec::f64_copysign_ri,
            instr_rr: None,
        }),
        Instr::Num(0xa7) => visitor.visit_un_op(UnOpInfo {
            _name: "i32_wrap_i64",
            output_type: Some(ValType::I32),
            instr_s: exec::i32_wrap_i64_s,
            instr_r: exec::i32_wrap_i64_r,
            instr_i: None,
        }),
        Instr::Num(0xa8) => visitor.visit_un_op(UnOpInfo {
            _name: "i32_trunc_f32_s",
            output_type: Some(ValType::I32),
            instr_s: exec::i32_trunc_f32_s_s,
            instr_r: exec::i32_trunc_f32_s_r,
            instr_i: None,
        }),
        Instr::Num(0xa9) => visitor.visit_un_op(UnOpInfo {
            _name: "i32_trunc_f32_u",
            output_type: Some(ValType::I32),
            instr_s: exec::i32_trunc_f32_u_s,
            instr_r: exec::i32_trunc_f32_u_r,
            instr_i: None,
        }),
        Instr::Num(0xaa) => visitor.visit_un_op(UnOpInfo {
            _name: "i32_trunc_f64_s",
            output_type: Some(ValType::I32),
            instr_s: exec::i32_trunc_f64_s_s,
            instr_r: exec::i32_trunc_f64_s_r,
            instr_i: None,
        }),
        Instr::Num(0xab) => visitor.visit_un_op(UnOpInfo {
            _name: "i32_trunc_f64_u",
            output_type: Some(ValType::I32),
            instr_s: exec::i32_trunc_f64_u_s,
            instr_r: exec::i32_trunc_f64_u_r,
            instr_i: None,
        }),
        Instr::Num(0xac) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_extend_i32_s",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_extend_i32_s_s,
            instr_r: exec::i64_extend_i32_s_r,
            instr_i: None,
        }),
        Instr::Num(0xad) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_extend_i32_u",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_extend_i32_u_s,
            instr_r: exec::i64_extend_i32_u_r,
            instr_i: None,
        }),
        Instr::Num(0xae) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_trunc_f32_s",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_trunc_f32_s_s,
            instr_r: exec::i64_trunc_f32_s_r,
            instr_i: None,
        }),
        Instr::Num(0xaf) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_trunc_f32_u",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_trunc_f32_u_s,
            instr_r: exec::i64_trunc_f32_u_r,
            instr_i: None,
        }),
        Instr::Num(0xb0) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_trunc_f64_s",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_trunc_f64_s_s,
            instr_r: exec::i64_trunc_f64_s_r,
            instr_i: None,
        }),
        Instr::Num(0xb1) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_trunc_f64_u",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_trunc_f64_u_s,
            instr_r: exec::i64_trunc_f64_u_r,
            instr_i: None,
        }),
        Instr::Num(0xb2) => visitor.visit_un_op(UnOpInfo {
            _name: "f32_convert_i32_s",
            output_type: Some(ValType::F32),
            instr_s: exec::f32_convert_i32_s_s,
            instr_r: exec::f32_convert_i32_s_r,
            instr_i: None,
        }),
        Instr::Num(0xb3) => visitor.visit_un_op(UnOpInfo {
            _name: "f32_convert_i32_u",
            output_type: Some(ValType::F32),
            instr_s: exec::f32_convert_i32_u_s,
            instr_r: exec::f32_convert_i32_u_r,
            instr_i: None,
        }),
        Instr::Num(0xb4) => visitor.visit_un_op(UnOpInfo {
            _name: "f32_convert_i64_s",
            output_type: Some(ValType::F32),
            instr_s: exec::f32_convert_i64_s_s,
            instr_r: exec::f32_convert_i64_s_r,
            instr_i: None,
        }),
        Instr::Num(0xb5) => visitor.visit_un_op(UnOpInfo {
            _name: "f32_convert_i64_u",
            output_type: Some(ValType::F32),
            instr_s: exec::f32_convert_i64_u_s,
            instr_r: exec::f32_convert_i64_u_r,
            instr_i: None,
        }),
        Instr::Num(0xb6) => visitor.visit_un_op(UnOpInfo {
            _name: "f32_demote_f64",
            output_type: Some(ValType::F32),
            instr_s: exec::f32_demote_f64_s,
            instr_r: exec::f32_demote_f64_r,
            instr_i: None,
        }),
        Instr::Num(0xb7) => visitor.visit_un_op(UnOpInfo {
            _name: "f64_convert_i32_s",
            output_type: Some(ValType::F64),
            instr_s: exec::f64_convert_i32_s_s,
            instr_r: exec::f64_convert_i32_s_r,
            instr_i: None,
        }),
        Instr::Num(0xb8) => visitor.visit_un_op(UnOpInfo {
            _name: "f64_convert_i32_u",
            output_type: Some(ValType::F64),
            instr_s: exec::f64_convert_i32_u_s,
            instr_r: exec::f64_convert_i32_u_r,
            instr_i: None,
        }),
        Instr::Num(0xb9) => visitor.visit_un_op(UnOpInfo {
            _name: "f64_convert_i64_s",
            output_type: Some(ValType::F64),
            instr_s: exec::f64_convert_i64_s_s,
            instr_r: exec::f64_convert_i64_s_r,
            instr_i: None,
        }),
        Instr::Num(0xba) => visitor.visit_un_op(UnOpInfo {
            _name: "f64_convert_i64_u",
            output_type: Some(ValType::F64),
            instr_s: exec::f64_convert_i64_u_s,
            instr_r: exec::f64_convert_i64_u_r,
            instr_i: None,
        }),
        Instr::Num(0xbb) => visitor.visit_un_op(UnOpInfo {
            _name: "f64_promote_f32",
            output_type: Some(ValType::F64),
            instr_s: exec::f64_promote_f32_s,
            instr_r: exec::f64_promote_f32_r,
            instr_i: None,
        }),
        Instr::Num(0xbc) => visitor.visit_un_op(UnOpInfo {
            _name: "i32_reinterpret_f32",
            output_type: Some(ValType::I32),
            instr_s: exec::i32_reinterpret_f32_s,
            instr_r: exec::i32_reinterpret_f32_r,
            instr_i: None,
        }),
        Instr::Num(0xbd) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_reinterpret_f64",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_reinterpret_f64_s,
            instr_r: exec::i64_reinterpret_f64_r,
            instr_i: None,
        }),
        Instr::Num(0xbe) => visitor.visit_un_op(UnOpInfo {
            _name: "f32_reinterpret_i32",
            output_type: Some(ValType::F32),
            instr_s: exec::f32_reinterpret_i32_s,
            instr_r: exec::f32_reinterpret_i32_r,
            instr_i: None,
        }),
        Instr::Num(0xbf) => visitor.visit_un_op(UnOpInfo {
            _name: "f64_reinterpret_i64",
            output_type: Some(ValType::F64),
            instr_s: exec::f64_reinterpret_i64_s,
            instr_r: exec::f64_reinterpret_i64_r,
            instr_i: None,
        }),
        Instr::Num(0xc0) => visitor.visit_un_op(UnOpInfo {
            _name: "i32_extend8_s",
            output_type: Some(ValType::I32),
            instr_s: exec::i32_extend8_s_s,
            instr_r: exec::i32_extend8_s_r,
            instr_i: None,
        }),
        Instr::Num(0xc1) => visitor.visit_un_op(UnOpInfo {
            _name: "i32_extend16_s",
            output_type: Some(ValType::I32),
            instr_s: exec::i32_extend16_s_s,
            instr_r: exec::i32_extend16_s_r,
            instr_i: None,
        }),
        Instr::Num(0xc2) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_extend8_s",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_extend8_s_s,
            instr_r: exec::i64_extend8_s_r,
            instr_i: None,
        }),
        Instr::Num(0xc3) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_extend16_s",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_extend16_s_s,
            instr_r: exec::i64_extend16_s_r,
            instr_i: None,
        }),
        Instr::Num(0xc4) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_extend32_s",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_extend32_s_s,
            instr_r: exec::i64_extend32_s_r,
            instr_i: None,
        }),
        Instr::RefNull(type_) => visitor.visit_ref_null(type_.to_ref().unwrap()),
        Instr::RefIsNull => visitor.visit_ref_is_null(),
        Instr::RefFunc(func_idx) => visitor.visit_ref_func(*func_idx),
        Instr::TruncSat(0) => visitor.visit_un_op(UnOpInfo {
            _name: "i32_trunc_sat_f32_s",
            output_type: Some(ValType::I32),
            instr_s: exec::i32_trunc_sat_f32_s_s,
            instr_r: exec::i32_trunc_sat_f32_s_r,
            instr_i: None,
        }),
        Instr::TruncSat(1) => visitor.visit_un_op(UnOpInfo {
            _name: "i32_trunc_sat_f32_u",
            output_type: Some(ValType::I32),
            instr_s: exec::i32_trunc_sat_f32_u_s,
            instr_r: exec::i32_trunc_sat_f32_u_r,
            instr_i: None,
        }),
        Instr::TruncSat(2) => visitor.visit_un_op(UnOpInfo {
            _name: "i32_trunc_sat_f64_s",
            output_type: Some(ValType::I32),
            instr_s: exec::i32_trunc_sat_f64_s_s,
            instr_r: exec::i32_trunc_sat_f64_s_r,
            instr_i: None,
        }),
        Instr::TruncSat(3) => visitor.visit_un_op(UnOpInfo {
            _name: "i32_trunc_sat_f64_u",
            output_type: Some(ValType::I32),
            instr_s: exec::i32_trunc_sat_f64_u_s,
            instr_r: exec::i32_trunc_sat_f64_u_r,
            instr_i: None,
        }),
        Instr::TruncSat(4) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_trunc_sat_f32_s",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_trunc_sat_f32_s_s,
            instr_r: exec::i64_trunc_sat_f32_s_r,
            instr_i: None,
        }),
        Instr::TruncSat(5) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_trunc_sat_f32_u",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_trunc_sat_f32_u_s,
            instr_r: exec::i64_trunc_sat_f32_u_r,
            instr_i: None,
        }),
        Instr::TruncSat(6) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_trunc_sat_f64_s",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_trunc_sat_f64_s_s,
            instr_r: exec::i64_trunc_sat_f64_s_r,
            instr_i: None,
        }),
        Instr::TruncSat(7) => visitor.visit_un_op(UnOpInfo {
            _name: "i64_trunc_sat_f64_u",
            output_type: Some(ValType::I64),
            instr_s: exec::i64_trunc_sat_f64_u_s,
            instr_r: exec::i64_trunc_sat_f64_u_r,
            instr_i: None,
        }),
        Instr::MemoryInit { data, memory: 0 } => visitor.visit_memory_init(*data),
        Instr::DataDrop(data_idx) => visitor.visit_data_drop(*data_idx),
        Instr::MemoryCopy { dst: 0, src: 0 } => visitor.visit_memory_copy(),
        Instr::MemoryFill(0) => visitor.visit_memory_fill(),
        Instr::TableInit { elem, table } => visitor.visit_table_init(*table, *elem),
        Instr::ElemDrop(elem_idx) => visitor.visit_elem_drop(*elem_idx),
        Instr::TableCopy { dst, src } => visitor.visit_table_copy(*dst, *src),
        Instr::TableGrow(table_idx) => visitor.visit_table_grow(*table_idx),
        Instr::TableSize(table_idx) => visitor.visit_table_size(*table_idx),
        Instr::TableFill(table_idx) => visitor.visit_table_fill(*table_idx),
        Instr::SimdMem(..)
        | Instr::SimdMemLane(..)
        | Instr::V128Const(_)
        | Instr::I8x16Shuffle(_)
        | Instr::SimdLane(..)
        | Instr::Simd(_) => visit_simd_instr(instr, visitor),
        Instr::ExtMath(sub) => visit_ext_math_instr(*sub, visitor),
        _ => Err(DecodeError::new("illegal opcode"))?,
    }
}

/// Visits the subset of the Wasm SIMD proposal that stitch implements.
///
/// This covers everything needed for packed `f32x4` math: `v128`
/// load/store/const, `i8x16.shuffle`, `f32x4` splat/extract/replace lane,
/// the `f32x4` comparisons and arithmetic (including `pmin`/`pmax` and the
/// rounding instructions), and the `v128` bitwise instructions. All other
/// SIMD instructions are refused with "illegal opcode".
fn visit_simd_instr<V>(instr: &Instr, visitor: &mut V) -> Result<(), V::Error>
where
    V: InstrVisitor,
    V::Error: From<DecodeError>,
{
    let sub = match instr {
        Instr::SimdMem(0x00, arg) if arg.memory == 0 => return visitor.visit_v128_load(mem_arg(arg)),
        Instr::SimdMem(0x0b, arg) if arg.memory == 0 => return visitor.visit_v128_store(mem_arg(arg)),
        Instr::V128Const(bytes) => return visitor.visit_v128_const(V128::from_bytes(**bytes)),
        Instr::I8x16Shuffle(lanes) => return visitor.visit_i8x16_shuffle(**lanes),
        Instr::SimdLane(0x1f, lane) => return visitor.visit_f32x4_extract_lane(*lane),
        Instr::SimdLane(0x20, lane) => return visitor.visit_f32x4_replace_lane(*lane),
        Instr::Simd(sub) => *sub,
        _ => return Err(DecodeError::new("illegal opcode"))?,
    };
    match sub {
        // f32x4.splat
        19 => visitor.visit_f32x4_splat(),
        // f32x4.eq/ne/lt/gt/le/ge
        65 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_eq",
            instr: exec::f32x4_eq,
        }),
        66 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_ne",
            instr: exec::f32x4_ne,
        }),
        67 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_lt",
            instr: exec::f32x4_lt,
        }),
        68 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_gt",
            instr: exec::f32x4_gt,
        }),
        69 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_le",
            instr: exec::f32x4_le,
        }),
        70 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_ge",
            instr: exec::f32x4_ge,
        }),
        // v128.not/and/andnot/or/xor/bitselect/any_true
        77 => visitor.visit_v128_un_op(V128UnOpInfo {
            _name: "v128_not",
            instr: exec::v128_not,
        }),
        78 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "v128_and",
            instr: exec::v128_and,
        }),
        79 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "v128_andnot",
            instr: exec::v128_andnot,
        }),
        80 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "v128_or",
            instr: exec::v128_or,
        }),
        81 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "v128_xor",
            instr: exec::v128_xor,
        }),
        82 => visitor.visit_v128_bitselect(),
        83 => visitor.visit_v128_any_true(),
        // f32x4.ceil/floor/trunc/nearest
        103 => visitor.visit_v128_un_op(V128UnOpInfo {
            _name: "f32x4_ceil",
            instr: exec::f32x4_ceil,
        }),
        104 => visitor.visit_v128_un_op(V128UnOpInfo {
            _name: "f32x4_floor",
            instr: exec::f32x4_floor,
        }),
        105 => visitor.visit_v128_un_op(V128UnOpInfo {
            _name: "f32x4_trunc",
            instr: exec::f32x4_trunc,
        }),
        106 => visitor.visit_v128_un_op(V128UnOpInfo {
            _name: "f32x4_nearest",
            instr: exec::f32x4_nearest,
        }),
        // f32x4.abs/neg/sqrt/add/sub/mul/div/min/max/pmin/pmax
        224 => visitor.visit_v128_un_op(V128UnOpInfo {
            _name: "f32x4_abs",
            instr: exec::f32x4_abs,
        }),
        225 => visitor.visit_v128_un_op(V128UnOpInfo {
            _name: "f32x4_neg",
            instr: exec::f32x4_neg,
        }),
        227 => visitor.visit_v128_un_op(V128UnOpInfo {
            _name: "f32x4_sqrt",
            instr: exec::f32x4_sqrt,
        }),
        228 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_add",
            instr: exec::f32x4_add,
        }),
        229 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_sub",
            instr: exec::f32x4_sub,
        }),
        230 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_mul",
            instr: exec::f32x4_mul,
        }),
        231 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_div",
            instr: exec::f32x4_div,
        }),
        232 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_min",
            instr: exec::f32x4_min,
        }),
        233 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_max",
            instr: exec::f32x4_max,
        }),
        234 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_pmin",
            instr: exec::f32x4_pmin,
        }),
        235 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_pmax",
            instr: exec::f32x4_pmax,
        }),
        _ => Err(DecodeError::new("illegal opcode"))?,
    }
}

/// Visits the NONSTANDARD float math opcodes (prefix byte 0xE0, enabled
/// via [`Extensions::ext_math`](crate::Extensions::ext_math)).
///
/// Subopcode layout (one byte):
/// - 0x00..=0x0C: scalar f32 sin, cos, tan, asin, acos, atan, exp, ln,
///   atan2, pow, rmin, rmax, rem
/// - 0x10..=0x1C: scalar f64, same order
/// - 0x20..=0x2C: packed f32x4, same order
/// - 0x2D..=0x2F: packed dot-product reductions (dot2, dot3, dot4)
fn visit_ext_math_instr<V>(sub: u8, visitor: &mut V) -> Result<(), V::Error>
where
    V: InstrVisitor,
    V::Error: From<DecodeError>,
{
    fn un_op_info(
        name: &'static str,
        type_: ValType,
        instr_s: ThreadedInstr,
        instr_r: ThreadedInstr,
    ) -> UnOpInfo {
        UnOpInfo {
            _name: name,
            output_type: Some(type_),
            instr_s,
            instr_r,
            instr_i: None,
        }
    }

    fn bin_op_info(
        name: &'static str,
        type_: ValType,
        instrs: [ThreadedInstr; 7],
    ) -> BinOpInfo {
        let [ss, rs, is, ir, sr, si, ri] = instrs;
        BinOpInfo {
            _name: name,
            output_type: Some(type_),
            instr_ss: ss,
            instr_rs: rs,
            instr_is: is,
            instr_ir: ir,
            instr_ii: None,
            instr_sr: sr,
            instr_si: si,
            instr_ri: ri,
            instr_rr: None,
        }
    }

    match sub {
        // Scalar f32
        0x00 => visitor.visit_un_op(un_op_info(
            "f32_sin",
            ValType::F32,
            exec::f32_sin_s,
            exec::f32_sin_r,
        )),
        0x01 => visitor.visit_un_op(un_op_info(
            "f32_cos",
            ValType::F32,
            exec::f32_cos_s,
            exec::f32_cos_r,
        )),
        0x02 => visitor.visit_un_op(un_op_info(
            "f32_tan",
            ValType::F32,
            exec::f32_tan_s,
            exec::f32_tan_r,
        )),
        0x03 => visitor.visit_un_op(un_op_info(
            "f32_asin",
            ValType::F32,
            exec::f32_asin_s,
            exec::f32_asin_r,
        )),
        0x04 => visitor.visit_un_op(un_op_info(
            "f32_acos",
            ValType::F32,
            exec::f32_acos_s,
            exec::f32_acos_r,
        )),
        0x05 => visitor.visit_un_op(un_op_info(
            "f32_atan",
            ValType::F32,
            exec::f32_atan_s,
            exec::f32_atan_r,
        )),
        0x06 => visitor.visit_un_op(un_op_info(
            "f32_exp",
            ValType::F32,
            exec::f32_exp_s,
            exec::f32_exp_r,
        )),
        0x07 => visitor.visit_un_op(un_op_info(
            "f32_ln",
            ValType::F32,
            exec::f32_ln_s,
            exec::f32_ln_r,
        )),
        0x08 => visitor.visit_bin_op(bin_op_info(
            "f32_atan2",
            ValType::F32,
            [
                exec::f32_atan2_ss,
                exec::f32_atan2_rs,
                exec::f32_atan2_is,
                exec::f32_atan2_ir,
                exec::f32_atan2_sr,
                exec::f32_atan2_si,
                exec::f32_atan2_ri,
            ],
        )),
        0x09 => visitor.visit_bin_op(bin_op_info(
            "f32_pow",
            ValType::F32,
            [
                exec::f32_pow_ss,
                exec::f32_pow_rs,
                exec::f32_pow_is,
                exec::f32_pow_ir,
                exec::f32_pow_sr,
                exec::f32_pow_si,
                exec::f32_pow_ri,
            ],
        )),
        0x0A => visitor.visit_bin_op(bin_op_info(
            "f32_rmin",
            ValType::F32,
            [
                exec::f32_rmin_ss,
                exec::f32_rmin_rs,
                exec::f32_rmin_is,
                exec::f32_rmin_ir,
                exec::f32_rmin_sr,
                exec::f32_rmin_si,
                exec::f32_rmin_ri,
            ],
        )),
        0x0B => visitor.visit_bin_op(bin_op_info(
            "f32_rmax",
            ValType::F32,
            [
                exec::f32_rmax_ss,
                exec::f32_rmax_rs,
                exec::f32_rmax_is,
                exec::f32_rmax_ir,
                exec::f32_rmax_sr,
                exec::f32_rmax_si,
                exec::f32_rmax_ri,
            ],
        )),
        0x0C => visitor.visit_bin_op(bin_op_info(
            "f32_rem",
            ValType::F32,
            [
                exec::f32_rem_ss,
                exec::f32_rem_rs,
                exec::f32_rem_is,
                exec::f32_rem_ir,
                exec::f32_rem_sr,
                exec::f32_rem_si,
                exec::f32_rem_ri,
            ],
        )),
        // Scalar f64
        0x10 => visitor.visit_un_op(un_op_info(
            "f64_sin",
            ValType::F64,
            exec::f64_sin_s,
            exec::f64_sin_r,
        )),
        0x11 => visitor.visit_un_op(un_op_info(
            "f64_cos",
            ValType::F64,
            exec::f64_cos_s,
            exec::f64_cos_r,
        )),
        0x12 => visitor.visit_un_op(un_op_info(
            "f64_tan",
            ValType::F64,
            exec::f64_tan_s,
            exec::f64_tan_r,
        )),
        0x13 => visitor.visit_un_op(un_op_info(
            "f64_asin",
            ValType::F64,
            exec::f64_asin_s,
            exec::f64_asin_r,
        )),
        0x14 => visitor.visit_un_op(un_op_info(
            "f64_acos",
            ValType::F64,
            exec::f64_acos_s,
            exec::f64_acos_r,
        )),
        0x15 => visitor.visit_un_op(un_op_info(
            "f64_atan",
            ValType::F64,
            exec::f64_atan_s,
            exec::f64_atan_r,
        )),
        0x16 => visitor.visit_un_op(un_op_info(
            "f64_exp",
            ValType::F64,
            exec::f64_exp_s,
            exec::f64_exp_r,
        )),
        0x17 => visitor.visit_un_op(un_op_info(
            "f64_ln",
            ValType::F64,
            exec::f64_ln_s,
            exec::f64_ln_r,
        )),
        0x18 => visitor.visit_bin_op(bin_op_info(
            "f64_atan2",
            ValType::F64,
            [
                exec::f64_atan2_ss,
                exec::f64_atan2_rs,
                exec::f64_atan2_is,
                exec::f64_atan2_ir,
                exec::f64_atan2_sr,
                exec::f64_atan2_si,
                exec::f64_atan2_ri,
            ],
        )),
        0x19 => visitor.visit_bin_op(bin_op_info(
            "f64_pow",
            ValType::F64,
            [
                exec::f64_pow_ss,
                exec::f64_pow_rs,
                exec::f64_pow_is,
                exec::f64_pow_ir,
                exec::f64_pow_sr,
                exec::f64_pow_si,
                exec::f64_pow_ri,
            ],
        )),
        0x1A => visitor.visit_bin_op(bin_op_info(
            "f64_rmin",
            ValType::F64,
            [
                exec::f64_rmin_ss,
                exec::f64_rmin_rs,
                exec::f64_rmin_is,
                exec::f64_rmin_ir,
                exec::f64_rmin_sr,
                exec::f64_rmin_si,
                exec::f64_rmin_ri,
            ],
        )),
        0x1B => visitor.visit_bin_op(bin_op_info(
            "f64_rmax",
            ValType::F64,
            [
                exec::f64_rmax_ss,
                exec::f64_rmax_rs,
                exec::f64_rmax_is,
                exec::f64_rmax_ir,
                exec::f64_rmax_sr,
                exec::f64_rmax_si,
                exec::f64_rmax_ri,
            ],
        )),
        0x1C => visitor.visit_bin_op(bin_op_info(
            "f64_rem",
            ValType::F64,
            [
                exec::f64_rem_ss,
                exec::f64_rem_rs,
                exec::f64_rem_is,
                exec::f64_rem_ir,
                exec::f64_rem_sr,
                exec::f64_rem_si,
                exec::f64_rem_ri,
            ],
        )),
        // Packed f32x4
        0x20 => visitor.visit_v128_un_op(V128UnOpInfo {
            _name: "f32x4_sin",
            instr: exec::f32x4_sin,
        }),
        0x21 => visitor.visit_v128_un_op(V128UnOpInfo {
            _name: "f32x4_cos",
            instr: exec::f32x4_cos,
        }),
        0x22 => visitor.visit_v128_un_op(V128UnOpInfo {
            _name: "f32x4_tan",
            instr: exec::f32x4_tan,
        }),
        0x23 => visitor.visit_v128_un_op(V128UnOpInfo {
            _name: "f32x4_asin",
            instr: exec::f32x4_asin,
        }),
        0x24 => visitor.visit_v128_un_op(V128UnOpInfo {
            _name: "f32x4_acos",
            instr: exec::f32x4_acos,
        }),
        0x25 => visitor.visit_v128_un_op(V128UnOpInfo {
            _name: "f32x4_atan",
            instr: exec::f32x4_atan,
        }),
        0x26 => visitor.visit_v128_un_op(V128UnOpInfo {
            _name: "f32x4_exp",
            instr: exec::f32x4_exp,
        }),
        0x27 => visitor.visit_v128_un_op(V128UnOpInfo {
            _name: "f32x4_ln",
            instr: exec::f32x4_ln,
        }),
        0x28 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_atan2",
            instr: exec::f32x4_atan2,
        }),
        0x29 => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_pow",
            instr: exec::f32x4_pow,
        }),
        0x2A => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_rmin",
            instr: exec::f32x4_rmin,
        }),
        0x2B => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_rmax",
            instr: exec::f32x4_rmax,
        }),
        0x2C => visitor.visit_v128_bin_op(V128BinOpInfo {
            _name: "f32x4_rem",
            instr: exec::f32x4_rem,
        }),
        // Packed reductions: left-associated f32 dot product over the
        // first 2/3/4 lanes.
        0x2D => visitor.visit_v128_reduce_op(V128ReduceOpInfo {
            _name: "f32x4_dot2",
            instr: exec::f32x4_dot2,
        }),
        0x2E => visitor.visit_v128_reduce_op(V128ReduceOpInfo {
            _name: "f32x4_dot3",
            instr: exec::f32x4_dot3,
        }),
        0x2F => visitor.visit_v128_reduce_op(V128ReduceOpInfo {
            _name: "f32x4_dot4",
            instr: exec::f32x4_dot4,
        }),
        _ => Err(DecodeError::new("illegal opcode"))?,
    }
}
