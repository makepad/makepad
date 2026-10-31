//! The module validator, following the algorithm of the core spec's
//! validation appendix: an operand stack of possibly-unknown types and a
//! stack of control frames, checked instruction by instruction, with every
//! block, loop and if checked against its parameter and result types.
//! Covers everything [`binary`](crate::binary) decodes.

use crate::{
    binary::{
        ext_math_class, BlockType, DataMode, ElemItems, ElemMode, ExtMathClass, ExternKind,
        Func, FuncType, GlobalType, ImportDesc, Instr, Limits, MemArg, MemType, Module,
        TableType,
    },
    decode::DecodeError,
    val::ValType,
};

type Res<T> = Result<T, DecodeError>;

fn err<T>(msg: impl Into<Box<str>>) -> Res<T> {
    Err(DecodeError::new(msg))
}

const I32: ValType = ValType::I32;
const I64: ValType = ValType::I64;
const F32: ValType = ValType::F32;
const F64: ValType = ValType::F64;
const V128: ValType = ValType::V128;

/// One value type as a slice, for single-result blocks.
fn one(ty: ValType) -> &'static [ValType] {
    match ty {
        ValType::I32 => &[ValType::I32],
        ValType::I64 => &[ValType::I64],
        ValType::F32 => &[ValType::F32],
        ValType::F64 => &[ValType::F64],
        ValType::V128 => &[ValType::V128],
        ValType::FuncRef => &[ValType::FuncRef],
        ValType::ExternRef => &[ValType::ExternRef],
    }
}

/// What instructions may refer to, indexed the way instructions index it.
struct Ctx<'m> {
    types: &'m [FuncType],
    funcs: Vec<u32>,
    tables: Vec<TableType>,
    memories: Vec<MemType>,
    globals: Vec<GlobalType>,
    num_imported_globals: usize,
    elem_types: Vec<ValType>,
    data_count: Option<u32>,
    /// Functions a `ref.func` in a body may name: those mentioned outside
    /// function bodies.
    refs: Vec<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BlockKind {
    Block,
    Loop,
    If,
    Else,
}

#[derive(Clone, Copy, Debug)]
struct Block<'m> {
    kind: BlockKind,
    params: &'m [ValType],
    results: &'m [ValType],
    height: usize,
    is_unreachable: bool,
}

impl<'m> Block<'m> {
    fn label_types(&self) -> &'m [ValType] {
        if self.kind == BlockKind::Loop {
            self.params
        } else {
            self.results
        }
    }
}

/// An operand type; `None` is the unknown type of values below an
/// unconditional branch.
type OpdType = Option<ValType>;

struct Validation<'c, 'm> {
    ctx: &'c Ctx<'m>,
    locals: Vec<ValType>,
    results: &'m [ValType],
    opds: Vec<OpdType>,
    blocks: Vec<Block<'m>>,
    aux_opds: Vec<OpdType>,
}

impl<'c, 'm> Validation<'c, 'm> {
    fn push_opd(&mut self, ty: ValType) {
        self.opds.push(Some(ty));
    }

    fn pop_any(&mut self) -> Res<OpdType> {
        let block = self.blocks.last().unwrap();
        if self.opds.len() == block.height {
            if block.is_unreachable {
                return Ok(None);
            }
            return err("type mismatch: operand stack underflow");
        }
        Ok(self.opds.pop().unwrap())
    }

    /// Pops a value of type `expect`.
    fn pop_opd(&mut self, expect: ValType) -> Res<OpdType> {
        let actual = self.pop_any()?;
        match actual {
            Some(actual) if actual != expect => err(format!(
                "type mismatch: expected {expect}, found {actual}"
            )),
            _ => Ok(actual),
        }
    }

    fn pop_ref(&mut self) -> Res<OpdType> {
        let actual = self.pop_any()?;
        match actual {
            Some(actual) if !actual.is_ref() => err("type mismatch: expected a reference"),
            _ => Ok(actual),
        }
    }

    fn pop_opds(&mut self, types: &[ValType]) -> Res<()> {
        for ty in types.iter().rev() {
            self.pop_opd(*ty)?;
        }
        Ok(())
    }

    fn push_opds(&mut self, types: &[ValType]) {
        for ty in types {
            self.push_opd(*ty);
        }
    }

    fn push_block(&mut self, kind: BlockKind, params: &'m [ValType], results: &'m [ValType]) {
        let height = self.opds.len();
        self.push_opds(params);
        self.blocks.push(Block {
            kind,
            params,
            results,
            height,
            is_unreachable: false,
        });
    }

    fn pop_block(&mut self) -> Res<Block<'m>> {
        let Some(block) = self.blocks.last().copied() else {
            return err("unexpected end");
        };
        self.pop_opds(block.results)?;
        if self.opds.len() != block.height {
            return err("type mismatch: values remaining on stack at end of block");
        }
        self.blocks.pop();
        Ok(block)
    }

    fn label(&self, depth: u32) -> Res<&'m [ValType]> {
        let depth = depth as usize;
        if depth >= self.blocks.len() {
            return err("unknown label");
        }
        Ok(self.blocks[self.blocks.len() - 1 - depth].label_types())
    }

    fn set_unreachable(&mut self) {
        let block = self.blocks.last_mut().unwrap();
        self.opds.truncate(block.height);
        block.is_unreachable = true;
    }

    fn block_type(&self, ty: &BlockType) -> Res<(&'m [ValType], &'m [ValType])> {
        Ok(match ty {
            BlockType::Empty => (&[], &[]),
            BlockType::Value(ty) => (&[], one(*ty)),
            BlockType::Func(index) => {
                let ty = self.func_type(*index)?;
                (&ty.params, &ty.results)
            }
        })
    }

    fn func_type(&self, ty: u32) -> Res<&'m FuncType> {
        self.ctx
            .types
            .get(ty as usize)
            .ok_or_else(|| DecodeError::new("unknown type"))
    }

    fn func(&self, func: u32) -> Res<&'m FuncType> {
        let ty = *self
            .ctx
            .funcs
            .get(func as usize)
            .ok_or_else(|| DecodeError::new("unknown function"))?;
        self.func_type(ty)
    }

    fn local(&self, index: u32) -> Res<ValType> {
        self.locals
            .get(index as usize)
            .copied()
            .ok_or_else(|| DecodeError::new("unknown local"))
    }

    fn global(&self, index: u32) -> Res<GlobalType> {
        self.ctx
            .globals
            .get(index as usize)
            .copied()
            .ok_or_else(|| DecodeError::new("unknown global"))
    }

    fn table(&self, index: u32) -> Res<TableType> {
        self.ctx
            .tables
            .get(index as usize)
            .copied()
            .ok_or_else(|| DecodeError::new("unknown table"))
    }

    fn memory(&self, index: u32) -> Res<()> {
        if index as usize >= self.ctx.memories.len() {
            return err("unknown memory");
        }
        Ok(())
    }

    fn elem(&self, index: u32) -> Res<ValType> {
        self.ctx
            .elem_types
            .get(index as usize)
            .copied()
            .ok_or_else(|| DecodeError::new("unknown elem segment"))
    }

    fn data(&self, index: u32) -> Res<()> {
        match self.ctx.data_count {
            None => err("data count section required"),
            Some(count) if index >= count => err("unknown data segment"),
            _ => Ok(()),
        }
    }

    fn mem_arg(&self, arg: &MemArg, natural: u32) -> Res<()> {
        self.memory(arg.memory)?;
        if arg.align > natural {
            return err("alignment must not be larger than natural");
        }
        Ok(())
    }

    fn atomic_arg(&self, arg: &MemArg, natural: u32) -> Res<()> {
        self.memory(arg.memory)?;
        if arg.align != natural {
            return err("alignment must be natural for atomics");
        }
        Ok(())
    }

    fn un(&mut self, input: ValType, output: ValType) -> Res<()> {
        self.pop_opd(input)?;
        self.push_opd(output);
        Ok(())
    }

    fn bin(&mut self, input: ValType, output: ValType) -> Res<()> {
        self.pop_opd(input)?;
        self.pop_opd(input)?;
        self.push_opd(output);
        Ok(())
    }

    fn call(&mut self, ty: &FuncType) -> Res<()> {
        self.pop_opds(&ty.params)?;
        self.push_opds(&ty.results);
        Ok(())
    }

    fn instr(&mut self, instr: &Instr) -> Res<()> {
        match instr {
            Instr::Unreachable => self.set_unreachable(),
            Instr::Nop => {}
            Instr::Block(ty) => {
                let (params, results) = self.block_type(ty)?;
                self.pop_opds(params)?;
                self.push_block(BlockKind::Block, params, results);
            }
            Instr::Loop(ty) => {
                let (params, results) = self.block_type(ty)?;
                self.pop_opds(params)?;
                self.push_block(BlockKind::Loop, params, results);
            }
            Instr::If(ty) => {
                let (params, results) = self.block_type(ty)?;
                self.pop_opd(I32)?;
                self.pop_opds(params)?;
                self.push_block(BlockKind::If, params, results);
            }
            Instr::Else => {
                let block = self.pop_block()?;
                if block.kind != BlockKind::If {
                    return err("else without if");
                }
                self.push_block(BlockKind::Else, block.params, block.results);
            }
            Instr::End => {
                let block = self.pop_block()?;
                if block.kind == BlockKind::If && block.params != block.results {
                    return err("type mismatch: if without else");
                }
                self.push_opds(block.results);
            }
            Instr::Br(depth) => {
                let types = self.label(*depth)?;
                self.pop_opds(types)?;
                self.set_unreachable();
            }
            Instr::BrIf(depth) => {
                self.pop_opd(I32)?;
                let types = self.label(*depth)?;
                self.pop_opds(types)?;
                self.push_opds(types);
            }
            Instr::BrTable(targets, default) => {
                self.pop_opd(I32)?;
                let default_types = self.label(*default)?;
                for target in targets.iter() {
                    let types = self.label(*target)?;
                    if types.len() != default_types.len() {
                        return err("type mismatch: br_table arity");
                    }
                    let mut aux_opds = std::mem::take(&mut self.aux_opds);
                    for ty in types.iter().rev() {
                        aux_opds.push(self.pop_opd(*ty)?);
                    }
                    while let Some(opd) = aux_opds.pop() {
                        self.opds.push(opd);
                    }
                    self.aux_opds = aux_opds;
                }
                self.pop_opds(default_types)?;
                self.set_unreachable();
            }
            Instr::Return => {
                self.pop_opds(self.results)?;
                self.set_unreachable();
            }
            Instr::Call(func) => {
                let ty = self.func(*func)?;
                self.call(ty)?;
            }
            Instr::CallIndirect { ty, table } => {
                if self.table(*table)?.elem != ValType::FuncRef {
                    return err("type mismatch: call_indirect on a non-funcref table");
                }
                let ty = self.func_type(*ty)?;
                self.pop_opd(I32)?;
                self.call(ty)?;
            }
            Instr::ReturnCall(func) => {
                let ty = self.func(*func)?;
                if ty.results != self.results {
                    return err("type mismatch: return_call results");
                }
                self.pop_opds(&ty.params)?;
                self.set_unreachable();
            }
            Instr::ReturnCallIndirect { ty, table } => {
                if self.table(*table)?.elem != ValType::FuncRef {
                    return err("type mismatch: call_indirect on a non-funcref table");
                }
                let ty = self.func_type(*ty)?;
                if ty.results != self.results {
                    return err("type mismatch: return_call_indirect results");
                }
                self.pop_opd(I32)?;
                self.pop_opds(&ty.params)?;
                self.set_unreachable();
            }
            Instr::Drop => {
                self.pop_any()?;
            }
            Instr::Select => {
                self.pop_opd(I32)?;
                let t1 = self.pop_any()?;
                let t2 = self.pop_any()?;
                if t1.map_or(false, |t| t.is_ref()) || t2.map_or(false, |t| t.is_ref()) {
                    return err("type mismatch: select of references needs a type");
                }
                match (t1, t2) {
                    (Some(a), Some(b)) if a != b => return err("type mismatch: select"),
                    _ => {}
                }
                self.opds.push(t1.or(t2));
            }
            Instr::SelectT(ty) => {
                self.pop_opd(I32)?;
                self.pop_opd(*ty)?;
                self.pop_opd(*ty)?;
                self.push_opd(*ty);
            }
            Instr::LocalGet(index) => {
                let ty = self.local(*index)?;
                self.push_opd(ty);
            }
            Instr::LocalSet(index) => {
                let ty = self.local(*index)?;
                self.pop_opd(ty)?;
            }
            Instr::LocalTee(index) => {
                let ty = self.local(*index)?;
                self.pop_opd(ty)?;
                self.push_opd(ty);
            }
            Instr::GlobalGet(index) => {
                let ty = self.global(*index)?;
                self.push_opd(ty.ty);
            }
            Instr::GlobalSet(index) => {
                let ty = self.global(*index)?;
                if !ty.mutable {
                    return err("global is immutable");
                }
                self.pop_opd(ty.ty)?;
            }
            Instr::TableGet(table) => {
                let ty = self.table(*table)?;
                self.pop_opd(I32)?;
                self.push_opd(ty.elem);
            }
            Instr::TableSet(table) => {
                let ty = self.table(*table)?;
                self.pop_opd(ty.elem)?;
                self.pop_opd(I32)?;
            }
            Instr::Load(op, arg) => {
                let (natural, ty) = load_type(*op).ok_or_else(|| DecodeError::new("bad load opcode"))?;
                self.mem_arg(arg, natural)?;
                self.un(I32, ty)?;
            }
            Instr::Store(op, arg) => {
                let (natural, ty) =
                    store_type(*op).ok_or_else(|| DecodeError::new("bad store opcode"))?;
                self.mem_arg(arg, natural)?;
                self.pop_opd(ty)?;
                self.pop_opd(I32)?;
            }
            Instr::MemorySize(memory) => {
                self.memory(*memory)?;
                self.push_opd(I32);
            }
            Instr::MemoryGrow(memory) => {
                self.memory(*memory)?;
                self.un(I32, I32)?;
            }
            Instr::I32Const(_) => self.push_opd(I32),
            Instr::I64Const(_) => self.push_opd(I64),
            Instr::F32Const(_) => self.push_opd(F32),
            Instr::F64Const(_) => self.push_opd(F64),
            Instr::Num(op) => {
                let (inputs, output) =
                    num_type(*op).ok_or_else(|| DecodeError::new("bad numeric opcode"))?;
                self.pop_opds(inputs)?;
                self.push_opd(output);
            }
            Instr::RefNull(ty) => self.push_opd(*ty),
            Instr::RefIsNull => {
                self.pop_ref()?;
                self.push_opd(I32);
            }
            Instr::RefFunc(func) => {
                if *func as usize >= self.ctx.funcs.len() {
                    return err("unknown function");
                }
                if !self.ctx.refs[*func as usize] {
                    return err("undeclared function reference");
                }
                self.push_opd(ValType::FuncRef);
            }
            Instr::TruncSat(sub) => {
                let (input, output) = match sub {
                    0 | 1 => (F32, I32),
                    2 | 3 => (F64, I32),
                    4 | 5 => (F32, I64),
                    _ => (F64, I64),
                };
                self.un(input, output)?;
            }
            Instr::MemoryInit { data, memory } => {
                self.memory(*memory)?;
                self.data(*data)?;
                self.pop_opds(&[I32, I32, I32])?;
            }
            Instr::DataDrop(data) => self.data(*data)?,
            Instr::MemoryCopy { dst, src } => {
                self.memory(*dst)?;
                self.memory(*src)?;
                self.pop_opds(&[I32, I32, I32])?;
            }
            Instr::MemoryFill(memory) => {
                self.memory(*memory)?;
                self.pop_opds(&[I32, I32, I32])?;
            }
            Instr::TableInit { elem, table } => {
                let table = self.table(*table)?;
                if self.elem(*elem)? != table.elem {
                    return err("type mismatch: table.init");
                }
                self.pop_opds(&[I32, I32, I32])?;
            }
            Instr::ElemDrop(elem) => {
                self.elem(*elem)?;
            }
            Instr::TableCopy { dst, src } => {
                if self.table(*dst)?.elem != self.table(*src)?.elem {
                    return err("type mismatch: table.copy");
                }
                self.pop_opds(&[I32, I32, I32])?;
            }
            Instr::TableGrow(table) => {
                let ty = self.table(*table)?;
                self.pop_opd(I32)?;
                self.pop_opd(ty.elem)?;
                self.push_opd(I32);
            }
            Instr::TableSize(table) => {
                self.table(*table)?;
                self.push_opd(I32);
            }
            Instr::TableFill(table) => {
                let ty = self.table(*table)?;
                self.pop_opd(I32)?;
                self.pop_opd(ty.elem)?;
                self.pop_opd(I32)?;
            }
            Instr::SimdMem(sub, arg) => {
                let natural = match sub {
                    0x00 | 0x0b => 4,
                    0x01..=0x06 | 0x0a | 0x5d => 3,
                    0x09 | 0x5c => 2,
                    0x08 => 1,
                    0x07 => 0,
                    _ => return err("bad simd memory opcode"),
                };
                self.mem_arg(arg, natural)?;
                if *sub == 0x0b {
                    self.pop_opd(V128)?;
                    self.pop_opd(I32)?;
                } else {
                    self.un(I32, V128)?;
                }
            }
            Instr::SimdMemLane(sub, arg, lane) => {
                let natural = ((*sub - 0x54) % 4) as u32;
                self.mem_arg(arg, natural)?;
                if *lane as u32 >= 16 >> natural {
                    return err("invalid lane index");
                }
                self.pop_opd(V128)?;
                self.pop_opd(I32)?;
                if *sub < 0x58 {
                    self.push_opd(V128);
                }
            }
            Instr::V128Const(_) => self.push_opd(V128),
            Instr::I8x16Shuffle(lanes) => {
                if lanes.iter().any(|lane| *lane >= 32) {
                    return err("invalid lane index");
                }
                self.bin(V128, V128)?;
            }
            Instr::SimdLane(sub, lane) => {
                let (lanes, scalar, extract) = match sub {
                    0x15 | 0x16 => (16, I32, true),
                    0x17 => (16, I32, false),
                    0x18 | 0x19 => (8, I32, true),
                    0x1a => (8, I32, false),
                    0x1b => (4, I32, true),
                    0x1c => (4, I32, false),
                    0x1d => (2, I64, true),
                    0x1e => (2, I64, false),
                    0x1f => (4, F32, true),
                    0x20 => (4, F32, false),
                    0x21 => (2, F64, true),
                    _ => (2, F64, false),
                };
                if *lane >= lanes {
                    return err("invalid lane index");
                }
                if extract {
                    self.un(V128, scalar)?;
                } else {
                    self.pop_opd(scalar)?;
                    self.un(V128, V128)?;
                }
            }
            Instr::Simd(sub) => self.simd(*sub)?,
            Instr::Atomic(sub, arg) => self.atomic(*sub, arg)?,
            Instr::AtomicFence => {}
            Instr::ExtMath(sub) => match ext_math_class(*sub) {
                Some(ExtMathClass::Un(ty)) => self.un(ty, ty)?,
                Some(ExtMathClass::Bin(ty)) => self.bin(ty, ty)?,
                Some(ExtMathClass::Reduce) => self.bin(V128, F32)?,
                None => return err("illegal opcode"),
            },
        }
        Ok(())
    }

    fn simd(&mut self, sub: u16) -> Res<()> {
        match simd_class(sub) {
            Some(SimdClass::Un) => self.un(V128, V128),
            Some(SimdClass::Bin) => self.bin(V128, V128),
            Some(SimdClass::Tern) => {
                self.pop_opds(&[V128, V128, V128])?;
                self.push_opd(V128);
                Ok(())
            }
            Some(SimdClass::Test) => self.un(V128, I32),
            Some(SimdClass::Shift) => {
                self.pop_opd(I32)?;
                self.un(V128, V128)
            }
            Some(SimdClass::Splat(ty)) => self.un(ty, V128),
            None => err(format!("unknown opcode 0xfd {sub}")),
        }
    }

    fn atomic(&mut self, sub: u16, arg: &MemArg) -> Res<()> {
        match sub {
            0x00 => {
                self.atomic_arg(arg, 2)?;
                self.bin(I32, I32)
            }
            0x01 => {
                self.atomic_arg(arg, 2)?;
                self.pop_opds(&[I32, I32, I64])?;
                self.push_opd(I32);
                Ok(())
            }
            0x02 => {
                self.atomic_arg(arg, 3)?;
                self.pop_opds(&[I32, I64, I64])?;
                self.push_opd(I32);
                Ok(())
            }
            0x10..=0x4e => {
                let (natural, ty) = match (sub - 0x10) % 7 {
                    0 => (2, I32),
                    1 => (3, I64),
                    2 => (0, I32),
                    3 => (1, I32),
                    4 => (0, I64),
                    5 => (1, I64),
                    _ => (2, I64),
                };
                self.atomic_arg(arg, natural)?;
                match sub {
                    0x10..=0x16 => self.un(I32, ty),
                    0x17..=0x1d => {
                        self.pop_opd(ty)?;
                        self.pop_opd(I32)?;
                        Ok(())
                    }
                    0x1e..=0x47 => {
                        self.pop_opd(ty)?;
                        self.un(I32, ty)
                    }
                    _ => {
                        self.pop_opd(ty)?;
                        self.pop_opd(ty)?;
                        self.un(I32, ty)
                    }
                }
            }
            _ => err(format!("unknown opcode 0xfe {sub}")),
        }
    }
}

/// The natural alignment (log2) and result type of a load opcode.
pub(crate) fn load_type(op: u8) -> Option<(u32, ValType)> {
    Some(match op {
        0x28 => (2, I32),
        0x29 => (3, I64),
        0x2a => (2, F32),
        0x2b => (3, F64),
        0x2c | 0x2d => (0, I32),
        0x2e | 0x2f => (1, I32),
        0x30 | 0x31 => (0, I64),
        0x32 | 0x33 => (1, I64),
        0x34 | 0x35 => (2, I64),
        _ => return None,
    })
}

/// The natural alignment (log2) and stored type of a store opcode.
pub(crate) fn store_type(op: u8) -> Option<(u32, ValType)> {
    Some(match op {
        0x36 => (2, I32),
        0x37 => (3, I64),
        0x38 => (2, F32),
        0x39 => (3, F64),
        0x3a => (0, I32),
        0x3b => (1, I32),
        0x3c => (0, I64),
        0x3d => (1, I64),
        0x3e => (2, I64),
        _ => return None,
    })
}

/// The operand types and result type of a numeric opcode (0x45..=0xc4).
pub(crate) fn num_type(op: u8) -> Option<(&'static [ValType], ValType)> {
    const UN_I32: &[ValType] = &[I32];
    const UN_I64: &[ValType] = &[I64];
    const UN_F32: &[ValType] = &[F32];
    const UN_F64: &[ValType] = &[F64];
    const BIN_I32: &[ValType] = &[I32, I32];
    const BIN_I64: &[ValType] = &[I64, I64];
    const BIN_F32: &[ValType] = &[F32, F32];
    const BIN_F64: &[ValType] = &[F64, F64];
    Some(match op {
        0x45 => (UN_I32, I32),
        0x46..=0x4f => (BIN_I32, I32),
        0x50 => (UN_I64, I32),
        0x51..=0x5a => (BIN_I64, I32),
        0x5b..=0x60 => (BIN_F32, I32),
        0x61..=0x66 => (BIN_F64, I32),
        0x67..=0x69 => (UN_I32, I32),
        0x6a..=0x78 => (BIN_I32, I32),
        0x79..=0x7b => (UN_I64, I64),
        0x7c..=0x8a => (BIN_I64, I64),
        0x8b..=0x91 => (UN_F32, F32),
        0x92..=0x98 => (BIN_F32, F32),
        0x99..=0x9f => (UN_F64, F64),
        0xa0..=0xa6 => (BIN_F64, F64),
        0xa7 => (UN_I64, I32),
        0xa8 | 0xa9 => (UN_F32, I32),
        0xaa | 0xab => (UN_F64, I32),
        0xac | 0xad => (UN_I32, I64),
        0xae | 0xaf => (UN_F32, I64),
        0xb0 | 0xb1 => (UN_F64, I64),
        0xb2 | 0xb3 => (UN_I32, F32),
        0xb4 | 0xb5 => (UN_I64, F32),
        0xb6 => (UN_F64, F32),
        0xb7 | 0xb8 => (UN_I32, F64),
        0xb9 | 0xba => (UN_I64, F64),
        0xbb => (UN_F32, F64),
        0xbc => (UN_F32, I32),
        0xbd => (UN_F64, I64),
        0xbe => (UN_I32, F32),
        0xbf => (UN_I64, F64),
        0xc0 | 0xc1 => (UN_I32, I32),
        0xc2..=0xc4 => (UN_I64, I64),
        _ => return None,
    })
}

#[derive(Clone, Copy)]
pub(crate) enum SimdClass {
    Un,
    Bin,
    Tern,
    Test,
    Shift,
    Splat(ValType),
}

/// The shape of a 0xfd opcode without immediates.
pub(crate) fn simd_class(sub: u16) -> Option<SimdClass> {
    use SimdClass::*;
    Some(match sub {
        0x0e => Bin,
        0x0f..=0x11 => Splat(I32),
        0x12 => Splat(I64),
        0x13 => Splat(F32),
        0x14 => Splat(F64),
        0x23..=0x4c => Bin,
        0x4d => Un,
        0x4e..=0x51 => Bin,
        0x52 => Tern,
        0x53 => Test,
        0x5e | 0x5f => Un,
        0x60..=0x62 => Un,
        0x63 | 0x64 => Test,
        0x65 | 0x66 => Bin,
        0x67..=0x6a => Un,
        0x6b..=0x6d => Shift,
        0x6e..=0x73 => Bin,
        0x74 | 0x75 => Un,
        0x76..=0x79 => Bin,
        0x7a => Un,
        0x7b => Bin,
        0x7c..=0x81 => Un,
        0x82 => Bin,
        0x83 | 0x84 => Test,
        0x85 | 0x86 => Bin,
        0x87..=0x8a => Un,
        0x8b..=0x8d => Shift,
        0x8e..=0x93 => Bin,
        0x94 => Un,
        0x95..=0x99 | 0x9b..=0x9f => Bin,
        0xa0 | 0xa1 => Un,
        0xa3 | 0xa4 => Test,
        0xa7..=0xaa => Un,
        0xab..=0xad => Shift,
        0xae | 0xb1 | 0xb5..=0xba | 0xbc..=0xbf => Bin,
        0xc0 | 0xc1 => Un,
        0xc3 | 0xc4 => Test,
        0xc7..=0xca => Un,
        0xcb..=0xcd => Shift,
        0xce | 0xd1 | 0xd5..=0xdf => Bin,
        0xe0 | 0xe1 | 0xe3 => Un,
        0xe4..=0xeb => Bin,
        0xec | 0xed | 0xef => Un,
        0xf0..=0xf7 => Bin,
        0xf8..=0xff => Un,
        0x100 => Bin,
        0x101..=0x104 => Un,
        0x105..=0x10c => Tern,
        0x10d..=0x112 => Bin,
        0x113 => Tern,
        _ => return None,
    })
}

/// Validates a constant expression producing `expect`. `globals_visible` is
/// how many globals a `global.get` may name (the imported ones); they must be
/// immutable.
fn const_expr(ctx: &Ctx, expr: &[Instr], expect: ValType, globals_visible: usize) -> Res<()> {
    let mut stack: Vec<ValType> = Vec::new();
    let Some((Instr::End, body)) = expr.split_last() else {
        return err("constant expression without end");
    };
    for instr in body {
        match instr {
            Instr::I32Const(_) => stack.push(I32),
            Instr::I64Const(_) => stack.push(I64),
            Instr::F32Const(_) => stack.push(F32),
            Instr::F64Const(_) => stack.push(F64),
            Instr::V128Const(_) => stack.push(V128),
            Instr::RefNull(ty) => stack.push(*ty),
            Instr::RefFunc(func) => {
                if *func as usize >= ctx.funcs.len() {
                    return err("unknown function");
                }
                stack.push(ValType::FuncRef);
            }
            Instr::GlobalGet(index) => {
                let index = *index as usize;
                if index >= globals_visible {
                    return err("unknown global in constant expression");
                }
                let ty = ctx.globals[index];
                if ty.mutable {
                    return err("constant expression required");
                }
                stack.push(ty.ty);
            }
            Instr::Num(op @ (0x6a | 0x6b | 0x6c | 0x7c | 0x7d | 0x7e)) => {
                let ty = if *op < 0x70 { I32 } else { I64 };
                let b = stack.pop();
                let a = stack.pop();
                if a != Some(ty) || b != Some(ty) {
                    return err("type mismatch in constant expression");
                }
                stack.push(ty);
            }
            _ => return err("constant expression required"),
        }
    }
    if stack != [expect] {
        return err("type mismatch in constant expression");
    }
    Ok(())
}

fn check_limits(limits: &Limits, bound: u64, what: &str) -> Res<()> {
    if limits.min as u64 > bound || limits.max.map_or(false, |max| max as u64 > bound) {
        return err(format!("{what} size must be at most {bound}"));
    }
    if limits.max.map_or(false, |max| max < limits.min) {
        return err("size minimum must not be greater than maximum");
    }
    Ok(())
}

/// Validates a decoded module: its types, imports, exports, segments,
/// constant expressions and every function body.
pub fn validate(module: &Module) -> Result<(), DecodeError> {
    let num_types = module.types.len() as u32;
    for import in &module.imports {
        if let ImportDesc::Func(ty) = import.desc {
            if ty >= num_types {
                return err("unknown type");
            }
        }
    }
    for func in &module.funcs {
        if func.ty >= num_types {
            return err("unknown type");
        }
    }
    let funcs = module.func_type_indices();
    let mut ctx = Ctx {
        types: &module.types,
        refs: vec![false; funcs.len()],
        funcs,
        tables: module.table_types(),
        memories: module.memory_types(),
        globals: module.global_types(),
        num_imported_globals: module.num_imported(ExternKind::Global) as usize,
        elem_types: module
            .elems
            .iter()
            .map(|elem| elem.items.elem_type())
            .collect(),
        data_count: module.data_count,
    };
    for table in &ctx.tables {
        check_limits(&table.limits, u32::MAX as u64, "table")?;
    }
    for memory in &ctx.memories {
        check_limits(&memory.limits, 65536, "memory")?;
        if memory.shared && memory.limits.max.is_none() {
            return err("shared memory must have maximum");
        }
    }

    // C.refs: every function named outside the code section.
    let mark = |expr: &[Instr], refs: &mut Vec<bool>| {
        for instr in expr {
            if let Instr::RefFunc(func) = instr {
                if let Some(slot) = refs.get_mut(*func as usize) {
                    *slot = true;
                }
            }
        }
    };
    for global in &module.globals {
        mark(&global.init, &mut ctx.refs);
    }
    for elem in &module.elems {
        match &elem.items {
            ElemItems::Funcs(funcs) => {
                for func in funcs {
                    if let Some(slot) = ctx.refs.get_mut(*func as usize) {
                        *slot = true;
                    }
                }
            }
            ElemItems::Exprs(_, exprs) => {
                for expr in exprs {
                    mark(expr, &mut ctx.refs);
                }
            }
        }
    }
    for export in &module.exports {
        if export.kind == ExternKind::Func {
            if let Some(slot) = ctx.refs.get_mut(export.index as usize) {
                *slot = true;
            }
        }
    }

    for global in &module.globals {
        const_expr(&ctx, &global.init, global.ty.ty, ctx.num_imported_globals)?;
    }

    let mut export_names = std::collections::HashSet::new();
    for export in &module.exports {
        if !export_names.insert(export.name.as_str()) {
            return err("duplicate export name");
        }
        let bound = match export.kind {
            ExternKind::Func => ctx.funcs.len(),
            ExternKind::Table => ctx.tables.len(),
            ExternKind::Memory => ctx.memories.len(),
            ExternKind::Global => ctx.globals.len(),
        };
        if export.index as usize >= bound {
            return err("unknown export index");
        }
    }

    if let Some(start) = module.start {
        let ty = *ctx
            .funcs
            .get(start as usize)
            .ok_or_else(|| DecodeError::new("unknown function"))?;
        let ty = &module.types[ty as usize];
        if !ty.params.is_empty() || !ty.results.is_empty() {
            return err("start function must have type [] -> []");
        }
    }

    for elem in &module.elems {
        let elem_type = elem.items.elem_type();
        if let ElemMode::Active { table, offset } = &elem.mode {
            let table = ctx
                .tables
                .get(*table as usize)
                .ok_or_else(|| DecodeError::new("unknown table"))?;
            if table.elem != elem_type {
                return err("type mismatch: element segment type");
            }
            const_expr(&ctx, offset, I32, ctx.num_imported_globals)?;
        }
        match &elem.items {
            ElemItems::Funcs(funcs) => {
                if funcs.iter().any(|func| *func as usize >= ctx.funcs.len()) {
                    return err("unknown function");
                }
            }
            ElemItems::Exprs(ty, exprs) => {
                for expr in exprs {
                    const_expr(&ctx, expr, *ty, ctx.num_imported_globals)?;
                }
            }
        }
    }

    for data in &module.datas {
        if let DataMode::Active { memory, offset } = &data.mode {
            if *memory as usize >= ctx.memories.len() {
                return err("unknown memory");
            }
            const_expr(&ctx, offset, I32, ctx.num_imported_globals)?;
        }
    }
    if let Some(count) = module.data_count {
        if count as usize != module.datas.len() {
            return err("data count and data section have inconsistent lengths");
        }
    }

    let num_imported_funcs = module.num_imported_funcs() as usize;
    for (i, func) in module.funcs.iter().enumerate() {
        validate_func(&ctx, func)
            .map_err(|msg| DecodeError::new(format!("function {}: {msg}", num_imported_funcs + i)))?;
    }
    Ok(())
}

fn validate_func(ctx: &Ctx, func: &Func) -> Res<()> {
    let ty = &ctx.types[func.ty as usize];
    let mut locals = ty.params.clone();
    locals.extend_from_slice(&func.locals);
    let mut v = Validation {
        ctx,
        locals,
        results: &ty.results,
        opds: Vec::new(),
        blocks: Vec::new(),
        aux_opds: Vec::new(),
    };
    v.push_block(BlockKind::Block, &[], &ty.results);
    for (pc, instr) in func.body.iter().enumerate() {
        if v.blocks.is_empty() {
            return err("operators remaining after end of function");
        }
        v.instr(instr)
            .map_err(|msg| DecodeError::new(format!("at instruction {pc} ({instr:?}): {msg}")))?;
    }
    if !v.blocks.is_empty() {
        return err("unexpected end of function body");
    }
    Ok(())
}
