use {
    crate::{
        aliasable_box::AliasableBox,
        config::Extensions,
        decode::{Decode, DecodeError, Decoder},
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
    pub(crate) expr: Arc<[u8]>,
}

impl Decode for UncompiledCode {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        use std::iter;

        let mut code_decoder = decoder.decode_decoder()?;
        Ok(Self {
            locals: {
                let mut locals = Vec::new();
                for _ in 0u32..code_decoder.decode()? {
                    let count = code_decoder.decode()?;
                    if count > usize::try_from(u32::MAX).unwrap() - locals.len() {
                        return Err(DecodeError::new("too many locals"));
                    }
                    locals.extend(iter::repeat(code_decoder.decode::<ValType>()?).take(count));
                }
                locals.into()
            },
            expr: code_decoder.read_bytes_until_end().into(),
        })
    }
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

impl Decode for BlockType {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        fn decode_i33_tail(decoder: &mut Decoder<'_>, mut value: i64) -> Result<i64, DecodeError> {
            let mut shift = 0;
            loop {
                let byte = decoder.read_byte()?;
                if shift >= 26 && byte >> 33 - shift != 0 {
                    let sign = (byte << 1) as i8 >> (33 - shift);
                    if byte & 0x80 != 0x00 || sign != 0 && sign != -1 {
                        return Err(DecodeError::new("malformed s33"));
                    }
                }
                value |= ((byte & 0x7F) as i64) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }
            let shift = 58 - shift.min(26);
            Ok(value << shift >> shift)
        }

        match decoder.read_byte()? {
            0x40 => Ok(BlockType::ValType(None)),
            0x7F => Ok(BlockType::ValType(Some(ValType::I32))),
            0x7E => Ok(BlockType::ValType(Some(ValType::I64))),
            0x7D => Ok(BlockType::ValType(Some(ValType::F32))),
            0x7C => Ok(BlockType::ValType(Some(ValType::F64))),
            0x7B => Ok(BlockType::ValType(Some(ValType::V128))),
            0x70 => Ok(BlockType::ValType(Some(ValType::FuncRef))),
            0x6F => Ok(BlockType::ValType(Some(ValType::ExternRef))),
            byte => {
                let value = (byte & 0x7F) as i64;
                let value = if byte & 0x80 == 0x00 {
                    value
                } else {
                    decode_i33_tail(decoder, value)?
                };
                if value < 0 {
                    return Err(DecodeError::new(""));
                }
                Ok(BlockType::TypeIdx(value as u32))
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MemArg {
    pub(crate) align: u32,
    pub(crate) offset: u32,
}

impl Decode for MemArg {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            align: decoder.decode()?,
            offset: decoder.decode()?,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LoadInfo {
    pub(crate) max_align: u32,
    pub(crate) op: UnOpInfo,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct StoreInfo {
    pub(crate) max_align: u32,
    pub(crate) op: BinOpInfo,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UnOpInfo {
    pub(crate) _name: &'static str,
    pub(crate) input_type: ValType,
    pub(crate) output_type: Option<ValType>,
    pub(crate) instr_s: ThreadedInstr,
    pub(crate) instr_r: ThreadedInstr,
    pub(crate) instr_i: Option<ThreadedInstr>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BinOpInfo {
    pub(crate) _name: &'static str,
    pub(crate) input_type_0: ValType,
    pub(crate) input_type_1: ValType,
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

pub(crate) fn decode_instr<V>(
    decoder: &mut Decoder<'_>,
    label_idxs: &mut Vec<u32>,
    visitor: &mut V,
    exts: Extensions,
) -> Result<(), V::Error>
where
    V: InstrVisitor,
    V::Error: From<DecodeError>,
{
    match decoder.read_byte()? {
        0x00 => visitor.visit_unreachable(),
        0x01 => visitor.visit_nop(),
        0x02 => visitor.visit_block(decoder.decode()?),
        0x03 => visitor.visit_loop(decoder.decode()?),
        0x04 => visitor.visit_if(decoder.decode()?),
        0x05 => visitor.visit_else(),
        0x0B => visitor.visit_end(),
        0x0C => visitor.visit_br(decoder.decode()?),
        0x0D => visitor.visit_br_if(decoder.decode()?),
        0x0E => {
            label_idxs.clear();
            for label_idx in decoder.decode_iter()? {
                label_idxs.push(label_idx?);
            }
            visitor.visit_br_table(&label_idxs, decoder.decode()?)?;
            Ok(())
        }
        0x0F => visitor.visit_return(),
        0x10 => visitor.visit_call(decoder.decode()?),
        0x11 => {
            let type_idx = decoder.decode()?;
            let table_idx = decoder.decode()?;
            visitor.visit_call_indirect(table_idx, type_idx)
        }
        0x1A => visitor.visit_drop(),
        0x1B => visitor.visit_select(None),
        0x1C => {
            if decoder.decode::<u32>()? != 1 {
                return Err(DecodeError::new(""))?;
            }
            visitor.visit_select(Some(decoder.decode()?))?;
            Ok(())
        }
        0x20 => visitor.visit_local_get(decoder.decode()?),
        0x21 => visitor.visit_local_set(decoder.decode()?),
        0x22 => visitor.visit_local_tee(decoder.decode()?),
        0x23 => visitor.visit_global_get(decoder.decode()?),
        0x24 => visitor.visit_global_set(decoder.decode()?),
        0x25 => visitor.visit_table_get(decoder.decode()?),
        0x26 => visitor.visit_table_set(decoder.decode()?),
        0x28 => visitor.visit_load(
            decoder.decode()?,
            LoadInfo {
                max_align: 2,
                op: UnOpInfo {
                    _name: "i32_load",
                    input_type: ValType::I32,
                    output_type: Some(ValType::I32),
                    instr_s: exec::i32_load_s,
                    instr_r: exec::i32_load_r,
                    instr_i: Some(exec::i32_load_i),
                },
            },
        ),
        0x29 => visitor.visit_load(
            decoder.decode()?,
            LoadInfo {
                max_align: 3,
                op: UnOpInfo {
                    _name: "i64_load",
                    input_type: ValType::I32,
                    output_type: Some(ValType::I64),
                    instr_s: exec::i64_load_s,
                    instr_r: exec::i64_load_r,
                    instr_i: Some(exec::i64_load_i),
                },
            },
        ),
        0x2A => visitor.visit_load(
            decoder.decode()?,
            LoadInfo {
                max_align: 2,
                op: UnOpInfo {
                    _name: "f32_load",
                    input_type: ValType::I32,
                    output_type: Some(ValType::F32),
                    instr_s: exec::f32_load_s,
                    instr_r: exec::f32_load_r,
                    instr_i: Some(exec::f32_load_i),
                },
            },
        ),
        0x2B => visitor.visit_load(
            decoder.decode()?,
            LoadInfo {
                max_align: 3,
                op: UnOpInfo {
                    _name: "f64_load",
                    input_type: ValType::I32,
                    output_type: Some(ValType::F64),
                    instr_s: exec::f64_load_s,
                    instr_r: exec::f64_load_r,
                    instr_i: Some(exec::f64_load_i),
                },
            },
        ),
        0x2C => visitor.visit_load(
            decoder.decode()?,
            LoadInfo {
                max_align: 0,
                op: UnOpInfo {
                    _name: "i32_load8_s",
                    input_type: ValType::I32,
                    output_type: Some(ValType::I32),
                    instr_s: exec::i32_load8_s_s,
                    instr_r: exec::i32_load8_s_r,
                    instr_i: Some(exec::i32_load8_s_i),
                },
            },
        ),
        0x2D => visitor.visit_load(
            decoder.decode()?,
            LoadInfo {
                max_align: 0,
                op: UnOpInfo {
                    _name: "i32_load8_u",
                    input_type: ValType::I32,
                    output_type: Some(ValType::I32),
                    instr_s: exec::i32_load8_u_s,
                    instr_r: exec::i32_load8_u_r,
                    instr_i: Some(exec::i32_load8_u_i),
                },
            },
        ),
        0x2E => visitor.visit_load(
            decoder.decode()?,
            LoadInfo {
                max_align: 1,
                op: UnOpInfo {
                    _name: "i32_load16_s",
                    input_type: ValType::I32,
                    output_type: Some(ValType::I32),
                    instr_s: exec::i32_load16_s_s,
                    instr_r: exec::i32_load16_s_r,
                    instr_i: Some(exec::i32_load16_s_i),
                },
            },
        ),
        0x2F => visitor.visit_load(
            decoder.decode()?,
            LoadInfo {
                max_align: 1,
                op: UnOpInfo {
                    _name: "i32_load16_u",
                    input_type: ValType::I32,
                    output_type: Some(ValType::I32),
                    instr_s: exec::i32_load16_u_s,
                    instr_r: exec::i32_load16_u_r,
                    instr_i: Some(exec::i32_load16_u_i),
                },
            },
        ),
        0x30 => visitor.visit_load(
            decoder.decode()?,
            LoadInfo {
                max_align: 0,
                op: UnOpInfo {
                    _name: "i64_load8_s",
                    input_type: ValType::I32,
                    output_type: Some(ValType::I64),
                    instr_s: exec::i64_load8_s_s,
                    instr_r: exec::i64_load8_s_r,
                    instr_i: Some(exec::i64_load8_s_i),
                },
            },
        ),
        0x31 => visitor.visit_load(
            decoder.decode()?,
            LoadInfo {
                max_align: 0,
                op: UnOpInfo {
                    _name: "i64_load8_u",
                    input_type: ValType::I32,
                    output_type: Some(ValType::I64),
                    instr_s: exec::i64_load8_u_s,
                    instr_r: exec::i64_load8_u_r,
                    instr_i: Some(exec::i64_load8_u_i),
                },
            },
        ),
        0x32 => visitor.visit_load(
            decoder.decode()?,
            LoadInfo {
                max_align: 1,
                op: UnOpInfo {
                    _name: "i64_load16_s",
                    input_type: ValType::I32,
                    output_type: Some(ValType::I64),
                    instr_s: exec::i64_load16_s_s,
                    instr_r: exec::i64_load16_s_r,
                    instr_i: Some(exec::i64_load16_s_i),
                },
            },
        ),
        0x33 => visitor.visit_load(
            decoder.decode()?,
            LoadInfo {
                max_align: 1,
                op: UnOpInfo {
                    _name: "i64_load16_u",
                    input_type: ValType::I32,
                    output_type: Some(ValType::I64),
                    instr_s: exec::i64_load16_u_s,
                    instr_r: exec::i64_load16_u_r,
                    instr_i: Some(exec::i64_load16_u_i),
                },
            },
        ),
        0x34 => visitor.visit_load(
            decoder.decode()?,
            LoadInfo {
                max_align: 2,
                op: UnOpInfo {
                    _name: "i64_load32_s",
                    input_type: ValType::I32,
                    output_type: Some(ValType::I64),
                    instr_s: exec::i64_load32_s_s,
                    instr_r: exec::i64_load32_s_r,
                    instr_i: Some(exec::i64_load32_s_i),
                },
            },
        ),
        0x35 => visitor.visit_load(
            decoder.decode()?,
            LoadInfo {
                max_align: 2,
                op: UnOpInfo {
                    _name: "i64_load32_u",
                    input_type: ValType::I32,
                    output_type: Some(ValType::I64),
                    instr_s: exec::i64_load32_u_s,
                    instr_r: exec::i64_load32_u_r,
                    instr_i: Some(exec::i64_load32_u_i),
                },
            },
        ),
        0x36 => visitor.visit_store(
            decoder.decode()?,
            StoreInfo {
                max_align: 2,
                op: BinOpInfo {
                    _name: "i32_store",
                    input_type_0: ValType::I32,
                    input_type_1: ValType::I32,
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
        0x37 => visitor.visit_store(
            decoder.decode()?,
            StoreInfo {
                max_align: 3,
                op: BinOpInfo {
                    _name: "i64_store",
                    input_type_0: ValType::I32,
                    input_type_1: ValType::I64,
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
        0x38 => visitor.visit_store(
            decoder.decode()?,
            StoreInfo {
                max_align: 2,
                op: BinOpInfo {
                    _name: "f32_store",
                    input_type_0: ValType::I32,
                    input_type_1: ValType::F32,
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
        0x39 => visitor.visit_store(
            decoder.decode()?,
            StoreInfo {
                max_align: 3,
                op: BinOpInfo {
                    _name: "f64_store",
                    input_type_0: ValType::I32,
                    input_type_1: ValType::F64,
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
        0x3A => visitor.visit_store(
            decoder.decode()?,
            StoreInfo {
                max_align: 0,
                op: BinOpInfo {
                    _name: "i32_store8",
                    input_type_0: ValType::I32,
                    input_type_1: ValType::I32,
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
        0x3B => visitor.visit_store(
            decoder.decode()?,
            StoreInfo {
                max_align: 1,
                op: BinOpInfo {
                    _name: "i32_store16",
                    input_type_0: ValType::I32,
                    input_type_1: ValType::I32,
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
        0x3C => visitor.visit_store(
            decoder.decode()?,
            StoreInfo {
                max_align: 0,
                op: BinOpInfo {
                    _name: "i64_store8",
                    input_type_0: ValType::I32,
                    input_type_1: ValType::I64,
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
        0x3D => visitor.visit_store(
            decoder.decode()?,
            StoreInfo {
                max_align: 1,
                op: BinOpInfo {
                    _name: "i64_store16",
                    input_type_0: ValType::I32,
                    input_type_1: ValType::I64,
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
        0x3E => visitor.visit_store(
            decoder.decode()?,
            StoreInfo {
                max_align: 2,
                op: BinOpInfo {
                    _name: "i64_store32",
                    input_type_0: ValType::I32,
                    input_type_1: ValType::I64,
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
        0x3F => {
            if decoder.read_byte()? != 0x00 {
                return Err(DecodeError::new("expected zero byte"))?;
            }
            visitor.visit_memory_size()
        }
        0x40 => {
            if decoder.read_byte()? != 0x00 {
                return Err(DecodeError::new("expected zero byte"))?;
            }
            visitor.visit_memory_grow()
        }
        0x41 => visitor.visit_i32_const(decoder.decode()?),
        0x42 => visitor.visit_i64_const(decoder.decode()?),
        0x43 => visitor.visit_f32_const(decoder.decode()?),
        0x44 => visitor.visit_f64_const(decoder.decode()?),
        0x45 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_eqz",
            input_type: ValType::I32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_eqz_s,
            instr_r: exec::i32_eqz_r,
            instr_i: None,
        }),
        0x46 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_eq",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x47 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_ne",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x48 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_lt_s",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x49 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_lt_u",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x4A => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_gt_s",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x4B => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_gt_u",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x4C => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_le_s",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x4D => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_le_u",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x4E => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_ge_s",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x4F => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_ge_u",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x50 => visitor.visit_un_op(UnOpInfo {
            _name: "i64_eqz",
            input_type: ValType::I64,
            output_type: Some(ValType::I32),
            instr_s: exec::i64_eqz_s,
            instr_r: exec::i64_eqz_r,
            instr_i: None,
        }),
        0x51 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_eq",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x52 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_ne",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x53 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_lt_s",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x54 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_lt_u",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x55 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_gt_s",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x56 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_gt_u",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x57 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_le_s",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x58 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_le_u",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x59 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_ge_s",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x5A => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_ge_u",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x5B => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_eq",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
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
        0x5C => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_ne",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
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
        0x5D => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_lt",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
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
        0x5E => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_gt",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
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
        0x5F => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_le",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
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
        0x60 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_ge",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
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
        0x61 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_eq",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
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
        0x62 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_ne",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
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
        0x63 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_lt",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
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
        0x64 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_gt",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
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
        0x65 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_le",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
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
        0x66 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_ge",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
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
        0x67 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_clz",
            input_type: ValType::I32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_clz_s,
            instr_r: exec::i32_clz_r,
            instr_i: None,
        }),
        0x68 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_ctz",
            input_type: ValType::I32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_ctz_s,
            instr_r: exec::i32_ctz_r,
            instr_i: None,
        }),
        0x69 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_popcnt",
            input_type: ValType::I32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_popcnt_s,
            instr_r: exec::i32_popcnt_r,
            instr_i: None,
        }),
        0x6A => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_add",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x6B => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_sub",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x6C => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_mul",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x6D => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_div_s",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x6E => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_div_u",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x6F => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_rem_s",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x70 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_rem_u",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x71 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_and",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x72 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_or",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x73 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_xor",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x74 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_shl",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x75 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_shr_s",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x76 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_shr_u",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x77 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_rotl",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x78 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_rotr",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
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
        0x79 => visitor.visit_un_op(UnOpInfo {
            _name: "i64_clz",
            input_type: ValType::I64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_clz_s,
            instr_r: exec::i64_clz_r,
            instr_i: None,
        }),
        0x7A => visitor.visit_un_op(UnOpInfo {
            _name: "i64_ctz",
            input_type: ValType::I64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_ctz_s,
            instr_r: exec::i64_ctz_r,
            instr_i: None,
        }),
        0x7B => visitor.visit_un_op(UnOpInfo {
            _name: "i64_popcnt",
            input_type: ValType::I64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_popcnt_s,
            instr_r: exec::i64_popcnt_r,
            instr_i: None,
        }),
        0x7C => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_add",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x7D => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_sub",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x7E => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_mul",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x7F => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_div_s",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x80 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_div_u",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x81 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_rem_s",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x82 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_rem_u",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x83 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_and",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x84 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_or",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x85 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_xor",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x86 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_shl",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x87 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_shr_s",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x88 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_shr_u",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x89 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_rotl",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x8A => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_rotr",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
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
        0x8B => visitor.visit_un_op(UnOpInfo {
            _name: "f32_abs",
            input_type: ValType::F32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_abs_s,
            instr_r: exec::f32_abs_r,
            instr_i: None,
        }),
        0x8C => visitor.visit_un_op(UnOpInfo {
            _name: "f32_neg",
            input_type: ValType::F32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_neg_s,
            instr_r: exec::f32_neg_r,
            instr_i: None,
        }),
        0x8D => visitor.visit_un_op(UnOpInfo {
            _name: "f32_ceil",
            input_type: ValType::F32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_ceil_s,
            instr_r: exec::f32_ceil_r,
            instr_i: None,
        }),
        0x8E => visitor.visit_un_op(UnOpInfo {
            _name: "f32_floor",
            input_type: ValType::F32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_floor_s,
            instr_r: exec::f32_floor_r,
            instr_i: None,
        }),
        0x8F => visitor.visit_un_op(UnOpInfo {
            _name: "f32_trunc",
            input_type: ValType::F32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_trunc_s,
            instr_r: exec::f32_trunc_r,
            instr_i: None,
        }),
        0x90 => visitor.visit_un_op(UnOpInfo {
            _name: "f32_nearest",
            input_type: ValType::F32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_nearest_s,
            instr_r: exec::f32_nearest_r,
            instr_i: None,
        }),
        0x91 => visitor.visit_un_op(UnOpInfo {
            _name: "f32_sqrt",
            input_type: ValType::F32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_sqrt_s,
            instr_r: exec::f32_sqrt_r,
            instr_i: None,
        }),
        0x92 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_add",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
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
        0x93 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_sub",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
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
        0x94 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_mul",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
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
        0x95 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_div",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
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
        0x96 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_min",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
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
        0x97 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_max",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
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
        0x98 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_copysign",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
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
        0x99 => visitor.visit_un_op(UnOpInfo {
            _name: "f64_abs",
            input_type: ValType::F64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_abs_s,
            instr_r: exec::f64_abs_r,
            instr_i: None,
        }),
        0x9A => visitor.visit_un_op(UnOpInfo {
            _name: "f64_neg",
            input_type: ValType::F64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_neg_s,
            instr_r: exec::f64_neg_r,
            instr_i: None,
        }),
        0x9B => visitor.visit_un_op(UnOpInfo {
            _name: "f64_ceil",
            input_type: ValType::F64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_ceil_s,
            instr_r: exec::f64_ceil_r,
            instr_i: None,
        }),
        0x9C => visitor.visit_un_op(UnOpInfo {
            _name: "f64_floor",
            input_type: ValType::F64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_floor_s,
            instr_r: exec::f64_floor_r,
            instr_i: None,
        }),
        0x9D => visitor.visit_un_op(UnOpInfo {
            _name: "f64_trunc",
            input_type: ValType::F64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_trunc_s,
            instr_r: exec::f64_trunc_r,
            instr_i: None,
        }),
        0x9E => visitor.visit_un_op(UnOpInfo {
            _name: "f64_nearest",
            input_type: ValType::F64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_nearest_s,
            instr_r: exec::f64_nearest_r,
            instr_i: None,
        }),
        0x9F => visitor.visit_un_op(UnOpInfo {
            _name: "f64_sqrt",
            input_type: ValType::F64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_sqrt_s,
            instr_r: exec::f64_sqrt_r,
            instr_i: None,
        }),
        0xA0 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_add",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
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
        0xA1 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_sub",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
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
        0xA2 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_mul",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
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
        0xA3 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_div",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
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
        0xA4 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_min",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
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
        0xA5 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_max",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
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
        0xA6 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_copysign",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
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
        0xA7 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_wrap_i64",
            input_type: ValType::I64,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_wrap_i64_s,
            instr_r: exec::i32_wrap_i64_r,
            instr_i: None,
        }),
        0xA8 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_trunc_f32_s",
            input_type: ValType::F32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_trunc_f32_s_s,
            instr_r: exec::i32_trunc_f32_s_r,
            instr_i: None,
        }),
        0xA9 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_trunc_f32_u",
            input_type: ValType::F32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_trunc_f32_u_s,
            instr_r: exec::i32_trunc_f32_u_r,
            instr_i: None,
        }),
        0xAA => visitor.visit_un_op(UnOpInfo {
            _name: "i32_trunc_f64_s",
            input_type: ValType::F64,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_trunc_f64_s_s,
            instr_r: exec::i32_trunc_f64_s_r,
            instr_i: None,
        }),
        0xAB => visitor.visit_un_op(UnOpInfo {
            _name: "i32_trunc_f64_u",
            input_type: ValType::F64,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_trunc_f64_u_s,
            instr_r: exec::i32_trunc_f64_u_r,
            instr_i: None,
        }),
        0xAC => visitor.visit_un_op(UnOpInfo {
            _name: "i64_extend_i32_s",
            input_type: ValType::I32,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_extend_i32_s_s,
            instr_r: exec::i64_extend_i32_s_r,
            instr_i: None,
        }),
        0xAD => visitor.visit_un_op(UnOpInfo {
            _name: "i64_extend_i32_u",
            input_type: ValType::I32,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_extend_i32_u_s,
            instr_r: exec::i64_extend_i32_u_r,
            instr_i: None,
        }),
        0xAE => visitor.visit_un_op(UnOpInfo {
            _name: "i64_trunc_f32_s",
            input_type: ValType::F32,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_trunc_f32_s_s,
            instr_r: exec::i64_trunc_f32_s_r,
            instr_i: None,
        }),
        0xAF => visitor.visit_un_op(UnOpInfo {
            _name: "i64_trunc_f32_u",
            input_type: ValType::F32,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_trunc_f32_u_s,
            instr_r: exec::i64_trunc_f32_u_r,
            instr_i: None,
        }),
        0xB0 => visitor.visit_un_op(UnOpInfo {
            _name: "i64_trunc_f64_s",
            input_type: ValType::F64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_trunc_f64_s_s,
            instr_r: exec::i64_trunc_f64_s_r,
            instr_i: None,
        }),
        0xB1 => visitor.visit_un_op(UnOpInfo {
            _name: "i64_trunc_f64_u",
            input_type: ValType::F64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_trunc_f64_u_s,
            instr_r: exec::i64_trunc_f64_u_r,
            instr_i: None,
        }),
        0xB2 => visitor.visit_un_op(UnOpInfo {
            _name: "f32_convert_i32_s",
            input_type: ValType::I32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_convert_i32_s_s,
            instr_r: exec::f32_convert_i32_s_r,
            instr_i: None,
        }),
        0xB3 => visitor.visit_un_op(UnOpInfo {
            _name: "f32_convert_i32_u",
            input_type: ValType::I32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_convert_i32_u_s,
            instr_r: exec::f32_convert_i32_u_r,
            instr_i: None,
        }),
        0xB4 => visitor.visit_un_op(UnOpInfo {
            _name: "f32_convert_i64_s",
            input_type: ValType::I64,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_convert_i64_s_s,
            instr_r: exec::f32_convert_i64_s_r,
            instr_i: None,
        }),
        0xB5 => visitor.visit_un_op(UnOpInfo {
            _name: "f32_convert_i64_u",
            input_type: ValType::I64,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_convert_i64_u_s,
            instr_r: exec::f32_convert_i64_u_r,
            instr_i: None,
        }),
        0xB6 => visitor.visit_un_op(UnOpInfo {
            _name: "f32_demote_f64",
            input_type: ValType::F64,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_demote_f64_s,
            instr_r: exec::f32_demote_f64_r,
            instr_i: None,
        }),
        0xB7 => visitor.visit_un_op(UnOpInfo {
            _name: "f64_convert_i32_s",
            input_type: ValType::I32,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_convert_i32_s_s,
            instr_r: exec::f64_convert_i32_s_r,
            instr_i: None,
        }),
        0xB8 => visitor.visit_un_op(UnOpInfo {
            _name: "f64_convert_i32_u",
            input_type: ValType::I32,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_convert_i32_u_s,
            instr_r: exec::f64_convert_i32_u_r,
            instr_i: None,
        }),
        0xB9 => visitor.visit_un_op(UnOpInfo {
            _name: "f64_convert_i64_s",
            input_type: ValType::I64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_convert_i64_s_s,
            instr_r: exec::f64_convert_i64_s_r,
            instr_i: None,
        }),
        0xBA => visitor.visit_un_op(UnOpInfo {
            _name: "f64_convert_i64_u",
            input_type: ValType::I64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_convert_i64_u_s,
            instr_r: exec::f64_convert_i64_u_r,
            instr_i: None,
        }),
        0xBB => visitor.visit_un_op(UnOpInfo {
            _name: "f64_promote_f32",
            input_type: ValType::F32,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_promote_f32_s,
            instr_r: exec::f64_promote_f32_r,
            instr_i: None,
        }),
        0xBC => visitor.visit_un_op(UnOpInfo {
            _name: "i32_reinterpret_f32",
            input_type: ValType::F32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_reinterpret_f32_s,
            instr_r: exec::i32_reinterpret_f32_r,
            instr_i: None,
        }),
        0xBD => visitor.visit_un_op(UnOpInfo {
            _name: "i64_reinterpret_f64",
            input_type: ValType::F64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_reinterpret_f64_s,
            instr_r: exec::i64_reinterpret_f64_r,
            instr_i: None,
        }),
        0xBE => visitor.visit_un_op(UnOpInfo {
            _name: "f32_reinterpret_i32",
            input_type: ValType::I32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_reinterpret_i32_s,
            instr_r: exec::f32_reinterpret_i32_r,
            instr_i: None,
        }),
        0xBF => visitor.visit_un_op(UnOpInfo {
            _name: "f64_reinterpret_i64",
            input_type: ValType::I64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_reinterpret_i64_s,
            instr_r: exec::f64_reinterpret_i64_r,
            instr_i: None,
        }),
        0xC0 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_extend8_s",
            input_type: ValType::I32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_extend8_s_s,
            instr_r: exec::i32_extend8_s_r,
            instr_i: None,
        }),
        0xC1 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_extend16_s",
            input_type: ValType::I32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_extend16_s_s,
            instr_r: exec::i32_extend16_s_r,
            instr_i: None,
        }),
        0xC2 => visitor.visit_un_op(UnOpInfo {
            _name: "i64_extend8_s",
            input_type: ValType::I64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_extend8_s_s,
            instr_r: exec::i64_extend8_s_r,
            instr_i: None,
        }),
        0xC3 => visitor.visit_un_op(UnOpInfo {
            _name: "i64_extend16_s",
            input_type: ValType::I64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_extend16_s_s,
            instr_r: exec::i64_extend16_s_r,
            instr_i: None,
        }),
        0xC4 => visitor.visit_un_op(UnOpInfo {
            _name: "i64_extend32_s",
            input_type: ValType::I64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_extend32_s_s,
            instr_r: exec::i64_extend32_s_r,
            instr_i: None,
        }),
        0xD0 => visitor.visit_ref_null(decoder.decode()?),
        0xD1 => visitor.visit_ref_is_null(),
        0xD2 => visitor.visit_ref_func(decoder.decode()?),
        0xFC => match decoder.decode::<u32>()? {
            0 => visitor.visit_un_op(UnOpInfo {
                _name: "i32_trunc_sat_f32_s",
                input_type: ValType::F32,
                output_type: Some(ValType::I32),
                instr_s: exec::i32_trunc_sat_f32_s_s,
                instr_r: exec::i32_trunc_sat_f32_s_r,
                instr_i: None,
            }),
            1 => visitor.visit_un_op(UnOpInfo {
                _name: "i32_trunc_sat_f32_u",
                input_type: ValType::F32,
                output_type: Some(ValType::I32),
                instr_s: exec::i32_trunc_sat_f32_u_s,
                instr_r: exec::i32_trunc_sat_f32_u_r,
                instr_i: None,
            }),
            2 => visitor.visit_un_op(UnOpInfo {
                _name: "i32_trunc_sat_f64_s",
                input_type: ValType::F64,
                output_type: Some(ValType::I32),
                instr_s: exec::i32_trunc_sat_f64_s_s,
                instr_r: exec::i32_trunc_sat_f64_s_r,
                instr_i: None,
            }),
            3 => visitor.visit_un_op(UnOpInfo {
                _name: "i32_trunc_sat_f64_u",
                input_type: ValType::F64,
                output_type: Some(ValType::I32),
                instr_s: exec::i32_trunc_sat_f64_u_s,
                instr_r: exec::i32_trunc_sat_f64_u_r,
                instr_i: None,
            }),
            4 => visitor.visit_un_op(UnOpInfo {
                _name: "i64_trunc_sat_f32_s",
                input_type: ValType::F32,
                output_type: Some(ValType::I64),
                instr_s: exec::i64_trunc_sat_f32_s_s,
                instr_r: exec::i64_trunc_sat_f32_s_r,
                instr_i: None,
            }),
            5 => visitor.visit_un_op(UnOpInfo {
                _name: "i64_trunc_sat_f32_u",
                input_type: ValType::F32,
                output_type: Some(ValType::I64),
                instr_s: exec::i64_trunc_sat_f32_u_s,
                instr_r: exec::i64_trunc_sat_f32_u_r,
                instr_i: None,
            }),
            6 => visitor.visit_un_op(UnOpInfo {
                _name: "i64_trunc_sat_f64_s",
                input_type: ValType::F64,
                output_type: Some(ValType::I64),
                instr_s: exec::i64_trunc_sat_f64_s_s,
                instr_r: exec::i64_trunc_sat_f64_s_r,
                instr_i: None,
            }),
            7 => visitor.visit_un_op(UnOpInfo {
                _name: "i64_trunc_sat_f64_u",
                input_type: ValType::F64,
                output_type: Some(ValType::I64),
                instr_s: exec::i64_trunc_sat_f64_u_s,
                instr_r: exec::i64_trunc_sat_f64_u_r,
                instr_i: None,
            }),
            8 => {
                let data_idx = decoder.decode()?;
                if decoder.read_byte()? != 0x00 {
                    return Err(DecodeError::new("expected zero byte"))?;
                }
                visitor.visit_memory_init(data_idx)
            }
            9 => visitor.visit_data_drop(decoder.decode()?),
            10 => {
                if decoder.read_byte()? != 0x00 {
                    return Err(DecodeError::new("expected zero byte"))?;
                }
                if decoder.read_byte()? != 0x00 {
                    return Err(DecodeError::new("expected zero byte"))?;
                }
                visitor.visit_memory_copy()
            }
            11 => {
                if decoder.read_byte()? != 0x00 {
                    return Err(DecodeError::new("expected zero byte"))?;
                }
                visitor.visit_memory_fill()
            }
            12 => {
                let elem_idx = decoder.decode()?;
                let table_idx = decoder.decode()?;
                visitor.visit_table_init(table_idx, elem_idx)
            }
            13 => visitor.visit_elem_drop(decoder.decode()?),
            14 => visitor.visit_table_copy(decoder.decode()?, decoder.decode()?),
            15 => visitor.visit_table_grow(decoder.decode()?),
            16 => visitor.visit_table_size(decoder.decode()?),
            17 => visitor.visit_table_fill(decoder.decode()?),
            _ => Err(DecodeError::new("illegal opcode"))?,
        },
        0xFD => decode_simd_instr(decoder, visitor),
        0xE0 => {
            if !exts.ext_math {
                return Err(DecodeError::new("illegal opcode"))?;
            }
            decode_ext_math_instr(decoder, visitor)
        }
        _ => Err(DecodeError::new("illegal opcode"))?,
    }
}

/// Decodes the subset of the Wasm SIMD proposal that stitch implements.
///
/// This covers everything needed for packed `f32x4` math: `v128`
/// load/store/const, `i8x16.shuffle`, `f32x4` splat/extract/replace lane,
/// the `f32x4` comparisons and arithmetic (including `pmin`/`pmax` and the
/// rounding instructions), and the `v128` bitwise instructions. All other
/// SIMD instructions are rejected with "illegal opcode", exactly as before.
fn decode_simd_instr<V>(decoder: &mut Decoder<'_>, visitor: &mut V) -> Result<(), V::Error>
where
    V: InstrVisitor,
    V::Error: From<DecodeError>,
{
    fn decode_lane_idx<const MAX: u8>(decoder: &mut Decoder<'_>) -> Result<u8, DecodeError> {
        let lane = decoder.read_byte()?;
        if lane >= MAX {
            return Err(DecodeError::new("invalid lane index"));
        }
        Ok(lane)
    }

    match decoder.decode::<u32>()? {
        // v128.load
        0 => visitor.visit_v128_load(decoder.decode()?),
        // v128.store
        11 => visitor.visit_v128_store(decoder.decode()?),
        // v128.const
        12 => {
            let bytes: [u8; 16] = decoder.read_bytes(16)?.try_into().unwrap();
            visitor.visit_v128_const(V128::from_bytes(bytes))
        }
        // i8x16.shuffle
        13 => {
            let lanes: [u8; 16] = decoder.read_bytes(16)?.try_into().unwrap();
            if lanes.iter().any(|lane| *lane >= 32) {
                return Err(DecodeError::new("invalid lane index"))?;
            }
            visitor.visit_i8x16_shuffle(lanes)
        }
        // f32x4.splat
        19 => visitor.visit_f32x4_splat(),
        // f32x4.extract_lane
        31 => visitor.visit_f32x4_extract_lane(decode_lane_idx::<4>(decoder)?),
        // f32x4.replace_lane
        32 => visitor.visit_f32x4_replace_lane(decode_lane_idx::<4>(decoder)?),
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

/// Decodes the NONSTANDARD float math opcodes (prefix byte 0xE0, enabled
/// via [`Extensions::ext_math`]).
///
/// Subopcode layout (one byte):
/// - 0x00..=0x0C: scalar f32 sin, cos, tan, asin, acos, atan, exp, ln,
///   atan2, pow, rmin, rmax, rem
/// - 0x10..=0x1C: scalar f64, same order
/// - 0x20..=0x2C: packed f32x4, same order
/// - 0x2D..=0x2F: packed dot-product reductions (dot2, dot3, dot4)
fn decode_ext_math_instr<V>(decoder: &mut Decoder<'_>, visitor: &mut V) -> Result<(), V::Error>
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
            input_type: type_,
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
            input_type_0: type_,
            input_type_1: type_,
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

    match decoder.read_byte()? {
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
