//! The Wasm binary format as plain data: every section decoded, function
//! bodies as typed instruction lists (structured control kept as
//! Block/Loop/If/Else/End markers, exactly as the binary has them).
//!
//! This is the one decoder: [`Module::new`](crate::Module::new) loads
//! through it, and tools that rewrite modules read with it, change the plain
//! data and encode it again. [`validate`] checks a decoded module.
//!
//! It decodes the core format with multi-value, sign extension, saturating
//! truncation, bulk memory, reference types, fixed and relaxed SIMD, threads
//! (atomics), tail calls, multi-memory and extended constant expressions.
//! Anything else (memory64, exceptions, GC, unknown opcodes) is a decode
//! error, so a tool never rewrites code it cannot see. The engine runs a
//! subset of that and refuses the rest when it loads a module.

use crate::decode::{DecodeError, Decoder};

pub use crate::{limits::Limits, validate::validate, val::ValType};

/// Which encodings beyond the core format the decoder accepts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Features {
    /// Memory indices on memory instructions (multi-memory). Without it
    /// those immediates are the single zero byte of the core format.
    pub multi_memory: bool,
    /// The nonstandard float math opcodes (prefix 0xE0, see
    /// [`Extensions::ext_math`](crate::Extensions::ext_math)).
    pub ext_math: bool,
}

impl Default for Features {
    /// Every standard proposal the decoder knows; no nonstandard opcodes.
    fn default() -> Features {
        Features {
            multi_memory: true,
            ext_math: false,
        }
    }
}

impl ValType {
    /// The value type with this binary encoding.
    pub fn from_byte(byte: u8) -> Option<ValType> {
        Some(match byte {
            0x7f => ValType::I32,
            0x7e => ValType::I64,
            0x7d => ValType::F32,
            0x7c => ValType::F64,
            0x7b => ValType::V128,
            0x70 => ValType::FuncRef,
            0x6f => ValType::ExternRef,
            _ => return None,
        })
    }

    /// The binary encoding of this value type.
    pub fn to_byte(self) -> u8 {
        match self {
            ValType::I32 => 0x7f,
            ValType::I64 => 0x7e,
            ValType::F32 => 0x7d,
            ValType::F64 => 0x7c,
            ValType::V128 => 0x7b,
            ValType::FuncRef => 0x70,
            ValType::ExternRef => 0x6f,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct FuncType {
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct TableType {
    pub elem: ValType,
    pub limits: Limits,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct MemType {
    pub limits: Limits,
    pub shared: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct GlobalType {
    pub ty: ValType,
    pub mutable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportDesc {
    Func(u32),
    Table(TableType),
    Memory(MemType),
    Global(GlobalType),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Import {
    pub module: String,
    pub name: String,
    pub desc: ImportDesc,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ExternKind {
    Func,
    Table,
    Memory,
    Global,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Export {
    pub name: String,
    pub kind: ExternKind,
    pub index: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Global {
    pub ty: GlobalType,
    /// A constant expression, `End` included.
    pub init: Vec<Instr>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ElemMode {
    Active { table: u32, offset: Vec<Instr> },
    Passive,
    Declarative,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ElemItems {
    /// `funcref` entries written as plain function indices.
    Funcs(Vec<u32>),
    /// Entries as constant expressions of the given reference type.
    Exprs(ValType, Vec<Vec<Instr>>),
}

impl ElemItems {
    pub fn len(&self) -> usize {
        match self {
            ElemItems::Funcs(funcs) => funcs.len(),
            ElemItems::Exprs(_, exprs) => exprs.len(),
        }
    }

    pub fn elem_type(&self) -> ValType {
        match self {
            ElemItems::Funcs(_) => ValType::FuncRef,
            ElemItems::Exprs(ty, _) => *ty,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Elem {
    pub mode: ElemMode,
    pub items: ElemItems,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataMode {
    Active { memory: u32, offset: Vec<Instr> },
    Passive,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Data {
    pub mode: DataMode,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Func {
    pub ty: u32,
    /// The declared (non-parameter) locals, one entry per local.
    pub locals: Vec<ValType>,
    /// The body, its final `End` included.
    pub body: Vec<Instr>,
}

/// Where a custom section sat: after the standard section with this order
/// rank (see [`section_rank`]), 0 meaning before every standard section.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Custom {
    pub name: String,
    pub data: Vec<u8>,
    pub after_rank: u8,
}

/// The parts of the `name` custom section that survive renumbering.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Names {
    pub module: Option<String>,
    pub funcs: Vec<(u32, String)>,
    pub globals: Vec<(u32, String)>,
    /// Where the `name` section sat, as for [`Custom`].
    pub after_rank: u8,
}

/// One section of the decoded bytes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SectionLayout {
    pub id: u8,
    /// A custom section's name; empty for standard sections.
    pub name: String,
    /// The whole section, its id and size included.
    pub bytes: std::ops::Range<usize>,
    /// Its payload (for a custom section, its name included).
    pub payload: std::ops::Range<usize>,
}

/// Where the parts of a module sit in the decoded bytes, for size
/// accounting.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Layout {
    /// Every section, in order.
    pub sections: Vec<SectionLayout>,
    /// Every function body's code section entry (its size prefix and
    /// locals included), in definition order.
    pub funcs: Vec<std::ops::Range<usize>>,
    /// The bytes of every data segment.
    pub datas: Vec<std::ops::Range<usize>>,
}

#[derive(Clone, Debug, Default)]
pub struct Module {
    pub types: Vec<FuncType>,
    pub imports: Vec<Import>,
    pub funcs: Vec<Func>,
    pub tables: Vec<TableType>,
    pub memories: Vec<MemType>,
    pub globals: Vec<Global>,
    pub exports: Vec<Export>,
    pub start: Option<u32>,
    pub elems: Vec<Elem>,
    /// The data count section's value, when the module has one.
    pub data_count: Option<u32>,
    pub datas: Vec<Data>,
    pub customs: Vec<Custom>,
    pub names: Option<Names>,
}

impl Module {
    /// Decodes a module with the default [`Features`].
    pub fn decode(bytes: &[u8]) -> Result<Module, DecodeError> {
        decode_module(bytes, Features::default(), None)
    }

    /// Decodes a module with the given [`Features`].
    pub fn decode_with(bytes: &[u8], features: Features) -> Result<Module, DecodeError> {
        decode_module(bytes, features, None)
    }

    /// Decodes a module and where its sections, function bodies and data
    /// segments sit in `bytes`.
    pub fn decode_with_layout(
        bytes: &[u8],
        features: Features,
    ) -> Result<(Module, Layout), DecodeError> {
        let mut layout = Layout::default();
        let module = decode_module(bytes, features, Some(&mut layout))?;
        Ok((module, layout))
    }

    pub fn num_imported(&self, kind: ExternKind) -> u32 {
        self.imports
            .iter()
            .filter(|import| import_kind(&import.desc) == kind)
            .count() as u32
    }

    pub fn num_imported_funcs(&self) -> u32 {
        self.num_imported(ExternKind::Func)
    }

    /// The type index of every function, imported ones first.
    pub fn func_type_indices(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.imports.len() + self.funcs.len());
        for import in &self.imports {
            if let ImportDesc::Func(ty) = import.desc {
                out.push(ty);
            }
        }
        out.extend(self.funcs.iter().map(|func| func.ty));
        out
    }

    pub fn global_types(&self) -> Vec<GlobalType> {
        let mut out = Vec::new();
        for import in &self.imports {
            if let ImportDesc::Global(ty) = import.desc {
                out.push(ty);
            }
        }
        out.extend(self.globals.iter().map(|global| global.ty));
        out
    }

    pub fn table_types(&self) -> Vec<TableType> {
        let mut out = Vec::new();
        for import in &self.imports {
            if let ImportDesc::Table(ty) = import.desc {
                out.push(ty);
            }
        }
        out.extend(self.tables.iter().copied());
        out
    }

    pub fn memory_types(&self) -> Vec<MemType> {
        let mut out = Vec::new();
        for import in &self.imports {
            if let ImportDesc::Memory(ty) = import.desc {
                out.push(ty);
            }
        }
        out.extend(self.memories.iter().copied());
        out
    }
}

pub fn import_kind(desc: &ImportDesc) -> ExternKind {
    match desc {
        ImportDesc::Func(_) => ExternKind::Func,
        ImportDesc::Table(_) => ExternKind::Table,
        ImportDesc::Memory(_) => ExternKind::Memory,
        ImportDesc::Global(_) => ExternKind::Global,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum BlockType {
    Empty,
    Value(ValType),
    Func(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct MemArg {
    /// log2 of the alignment.
    pub align: u32,
    pub offset: u32,
    pub memory: u32,
}

/// One instruction. Opcode groups whose immediates have the same shape share
/// a variant and keep their opcode byte (or prefixed sub-opcode).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Instr {
    Unreachable,
    Nop,
    Block(BlockType),
    Loop(BlockType),
    If(BlockType),
    Else,
    End,
    Br(u32),
    BrIf(u32),
    BrTable(Box<[u32]>, u32),
    Return,
    Call(u32),
    CallIndirect { ty: u32, table: u32 },
    ReturnCall(u32),
    ReturnCallIndirect { ty: u32, table: u32 },
    Drop,
    Select,
    SelectT(ValType),
    LocalGet(u32),
    LocalSet(u32),
    LocalTee(u32),
    GlobalGet(u32),
    GlobalSet(u32),
    TableGet(u32),
    TableSet(u32),
    /// Opcodes 0x28..=0x35.
    Load(u8, MemArg),
    /// Opcodes 0x36..=0x3e.
    Store(u8, MemArg),
    MemorySize(u32),
    MemoryGrow(u32),
    I32Const(i32),
    I64Const(i64),
    /// Raw bits, so NaN payloads survive.
    F32Const(u32),
    F64Const(u64),
    /// The numeric opcodes without immediates, 0x45..=0xc4.
    Num(u8),
    RefNull(ValType),
    RefIsNull,
    RefFunc(u32),
    /// 0xfc 0..=7.
    TruncSat(u8),
    MemoryInit { data: u32, memory: u32 },
    DataDrop(u32),
    MemoryCopy { dst: u32, src: u32 },
    MemoryFill(u32),
    TableInit { elem: u32, table: u32 },
    ElemDrop(u32),
    TableCopy { dst: u32, src: u32 },
    TableGrow(u32),
    TableSize(u32),
    TableFill(u32),
    /// 0xfd sub-opcodes with a memarg: loads, splats, store, load_zero.
    SimdMem(u16, MemArg),
    /// 0xfd 0x54..=0x5b: lane loads/stores.
    SimdMemLane(u16, MemArg, u8),
    V128Const(Box<[u8; 16]>),
    I8x16Shuffle(Box<[u8; 16]>),
    /// 0xfd 0x15..=0x22: extract/replace lane.
    SimdLane(u16, u8),
    /// Every other 0xfd sub-opcode (no immediates).
    Simd(u16),
    /// 0xfe sub-opcodes with a memarg.
    Atomic(u16, MemArg),
    AtomicFence,
    /// The nonstandard 0xe0 float math sub-opcodes (see
    /// [`Features::ext_math`]).
    ExtMath(u8),
}

impl Instr {
    /// Instructions that open a structured block.
    pub fn is_block_start(&self) -> bool {
        matches!(self, Instr::Block(_) | Instr::Loop(_) | Instr::If(_))
    }
}

/// The order rank of a standard section id (the data count section sits
/// between the element and code sections).
pub fn section_rank(id: u8) -> Option<u8> {
    Some(match id {
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 5,
        6 => 7,
        7 => 8,
        8 => 9,
        9 => 10,
        12 => 11,
        10 => 12,
        11 => 13,
        _ => return None,
    })
}

fn err<T>(msg: impl Into<Box<str>>) -> Result<T, DecodeError> {
    Err(DecodeError::new(msg))
}

fn val_type(d: &mut Decoder) -> Result<ValType, DecodeError> {
    let byte = d.read_byte()?;
    ValType::from_byte(byte)
        .ok_or_else(|| DecodeError::new(format!("unknown value type 0x{byte:02x}")))
}

fn ref_type(d: &mut Decoder) -> Result<ValType, DecodeError> {
    let ty = val_type(d)?;
    if !ty.is_ref() {
        return err("malformed reference type");
    }
    Ok(ty)
}

fn limits(d: &mut Decoder, allow_shared: bool) -> Result<(Limits, bool), DecodeError> {
    let flags = d.read_byte()?;
    let (has_max, shared) = match flags {
        0 => (false, false),
        1 => (true, false),
        2 if allow_shared => (false, true),
        3 if allow_shared => (true, true),
        _ => return err(format!("unsupported limits flags 0x{flags:02x}")),
    };
    let min = d.decode()?;
    let max = if has_max { Some(d.decode()?) } else { None };
    Ok((Limits { min, max }, shared))
}

fn table_type(d: &mut Decoder) -> Result<TableType, DecodeError> {
    let elem = ref_type(d)?;
    let (limits, _) = limits(d, false)?;
    Ok(TableType { elem, limits })
}

fn mem_type(d: &mut Decoder) -> Result<MemType, DecodeError> {
    let (limits, shared) = limits(d, true)?;
    Ok(MemType { limits, shared })
}

fn global_type(d: &mut Decoder) -> Result<GlobalType, DecodeError> {
    let ty = val_type(d)?;
    let mutable = match d.read_byte()? {
        0 => false,
        1 => true,
        _ => return err("malformed mutability"),
    };
    Ok(GlobalType { ty, mutable })
}

fn s33(d: &mut Decoder) -> Result<i64, DecodeError> {
    let mut result = 0i64;
    let mut shift = 0;
    loop {
        let byte = d.read_byte()?;
        if shift >= 33 {
            return err("integer representation too long");
        }
        result |= ((byte & 0x7f) as i64) << shift;
        if shift + 7 > 33 {
            // The unused bits of the last byte must repeat its sign bit.
            let used = 33 - shift;
            let payload = byte & 0x7f;
            let high = payload >> used;
            let expected = if (payload >> (used - 1)) & 1 != 0 {
                (1u8 << (7 - used)) - 1
            } else {
                0
            };
            if byte & 0x80 != 0 || high != expected {
                return err("integer too large");
            }
        }
        shift += 7;
        if byte & 0x80 == 0 {
            if shift < 64 && byte & 0x40 != 0 {
                result |= -1i64 << shift;
            }
            return Ok(result);
        }
    }
}

fn block_type(d: &mut Decoder) -> Result<BlockType, DecodeError> {
    let peek = d.peek_byte()?;
    if peek == 0x40 {
        d.read_byte()?;
        return Ok(BlockType::Empty);
    }
    if let Some(ty) = ValType::from_byte(peek) {
        d.read_byte()?;
        return Ok(BlockType::Value(ty));
    }
    let index = s33(d)?;
    if index < 0 || index > u32::MAX as i64 {
        return err("malformed block type");
    }
    Ok(BlockType::Func(index as u32))
}

fn mem_arg(d: &mut Decoder, features: Features) -> Result<MemArg, DecodeError> {
    let mut align: u32 = d.decode()?;
    let memory = if features.multi_memory && align & 0x40 != 0 {
        align &= !0x40;
        d.decode()?
    } else {
        0
    };
    let offset = d.decode()?;
    Ok(MemArg {
        align,
        offset,
        memory,
    })
}

/// A memory index immediate: a LEB index with multi-memory, else the core
/// format's single zero byte.
fn mem_index(d: &mut Decoder, features: Features) -> Result<u32, DecodeError> {
    if features.multi_memory {
        d.decode()
    } else if d.read_byte()? != 0 {
        err("zero byte expected")
    } else {
        Ok(0)
    }
}

fn bytes16(d: &mut Decoder) -> Result<Box<[u8; 16]>, DecodeError> {
    let mut out = [0u8; 16];
    out.copy_from_slice(d.read_bytes(16)?);
    Ok(Box::new(out))
}

impl Instr {
    /// Decodes one instruction.
    pub fn decode(bytes: &[u8], features: Features) -> Result<(Instr, usize), DecodeError> {
        let mut d = Decoder::new(bytes);
        let instr = instr(&mut d, features)?;
        Ok((instr, d.position()))
    }
}

fn instr(d: &mut Decoder, features: Features) -> Result<Instr, DecodeError> {
    let op = d.read_byte()?;
    Ok(match op {
        0x00 => Instr::Unreachable,
        0x01 => Instr::Nop,
        0x02 => Instr::Block(block_type(d)?),
        0x03 => Instr::Loop(block_type(d)?),
        0x04 => Instr::If(block_type(d)?),
        0x05 => Instr::Else,
        0x0b => Instr::End,
        0x0c => Instr::Br(d.decode()?),
        0x0d => Instr::BrIf(d.decode()?),
        0x0e => {
            let count: u32 = d.decode()?;
            if count as usize > d.remaining() {
                return err("br_table too long");
            }
            let mut targets = Vec::with_capacity(count as usize);
            for _ in 0..count {
                targets.push(d.decode()?);
            }
            Instr::BrTable(targets.into_boxed_slice(), d.decode()?)
        }
        0x0f => Instr::Return,
        0x10 => Instr::Call(d.decode()?),
        0x11 => {
            let ty = d.decode()?;
            Instr::CallIndirect {
                ty,
                table: d.decode()?,
            }
        }
        0x12 => Instr::ReturnCall(d.decode()?),
        0x13 => {
            let ty = d.decode()?;
            Instr::ReturnCallIndirect {
                ty,
                table: d.decode()?,
            }
        }
        0x1a => Instr::Drop,
        0x1b => Instr::Select,
        0x1c => {
            if d.decode::<u32>()? != 1 {
                return err("invalid result arity");
            }
            Instr::SelectT(val_type(d)?)
        }
        0x20 => Instr::LocalGet(d.decode()?),
        0x21 => Instr::LocalSet(d.decode()?),
        0x22 => Instr::LocalTee(d.decode()?),
        0x23 => Instr::GlobalGet(d.decode()?),
        0x24 => Instr::GlobalSet(d.decode()?),
        0x25 => Instr::TableGet(d.decode()?),
        0x26 => Instr::TableSet(d.decode()?),
        0x28..=0x35 => Instr::Load(op, mem_arg(d, features)?),
        0x36..=0x3e => Instr::Store(op, mem_arg(d, features)?),
        0x3f => Instr::MemorySize(mem_index(d, features)?),
        0x40 => Instr::MemoryGrow(mem_index(d, features)?),
        0x41 => Instr::I32Const(d.decode()?),
        0x42 => Instr::I64Const(d.decode()?),
        0x43 => Instr::F32Const(u32::from_le_bytes(d.read_bytes(4)?.try_into().unwrap())),
        0x44 => Instr::F64Const(u64::from_le_bytes(d.read_bytes(8)?.try_into().unwrap())),
        0x45..=0xc4 => Instr::Num(op),
        0xd0 => Instr::RefNull(ref_type(d)?),
        0xd1 => Instr::RefIsNull,
        0xd2 => Instr::RefFunc(d.decode()?),
        0xfc => {
            let sub: u32 = d.decode()?;
            match sub {
                0..=7 => Instr::TruncSat(sub as u8),
                8 => {
                    let data = d.decode()?;
                    Instr::MemoryInit {
                        data,
                        memory: mem_index(d, features)?,
                    }
                }
                9 => Instr::DataDrop(d.decode()?),
                10 => {
                    let dst = mem_index(d, features)?;
                    Instr::MemoryCopy {
                        dst,
                        src: mem_index(d, features)?,
                    }
                }
                11 => Instr::MemoryFill(mem_index(d, features)?),
                12 => {
                    let elem = d.decode()?;
                    Instr::TableInit {
                        elem,
                        table: d.decode()?,
                    }
                }
                13 => Instr::ElemDrop(d.decode()?),
                14 => {
                    let dst = d.decode()?;
                    Instr::TableCopy {
                        dst,
                        src: d.decode()?,
                    }
                }
                15 => Instr::TableGrow(d.decode()?),
                16 => Instr::TableSize(d.decode()?),
                17 => Instr::TableFill(d.decode()?),
                _ => return err(format!("unknown opcode 0xfc {sub}")),
            }
        }
        0xfd => {
            let sub: u32 = d.decode()?;
            if sub > 0x113 {
                return err(format!("unknown opcode 0xfd {sub}"));
            }
            let sub = sub as u16;
            match sub {
                0x00..=0x0b | 0x5c | 0x5d => Instr::SimdMem(sub, mem_arg(d, features)?),
                0x0c => Instr::V128Const(bytes16(d)?),
                0x0d => Instr::I8x16Shuffle(bytes16(d)?),
                0x15..=0x22 => Instr::SimdLane(sub, d.read_byte()?),
                0x54..=0x5b => {
                    let arg = mem_arg(d, features)?;
                    Instr::SimdMemLane(sub, arg, d.read_byte()?)
                }
                _ => Instr::Simd(sub),
            }
        }
        0xfe => {
            let sub: u32 = d.decode()?;
            match sub {
                0x03 => {
                    if d.read_byte()? != 0 {
                        return err("malformed atomic.fence");
                    }
                    Instr::AtomicFence
                }
                0x00..=0x02 | 0x10..=0x4e => Instr::Atomic(sub as u16, mem_arg(d, features)?),
                _ => return err(format!("unknown opcode 0xfe {sub}")),
            }
        }
        0xe0 if features.ext_math => {
            let sub = d.read_byte()?;
            if ext_math_class(sub).is_none() {
                return err(format!("unknown opcode 0xe0 {sub}"));
            }
            Instr::ExtMath(sub)
        }
        _ => return err(format!("unknown opcode 0x{op:02x}")),
    })
}

/// The shape of a nonstandard 0xe0 opcode: the operand type of a unary or
/// binary scalar op, or a v128 unary/binary op, or a v128 x v128 -> f32
/// reduction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExtMathClass {
    Un(ValType),
    Bin(ValType),
    Reduce,
}

pub(crate) fn ext_math_class(sub: u8) -> Option<ExtMathClass> {
    use ExtMathClass::*;
    Some(match sub {
        0x00..=0x07 => Un(ValType::F32),
        0x08..=0x0c => Bin(ValType::F32),
        0x10..=0x17 => Un(ValType::F64),
        0x18..=0x1c => Bin(ValType::F64),
        0x20..=0x27 => Un(ValType::V128),
        0x28..=0x2c => Bin(ValType::V128),
        0x2d..=0x2f => Reduce,
        _ => return None,
    })
}

/// Decodes instructions up to and including the `End` that closes depth 0.
fn expr(d: &mut Decoder, features: Features) -> Result<Vec<Instr>, DecodeError> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    loop {
        let instr = instr(d, features)?;
        match instr {
            Instr::Block(_) | Instr::Loop(_) | Instr::If(_) => depth += 1,
            Instr::End => {
                if depth == 0 {
                    out.push(instr);
                    return Ok(out);
                }
                depth -= 1;
            }
            _ => {}
        }
        out.push(instr);
    }
}

/// Where every instruction of a function body sits: `entry` is the body's
/// code section entry (size prefix included, see [`Layout::funcs`]); the
/// offsets are relative to its start, one per instruction of
/// [`Func::body`].
pub fn body_offsets(entry: &[u8], features: Features) -> Result<Vec<usize>, DecodeError> {
    let mut d = Decoder::new(entry);
    let size: u32 = d.decode()?;
    if d.remaining() != size as usize {
        return err("section size mismatch in function body");
    }
    let groups: u32 = d.decode()?;
    for _ in 0..groups {
        d.decode::<u32>()?;
        val_type(&mut d)?;
    }
    let mut out = Vec::new();
    while !d.is_at_end() {
        out.push(d.position());
        instr(&mut d, features)?;
    }
    Ok(out)
}

/// Engines refuse functions with more locals than this; a body declaring
/// more is not a usable module.
const MAX_LOCALS: u64 = 50_000;

fn func_body(bytes: &[u8], ty: u32, features: Features) -> Result<Func, DecodeError> {
    let mut d = Decoder::new(bytes);
    let groups: u32 = d.decode()?;
    let mut locals = Vec::new();
    let mut total = 0u64;
    for _ in 0..groups {
        let count: u32 = d.decode()?;
        let ty = val_type(&mut d)?;
        total += count as u64;
        if total > MAX_LOCALS {
            return err("too many locals");
        }
        locals.extend(std::iter::repeat(ty).take(count as usize));
    }
    let body = expr(&mut d, features)?;
    if !d.is_at_end() {
        return err("section size mismatch in function body");
    }
    Ok(Func { ty, locals, body })
}

fn vec_count(d: &mut Decoder) -> Result<usize, DecodeError> {
    let count: u32 = d.decode()?;
    // Every entry takes at least one byte.
    if count as usize > d.remaining() {
        return err("count exceeds section size");
    }
    Ok(count as usize)
}

fn name(d: &mut Decoder) -> Result<String, DecodeError> {
    let len: u32 = d.decode()?;
    let bytes = d.read_bytes(len as usize)?;
    String::from_utf8(bytes.to_vec()).map_err(|_| DecodeError::new("malformed UTF-8 encoding"))
}

fn elem_segment(d: &mut Decoder, features: Features) -> Result<Elem, DecodeError> {
    let flags: u32 = d.decode()?;
    if flags > 7 {
        return err(format!("malformed elements segment kind {flags}"));
    }
    let passive_or_declarative = flags & 1 != 0;
    let explicit_table = flags & 2 != 0;
    let uses_exprs = flags & 4 != 0;
    let mode = if passive_or_declarative {
        if explicit_table {
            ElemMode::Declarative
        } else {
            ElemMode::Passive
        }
    } else {
        let table = if explicit_table { d.decode()? } else { 0 };
        ElemMode::Active {
            table,
            offset: expr(d, features)?,
        }
    };
    // Flags 0 and 4 have an implicit element type; the others spell it.
    let explicit_type = flags & 3 != 0;
    let items = if uses_exprs {
        let ty = if explicit_type {
            ref_type(d)?
        } else {
            ValType::FuncRef
        };
        let count = vec_count(d)?;
        let mut exprs = Vec::with_capacity(count);
        for _ in 0..count {
            exprs.push(expr(d, features)?);
        }
        ElemItems::Exprs(ty, exprs)
    } else {
        if explicit_type && d.read_byte()? != 0x00 {
            return err("malformed element kind");
        }
        let count = vec_count(d)?;
        let mut funcs = Vec::with_capacity(count);
        for _ in 0..count {
            funcs.push(d.decode()?);
        }
        ElemItems::Funcs(funcs)
    };
    Ok(Elem { mode, items })
}

fn data_segment(
    d: &mut Decoder,
    features: Features,
    base: usize,
    layout: &mut Option<&mut Layout>,
) -> Result<Data, DecodeError> {
    let mode = match d.decode::<u32>()? {
        0 => DataMode::Active {
            memory: 0,
            offset: expr(d, features)?,
        },
        1 => DataMode::Passive,
        2 => {
            let memory = d.decode()?;
            DataMode::Active {
                memory,
                offset: expr(d, features)?,
            }
        }
        flags => return err(format!("malformed data segment kind {flags}")),
    };
    let len: u32 = d.decode()?;
    let start = base + d.position();
    let bytes = d.read_bytes(len as usize)?.to_vec();
    if let Some(layout) = layout {
        layout.datas.push(start..start + bytes.len());
    }
    Ok(Data { mode, bytes })
}

fn name_map(d: &mut Decoder) -> Result<Vec<(u32, String)>, DecodeError> {
    let count = vec_count(d)?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let index = d.decode()?;
        out.push((index, name(d)?));
    }
    Ok(out)
}

/// Reads the subsections of a `name` section that a rewriting tool can keep
/// correct under renumbering; the others (local names, labels, ...) are left
/// out.
fn names_section(data: &[u8], after_rank: u8) -> Result<Names, DecodeError> {
    let mut d = Decoder::new(data);
    let mut names = Names {
        after_rank,
        ..Names::default()
    };
    while !d.is_at_end() {
        let id = d.read_byte()?;
        let len: u32 = d.decode()?;
        let mut sub = Decoder::new(d.read_bytes(len as usize)?);
        match id {
            0 => names.module = Some(name(&mut sub)?),
            1 => names.funcs = name_map(&mut sub)?,
            7 => names.globals = name_map(&mut sub)?,
            _ => {}
        }
    }
    Ok(names)
}

fn decode_module(
    buf: &[u8],
    features: Features,
    mut layout: Option<&mut Layout>,
) -> Result<Module, DecodeError> {
    let mut d = Decoder::new(buf);
    if d.read_bytes(4).ok() != Some(b"\0asm".as_slice()) {
        return err("magic header not detected");
    }
    if d.read_bytes(4)? != [1, 0, 0, 0] {
        return err("unknown binary version");
    }
    let mut module = Module::default();
    let mut func_types: Vec<u32> = Vec::new();
    let mut code_seen = false;
    let mut last_rank = 0u8;
    while !d.is_at_end() {
        let section_start = d.position();
        let id = d.read_byte()?;
        let len: u32 = d.decode()?;
        let base = d.position();
        let payload = d.read_bytes(len as usize)?;
        let mut s = Decoder::new(payload);
        if let Some(layout) = &mut layout {
            layout.sections.push(SectionLayout {
                id,
                name: if id == 0 {
                    name(&mut s.clone())?
                } else {
                    String::new()
                },
                bytes: section_start..d.position(),
                payload: base..d.position(),
            });
        }
        if id == 0 {
            let name = name(&mut s)?;
            let data = payload[s.position()..].to_vec();
            if name == "name" {
                // A malformed name section is debug info we can drop.
                module.names = names_section(&data, last_rank).ok();
            } else {
                module.customs.push(Custom {
                    name,
                    data,
                    after_rank: last_rank,
                });
            }
            continue;
        }
        let rank =
            section_rank(id).ok_or_else(|| DecodeError::new(format!("malformed section id {id}")))?;
        if rank <= last_rank {
            return err("unexpected content after last section");
        }
        last_rank = rank;
        match id {
            1 => {
                for _ in 0..vec_count(&mut s)? {
                    if s.read_byte()? != 0x60 {
                        return err("malformed function type");
                    }
                    let mut ty = FuncType::default();
                    for _ in 0..vec_count(&mut s)? {
                        ty.params.push(val_type(&mut s)?);
                    }
                    for _ in 0..vec_count(&mut s)? {
                        ty.results.push(val_type(&mut s)?);
                    }
                    module.types.push(ty);
                }
            }
            2 => {
                for _ in 0..vec_count(&mut s)? {
                    let module_name = name(&mut s)?;
                    let name = name(&mut s)?;
                    let desc = match s.read_byte()? {
                        0 => ImportDesc::Func(s.decode()?),
                        1 => ImportDesc::Table(table_type(&mut s)?),
                        2 => ImportDesc::Memory(mem_type(&mut s)?),
                        3 => ImportDesc::Global(global_type(&mut s)?),
                        kind => return err(format!("unsupported import kind {kind}")),
                    };
                    module.imports.push(Import {
                        module: module_name,
                        name,
                        desc,
                    });
                }
            }
            3 => {
                for _ in 0..vec_count(&mut s)? {
                    func_types.push(s.decode()?);
                }
            }
            4 => {
                for _ in 0..vec_count(&mut s)? {
                    module.tables.push(table_type(&mut s)?);
                }
            }
            5 => {
                for _ in 0..vec_count(&mut s)? {
                    module.memories.push(mem_type(&mut s)?);
                }
            }
            6 => {
                for _ in 0..vec_count(&mut s)? {
                    let ty = global_type(&mut s)?;
                    module.globals.push(Global {
                        ty,
                        init: expr(&mut s, features)?,
                    });
                }
            }
            7 => {
                for _ in 0..vec_count(&mut s)? {
                    let name = name(&mut s)?;
                    let kind = match s.read_byte()? {
                        0 => ExternKind::Func,
                        1 => ExternKind::Table,
                        2 => ExternKind::Memory,
                        3 => ExternKind::Global,
                        kind => return err(format!("unsupported export kind {kind}")),
                    };
                    module.exports.push(Export {
                        name,
                        kind,
                        index: s.decode()?,
                    });
                }
            }
            8 => module.start = Some(s.decode()?),
            9 => {
                for _ in 0..vec_count(&mut s)? {
                    module.elems.push(elem_segment(&mut s, features)?);
                }
            }
            12 => module.data_count = Some(s.decode()?),
            10 => {
                code_seen = true;
                let count = vec_count(&mut s)?;
                if count != func_types.len() {
                    return err("function and code section have inconsistent lengths");
                }
                for ty in func_types.iter().copied() {
                    let start = base + s.position();
                    let size: u32 = s.decode()?;
                    module
                        .funcs
                        .push(func_body(s.read_bytes(size as usize)?, ty, features)?);
                    if let Some(layout) = &mut layout {
                        layout.funcs.push(start..base + s.position());
                    }
                }
            }
            11 => {
                for _ in 0..vec_count(&mut s)? {
                    module
                        .datas
                        .push(data_segment(&mut s, features, base, &mut layout)?);
                }
            }
            _ => unreachable!(),
        }
        if !s.is_at_end() {
            return err(format!("section size mismatch in section {id}"));
        }
    }
    if !code_seen && !func_types.is_empty() {
        return err("function and code section have inconsistent lengths");
    }
    Ok(module)
}
