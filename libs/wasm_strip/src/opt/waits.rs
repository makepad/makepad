//! Waits on a shared memory, made safe on a thread that may not block.
//!
//! A browser's main thread (and an audio worklet) may not block: there
//! `memory.atomic.wait32/64` traps ("Atomics.wait cannot be called in this
//! context"). Rust's std reaches it from every contended `Mutex`, `RwLock`,
//! `Once`/`OnceLock` and `Condvar`, so any lock the UI thread shares with
//! a worker is a trap waiting for the moment both want it.
//!
//! Every wait of memory 0 therefore becomes a call to a function this pass
//! adds (one per wait width and memory offset). It waits as before, unless
//! the exported mutable global [`CANNOT_BLOCK_EXPORT`] is set: then it
//! answers "not equal" at once, and the caller (std's futex loops all check
//! their condition again after any return) spins until the lock is free or
//! the condition holds. Globals are per instance, so per thread: the web
//! runtime sets it on the threads that may not block, every other thread
//! keeps 0 and blocks. Applies only to a module whose memory 0 is shared.

use super::ir::*;

/// The exported global a host sets to 1 on a thread that may not block.
pub const CANNOT_BLOCK_EXPORT: &str = "__makepad_cannot_block";

const WAIT32: u16 = 0x01;
const WAIT64: u16 = 0x02;

/// Rewrites the module (see the module docs); returns how many sites it
/// rewrote.
pub fn run(module: &mut Module) -> usize {
    if !module.memory_types().first().is_some_and(|m| m.shared) {
        return 0;
    }
    // One helper per (width, offset, align) a wait uses.
    let mut helpers: Vec<(u16, MemArg)> = Vec::new();
    for instr in module.funcs.iter().flat_map(|f| f.body.iter()) {
        if let Instr::Atomic(sub @ (WAIT32 | WAIT64), arg) = instr {
            if arg.memory == 0 && !helpers.iter().any(|(s, a)| s == sub && a == arg) {
                helpers.push((*sub, *arg));
            }
        }
    }
    if helpers.is_empty() {
        return 0;
    }
    let global = module.num_imported(ExternKind::Global) + module.globals.len() as u32;
    module.globals.push(Global { ty: GlobalType { ty: ValType::I32, mutable: true }, init: vec![Instr::I32Const(0), Instr::End] });
    module.exports.push(Export { name: CANNOT_BLOCK_EXPORT.into(), kind: ExternKind::Global, index: global });
    let first = module.num_imported_funcs() + module.funcs.len() as u32;
    let mut sites = 0;
    for func in &mut module.funcs {
        for instr in &mut func.body {
            if let Instr::Atomic(sub @ (WAIT32 | WAIT64), arg) = instr {
                if let Some(k) = helpers.iter().position(|(s, a)| s == sub && a == arg) {
                    *instr = Instr::Call(first + k as u32);
                    sites += 1;
                }
            }
        }
    }
    for (k, (sub, arg)) in helpers.into_iter().enumerate() {
        let expected = if sub == WAIT32 { ValType::I32 } else { ValType::I64 };
        let sig = FuncType { params: vec![ValType::I32, expected, ValType::I64], results: vec![ValType::I32] };
        let ty = match module.types.iter().position(|t| *t == sig) {
            Some(ty) => ty as u32,
            None => {
                module.types.push(sig);
                module.types.len() as u32 - 1
            }
        };
        // if cannot_block { 1 (not equal) } else { wait(addr, expected, timeout) }
        let body = vec![
            Instr::GlobalGet(global),
            Instr::If(BlockType::Value(ValType::I32)),
            Instr::I32Const(1),
            Instr::Else,
            Instr::LocalGet(0),
            Instr::LocalGet(1),
            Instr::LocalGet(2),
            Instr::Atomic(sub, arg),
            Instr::End,
            Instr::End,
        ];
        module.funcs.push(Func { ty, locals: Vec::new(), body });
        if let Some(names) = &mut module.names {
            let width = if sub == WAIT32 { 32 } else { 64 };
            names.funcs.push((first + k as u32, format!("__shared_memory_wait{width}_{}", arg.offset)));
        }
    }
    sites
}
