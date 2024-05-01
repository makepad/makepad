use crate::{
    decode::{Decode, DecodeError, Decoder},
    exec::{self, ThreadedInstr},
    ref_::RefType,
    val::ValType,
};

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
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BinOpInfo {
    pub(crate) _name: &'static str,
    pub(crate) input_type_0: ValType,
    pub(crate) input_type_1: ValType,
    pub(crate) output_type: Option<ValType>,
    pub(crate) instr_ss: ThreadedInstr,
    pub(crate) instr_rs: ThreadedInstr,
    pub(crate) instr_sr: ThreadedInstr,
    pub(crate) instr_rr: ThreadedInstr,
}

pub(crate) fn decode_instr<V>(
    decoder: &mut Decoder<'_>,
    label_idxs: &mut Vec<u32>,
    visitor: &mut V,
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
                    instr_sr: exec::i32_store_sr,
                    instr_rr: exec::unreachable,
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
                    instr_sr: exec::i64_store_sr,
                    instr_rr: exec::unreachable,
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
                    instr_sr: exec::f32_store_sr,
                    instr_rr: exec::f32_store_rr,
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
                    instr_sr: exec::f64_store_sr,
                    instr_rr: exec::f64_store_rr,
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
                    instr_sr: exec::i32_store8_sr,
                    instr_rr: exec::unreachable,
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
                    instr_sr: exec::i32_store16_sr,
                    instr_rr: exec::unreachable,
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
                    instr_sr: exec::i64_store8_sr,
                    instr_rr: exec::unreachable,
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
                    instr_sr: exec::i64_store16_sr,
                    instr_rr: exec::unreachable,
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
                    instr_sr: exec::i64_store32_sr,
                    instr_rr: exec::unreachable,
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
        }),
        0x46 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_eq",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_eq_ss,
            instr_rs: exec::i32_eq_rs,
            instr_sr: exec::i32_eq_rs,
            instr_rr: exec::unreachable,
        }),
        0x47 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_ne",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_ne_ss,
            instr_rs: exec::i32_ne_rs,
            instr_sr: exec::i32_ne_rs,
            instr_rr: exec::unreachable,
        }),
        0x48 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_lt_s",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_lt_s_ss,
            instr_rs: exec::i32_lt_s_rs,
            instr_sr: exec::i32_lt_s_sr,
            instr_rr: exec::unreachable,
        }),
        0x49 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_lt_u",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_lt_u_ss,
            instr_rs: exec::i32_lt_u_rs,
            instr_sr: exec::i32_lt_u_sr,
            instr_rr: exec::unreachable,
        }),
        0x4A => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_gt_s",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_gt_s_ss,
            instr_rs: exec::i32_gt_s_rs,
            instr_sr: exec::i32_gt_s_sr,
            instr_rr: exec::unreachable,
        }),
        0x4B => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_gt_u",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_gt_u_ss,
            instr_rs: exec::i32_gt_u_rs,
            instr_sr: exec::i32_gt_u_sr,
            instr_rr: exec::unreachable,
        }),
        0x4C => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_le_s",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_le_s_ss,
            instr_rs: exec::i32_le_s_rs,
            instr_sr: exec::i32_le_s_sr,
            instr_rr: exec::unreachable,
        }),
        0x4D => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_le_u",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_le_u_ss,
            instr_rs: exec::i32_le_u_rs,
            instr_sr: exec::i32_le_u_sr,
            instr_rr: exec::unreachable,
        }),
        0x4E => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_ge_s",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_ge_s_ss,
            instr_rs: exec::i32_ge_s_rs,
            instr_sr: exec::i32_ge_s_sr,
            instr_rr: exec::unreachable,
        }),
        0x4F => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_ge_u",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_ge_u_ss,
            instr_rs: exec::i32_ge_u_rs,
            instr_sr: exec::i32_ge_u_sr,
            instr_rr: exec::unreachable,
        }),
        0x50 => visitor.visit_un_op(UnOpInfo {
            _name: "i64_eqz",
            input_type: ValType::I64,
            output_type: Some(ValType::I32),
            instr_s: exec::i64_eqz_s,
            instr_r: exec::i64_eqz_r,
        }),
        0x51 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_eq",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_eq_ss,
            instr_rs: exec::i64_eq_rs,
            instr_sr: exec::i64_eq_rs,
            instr_rr: exec::unreachable,
        }),
        0x52 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_ne",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_ne_ss,
            instr_rs: exec::i64_ne_rs,
            instr_sr: exec::i64_ne_rs,
            instr_rr: exec::unreachable,
        }),
        0x53 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_lt_s",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_lt_s_ss,
            instr_rs: exec::i64_lt_s_rs,
            instr_sr: exec::i64_lt_s_sr,
            instr_rr: exec::unreachable,
        }),
        0x54 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_lt_u",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_lt_u_ss,
            instr_rs: exec::i64_lt_u_rs,
            instr_sr: exec::i64_lt_u_sr,
            instr_rr: exec::unreachable,
        }),
        0x55 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_gt_s",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_gt_s_ss,
            instr_rs: exec::i64_gt_s_rs,
            instr_sr: exec::i64_gt_s_sr,
            instr_rr: exec::unreachable,
        }),
        0x56 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_gt_u",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_gt_u_ss,
            instr_rs: exec::i64_gt_u_rs,
            instr_sr: exec::i64_gt_u_sr,
            instr_rr: exec::unreachable,
        }),
        0x57 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_le_s",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_le_s_ss,
            instr_rs: exec::i64_le_s_rs,
            instr_sr: exec::i64_le_s_sr,
            instr_rr: exec::unreachable,
        }),
        0x58 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_le_u",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_le_u_ss,
            instr_rs: exec::i64_le_u_rs,
            instr_sr: exec::i64_le_u_sr,
            instr_rr: exec::unreachable,
        }),
        0x59 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_ge_s",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_ge_s_ss,
            instr_rs: exec::i64_ge_s_rs,
            instr_sr: exec::i64_ge_s_sr,
            instr_rr: exec::unreachable,
        }),
        0x5A => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_ge_u",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I32),
            instr_ss: exec::i64_ge_u_ss,
            instr_rs: exec::i64_ge_u_rs,
            instr_sr: exec::i64_ge_u_sr,
            instr_rr: exec::unreachable,
        }),
        0x5B => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_eq",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
            output_type: Some(ValType::I32),
            instr_ss: exec::f32_eq_ss,
            instr_rs: exec::f32_eq_rs,
            instr_sr: exec::f32_eq_rs,
            instr_rr: exec::unreachable,
        }),
        0x5C => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_ne",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
            output_type: Some(ValType::I32),
            instr_ss: exec::f32_ne_ss,
            instr_rs: exec::f32_ne_rs,
            instr_sr: exec::f32_ne_rs,
            instr_rr: exec::unreachable,
        }),
        0x5D => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_lt",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
            output_type: Some(ValType::I32),
            instr_ss: exec::f32_lt_ss,
            instr_rs: exec::f32_lt_rs,
            instr_sr: exec::f32_lt_sr,
            instr_rr: exec::unreachable,
        }),
        0x5E => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_gt",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
            output_type: Some(ValType::I32),
            instr_ss: exec::f32_gt_ss,
            instr_rs: exec::f32_gt_rs,
            instr_sr: exec::f32_gt_sr,
            instr_rr: exec::unreachable,
        }),
        0x5F => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_le",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
            output_type: Some(ValType::I32),
            instr_ss: exec::f32_le_ss,
            instr_rs: exec::f32_le_rs,
            instr_sr: exec::f32_le_sr,
            instr_rr: exec::unreachable,
        }),
        0x60 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_ge",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
            output_type: Some(ValType::I32),
            instr_ss: exec::f32_ge_ss,
            instr_rs: exec::f32_ge_rs,
            instr_sr: exec::f32_ge_sr,
            instr_rr: exec::unreachable,
        }),
        0x61 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_eq",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
            output_type: Some(ValType::I32),
            instr_ss: exec::f64_eq_ss,
            instr_rs: exec::f64_eq_rs,
            instr_sr: exec::f64_eq_rs,
            instr_rr: exec::unreachable,
        }),
        0x62 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_ne",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
            output_type: Some(ValType::I32),
            instr_ss: exec::f64_ne_ss,
            instr_rs: exec::f64_ne_rs,
            instr_sr: exec::f64_ne_rs,
            instr_rr: exec::unreachable,
        }),
        0x63 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_lt",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
            output_type: Some(ValType::I32),
            instr_ss: exec::f64_lt_ss,
            instr_rs: exec::f64_lt_rs,
            instr_sr: exec::f64_lt_sr,
            instr_rr: exec::unreachable,
        }),
        0x64 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_gt",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
            output_type: Some(ValType::I32),
            instr_ss: exec::f64_gt_ss,
            instr_rs: exec::f64_gt_rs,
            instr_sr: exec::f64_gt_sr,
            instr_rr: exec::unreachable,
        }),
        0x65 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_le",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
            output_type: Some(ValType::I32),
            instr_ss: exec::f64_le_ss,
            instr_rs: exec::f64_le_rs,
            instr_sr: exec::f64_le_sr,
            instr_rr: exec::unreachable,
        }),
        0x66 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_ge",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
            output_type: Some(ValType::I32),
            instr_ss: exec::f64_ge_ss,
            instr_rs: exec::f64_ge_rs,
            instr_sr: exec::f64_ge_sr,
            instr_rr: exec::unreachable,
        }),
        0x67 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_clz",
            input_type: ValType::I32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_clz_s,
            instr_r: exec::i32_clz_r,
        }),
        0x68 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_ctz",
            input_type: ValType::I32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_ctz_s,
            instr_r: exec::i32_ctz_r,
        }),
        0x69 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_popcnt",
            input_type: ValType::I32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_popcnt_s,
            instr_r: exec::i32_popcnt_r,
        }),
        0x6A => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_add",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_add_ss,
            instr_rs: exec::i32_add_rs,
            instr_sr: exec::i32_add_rs,
            instr_rr: exec::unreachable,
        }),
        0x6B => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_sub",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_sub_ss,
            instr_rs: exec::i32_sub_rs,
            instr_sr: exec::i32_sub_sr,
            instr_rr: exec::unreachable,
        }),
        0x6C => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_mul",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_mul_ss,
            instr_rs: exec::i32_mul_rs,
            instr_sr: exec::i32_mul_rs,
            instr_rr: exec::unreachable,
        }),
        0x6D => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_div_s",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_div_s_ss,
            instr_rs: exec::i32_div_s_rs,
            instr_sr: exec::i32_div_s_sr,
            instr_rr: exec::unreachable,
        }),
        0x6E => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_div_u",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_div_u_ss,
            instr_rs: exec::i32_div_u_rs,
            instr_sr: exec::i32_div_u_sr,
            instr_rr: exec::unreachable,
        }),
        0x6F => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_rem_s",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_rem_s_ss,
            instr_rs: exec::i32_rem_s_rs,
            instr_sr: exec::i32_rem_s_sr,
            instr_rr: exec::unreachable,
        }),
        0x70 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_rem_u",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_rem_u_ss,
            instr_rs: exec::i32_rem_u_rs,
            instr_sr: exec::i32_rem_u_sr,
            instr_rr: exec::unreachable,
        }),
        0x71 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_and",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_and_ss,
            instr_rs: exec::i32_and_rs,
            instr_sr: exec::i32_and_rs,
            instr_rr: exec::unreachable,
        }),
        0x72 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_or",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_or_ss,
            instr_rs: exec::i32_or_rs,
            instr_sr: exec::i32_or_rs,
            instr_rr: exec::unreachable,
        }),
        0x73 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_xor",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_xor_ss,
            instr_rs: exec::i32_xor_rs,
            instr_sr: exec::i32_xor_rs,
            instr_rr: exec::unreachable,
        }),
        0x74 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_shl",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_shl_ss,
            instr_rs: exec::i32_shl_rs,
            instr_sr: exec::i32_shl_sr,
            instr_rr: exec::unreachable,
        }),
        0x75 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_shr_s",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_shr_s_ss,
            instr_rs: exec::i32_shr_s_rs,
            instr_sr: exec::i32_shr_s_sr,
            instr_rr: exec::unreachable,
        }),
        0x76 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_shr_u",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_shr_u_ss,
            instr_rs: exec::i32_shr_u_rs,
            instr_sr: exec::i32_shr_u_sr,
            instr_rr: exec::unreachable,
        }),
        0x77 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_rotl",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_rotl_ss,
            instr_rs: exec::i32_rotl_rs,
            instr_sr: exec::i32_rotl_sr,
            instr_rr: exec::unreachable,
        }),
        0x78 => visitor.visit_bin_op(BinOpInfo {
            _name: "i32_rotr",
            input_type_0: ValType::I32,
            input_type_1: ValType::I32,
            output_type: Some(ValType::I32),
            instr_ss: exec::i32_rotr_ss,
            instr_rs: exec::i32_rotr_rs,
            instr_sr: exec::i32_rotr_sr,
            instr_rr: exec::unreachable,
        }),
        0x79 => visitor.visit_un_op(UnOpInfo {
            _name: "i64_clz",
            input_type: ValType::I64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_clz_s,
            instr_r: exec::i64_clz_r,
        }),
        0x7A => visitor.visit_un_op(UnOpInfo {
            _name: "i64_ctz",
            input_type: ValType::I64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_ctz_s,
            instr_r: exec::i64_ctz_r,
        }),
        0x7B => visitor.visit_un_op(UnOpInfo {
            _name: "i64_popcnt",
            input_type: ValType::I64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_popcnt_s,
            instr_r: exec::i64_popcnt_r,
        }),
        0x7C => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_add",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_add_ss,
            instr_rs: exec::i64_add_rs,
            instr_sr: exec::i64_add_rs,
            instr_rr: exec::unreachable,
        }),
        0x7D => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_sub",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_sub_ss,
            instr_rs: exec::i64_sub_rs,
            instr_sr: exec::i64_sub_sr,
            instr_rr: exec::unreachable,
        }),
        0x7E => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_mul",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_mul_ss,
            instr_rs: exec::i64_mul_rs,
            instr_sr: exec::i64_mul_rs,
            instr_rr: exec::unreachable,
        }),
        0x7F => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_div_s",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_div_s_ss,
            instr_rs: exec::i64_div_s_rs,
            instr_sr: exec::i64_div_s_sr,
            instr_rr: exec::unreachable,
        }),
        0x80 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_div_u",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_div_u_ss,
            instr_rs: exec::i64_div_u_rs,
            instr_sr: exec::i64_div_u_sr,
            instr_rr: exec::unreachable,
        }),
        0x81 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_rem_s",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_rem_s_ss,
            instr_rs: exec::i64_rem_s_rs,
            instr_sr: exec::i64_rem_s_sr,
            instr_rr: exec::unreachable,
        }),
        0x82 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_rem_u",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_rem_u_ss,
            instr_rs: exec::i64_rem_u_rs,
            instr_sr: exec::i64_rem_u_sr,
            instr_rr: exec::unreachable,
        }),
        0x83 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_and",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_and_ss,
            instr_rs: exec::i64_and_rs,
            instr_sr: exec::i64_and_rs,
            instr_rr: exec::unreachable,
        }),
        0x84 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_or",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_or_ss,
            instr_rs: exec::i64_or_rs,
            instr_sr: exec::i64_or_rs,
            instr_rr: exec::unreachable,
        }),
        0x85 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_xor",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_xor_ss,
            instr_rs: exec::i64_xor_rs,
            instr_sr: exec::i64_xor_rs,
            instr_rr: exec::unreachable,
        }),
        0x86 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_shl",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_shl_ss,
            instr_rs: exec::i64_shl_rs,
            instr_sr: exec::i64_shl_sr,
            instr_rr: exec::unreachable,
        }),
        0x87 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_shr_s",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_shr_s_ss,
            instr_rs: exec::i64_shr_s_rs,
            instr_sr: exec::i64_shr_s_sr,
            instr_rr: exec::unreachable,
        }),
        0x88 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_shr_u",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_shr_u_ss,
            instr_rs: exec::i64_shr_u_rs,
            instr_sr: exec::i64_shr_u_sr,
            instr_rr: exec::unreachable,
        }),
        0x89 => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_rotl",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_rotl_ss,
            instr_rs: exec::i64_rotl_rs,
            instr_sr: exec::i64_rotl_sr,
            instr_rr: exec::unreachable,
        }),
        0x8A => visitor.visit_bin_op(BinOpInfo {
            _name: "i64_rotr",
            input_type_0: ValType::I64,
            input_type_1: ValType::I64,
            output_type: Some(ValType::I64),
            instr_ss: exec::i64_rotr_ss,
            instr_rs: exec::i64_rotr_rs,
            instr_sr: exec::i64_rotr_sr,
            instr_rr: exec::unreachable,
        }),
        0x8B => visitor.visit_un_op(UnOpInfo {
            _name: "f32_abs",
            input_type: ValType::F32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_abs_s,
            instr_r: exec::f32_abs_r,
        }),
        0x8C => visitor.visit_un_op(UnOpInfo {
            _name: "f32_neg",
            input_type: ValType::F32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_neg_s,
            instr_r: exec::f32_neg_r,
        }),
        0x8D => visitor.visit_un_op(UnOpInfo {
            _name: "f32_ceil",
            input_type: ValType::F32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_ceil_s,
            instr_r: exec::f32_ceil_r,
        }),
        0x8E => visitor.visit_un_op(UnOpInfo {
            _name: "f32_floor",
            input_type: ValType::F32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_floor_s,
            instr_r: exec::f32_floor_r,
        }),
        0x8F => visitor.visit_un_op(UnOpInfo {
            _name: "f32_trunc",
            input_type: ValType::F32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_trunc_s,
            instr_r: exec::f32_trunc_r,
        }),
        0x90 => visitor.visit_un_op(UnOpInfo {
            _name: "f32_nearest",
            input_type: ValType::F32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_nearest_s,
            instr_r: exec::f32_nearest_r,
        }),
        0x91 => visitor.visit_un_op(UnOpInfo {
            _name: "f32_sqrt",
            input_type: ValType::F32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_sqrt_s,
            instr_r: exec::f32_sqrt_r,
        }),
        0x92 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_add",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
            output_type: Some(ValType::F32),
            instr_ss: exec::f32_add_ss,
            instr_rs: exec::f32_add_rs,
            instr_sr: exec::f32_add_rs,
            instr_rr: exec::unreachable,
        }),
        0x93 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_sub",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
            output_type: Some(ValType::F32),
            instr_ss: exec::f32_sub_ss,
            instr_rs: exec::f32_sub_rs,
            instr_sr: exec::f32_sub_sr,
            instr_rr: exec::unreachable,
        }),
        0x94 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_mul",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
            output_type: Some(ValType::F32),
            instr_ss: exec::f32_mul_ss,
            instr_rs: exec::f32_mul_rs,
            instr_sr: exec::f32_mul_rs,
            instr_rr: exec::unreachable,
        }),
        0x95 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_div",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
            output_type: Some(ValType::F32),
            instr_ss: exec::f32_div_ss,
            instr_rs: exec::f32_div_rs,
            instr_sr: exec::f32_div_sr,
            instr_rr: exec::unreachable,
        }),
        0x96 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_min",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
            output_type: Some(ValType::F32),
            instr_ss: exec::f32_min_ss,
            instr_rs: exec::f32_min_rs,
            instr_sr: exec::f32_min_rs,
            instr_rr: exec::unreachable,
        }),
        0x97 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_max",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
            output_type: Some(ValType::F32),
            instr_ss: exec::f32_max_ss,
            instr_rs: exec::f32_max_rs,
            instr_sr: exec::f32_max_rs,
            instr_rr: exec::unreachable,
        }),
        0x98 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_copysign",
            input_type_0: ValType::F32,
            input_type_1: ValType::F32,
            output_type: Some(ValType::F32),
            instr_ss: exec::f32_copysign_ss,
            instr_rs: exec::f32_copysign_rs,
            instr_sr: exec::f32_copysign_sr,
            instr_rr: exec::unreachable,
        }),
        0x99 => visitor.visit_un_op(UnOpInfo {
            _name: "f64_abs",
            input_type: ValType::F64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_abs_s,
            instr_r: exec::f64_abs_r,
        }),
        0x9A => visitor.visit_un_op(UnOpInfo {
            _name: "f64_neg",
            input_type: ValType::F64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_neg_s,
            instr_r: exec::f64_neg_r,
        }),
        0x9B => visitor.visit_un_op(UnOpInfo {
            _name: "f64_ceil",
            input_type: ValType::F64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_ceil_s,
            instr_r: exec::f64_ceil_r,
        }),
        0x9C => visitor.visit_un_op(UnOpInfo {
            _name: "f64_floor",
            input_type: ValType::F64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_floor_s,
            instr_r: exec::f64_floor_r,
        }),
        0x9D => visitor.visit_un_op(UnOpInfo {
            _name: "f64_trunc",
            input_type: ValType::F64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_trunc_s,
            instr_r: exec::f64_trunc_r,
        }),
        0x9E => visitor.visit_un_op(UnOpInfo {
            _name: "f64_nearest",
            input_type: ValType::F64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_nearest_s,
            instr_r: exec::f64_nearest_r,
        }),
        0x9F => visitor.visit_un_op(UnOpInfo {
            _name: "f64_sqrt",
            input_type: ValType::F64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_sqrt_s,
            instr_r: exec::f64_sqrt_r,
        }),
        0xA0 => visitor.visit_bin_op(BinOpInfo {
            _name: "f32_add",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
            output_type: Some(ValType::F64),
            instr_ss: exec::f64_add_ss,
            instr_rs: exec::f64_add_rs,
            instr_sr: exec::f64_add_rs,
            instr_rr: exec::unreachable,
        }),
        0xA1 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_sub",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
            output_type: Some(ValType::F64),
            instr_ss: exec::f64_sub_ss,
            instr_rs: exec::f64_sub_rs,
            instr_sr: exec::f64_sub_sr,
            instr_rr: exec::unreachable,
        }),
        0xA2 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_mul",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
            output_type: Some(ValType::F64),
            instr_ss: exec::f64_mul_ss,
            instr_rs: exec::f64_mul_rs,
            instr_sr: exec::f64_mul_rs,
            instr_rr: exec::unreachable,
        }),
        0xA3 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_div",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
            output_type: Some(ValType::F64),
            instr_ss: exec::f64_div_ss,
            instr_rs: exec::f64_div_rs,
            instr_sr: exec::f64_div_sr,
            instr_rr: exec::unreachable,
        }),
        0xA4 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_min",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
            output_type: Some(ValType::F64),
            instr_ss: exec::f64_min_ss,
            instr_rs: exec::f64_min_rs,
            instr_sr: exec::f64_min_rs,
            instr_rr: exec::unreachable,
        }),
        0xA5 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_max",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
            output_type: Some(ValType::F64),
            instr_ss: exec::f64_max_ss,
            instr_rs: exec::f64_max_rs,
            instr_sr: exec::f64_max_rs,
            instr_rr: exec::unreachable,
        }),
        0xA6 => visitor.visit_bin_op(BinOpInfo {
            _name: "f64_copysign",
            input_type_0: ValType::F64,
            input_type_1: ValType::F64,
            output_type: Some(ValType::F64),
            instr_ss: exec::f64_copysign_ss,
            instr_rs: exec::f64_copysign_rs,
            instr_sr: exec::f64_copysign_sr,
            instr_rr: exec::unreachable,
        }),
        0xA7 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_wrap_i64",
            input_type: ValType::I64,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_wrap_i64_s,
            instr_r: exec::i32_wrap_i64_r,
        }),
        0xA8 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_trunc_f32_s",
            input_type: ValType::F32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_trunc_f32_s_s,
            instr_r: exec::i32_trunc_f32_s_r,
        }),
        0xA9 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_trunc_f32_u",
            input_type: ValType::F32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_trunc_f32_u_s,
            instr_r: exec::i32_trunc_f32_u_r,
        }),
        0xAA => visitor.visit_un_op(UnOpInfo {
            _name: "i32_trunc_f64_s",
            input_type: ValType::F64,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_trunc_f64_s_s,
            instr_r: exec::i32_trunc_f64_s_r,
        }),
        0xAB => visitor.visit_un_op(UnOpInfo {
            _name: "i32_trunc_f64_u",
            input_type: ValType::F64,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_trunc_f64_u_s,
            instr_r: exec::i32_trunc_f64_u_r,
        }),
        0xAC => visitor.visit_un_op(UnOpInfo {
            _name: "i64_extend_i32_s",
            input_type: ValType::I32,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_extend_i32_s_s,
            instr_r: exec::i64_extend_i32_s_r,
        }),
        0xAD => visitor.visit_un_op(UnOpInfo {
            _name: "i64_extend_i32_u",
            input_type: ValType::I32,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_extend_i32_u_s,
            instr_r: exec::i64_extend_i32_u_r,
        }),
        0xAE => visitor.visit_un_op(UnOpInfo {
            _name: "i64_trunc_f32_s",
            input_type: ValType::F32,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_trunc_f32_s_s,
            instr_r: exec::i64_trunc_f32_s_r,
        }),
        0xAF => visitor.visit_un_op(UnOpInfo {
            _name: "i64_trunc_f32_u",
            input_type: ValType::F32,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_trunc_f32_u_s,
            instr_r: exec::i64_trunc_f32_u_r,
        }),
        0xB0 => visitor.visit_un_op(UnOpInfo {
            _name: "i64_trunc_f64_s",
            input_type: ValType::F64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_trunc_f64_s_s,
            instr_r: exec::i64_trunc_f64_s_r,
        }),
        0xB1 => visitor.visit_un_op(UnOpInfo {
            _name: "i64_trunc_f64_u",
            input_type: ValType::F64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_trunc_f64_u_s,
            instr_r: exec::i64_trunc_f64_u_r,
        }),
        0xB2 => visitor.visit_un_op(UnOpInfo {
            _name: "f32_convert_i32_s",
            input_type: ValType::I32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_convert_i32_s_s,
            instr_r: exec::f32_convert_i32_s_r,
        }),
        0xB3 => visitor.visit_un_op(UnOpInfo {
            _name: "f32_convert_i32_u",
            input_type: ValType::I32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_convert_i32_u_s,
            instr_r: exec::f32_convert_i32_u_r,
        }),
        0xB4 => visitor.visit_un_op(UnOpInfo {
            _name: "f32_convert_i64_s",
            input_type: ValType::I64,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_convert_i64_s_s,
            instr_r: exec::f32_convert_i64_s_r,
        }),
        0xB5 => visitor.visit_un_op(UnOpInfo {
            _name: "f32_convert_i64_u",
            input_type: ValType::I64,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_convert_i64_u_s,
            instr_r: exec::f32_convert_i64_u_r,
        }),
        0xB6 => visitor.visit_un_op(UnOpInfo {
            _name: "f32_demote_f64",
            input_type: ValType::F64,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_demote_f64_s,
            instr_r: exec::f32_demote_f64_r,
        }),
        0xB7 => visitor.visit_un_op(UnOpInfo {
            _name: "f64_convert_i32_s",
            input_type: ValType::I32,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_convert_i32_s_s,
            instr_r: exec::f64_convert_i32_s_r,
        }),
        0xB8 => visitor.visit_un_op(UnOpInfo {
            _name: "f64_convert_i32_u",
            input_type: ValType::I32,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_convert_i32_u_s,
            instr_r: exec::f64_convert_i32_u_r,
        }),
        0xB9 => visitor.visit_un_op(UnOpInfo {
            _name: "f64_convert_i64_s",
            input_type: ValType::I64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_convert_i64_s_s,
            instr_r: exec::f64_convert_i64_s_r,
        }),
        0xBA => visitor.visit_un_op(UnOpInfo {
            _name: "f64_convert_i64_u",
            input_type: ValType::I64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_convert_i64_u_s,
            instr_r: exec::f64_convert_i64_u_r,
        }),
        0xBB => visitor.visit_un_op(UnOpInfo {
            _name: "f64_promote_f32",
            input_type: ValType::F32,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_promote_f32_s,
            instr_r: exec::f64_promote_f32_r,
        }),
        0xBC => visitor.visit_un_op(UnOpInfo {
            _name: "i32_reinterpret_f32",
            input_type: ValType::F32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_reinterpret_f32_s,
            instr_r: exec::i32_reinterpret_f32_r,
        }),
        0xBD => visitor.visit_un_op(UnOpInfo {
            _name: "i64_reinterpret_f64",
            input_type: ValType::F64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_reinterpret_f64_s,
            instr_r: exec::i64_reinterpret_f64_r,
        }),
        0xBE => visitor.visit_un_op(UnOpInfo {
            _name: "f32_reinterpret_i32",
            input_type: ValType::I32,
            output_type: Some(ValType::F32),
            instr_s: exec::f32_reinterpret_i32_s,
            instr_r: exec::f32_reinterpret_i32_r,
        }),
        0xBF => visitor.visit_un_op(UnOpInfo {
            _name: "f64_reinterpret_i64",
            input_type: ValType::I64,
            output_type: Some(ValType::F64),
            instr_s: exec::f64_reinterpret_i64_s,
            instr_r: exec::f64_reinterpret_i64_r,
        }),
        0xC0 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_extend8_s",
            input_type: ValType::I32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_extend8_s_s,
            instr_r: exec::i32_extend8_s_r,
        }),
        0xC1 => visitor.visit_un_op(UnOpInfo {
            _name: "i32_extend16_s",
            input_type: ValType::I32,
            output_type: Some(ValType::I32),
            instr_s: exec::i32_extend16_s_s,
            instr_r: exec::i32_extend16_s_r,
        }),
        0xC2 => visitor.visit_un_op(UnOpInfo {
            _name: "i64_extend8_s",
            input_type: ValType::I64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_extend8_s_s,
            instr_r: exec::i64_extend8_s_r,
        }),
        0xC3 => visitor.visit_un_op(UnOpInfo {
            _name: "i64_extend16_s",
            input_type: ValType::I64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_extend16_s_s,
            instr_r: exec::i64_extend16_s_r,
        }),
        0xC4 => visitor.visit_un_op(UnOpInfo {
            _name: "i64_extend32_s",
            input_type: ValType::I64,
            output_type: Some(ValType::I64),
            instr_s: exec::i64_extend32_s_s,
            instr_r: exec::i64_extend32_s_r,
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
            }),
            1 => visitor.visit_un_op(UnOpInfo {
                _name: "i32_trunc_sat_f32_u",
                input_type: ValType::F32,
                output_type: Some(ValType::I32),
                instr_s: exec::i32_trunc_sat_f32_u_s,
                instr_r: exec::i32_trunc_sat_f32_u_r,
            }),
            2 => visitor.visit_un_op(UnOpInfo {
                _name: "i32_trunc_sat_f64_s",
                input_type: ValType::F64,
                output_type: Some(ValType::I32),
                instr_s: exec::i32_trunc_sat_f64_s_s,
                instr_r: exec::i32_trunc_sat_f64_s_r,
            }),
            3 => visitor.visit_un_op(UnOpInfo {
                _name: "i32_trunc_sat_f64_u",
                input_type: ValType::F64,
                output_type: Some(ValType::I32),
                instr_s: exec::i32_trunc_sat_f64_u_s,
                instr_r: exec::i32_trunc_sat_f64_u_r,
            }),
            4 => visitor.visit_un_op(UnOpInfo {
                _name: "i64_trunc_sat_f32_s",
                input_type: ValType::F32,
                output_type: Some(ValType::I64),
                instr_s: exec::i64_trunc_sat_f32_s_s,
                instr_r: exec::i64_trunc_sat_f32_s_r,
            }),
            5 => visitor.visit_un_op(UnOpInfo {
                _name: "i64_trunc_sat_f32_u",
                input_type: ValType::F32,
                output_type: Some(ValType::I64),
                instr_s: exec::i64_trunc_sat_f32_u_s,
                instr_r: exec::i64_trunc_sat_f32_u_r,
            }),
            6 => visitor.visit_un_op(UnOpInfo {
                _name: "i64_trunc_sat_f64_s",
                input_type: ValType::F64,
                output_type: Some(ValType::I64),
                instr_s: exec::i64_trunc_sat_f64_s_s,
                instr_r: exec::i64_trunc_sat_f64_s_r,
            }),
            7 => visitor.visit_un_op(UnOpInfo {
                _name: "i64_trunc_sat_f64_u",
                input_type: ValType::F64,
                output_type: Some(ValType::I64),
                instr_s: exec::i64_trunc_sat_f64_u_s,
                instr_r: exec::i64_trunc_sat_f64_u_r,
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
        _ => Err(DecodeError::new("illegal opcode"))?,
    }
}
