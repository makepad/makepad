//! The module the optimiser works on: every section decoded, function bodies
//! as flat instruction lists (structured control kept as Block/Loop/If/Else/End
//! markers, exactly as the binary format has them).

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum ValType {
    I32,
    I64,
    F32,
    F64,
    V128,
    FuncRef,
    ExternRef,
}

impl ValType {
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

    pub fn is_ref(self) -> bool {
        matches!(self, ValType::FuncRef | ValType::ExternRef)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct FuncType {
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Limits {
    pub min: u32,
    pub max: Option<u32>,
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
/// rank (see `section_rank`), 0 meaning before every standard section.
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
    /// Where the `name` section sat, as for `Custom`.
    pub after_rank: u8,
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
}

impl Instr {
    /// Instructions that open a structured block.
    pub fn is_block_start(&self) -> bool {
        matches!(self, Instr::Block(_) | Instr::Loop(_) | Instr::If(_))
    }
}
