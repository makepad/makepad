//! Bulk memory operations on a shared memory, made safe on every engine.
//!
//! WebKit (Safari 26.2) keeps a per-instance copy of a shared memory's
//! size that only that instance's own `memory.grow` updates: once another
//! thread grows the memory, `memory.size`, `memory.copy` and `memory.fill`
//! in this instance still see the old size, and a copy or fill that
//! reaches the grown pages traps as out of bounds (plain loads and stores
//! see the whole memory). In a threaded program every thread allocates, so
//! any `memcpy` or `memset` can land in pages another thread grew.
//!
//! Every `memory.copy` and `memory.fill` of memory 0 therefore becomes a
//! call to one of two functions this pass adds. Each runs the bulk
//! instruction when its range lies within what `memory.size` reports (on a
//! conforming engine: always), and otherwise copies or fills with plain
//! 8-byte and 1-byte loads and stores, with `memory.copy`'s overlap
//! semantics. An access past the memory's true end still traps there.
//! Applies only to a module whose memory 0 is shared.

use super::ir::*;

const I32_LOAD8_U: u8 = 0x2d;
const I64_LOAD: u8 = 0x29;
const I32_STORE8: u8 = 0x3a;
const I64_STORE: u8 = 0x37;
const I32_EQZ: u8 = 0x45;
const I32_LT_U: u8 = 0x49;
const I32_LE_U: u8 = 0x4d;
const I32_GE_U: u8 = 0x4f;
const I64_LE_U: u8 = 0x58;
const I32_ADD: u8 = 0x6a;
const I32_SUB: u8 = 0x6b;
const I32_AND: u8 = 0x71;
const I64_ADD: u8 = 0x7c;
const I64_MUL: u8 = 0x7e;
const I64_SHL: u8 = 0x86;
const I64_EXTEND_I32_U: u8 = 0xad;

fn mem(align: u32) -> MemArg {
    MemArg { align, offset: 0, memory: 0 }
}

/// Whether the module's memory 0 is a shared memory.
fn shared_memory(module: &Module) -> bool {
    module.memory_types().first().is_some_and(|m| m.shared)
}

/// Rewrites the module (see the module docs); returns how many sites it
/// rewrote.
pub fn run(module: &mut Module) -> usize {
    if !shared_memory(module) {
        return 0;
    }
    let sites = module
        .funcs
        .iter()
        .flat_map(|f| f.body.iter())
        .filter(|i| matches!(i, Instr::MemoryCopy { dst: 0, src: 0 } | Instr::MemoryFill(0)))
        .count();
    if sites == 0 {
        return 0;
    }
    let sig = FuncType { params: vec![ValType::I32; 3], results: Vec::new() };
    let ty = match module.types.iter().position(|t| *t == sig) {
        Some(ty) => ty as u32,
        None => {
            module.types.push(sig);
            module.types.len() as u32 - 1
        }
    };
    let imported = module.num_imported_funcs();
    let copy = imported + module.funcs.len() as u32;
    let fill = copy + 1;
    for func in &mut module.funcs {
        for instr in &mut func.body {
            match instr {
                Instr::MemoryCopy { dst: 0, src: 0 } => *instr = Instr::Call(copy),
                Instr::MemoryFill(0) => *instr = Instr::Call(fill),
                _ => {}
            }
        }
    }
    module.funcs.push(copy_func(ty));
    module.funcs.push(fill_func(ty));
    if let Some(names) = &mut module.names {
        names.funcs.push((copy, "__shared_memory_copy".into()));
        names.funcs.push((fill, "__shared_memory_fill".into()));
    }
    sites
}

/// `[end] <= memory.size * 64 KiB` for `end = base + n`, base and n in
/// locals: pushes an i32 condition.
fn within(b: &mut Vec<Instr>, base: u32, n: u32, limit: u32) {
    b.extend([
        Instr::LocalGet(base),
        Instr::Num(I64_EXTEND_I32_U),
        Instr::LocalGet(n),
        Instr::Num(I64_EXTEND_I32_U),
        Instr::Num(I64_ADD),
        Instr::LocalGet(limit),
        Instr::Num(I64_LE_U),
    ]);
}

/// The memory's size in bytes as this instance sees it, into `limit`.
fn size_into(b: &mut Vec<Instr>, limit: u32) {
    b.extend([
        Instr::MemorySize(0),
        Instr::Num(I64_EXTEND_I32_U),
        Instr::I64Const(16),
        Instr::Num(I64_SHL),
        Instr::LocalSet(limit),
    ]);
}

/// `copy(dst, src, n)`: `memory.copy` when in range, else a memmove loop.
fn copy_func(ty: u32) -> Func {
    let (d, s, n, limit, i) = (0, 1, 2, 3, 4);
    let mut b = Vec::new();
    size_into(&mut b, limit);
    within(&mut b, d, n, limit);
    within(&mut b, s, n, limit);
    b.extend([
        Instr::Num(I32_AND),
        Instr::If(BlockType::Empty),
        Instr::LocalGet(d),
        Instr::LocalGet(s),
        Instr::LocalGet(n),
        Instr::MemoryCopy { dst: 0, src: 0 },
        Instr::Return,
        Instr::End,
    ]);
    // dst <= src: forwards (a chunk is read whole before it is written,
    // and later chunks read at or after what was written).
    b.extend([Instr::LocalGet(d), Instr::LocalGet(s), Instr::Num(I32_LE_U), Instr::If(BlockType::Empty)]);
    // while n - i >= 8: 8 bytes
    b.extend([
        Instr::Block(BlockType::Empty),
        Instr::Loop(BlockType::Empty),
        Instr::LocalGet(n),
        Instr::LocalGet(i),
        Instr::Num(I32_SUB),
        Instr::I32Const(8),
        Instr::Num(I32_LT_U),
        Instr::BrIf(1),
        Instr::LocalGet(d),
        Instr::LocalGet(i),
        Instr::Num(I32_ADD),
        Instr::LocalGet(s),
        Instr::LocalGet(i),
        Instr::Num(I32_ADD),
        Instr::Load(I64_LOAD, mem(0)),
        Instr::Store(I64_STORE, mem(0)),
        Instr::LocalGet(i),
        Instr::I32Const(8),
        Instr::Num(I32_ADD),
        Instr::LocalSet(i),
        Instr::Br(0),
        Instr::End,
        Instr::End,
    ]);
    // while i < n: 1 byte
    b.extend([
        Instr::Block(BlockType::Empty),
        Instr::Loop(BlockType::Empty),
        Instr::LocalGet(i),
        Instr::LocalGet(n),
        Instr::Num(I32_GE_U),
        Instr::BrIf(1),
        Instr::LocalGet(d),
        Instr::LocalGet(i),
        Instr::Num(I32_ADD),
        Instr::LocalGet(s),
        Instr::LocalGet(i),
        Instr::Num(I32_ADD),
        Instr::Load(I32_LOAD8_U, mem(0)),
        Instr::Store(I32_STORE8, mem(0)),
        Instr::LocalGet(i),
        Instr::I32Const(1),
        Instr::Num(I32_ADD),
        Instr::LocalSet(i),
        Instr::Br(0),
        Instr::End,
        Instr::End,
    ]);
    // dst > src: backwards from the end.
    b.extend([Instr::Else, Instr::LocalGet(n), Instr::LocalSet(i)]);
    // while i >= 8: i -= 8, 8 bytes
    b.extend([
        Instr::Block(BlockType::Empty),
        Instr::Loop(BlockType::Empty),
        Instr::LocalGet(i),
        Instr::I32Const(8),
        Instr::Num(I32_LT_U),
        Instr::BrIf(1),
        Instr::LocalGet(i),
        Instr::I32Const(8),
        Instr::Num(I32_SUB),
        Instr::LocalSet(i),
        Instr::LocalGet(d),
        Instr::LocalGet(i),
        Instr::Num(I32_ADD),
        Instr::LocalGet(s),
        Instr::LocalGet(i),
        Instr::Num(I32_ADD),
        Instr::Load(I64_LOAD, mem(0)),
        Instr::Store(I64_STORE, mem(0)),
        Instr::Br(0),
        Instr::End,
        Instr::End,
    ]);
    // while i != 0: i -= 1, 1 byte
    b.extend([
        Instr::Block(BlockType::Empty),
        Instr::Loop(BlockType::Empty),
        Instr::LocalGet(i),
        Instr::Num(I32_EQZ),
        Instr::BrIf(1),
        Instr::LocalGet(i),
        Instr::I32Const(1),
        Instr::Num(I32_SUB),
        Instr::LocalSet(i),
        Instr::LocalGet(d),
        Instr::LocalGet(i),
        Instr::Num(I32_ADD),
        Instr::LocalGet(s),
        Instr::LocalGet(i),
        Instr::Num(I32_ADD),
        Instr::Load(I32_LOAD8_U, mem(0)),
        Instr::Store(I32_STORE8, mem(0)),
        Instr::Br(0),
        Instr::End,
        Instr::End,
    ]);
    b.extend([Instr::End, Instr::End]);
    Func { ty, locals: vec![ValType::I64, ValType::I32], body: b }
}

/// `fill(dst, value, n)`: `memory.fill` when in range, else a store loop.
fn fill_func(ty: u32) -> Func {
    let (d, v, n, limit, i, word) = (0, 1, 2, 3, 4, 5);
    let mut b = Vec::new();
    size_into(&mut b, limit);
    within(&mut b, d, n, limit);
    b.extend([
        Instr::If(BlockType::Empty),
        Instr::LocalGet(d),
        Instr::LocalGet(v),
        Instr::LocalGet(n),
        Instr::MemoryFill(0),
        Instr::Return,
        Instr::End,
    ]);
    // The byte in all eight lanes of a word.
    b.extend([
        Instr::LocalGet(v),
        Instr::I32Const(0xff),
        Instr::Num(I32_AND),
        Instr::Num(I64_EXTEND_I32_U),
        Instr::I64Const(0x0101_0101_0101_0101),
        Instr::Num(I64_MUL),
        Instr::LocalSet(word),
    ]);
    b.extend([
        Instr::Block(BlockType::Empty),
        Instr::Loop(BlockType::Empty),
        Instr::LocalGet(n),
        Instr::LocalGet(i),
        Instr::Num(I32_SUB),
        Instr::I32Const(8),
        Instr::Num(I32_LT_U),
        Instr::BrIf(1),
        Instr::LocalGet(d),
        Instr::LocalGet(i),
        Instr::Num(I32_ADD),
        Instr::LocalGet(word),
        Instr::Store(I64_STORE, mem(0)),
        Instr::LocalGet(i),
        Instr::I32Const(8),
        Instr::Num(I32_ADD),
        Instr::LocalSet(i),
        Instr::Br(0),
        Instr::End,
        Instr::End,
    ]);
    b.extend([
        Instr::Block(BlockType::Empty),
        Instr::Loop(BlockType::Empty),
        Instr::LocalGet(i),
        Instr::LocalGet(n),
        Instr::Num(I32_GE_U),
        Instr::BrIf(1),
        Instr::LocalGet(d),
        Instr::LocalGet(i),
        Instr::Num(I32_ADD),
        Instr::LocalGet(v),
        Instr::Store(I32_STORE8, mem(0)),
        Instr::LocalGet(i),
        Instr::I32Const(1),
        Instr::Num(I32_ADD),
        Instr::LocalSet(i),
        Instr::Br(0),
        Instr::End,
        Instr::End,
    ]);
    b.push(Instr::End);
    Func { ty, locals: vec![ValType::I64, ValType::I32, ValType::I64], body: b }
}
