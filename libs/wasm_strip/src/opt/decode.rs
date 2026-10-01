//! Binary decoder: bytes to `Module`. Decodes every section and every
//! instruction; anything it does not understand (memory64, exceptions, GC,
//! unknown opcodes) is an error, so the optimiser never rewrites code it
//! cannot see.

use super::ir::*;
use super::Res;

pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, pos: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn u8(&mut self) -> Res<u8> {
        let byte = *self
            .bytes
            .get(self.pos)
            .ok_or_else(|| "unexpected end".to_string())?;
        self.pos += 1;
        Ok(byte)
    }

    pub fn bytes(&mut self, len: usize) -> Res<&'a [u8]> {
        if self.bytes.len() - self.pos < len {
            return Err("unexpected end".into());
        }
        let out = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        Ok(out)
    }

    fn leb_u(&mut self, bits: u32) -> Res<u64> {
        let mut result = 0u64;
        let mut shift = 0;
        loop {
            let byte = self.u8()?;
            if shift >= bits {
                return Err("integer representation too long".into());
            }
            result |= ((byte & 0x7f) as u64) << shift;
            if shift + 7 > bits && (byte & 0x7f) >> (bits - shift) != 0 {
                return Err("integer too large".into());
            }
            shift += 7;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
        }
    }

    fn leb_s(&mut self, bits: u32) -> Res<i64> {
        let mut result = 0i64;
        let mut shift = 0;
        loop {
            let byte = self.u8()?;
            if shift >= bits {
                return Err("integer representation too long".into());
            }
            result |= ((byte & 0x7f) as i64) << shift;
            if shift + 7 > bits {
                // The unused bits of the last byte must repeat its sign bit.
                let used = bits - shift;
                let payload = byte & 0x7f;
                let high = payload >> used;
                let expected = if (payload >> (used - 1)) & 1 != 0 {
                    (1u8 << (7 - used)) - 1
                } else {
                    0
                };
                if byte & 0x80 != 0 || high != expected {
                    return Err("integer too large".into());
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

    pub fn u32(&mut self) -> Res<u32> {
        Ok(self.leb_u(32)? as u32)
    }

    pub fn i32(&mut self) -> Res<i32> {
        Ok(self.leb_s(32)? as i32)
    }

    pub fn i64(&mut self) -> Res<i64> {
        self.leb_s(64)
    }

    pub fn s33(&mut self) -> Res<i64> {
        self.leb_s(33)
    }

    pub fn name(&mut self) -> Res<String> {
        let len = self.u32()? as usize;
        let bytes = self.bytes(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| "malformed UTF-8 name".to_string())
    }
}

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

fn val_type(r: &mut Reader) -> Res<ValType> {
    let byte = r.u8()?;
    ValType::from_byte(byte).ok_or_else(|| format!("unknown value type 0x{byte:02x}"))
}

fn ref_type(r: &mut Reader) -> Res<ValType> {
    let ty = val_type(r)?;
    if !ty.is_ref() {
        return Err("expected a reference type".into());
    }
    Ok(ty)
}

fn limits(r: &mut Reader, allow_shared: bool) -> Res<(Limits, bool)> {
    let flags = r.u8()?;
    let (has_max, shared) = match flags {
        0 => (false, false),
        1 => (true, false),
        2 if allow_shared => (false, true),
        3 if allow_shared => (true, true),
        _ => return Err(format!("unsupported limits flags 0x{flags:02x}")),
    };
    let min = r.u32()?;
    let max = if has_max { Some(r.u32()?) } else { None };
    Ok((Limits { min, max }, shared))
}

fn table_type(r: &mut Reader) -> Res<TableType> {
    let elem = ref_type(r)?;
    let (limits, _) = limits(r, false)?;
    Ok(TableType { elem, limits })
}

fn mem_type(r: &mut Reader) -> Res<MemType> {
    let (limits, shared) = limits(r, true)?;
    Ok(MemType { limits, shared })
}

fn global_type(r: &mut Reader) -> Res<GlobalType> {
    let ty = val_type(r)?;
    let mutable = match r.u8()? {
        0 => false,
        1 => true,
        _ => return Err("malformed mutability".into()),
    };
    Ok(GlobalType { ty, mutable })
}

fn block_type(r: &mut Reader) -> Res<BlockType> {
    let peek = *r.bytes.get(r.pos).ok_or_else(|| "unexpected end".to_string())?;
    if peek == 0x40 {
        r.pos += 1;
        return Ok(BlockType::Empty);
    }
    if let Some(ty) = ValType::from_byte(peek) {
        r.pos += 1;
        return Ok(BlockType::Value(ty));
    }
    let index = r.s33()?;
    if index < 0 || index > u32::MAX as i64 {
        return Err("malformed block type".into());
    }
    Ok(BlockType::Func(index as u32))
}

fn mem_arg(r: &mut Reader) -> Res<MemArg> {
    let mut align = r.u32()?;
    let memory = if align & 0x40 != 0 {
        align &= !0x40;
        r.u32()?
    } else {
        0
    };
    let offset = r.u32()?;
    Ok(MemArg { align, offset, memory })
}

fn lane(r: &mut Reader) -> Res<u8> {
    r.u8()
}

fn bytes16(r: &mut Reader) -> Res<Box<[u8; 16]>> {
    let mut out = [0u8; 16];
    out.copy_from_slice(r.bytes(16)?);
    Ok(Box::new(out))
}

pub fn instr(r: &mut Reader) -> Res<Instr> {
    let op = r.u8()?;
    Ok(match op {
        0x00 => Instr::Unreachable,
        0x01 => Instr::Nop,
        0x02 => Instr::Block(block_type(r)?),
        0x03 => Instr::Loop(block_type(r)?),
        0x04 => Instr::If(block_type(r)?),
        0x05 => Instr::Else,
        0x0b => Instr::End,
        0x0c => Instr::Br(r.u32()?),
        0x0d => Instr::BrIf(r.u32()?),
        0x0e => {
            let count = r.u32()? as usize;
            if count > r.bytes.len() {
                return Err("br_table too long".into());
            }
            let mut targets = Vec::with_capacity(count);
            for _ in 0..count {
                targets.push(r.u32()?);
            }
            Instr::BrTable(targets.into_boxed_slice(), r.u32()?)
        }
        0x0f => Instr::Return,
        0x10 => Instr::Call(r.u32()?),
        0x11 => {
            let ty = r.u32()?;
            Instr::CallIndirect { ty, table: r.u32()? }
        }
        0x12 => Instr::ReturnCall(r.u32()?),
        0x13 => {
            let ty = r.u32()?;
            Instr::ReturnCallIndirect { ty, table: r.u32()? }
        }
        0x1a => Instr::Drop,
        0x1b => Instr::Select,
        0x1c => {
            if r.u32()? != 1 {
                return Err("invalid result arity".into());
            }
            Instr::SelectT(val_type(r)?)
        }
        0x20 => Instr::LocalGet(r.u32()?),
        0x21 => Instr::LocalSet(r.u32()?),
        0x22 => Instr::LocalTee(r.u32()?),
        0x23 => Instr::GlobalGet(r.u32()?),
        0x24 => Instr::GlobalSet(r.u32()?),
        0x25 => Instr::TableGet(r.u32()?),
        0x26 => Instr::TableSet(r.u32()?),
        0x28..=0x35 => Instr::Load(op, mem_arg(r)?),
        0x36..=0x3e => Instr::Store(op, mem_arg(r)?),
        0x3f => Instr::MemorySize(r.u32()?),
        0x40 => Instr::MemoryGrow(r.u32()?),
        0x41 => Instr::I32Const(r.i32()?),
        0x42 => Instr::I64Const(r.i64()?),
        0x43 => Instr::F32Const(u32::from_le_bytes(r.bytes(4)?.try_into().unwrap())),
        0x44 => Instr::F64Const(u64::from_le_bytes(r.bytes(8)?.try_into().unwrap())),
        0x45..=0xc4 => Instr::Num(op),
        0xd0 => Instr::RefNull(ref_type(r)?),
        0xd1 => Instr::RefIsNull,
        0xd2 => Instr::RefFunc(r.u32()?),
        0xfc => {
            let sub = r.u32()?;
            match sub {
                0..=7 => Instr::TruncSat(sub as u8),
                8 => {
                    let data = r.u32()?;
                    Instr::MemoryInit { data, memory: r.u32()? }
                }
                9 => Instr::DataDrop(r.u32()?),
                10 => {
                    let dst = r.u32()?;
                    Instr::MemoryCopy { dst, src: r.u32()? }
                }
                11 => Instr::MemoryFill(r.u32()?),
                12 => {
                    let elem = r.u32()?;
                    Instr::TableInit { elem, table: r.u32()? }
                }
                13 => Instr::ElemDrop(r.u32()?),
                14 => {
                    let dst = r.u32()?;
                    Instr::TableCopy { dst, src: r.u32()? }
                }
                15 => Instr::TableGrow(r.u32()?),
                16 => Instr::TableSize(r.u32()?),
                17 => Instr::TableFill(r.u32()?),
                _ => return Err(format!("unknown 0xfc opcode {sub}")),
            }
        }
        0xfd => {
            let sub = r.u32()?;
            if sub > 0x113 {
                return Err(format!("unknown 0xfd opcode {sub}"));
            }
            let sub = sub as u16;
            match sub {
                0x00..=0x0b | 0x5c | 0x5d => Instr::SimdMem(sub, mem_arg(r)?),
                0x0c => Instr::V128Const(bytes16(r)?),
                0x0d => Instr::I8x16Shuffle(bytes16(r)?),
                0x15..=0x22 => Instr::SimdLane(sub, lane(r)?),
                0x54..=0x5b => {
                    let arg = mem_arg(r)?;
                    Instr::SimdMemLane(sub, arg, lane(r)?)
                }
                _ => Instr::Simd(sub),
            }
        }
        0xfe => {
            let sub = r.u32()?;
            match sub {
                0x03 => {
                    if r.u8()? != 0 {
                        return Err("malformed atomic.fence".into());
                    }
                    Instr::AtomicFence
                }
                0x00..=0x02 | 0x10..=0x4e => Instr::Atomic(sub as u16, mem_arg(r)?),
                _ => return Err(format!("unknown 0xfe opcode {sub}")),
            }
        }
        _ => return Err(format!("unknown opcode 0x{op:02x}")),
    })
}

/// Decodes instructions up to and including the `End` that closes depth 0.
pub fn expr(r: &mut Reader) -> Res<Vec<Instr>> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    loop {
        let instr = instr(r)?;
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

/// Engines refuse functions with more locals than this; a body declaring
/// more is not a usable module.
const MAX_LOCALS: u64 = 50_000;

fn func_body(bytes: &[u8], ty: u32) -> Res<Func> {
    let mut r = Reader::new(bytes);
    let groups = r.u32()?;
    let mut locals = Vec::new();
    let mut total = 0u64;
    for _ in 0..groups {
        let count = r.u32()?;
        let ty = val_type(&mut r)?;
        total += count as u64;
        if total > MAX_LOCALS {
            return Err("too many locals".into());
        }
        locals.extend(std::iter::repeat(ty).take(count as usize));
    }
    let body = expr(&mut r)?;
    if !r.is_empty() {
        return Err("section size mismatch in function body".into());
    }
    Ok(Func { ty, locals, body })
}

fn vec_count(r: &mut Reader) -> Res<usize> {
    let count = r.u32()? as usize;
    // Every entry takes at least one byte.
    if count > r.bytes.len() - r.pos {
        return Err("count exceeds section size".into());
    }
    Ok(count)
}

fn elem_segment(r: &mut Reader) -> Res<Elem> {
    let flags = r.u32()?;
    if flags > 7 {
        return Err(format!("malformed elements segment kind {flags}"));
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
        let table = if explicit_table { r.u32()? } else { 0 };
        ElemMode::Active { table, offset: expr(r)? }
    };
    // Flags 0 and 4 have an implicit element type; the others spell it.
    let explicit_type = flags & 3 != 0;
    let items = if uses_exprs {
        let ty = if explicit_type { ref_type(r)? } else { ValType::FuncRef };
        let count = vec_count(r)?;
        let mut exprs = Vec::with_capacity(count);
        for _ in 0..count {
            exprs.push(expr(r)?);
        }
        ElemItems::Exprs(ty, exprs)
    } else {
        if explicit_type && r.u8()? != 0x00 {
            return Err("malformed element kind".into());
        }
        let count = vec_count(r)?;
        let mut funcs = Vec::with_capacity(count);
        for _ in 0..count {
            funcs.push(r.u32()?);
        }
        ElemItems::Funcs(funcs)
    };
    Ok(Elem { mode, items })
}

fn data_segment(r: &mut Reader) -> Res<Data> {
    let mode = match r.u32()? {
        0 => DataMode::Active { memory: 0, offset: expr(r)? },
        1 => DataMode::Passive,
        2 => {
            let memory = r.u32()?;
            DataMode::Active { memory, offset: expr(r)? }
        }
        flags => return Err(format!("malformed data segment kind {flags}")),
    };
    let len = r.u32()? as usize;
    let bytes = r.bytes(len)?.to_vec();
    Ok(Data { mode, bytes })
}

fn name_map(r: &mut Reader) -> Res<Vec<(u32, String)>> {
    let count = vec_count(r)?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let index = r.u32()?;
        out.push((index, r.name()?));
    }
    Ok(out)
}

/// Reads the subsections of a `name` section that the optimiser can keep
/// correct under renumbering; the others (local names, labels, ...) are left
/// out.
fn names_section(data: &[u8], after_rank: u8) -> Res<Names> {
    let mut r = Reader::new(data);
    let mut names = Names {
        after_rank,
        ..Names::default()
    };
    while !r.is_empty() {
        let id = r.u8()?;
        let len = r.u32()? as usize;
        let mut sub = Reader::new(r.bytes(len)?);
        match id {
            0 => names.module = Some(sub.name()?),
            1 => names.funcs = name_map(&mut sub)?,
            7 => names.globals = name_map(&mut sub)?,
            _ => {}
        }
    }
    Ok(names)
}

pub fn decode(buf: &[u8]) -> Res<Module> {
    let mut r = Reader::new(buf);
    if r.bytes(4).map_err(|_| "not a wasm file")? != b"\0asm" {
        return Err("not a wasm file".into());
    }
    if r.bytes(4)? != [1, 0, 0, 0] {
        return Err("unsupported wasm version".into());
    }
    let mut module = Module::default();
    let mut func_types: Vec<u32> = Vec::new();
    let mut code_seen = false;
    let mut last_rank = 0u8;
    while !r.is_empty() {
        let id = r.u8()?;
        let len = r.u32()? as usize;
        let payload = r.bytes(len)?;
        let mut s = Reader::new(payload);
        if id == 0 {
            let name = s.name()?;
            let data = payload[s.pos()..].to_vec();
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
        let rank = section_rank(id).ok_or_else(|| format!("unsupported section id {id}"))?;
        if rank <= last_rank {
            return Err("unexpected content after last section".into());
        }
        last_rank = rank;
        match id {
            1 => {
                for _ in 0..vec_count(&mut s)? {
                    if s.u8()? != 0x60 {
                        return Err("unsupported type form".into());
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
                    let module_name = s.name()?;
                    let name = s.name()?;
                    let desc = match s.u8()? {
                        0 => ImportDesc::Func(s.u32()?),
                        1 => ImportDesc::Table(table_type(&mut s)?),
                        2 => ImportDesc::Memory(mem_type(&mut s)?),
                        3 => ImportDesc::Global(global_type(&mut s)?),
                        kind => return Err(format!("unsupported import kind {kind}")),
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
                    func_types.push(s.u32()?);
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
                    module.globals.push(Global { ty, init: expr(&mut s)? });
                }
            }
            7 => {
                for _ in 0..vec_count(&mut s)? {
                    let name = s.name()?;
                    let kind = match s.u8()? {
                        0 => ExternKind::Func,
                        1 => ExternKind::Table,
                        2 => ExternKind::Memory,
                        3 => ExternKind::Global,
                        kind => return Err(format!("unsupported export kind {kind}")),
                    };
                    module.exports.push(Export {
                        name,
                        kind,
                        index: s.u32()?,
                    });
                }
            }
            8 => module.start = Some(s.u32()?),
            9 => {
                for _ in 0..vec_count(&mut s)? {
                    module.elems.push(elem_segment(&mut s)?);
                }
            }
            12 => module.data_count = Some(s.u32()?),
            10 => {
                code_seen = true;
                let count = vec_count(&mut s)?;
                if count != func_types.len() {
                    return Err("function and code section have inconsistent lengths".into());
                }
                for ty in func_types.iter().copied() {
                    let size = s.u32()? as usize;
                    module.funcs.push(func_body(s.bytes(size)?, ty)?);
                }
            }
            11 => {
                for _ in 0..vec_count(&mut s)? {
                    module.datas.push(data_segment(&mut s)?);
                }
            }
            _ => unreachable!(),
        }
        if !s.is_empty() {
            return Err(format!("section size mismatch in section {id}"));
        }
    }
    if !code_seen && !func_types.is_empty() {
        return Err("function and code section have inconsistent lengths".into());
    }
    Ok(module)
}
