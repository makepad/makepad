//! A full module validator following the algorithm of the core spec's
//! validation appendix: an operand stack of possibly-unknown types and a
//! stack of control frames. Covers MVP, multi-value, sign extension,
//! saturating truncation, bulk memory, reference types, tail calls, fixed
//! and relaxed SIMD, threads (atomics) and multi-memory; anything else was
//! already refused by the decoder.

use super::ir::*;
use super::Res;

struct Ctx<'a> {
    types: &'a [FuncType],
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

const I32: ValType = ValType::I32;
const I64: ValType = ValType::I64;
const F32: ValType = ValType::F32;
const F64: ValType = ValType::F64;
const V128: ValType = ValType::V128;

#[derive(Clone, Copy, Eq, PartialEq)]
enum FrameKind {
    Block,
    Loop,
    If,
    Else,
}

struct Frame {
    kind: FrameKind,
    params: Vec<ValType>,
    results: Vec<ValType>,
    height: usize,
    unreachable: bool,
}

struct FuncValidator<'c, 'a> {
    ctx: &'c Ctx<'a>,
    locals: Vec<ValType>,
    results: Vec<ValType>,
    vals: Vec<Option<ValType>>,
    ctrls: Vec<Frame>,
}

fn err<T>(msg: impl Into<String>) -> Res<T> {
    Err(msg.into())
}

impl<'c, 'a> FuncValidator<'c, 'a> {
    fn push(&mut self, ty: ValType) {
        self.vals.push(Some(ty));
    }

    fn push_unknown(&mut self, ty: Option<ValType>) {
        self.vals.push(ty);
    }

    fn pop_any(&mut self) -> Res<Option<ValType>> {
        let frame = self.ctrls.last().unwrap();
        if self.vals.len() == frame.height {
            if frame.unreachable {
                return Ok(None);
            }
            return err("type mismatch: operand stack underflow");
        }
        Ok(self.vals.pop().unwrap())
    }

    /// Pops a value of type `expect`; `None` when the stack below an
    /// unconditional branch supplied it.
    fn pop(&mut self, expect: ValType) -> Res<Option<ValType>> {
        let actual = self.pop_any()?;
        match actual {
            Some(actual) if actual != expect => err(format!(
                "type mismatch: expected {expect:?}, found {actual:?}"
            )),
            _ => Ok(actual),
        }
    }

    fn pop_ref(&mut self) -> Res<Option<ValType>> {
        let actual = self.pop_any()?;
        match actual {
            Some(actual) if !actual.is_ref() => err("type mismatch: expected a reference"),
            _ => Ok(actual),
        }
    }

    fn pop_vals(&mut self, types: &[ValType]) -> Res<Vec<Option<ValType>>> {
        let mut out = vec![None; types.len()];
        for (i, ty) in types.iter().enumerate().rev() {
            out[i] = self.pop(*ty)?;
        }
        Ok(out)
    }

    fn push_vals(&mut self, types: &[ValType]) {
        for ty in types {
            self.push(*ty);
        }
    }

    fn push_ctrl(&mut self, kind: FrameKind, params: Vec<ValType>, results: Vec<ValType>) {
        let height = self.vals.len();
        self.push_vals(&params);
        self.ctrls.push(Frame {
            kind,
            params,
            results,
            height,
            unreachable: false,
        });
    }

    fn pop_ctrl(&mut self) -> Res<Frame> {
        if self.ctrls.is_empty() {
            return err("unexpected end");
        }
        let results = self.ctrls.last().unwrap().results.clone();
        self.pop_vals(&results)?;
        let frame = self.ctrls.pop().unwrap();
        if self.vals.len() != frame.height {
            return err("type mismatch: values remaining on stack at end of block");
        }
        Ok(frame)
    }

    fn label_types(&self, depth: u32) -> Res<Vec<ValType>> {
        let depth = depth as usize;
        if depth >= self.ctrls.len() {
            return err("unknown label");
        }
        let frame = &self.ctrls[self.ctrls.len() - 1 - depth];
        Ok(if frame.kind == FrameKind::Loop {
            frame.params.clone()
        } else {
            frame.results.clone()
        })
    }

    fn unreachable(&mut self) {
        let frame = self.ctrls.last_mut().unwrap();
        self.vals.truncate(frame.height);
        frame.unreachable = true;
    }

    fn block_type(&self, ty: &BlockType) -> Res<(Vec<ValType>, Vec<ValType>)> {
        Ok(match ty {
            BlockType::Empty => (vec![], vec![]),
            BlockType::Value(ty) => (vec![], vec![*ty]),
            BlockType::Func(index) => {
                let ty = self
                    .ctx
                    .types
                    .get(*index as usize)
                    .ok_or_else(|| "unknown type".to_string())?;
                (ty.params.clone(), ty.results.clone())
            }
        })
    }

    fn func_type(&self, ty: u32) -> Res<&'a FuncType> {
        self.ctx
            .types
            .get(ty as usize)
            .ok_or_else(|| "unknown type".to_string())
    }

    fn local(&self, index: u32) -> Res<ValType> {
        self.locals
            .get(index as usize)
            .copied()
            .ok_or_else(|| "unknown local".to_string())
    }

    fn global(&self, index: u32) -> Res<GlobalType> {
        self.ctx
            .globals
            .get(index as usize)
            .copied()
            .ok_or_else(|| "unknown global".to_string())
    }

    fn table(&self, index: u32) -> Res<TableType> {
        self.ctx
            .tables
            .get(index as usize)
            .copied()
            .ok_or_else(|| "unknown table".to_string())
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
            .ok_or_else(|| "unknown elem segment".to_string())
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
        self.pop(input)?;
        self.push(output);
        Ok(())
    }

    fn bin(&mut self, input: ValType, output: ValType) -> Res<()> {
        self.pop(input)?;
        self.pop(input)?;
        self.push(output);
        Ok(())
    }

    fn call(&mut self, ty: &FuncType) -> Res<()> {
        self.pop_vals(&ty.params)?;
        self.push_vals(&ty.results);
        Ok(())
    }

    fn instr(&mut self, instr: &Instr) -> Res<()> {
        match instr {
            Instr::Unreachable => self.unreachable(),
            Instr::Nop => {}
            Instr::Block(ty) => {
                let (params, results) = self.block_type(ty)?;
                self.pop_vals(&params)?;
                self.push_ctrl(FrameKind::Block, params, results);
            }
            Instr::Loop(ty) => {
                let (params, results) = self.block_type(ty)?;
                self.pop_vals(&params)?;
                self.push_ctrl(FrameKind::Loop, params, results);
            }
            Instr::If(ty) => {
                let (params, results) = self.block_type(ty)?;
                self.pop(I32)?;
                self.pop_vals(&params)?;
                self.push_ctrl(FrameKind::If, params, results);
            }
            Instr::Else => {
                let frame = self.pop_ctrl()?;
                if frame.kind != FrameKind::If {
                    return err("else without if");
                }
                self.push_ctrl(FrameKind::Else, frame.params, frame.results);
            }
            Instr::End => {
                let frame = self.pop_ctrl()?;
                if frame.kind == FrameKind::If && frame.params != frame.results {
                    return err("type mismatch: if without else");
                }
                self.push_vals(&frame.results);
            }
            Instr::Br(depth) => {
                let types = self.label_types(*depth)?;
                self.pop_vals(&types)?;
                self.unreachable();
            }
            Instr::BrIf(depth) => {
                self.pop(I32)?;
                let types = self.label_types(*depth)?;
                self.pop_vals(&types)?;
                self.push_vals(&types);
            }
            Instr::BrTable(targets, default) => {
                self.pop(I32)?;
                let default_types = self.label_types(*default)?;
                for target in targets.iter() {
                    let types = self.label_types(*target)?;
                    if types.len() != default_types.len() {
                        return err("type mismatch: br_table arity");
                    }
                    let vals = self.pop_vals(&types)?;
                    for val in vals {
                        self.push_unknown(val);
                    }
                }
                self.pop_vals(&default_types)?;
                self.unreachable();
            }
            Instr::Return => {
                let results = self.results.clone();
                self.pop_vals(&results)?;
                self.unreachable();
            }
            Instr::Call(func) => {
                let ty = *self
                    .ctx
                    .funcs
                    .get(*func as usize)
                    .ok_or_else(|| "unknown function".to_string())?;
                let ty = self.func_type(ty)?;
                self.call(ty)?;
            }
            Instr::CallIndirect { ty, table } => {
                if self.table(*table)?.elem != ValType::FuncRef {
                    return err("type mismatch: call_indirect on a non-funcref table");
                }
                let ty = self.func_type(*ty)?;
                self.pop(I32)?;
                self.call(ty)?;
            }
            Instr::ReturnCall(func) => {
                let ty = *self
                    .ctx
                    .funcs
                    .get(*func as usize)
                    .ok_or_else(|| "unknown function".to_string())?;
                let ty = self.func_type(ty)?;
                if ty.results != self.results {
                    return err("type mismatch: return_call results");
                }
                self.pop_vals(&ty.params)?;
                self.unreachable();
            }
            Instr::ReturnCallIndirect { ty, table } => {
                if self.table(*table)?.elem != ValType::FuncRef {
                    return err("type mismatch: call_indirect on a non-funcref table");
                }
                let ty = self.func_type(*ty)?;
                if ty.results != self.results {
                    return err("type mismatch: return_call_indirect results");
                }
                self.pop(I32)?;
                self.pop_vals(&ty.params)?;
                self.unreachable();
            }
            Instr::Drop => {
                self.pop_any()?;
            }
            Instr::Select => {
                self.pop(I32)?;
                let t1 = self.pop_any()?;
                let t2 = self.pop_any()?;
                if t1.map_or(false, |t| t.is_ref()) || t2.map_or(false, |t| t.is_ref()) {
                    return err("type mismatch: select of references needs a type");
                }
                match (t1, t2) {
                    (Some(a), Some(b)) if a != b => return err("type mismatch: select"),
                    _ => {}
                }
                self.push_unknown(t1.or(t2));
            }
            Instr::SelectT(ty) => {
                self.pop(I32)?;
                self.pop(*ty)?;
                self.pop(*ty)?;
                self.push(*ty);
            }
            Instr::LocalGet(index) => {
                let ty = self.local(*index)?;
                self.push(ty);
            }
            Instr::LocalSet(index) => {
                let ty = self.local(*index)?;
                self.pop(ty)?;
            }
            Instr::LocalTee(index) => {
                let ty = self.local(*index)?;
                self.pop(ty)?;
                self.push(ty);
            }
            Instr::GlobalGet(index) => {
                let ty = self.global(*index)?;
                self.push(ty.ty);
            }
            Instr::GlobalSet(index) => {
                let ty = self.global(*index)?;
                if !ty.mutable {
                    return err("global is immutable");
                }
                self.pop(ty.ty)?;
            }
            Instr::TableGet(table) => {
                let ty = self.table(*table)?;
                self.pop(I32)?;
                self.push(ty.elem);
            }
            Instr::TableSet(table) => {
                let ty = self.table(*table)?;
                self.pop(ty.elem)?;
                self.pop(I32)?;
            }
            Instr::Load(op, arg) => {
                let (natural, ty) = match op {
                    0x28 => (2, I32),
                    0x29 => (3, I64),
                    0x2a => (2, F32),
                    0x2b => (3, F64),
                    0x2c | 0x2d => (0, I32),
                    0x2e | 0x2f => (1, I32),
                    0x30 | 0x31 => (0, I64),
                    0x32 | 0x33 => (1, I64),
                    0x34 | 0x35 => (2, I64),
                    _ => return err("bad load opcode"),
                };
                self.mem_arg(arg, natural)?;
                self.un(I32, ty)?;
            }
            Instr::Store(op, arg) => {
                let (natural, ty) = match op {
                    0x36 => (2, I32),
                    0x37 => (3, I64),
                    0x38 => (2, F32),
                    0x39 => (3, F64),
                    0x3a => (0, I32),
                    0x3b => (1, I32),
                    0x3c => (0, I64),
                    0x3d => (1, I64),
                    0x3e => (2, I64),
                    _ => return err("bad store opcode"),
                };
                self.mem_arg(arg, natural)?;
                self.pop(ty)?;
                self.pop(I32)?;
            }
            Instr::MemorySize(memory) => {
                self.memory(*memory)?;
                self.push(I32);
            }
            Instr::MemoryGrow(memory) => {
                self.memory(*memory)?;
                self.un(I32, I32)?;
            }
            Instr::I32Const(_) => self.push(I32),
            Instr::I64Const(_) => self.push(I64),
            Instr::F32Const(_) => self.push(F32),
            Instr::F64Const(_) => self.push(F64),
            Instr::Num(op) => self.num(*op)?,
            Instr::RefNull(ty) => self.push(*ty),
            Instr::RefIsNull => {
                self.pop_ref()?;
                self.push(I32);
            }
            Instr::RefFunc(func) => {
                if *func as usize >= self.ctx.funcs.len() {
                    return err("unknown function");
                }
                if !self.ctx.refs[*func as usize] {
                    return err("undeclared function reference");
                }
                self.push(ValType::FuncRef);
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
                self.pop_vals(&[I32, I32, I32])?;
            }
            Instr::DataDrop(data) => self.data(*data)?,
            Instr::MemoryCopy { dst, src } => {
                self.memory(*dst)?;
                self.memory(*src)?;
                self.pop_vals(&[I32, I32, I32])?;
            }
            Instr::MemoryFill(memory) => {
                self.memory(*memory)?;
                self.pop_vals(&[I32, I32, I32])?;
            }
            Instr::TableInit { elem, table } => {
                let table = self.table(*table)?;
                if self.elem(*elem)? != table.elem {
                    return err("type mismatch: table.init");
                }
                self.pop_vals(&[I32, I32, I32])?;
            }
            Instr::ElemDrop(elem) => {
                self.elem(*elem)?;
            }
            Instr::TableCopy { dst, src } => {
                if self.table(*dst)?.elem != self.table(*src)?.elem {
                    return err("type mismatch: table.copy");
                }
                self.pop_vals(&[I32, I32, I32])?;
            }
            Instr::TableGrow(table) => {
                let ty = self.table(*table)?;
                self.pop(I32)?;
                self.pop(ty.elem)?;
                self.push(I32);
            }
            Instr::TableSize(table) => {
                self.table(*table)?;
                self.push(I32);
            }
            Instr::TableFill(table) => {
                let ty = self.table(*table)?;
                self.pop(I32)?;
                self.pop(ty.elem)?;
                self.pop(I32)?;
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
                    self.pop(V128)?;
                    self.pop(I32)?;
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
                self.pop(V128)?;
                self.pop(I32)?;
                if *sub < 0x58 {
                    self.push(V128);
                }
            }
            Instr::V128Const(_) => self.push(V128),
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
                    self.pop(scalar)?;
                    self.un(V128, V128)?;
                }
            }
            Instr::Simd(sub) => self.simd(*sub)?,
            Instr::Atomic(sub, arg) => self.atomic(*sub, arg)?,
            Instr::AtomicFence => {}
        }
        Ok(())
    }

    fn num(&mut self, op: u8) -> Res<()> {
        match op {
            0x45 => self.un(I32, I32),
            0x46..=0x4f => self.bin(I32, I32),
            0x50 => self.un(I64, I32),
            0x51..=0x5a => self.bin(I64, I32),
            0x5b..=0x60 => self.bin(F32, I32),
            0x61..=0x66 => self.bin(F64, I32),
            0x67..=0x69 => self.un(I32, I32),
            0x6a..=0x78 => self.bin(I32, I32),
            0x79..=0x7b => self.un(I64, I64),
            0x7c..=0x8a => self.bin(I64, I64),
            0x8b..=0x91 => self.un(F32, F32),
            0x92..=0x98 => self.bin(F32, F32),
            0x99..=0x9f => self.un(F64, F64),
            0xa0..=0xa6 => self.bin(F64, F64),
            0xa7 => self.un(I64, I32),
            0xa8 | 0xa9 => self.un(F32, I32),
            0xaa | 0xab => self.un(F64, I32),
            0xac | 0xad => self.un(I32, I64),
            0xae | 0xaf => self.un(F32, I64),
            0xb0 | 0xb1 => self.un(F64, I64),
            0xb2 | 0xb3 => self.un(I32, F32),
            0xb4 | 0xb5 => self.un(I64, F32),
            0xb6 => self.un(F64, F32),
            0xb7 | 0xb8 => self.un(I32, F64),
            0xb9 | 0xba => self.un(I64, F64),
            0xbb => self.un(F32, F64),
            0xbc => self.un(F32, I32),
            0xbd => self.un(F64, I64),
            0xbe => self.un(I32, F32),
            0xbf => self.un(I64, F64),
            0xc0 | 0xc1 => self.un(I32, I32),
            0xc2..=0xc4 => self.un(I64, I64),
            _ => err("bad numeric opcode"),
        }
    }

    fn simd(&mut self, sub: u16) -> Res<()> {
        match simd_class(sub) {
            Some(SimdClass::Un) => self.un(V128, V128),
            Some(SimdClass::Bin) => self.bin(V128, V128),
            Some(SimdClass::Tern) => {
                self.pop_vals(&[V128, V128, V128])?;
                self.push(V128);
                Ok(())
            }
            Some(SimdClass::Test) => self.un(V128, I32),
            Some(SimdClass::Shift) => {
                self.pop(I32)?;
                self.un(V128, V128)
            }
            Some(SimdClass::Splat(ty)) => self.un(ty, V128),
            None => err(format!("unknown 0xfd opcode {sub}")),
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
                self.pop_vals(&[I32, I32, I64])?;
                self.push(I32);
                Ok(())
            }
            0x02 => {
                self.atomic_arg(arg, 3)?;
                self.pop_vals(&[I32, I64, I64])?;
                self.push(I32);
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
                        self.pop(ty)?;
                        self.pop(I32)?;
                        Ok(())
                    }
                    0x1e..=0x47 => {
                        self.pop(ty)?;
                        self.un(I32, ty)
                    }
                    _ => {
                        self.pop(ty)?;
                        self.pop(ty)?;
                        self.un(I32, ty)
                    }
                }
            }
            _ => err(format!("unknown 0xfe opcode {sub}")),
        }
    }
}

#[derive(Clone, Copy)]
enum SimdClass {
    Un,
    Bin,
    Tern,
    Test,
    Shift,
    Splat(ValType),
}

fn simd_class(sub: u16) -> Option<SimdClass> {
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

pub fn validate(module: &Module) -> Res<()> {
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
        elem_types: module.elems.iter().map(|elem| elem.items.elem_type()).collect(),
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
            .ok_or_else(|| "unknown function".to_string())?;
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
                .ok_or_else(|| "unknown table".to_string())?;
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
        validate_func(&ctx, func).map_err(|msg| {
            format!("function {}: {msg}", num_imported_funcs + i)
        })?;
    }
    Ok(())
}

fn validate_func(ctx: &Ctx, func: &Func) -> Res<()> {
    let ty = &ctx.types[func.ty as usize];
    let mut locals = ty.params.clone();
    locals.extend_from_slice(&func.locals);
    let mut v = FuncValidator {
        ctx,
        locals,
        results: ty.results.clone(),
        vals: Vec::new(),
        ctrls: Vec::new(),
    };
    v.push_ctrl(FrameKind::Block, vec![], ty.results.clone());
    for (pc, instr) in func.body.iter().enumerate() {
        if v.ctrls.is_empty() {
            return err("operators remaining after end of function");
        }
        v.instr(instr).map_err(|msg| format!("at instruction {pc} ({instr:?}): {msg}"))?;
    }
    if !v.ctrls.is_empty() {
        return err("unexpected end of function body");
    }
    Ok(())
}
