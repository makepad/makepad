//! `Module` to bytes. Every integer is written as the shortest LEB128, so
//! encoding a decoded module is the LEB re-encoding pass.

use super::ir::*;

pub fn u32(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

pub fn i64(out: &mut Vec<u8>, mut value: i64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        let done = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
        if done {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

pub fn i32(out: &mut Vec<u8>, value: i32) {
    i64(out, value as i64)
}

fn name(out: &mut Vec<u8>, name: &str) {
    u32(out, name.len() as u32);
    out.extend_from_slice(name.as_bytes());
}

fn limits(out: &mut Vec<u8>, limits: &Limits, shared: bool) {
    let flags = (limits.max.is_some() as u8) | ((shared as u8) << 1);
    out.push(flags);
    u32(out, limits.min);
    if let Some(max) = limits.max {
        u32(out, max);
    }
}

fn table_type(out: &mut Vec<u8>, ty: &TableType) {
    out.push(ty.elem.to_byte());
    limits(out, &ty.limits, false);
}

fn global_type(out: &mut Vec<u8>, ty: &GlobalType) {
    out.push(ty.ty.to_byte());
    out.push(ty.mutable as u8);
}

fn block_type(out: &mut Vec<u8>, ty: &BlockType) {
    match ty {
        BlockType::Empty => out.push(0x40),
        BlockType::Value(ty) => out.push(ty.to_byte()),
        BlockType::Func(index) => i64(out, *index as i64),
    }
}

fn mem_arg(out: &mut Vec<u8>, arg: &MemArg) {
    if arg.memory != 0 {
        u32(out, arg.align | 0x40);
        u32(out, arg.memory);
    } else {
        u32(out, arg.align);
    }
    u32(out, arg.offset);
}

fn prefixed(out: &mut Vec<u8>, prefix: u8, sub: u32) {
    out.push(prefix);
    u32(out, sub);
}

pub fn instr(out: &mut Vec<u8>, instr: &Instr) {
    match instr {
        Instr::Unreachable => out.push(0x00),
        Instr::Nop => out.push(0x01),
        Instr::Block(ty) => {
            out.push(0x02);
            block_type(out, ty);
        }
        Instr::Loop(ty) => {
            out.push(0x03);
            block_type(out, ty);
        }
        Instr::If(ty) => {
            out.push(0x04);
            block_type(out, ty);
        }
        Instr::Else => out.push(0x05),
        Instr::End => out.push(0x0b),
        Instr::Br(depth) => {
            out.push(0x0c);
            u32(out, *depth);
        }
        Instr::BrIf(depth) => {
            out.push(0x0d);
            u32(out, *depth);
        }
        Instr::BrTable(targets, default) => {
            out.push(0x0e);
            u32(out, targets.len() as u32);
            for target in targets.iter() {
                u32(out, *target);
            }
            u32(out, *default);
        }
        Instr::Return => out.push(0x0f),
        Instr::Call(func) => {
            out.push(0x10);
            u32(out, *func);
        }
        Instr::CallIndirect { ty, table } => {
            out.push(0x11);
            u32(out, *ty);
            u32(out, *table);
        }
        Instr::ReturnCall(func) => {
            out.push(0x12);
            u32(out, *func);
        }
        Instr::ReturnCallIndirect { ty, table } => {
            out.push(0x13);
            u32(out, *ty);
            u32(out, *table);
        }
        Instr::Drop => out.push(0x1a),
        Instr::Select => out.push(0x1b),
        Instr::SelectT(ty) => {
            out.push(0x1c);
            u32(out, 1);
            out.push(ty.to_byte());
        }
        Instr::LocalGet(index) => {
            out.push(0x20);
            u32(out, *index);
        }
        Instr::LocalSet(index) => {
            out.push(0x21);
            u32(out, *index);
        }
        Instr::LocalTee(index) => {
            out.push(0x22);
            u32(out, *index);
        }
        Instr::GlobalGet(index) => {
            out.push(0x23);
            u32(out, *index);
        }
        Instr::GlobalSet(index) => {
            out.push(0x24);
            u32(out, *index);
        }
        Instr::TableGet(index) => {
            out.push(0x25);
            u32(out, *index);
        }
        Instr::TableSet(index) => {
            out.push(0x26);
            u32(out, *index);
        }
        Instr::Load(op, arg) | Instr::Store(op, arg) => {
            out.push(*op);
            mem_arg(out, arg);
        }
        Instr::MemorySize(memory) => {
            out.push(0x3f);
            u32(out, *memory);
        }
        Instr::MemoryGrow(memory) => {
            out.push(0x40);
            u32(out, *memory);
        }
        Instr::I32Const(value) => {
            out.push(0x41);
            i32(out, *value);
        }
        Instr::I64Const(value) => {
            out.push(0x42);
            i64(out, *value);
        }
        Instr::F32Const(bits) => {
            out.push(0x43);
            out.extend_from_slice(&bits.to_le_bytes());
        }
        Instr::F64Const(bits) => {
            out.push(0x44);
            out.extend_from_slice(&bits.to_le_bytes());
        }
        Instr::Num(op) => out.push(*op),
        Instr::RefNull(ty) => {
            out.push(0xd0);
            out.push(ty.to_byte());
        }
        Instr::RefIsNull => out.push(0xd1),
        Instr::RefFunc(func) => {
            out.push(0xd2);
            u32(out, *func);
        }
        Instr::TruncSat(sub) => prefixed(out, 0xfc, *sub as u32),
        Instr::MemoryInit { data, memory } => {
            prefixed(out, 0xfc, 8);
            u32(out, *data);
            u32(out, *memory);
        }
        Instr::DataDrop(data) => {
            prefixed(out, 0xfc, 9);
            u32(out, *data);
        }
        Instr::MemoryCopy { dst, src } => {
            prefixed(out, 0xfc, 10);
            u32(out, *dst);
            u32(out, *src);
        }
        Instr::MemoryFill(memory) => {
            prefixed(out, 0xfc, 11);
            u32(out, *memory);
        }
        Instr::TableInit { elem, table } => {
            prefixed(out, 0xfc, 12);
            u32(out, *elem);
            u32(out, *table);
        }
        Instr::ElemDrop(elem) => {
            prefixed(out, 0xfc, 13);
            u32(out, *elem);
        }
        Instr::TableCopy { dst, src } => {
            prefixed(out, 0xfc, 14);
            u32(out, *dst);
            u32(out, *src);
        }
        Instr::TableGrow(table) => {
            prefixed(out, 0xfc, 15);
            u32(out, *table);
        }
        Instr::TableSize(table) => {
            prefixed(out, 0xfc, 16);
            u32(out, *table);
        }
        Instr::TableFill(table) => {
            prefixed(out, 0xfc, 17);
            u32(out, *table);
        }
        Instr::SimdMem(sub, arg) => {
            prefixed(out, 0xfd, *sub as u32);
            mem_arg(out, arg);
        }
        Instr::SimdMemLane(sub, arg, lane) => {
            prefixed(out, 0xfd, *sub as u32);
            mem_arg(out, arg);
            out.push(*lane);
        }
        Instr::V128Const(bytes) => {
            prefixed(out, 0xfd, 0x0c);
            out.extend_from_slice(&bytes[..]);
        }
        Instr::I8x16Shuffle(lanes) => {
            prefixed(out, 0xfd, 0x0d);
            out.extend_from_slice(&lanes[..]);
        }
        Instr::SimdLane(sub, lane) => {
            prefixed(out, 0xfd, *sub as u32);
            out.push(*lane);
        }
        Instr::Simd(sub) => prefixed(out, 0xfd, *sub as u32),
        Instr::Atomic(sub, arg) => {
            prefixed(out, 0xfe, *sub as u32);
            mem_arg(out, arg);
        }
        Instr::AtomicFence => {
            prefixed(out, 0xfe, 0x03);
            out.push(0);
        }
    }
}

pub fn expr(out: &mut Vec<u8>, instrs: &[Instr]) {
    for item in instrs {
        instr(out, item);
    }
}

/// A function body as it appears in the code section, without its size.
pub fn func_body(out: &mut Vec<u8>, func: &Func) {
    let mut groups: Vec<(u32, ValType)> = Vec::new();
    for ty in &func.locals {
        match groups.last_mut() {
            Some((count, last)) if last == ty => *count += 1,
            _ => groups.push((1, *ty)),
        }
    }
    u32(out, groups.len() as u32);
    for (count, ty) in groups {
        u32(out, count);
        out.push(ty.to_byte());
    }
    expr(out, &func.body);
}

fn section(out: &mut Vec<u8>, id: u8, payload: &[u8]) {
    out.push(id);
    u32(out, payload.len() as u32);
    out.extend_from_slice(payload);
}

fn custom(out: &mut Vec<u8>, section_name: &str, data: &[u8]) {
    let mut payload = Vec::with_capacity(section_name.len() + data.len() + 5);
    name(&mut payload, section_name);
    payload.extend_from_slice(data);
    section(out, 0, &payload);
}

fn name_map(out: &mut Vec<u8>, id: u8, map: &[(u32, String)]) {
    if map.is_empty() {
        return;
    }
    let mut sub = Vec::new();
    u32(&mut sub, map.len() as u32);
    for (index, item) in map {
        u32(&mut sub, *index);
        name(&mut sub, item);
    }
    out.push(id);
    u32(out, sub.len() as u32);
    out.extend_from_slice(&sub);
}

fn names_payload(names: &Names) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(module) = &names.module {
        let mut sub = Vec::new();
        name(&mut sub, module);
        out.push(0);
        u32(&mut out, sub.len() as u32);
        out.extend_from_slice(&sub);
    }
    name_map(&mut out, 1, &names.funcs);
    name_map(&mut out, 7, &names.globals);
    out
}

fn uses_data_indices(module: &Module) -> bool {
    module.funcs.iter().any(|func| {
        func.body
            .iter()
            .any(|instr| matches!(instr, Instr::MemoryInit { .. } | Instr::DataDrop(_)))
    })
}

fn elem_segment(out: &mut Vec<u8>, elem: &Elem) {
    let exprs = matches!(elem.items, ElemItems::Exprs(..));
    let funcref = elem.items.elem_type() == ValType::FuncRef;
    let flags: u32 = match &elem.mode {
        ElemMode::Active { table: 0, .. } if funcref => 0,
        ElemMode::Active { .. } => 2,
        ElemMode::Passive => 1,
        ElemMode::Declarative => 3,
    } | if exprs { 4 } else { 0 };
    u32(out, flags);
    if let ElemMode::Active { table, offset } = &elem.mode {
        if flags & 2 != 0 {
            u32(out, *table);
        }
        expr(out, offset);
    }
    let explicit_type = flags & 3 != 0;
    match &elem.items {
        ElemItems::Funcs(funcs) => {
            if explicit_type {
                out.push(0x00);
            }
            u32(out, funcs.len() as u32);
            for func in funcs {
                u32(out, *func);
            }
        }
        ElemItems::Exprs(ty, exprs) => {
            if explicit_type {
                out.push(ty.to_byte());
            }
            u32(out, exprs.len() as u32);
            for item in exprs {
                expr(out, item);
            }
        }
    }
}

pub fn encode(module: &Module) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\0asm");
    out.extend_from_slice(&[1, 0, 0, 0]);
    let customs_at = |out: &mut Vec<u8>, rank: u8| {
        for item in module.customs.iter().filter(|item| item.after_rank == rank) {
            custom(out, &item.name, &item.data);
        }
        if let Some(names) = &module.names {
            if names.after_rank == rank {
                let payload = names_payload(names);
                if !payload.is_empty() {
                    custom(out, "name", &payload);
                }
            }
        }
    };
    customs_at(&mut out, 0);
    let mut payload = Vec::new();
    for rank in 1..=13u8 {
        payload.clear();
        let present = match rank {
            1 if !module.types.is_empty() => {
                u32(&mut payload, module.types.len() as u32);
                for ty in &module.types {
                    payload.push(0x60);
                    u32(&mut payload, ty.params.len() as u32);
                    payload.extend(ty.params.iter().map(|ty| ty.to_byte()));
                    u32(&mut payload, ty.results.len() as u32);
                    payload.extend(ty.results.iter().map(|ty| ty.to_byte()));
                }
                Some(1)
            }
            2 if !module.imports.is_empty() => {
                u32(&mut payload, module.imports.len() as u32);
                for import in &module.imports {
                    name(&mut payload, &import.module);
                    name(&mut payload, &import.name);
                    match &import.desc {
                        ImportDesc::Func(ty) => {
                            payload.push(0);
                            u32(&mut payload, *ty);
                        }
                        ImportDesc::Table(ty) => {
                            payload.push(1);
                            table_type(&mut payload, ty);
                        }
                        ImportDesc::Memory(ty) => {
                            payload.push(2);
                            limits(&mut payload, &ty.limits, ty.shared);
                        }
                        ImportDesc::Global(ty) => {
                            payload.push(3);
                            global_type(&mut payload, ty);
                        }
                    }
                }
                Some(2)
            }
            3 if !module.funcs.is_empty() => {
                u32(&mut payload, module.funcs.len() as u32);
                for func in &module.funcs {
                    u32(&mut payload, func.ty);
                }
                Some(3)
            }
            4 if !module.tables.is_empty() => {
                u32(&mut payload, module.tables.len() as u32);
                for table in &module.tables {
                    table_type(&mut payload, table);
                }
                Some(4)
            }
            5 if !module.memories.is_empty() => {
                u32(&mut payload, module.memories.len() as u32);
                for memory in &module.memories {
                    limits(&mut payload, &memory.limits, memory.shared);
                }
                Some(5)
            }
            7 if !module.globals.is_empty() => {
                u32(&mut payload, module.globals.len() as u32);
                for global in &module.globals {
                    global_type(&mut payload, &global.ty);
                    expr(&mut payload, &global.init);
                }
                Some(6)
            }
            8 if !module.exports.is_empty() => {
                u32(&mut payload, module.exports.len() as u32);
                for export in &module.exports {
                    name(&mut payload, &export.name);
                    payload.push(match export.kind {
                        ExternKind::Func => 0,
                        ExternKind::Table => 1,
                        ExternKind::Memory => 2,
                        ExternKind::Global => 3,
                    });
                    u32(&mut payload, export.index);
                }
                Some(7)
            }
            9 => module.start.map(|start| {
                u32(&mut payload, start);
                8
            }),
            10 if !module.elems.is_empty() => {
                u32(&mut payload, module.elems.len() as u32);
                for elem in &module.elems {
                    elem_segment(&mut payload, elem);
                }
                Some(9)
            }
            11 if module.data_count.is_some() || uses_data_indices(module) => {
                u32(&mut payload, module.datas.len() as u32);
                Some(12)
            }
            12 if !module.funcs.is_empty() => {
                u32(&mut payload, module.funcs.len() as u32);
                let mut body = Vec::new();
                for func in &module.funcs {
                    body.clear();
                    func_body(&mut body, func);
                    u32(&mut payload, body.len() as u32);
                    payload.extend_from_slice(&body);
                }
                Some(10)
            }
            13 if !module.datas.is_empty() => {
                u32(&mut payload, module.datas.len() as u32);
                for data in &module.datas {
                    match &data.mode {
                        DataMode::Active { memory: 0, offset } => {
                            payload.push(0);
                            expr(&mut payload, offset);
                        }
                        DataMode::Active { memory, offset } => {
                            payload.push(2);
                            u32(&mut payload, *memory);
                            expr(&mut payload, offset);
                        }
                        DataMode::Passive => payload.push(1),
                    }
                    u32(&mut payload, data.bytes.len() as u32);
                    payload.extend_from_slice(&data.bytes);
                }
                Some(11)
            }
            _ => None,
        };
        if let Some(id) = present {
            section(&mut out, id, &payload);
        }
        customs_at(&mut out, rank);
    }
    out
}
