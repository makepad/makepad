//! Block coverage of a traced run (the web trace host `makepad-web-trace`): which functions and
//! which of their basic blocks the program executed.
//!
//! On (`enable`, before the program is loaded), every function's lowered RIR gets one byte store
//! at the head of each block (`prepare_rir_from`, before optimisation): the block's byte in one
//! process-wide table is set to 1. The stores travel with the RIR, so a body inlined into another
//! still marks its own blocks, and tiering, speculation and the order of compiles change nothing
//! about what is recorded. A function is covered when its entry block ran. The code cache is off
//! while coverage is on (the stores name table addresses of this process; cache.rs).
//!
//! A block is named by the source position of its first instruction (RIR's per-inst `pos`), so the
//! record holds for another lowering of the same source (the wasm back end lowers for 32-bit
//! pointers); functions are named by `jit::slot_key` (def path and type arguments).

use crate::jit::Unit;
use relax_backend::rir::{Func, Inst, Mem};
use relax_session::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};

static ON: AtomicBool = AtomicBool::new(false);
/// One byte per instrumented block (zero pages until touched).
const CAP: usize = 1 << 28;
static TABLE: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// One instrumented lowering of a slot: its blocks' bytes are `first..first + pos.len()`.
struct Range {
    unit: usize,
    slot: u32,
    first: usize,
    /// per block: the source position of its first instruction ((file << 32) | offset), 0 none
    pos: Vec<u64>,
}
static RANGES: Mutex<Vec<Range>> = Mutex::new(Vec::new());

/// Turns coverage on for the program loaded after this call (the code cache with it off).
pub fn enable() {
    let _k = crate::zone_alloc::kept();
    let t = unsafe { std::alloc::alloc_zeroed(std::alloc::Layout::from_size_align(CAP, 4096).unwrap()) };
    assert!(!t.is_null(), "coverage table");
    TABLE.store(t, Ordering::Release);
    ON.store(true, Ordering::Release);
}

pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Adds the block stores to slot `slot`'s freshly lowered body.
pub fn instrument(up: *const Unit, slot: u32, f: &mut Func) {
    let n = f.blocks.len();
    if n == 0 {
        return;
    }
    let first = NEXT.fetch_add(n, Ordering::Relaxed);
    if first + n > CAP {
        return;
    }
    let table = TABLE.load(Ordering::Acquire) as u64;
    let mut pos = Vec::with_capacity(n);
    for bi in 0..n {
        let at = f.blocks[bi].pos.first().copied().unwrap_or(f.blocks[bi].term_pos);
        pos.push(at);
        let a = f.vreg(relax_backend::rir::Cls::I);
        let one = f.vreg(relax_backend::rir::Cls::I);
        let b = &mut f.blocks[bi];
        let head = [
            Inst::Iconst(a, (table + (first + bi) as u64) as i64),
            Inst::Iconst(one, 1),
            Inst::Store(Mem::Int(1, false), a, 0, one),
        ];
        let positioned = b.pos.len() == b.insts.len();
        b.insts.splice(0..0, head);
        if positioned {
            b.pos.splice(0..0, [at; 3]);
        }
    }
    let _k = crate::zone_alloc::kept();
    RANGES.lock().unwrap_or_else(|e| e.into_inner()).push(Range { unit: up as usize, slot, first, pos });
}

/// A covered function of a unit.
pub struct FnCoverage {
    /// `jit::slot_key`: def path with type arguments
    pub key: String,
    /// inst, closure, shim, glue, ...
    pub kind: &'static str,
    pub krate: String,
    /// the file of its entry block, and its (line, column), 1-based
    pub file: String,
    pub line: usize,
    pub col: usize,
    /// blocks of its body as lowered, and the (line, column) of each block that ran
    pub blocks: usize,
    pub hit: Vec<(usize, usize)>,
}

/// The functions of `up` whose entry block ran, in first-instrumented order. Several lowerings of
/// one slot count as one function (the union of their blocks; `blocks` is the largest body).
pub unsafe fn functions(up: *const Unit) -> Vec<FnCoverage> {
    let u = &*up;
    let table = TABLE.load(Ordering::Acquire);
    if table.is_null() {
        return Vec::new();
    }
    let ranges = RANGES.lock().unwrap_or_else(|e| e.into_inner());
    let mut by_slot: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    let mut out: Vec<FnCoverage> = Vec::new();
    let mut seen: Vec<std::collections::HashSet<(usize, usize)>> = Vec::new();
    for r in ranges.iter().filter(|r| r.unit == up as usize) {
        let hit = |b: usize| std::ptr::read_volatile(table.add(r.first + b)) != 0;
        if !hit(0) {
            continue;
        }
        let lc = |p: u64| -> (String, usize, usize) {
            let (file, off) = ((p >> 32) as usize, (p & 0xffff_ffff) as u32);
            match (u.prog.file_paths.get(file), u.prog.files.get(file)) {
                (Some(path), Some(f)) if p != 0 => {
                    let (l, c) = f.line_col(off);
                    (path.clone(), l, c)
                }
                _ => (String::new(), 0, 0),
            }
        };
        let k = *by_slot.entry(r.slot).or_insert_with(|| {
            let (file, line, col) = lc(r.pos[0]);
            let f = &u.fns[r.slot as usize];
            let (kind, krate) = match &f.key {
                crate::jit::FnKey::Inst(d, _) => ("inst", u.prog.syms.str(u.prog.crates[u.prog.def(*d).krate as usize].name).to_string()),
                crate::jit::FnKey::Closure(..) => ("closure", String::new()),
                crate::jit::FnKey::Shim(_) => ("shim", String::new()),
                crate::jit::FnKey::Glue(..) => ("glue", String::new()),
                crate::jit::FnKey::Sandbox(_) => ("sandbox", String::new()),
                crate::jit::FnKey::Plain(_) => ("plain", String::new()),
            };
            out.push(FnCoverage { key: crate::jit::slot_key(u, r.slot), kind, krate, file, line, col, blocks: 0, hit: Vec::new() });
            seen.push(Default::default());
            out.len() - 1
        });
        out[k].blocks = out[k].blocks.max(r.pos.len());
        for b in 0..r.pos.len() {
            if hit(b) {
                let (_, l, c) = lc(r.pos[b]);
                if seen[k].insert((l, c)) {
                    out[k].hit.push((l, c));
                }
            }
        }
    }
    for f in &mut out {
        f.hit.sort_unstable();
    }
    out
}

/// Whether `addr` is a block byte of the coverage table, and then whether that block ran in ANY
/// lowering of its slot (the wasm export reads the block stores that travelled into optimised
/// code). Each lowering of a slot (baseline, then O2, ...) gets its own bytes, and the build the
/// export captures may have run little or not at all after it was installed: a block counts as
/// run when the same block (index, in a lowering with the same block count) of the same slot ran
/// in any lowering. Read once the run is over (the union is computed on the first call).
pub fn hit_at(addr: u64) -> Option<bool> {
    let t = TABLE.load(Ordering::Acquire);
    if t.is_null() {
        return None;
    }
    let base = t as u64;
    if addr < base || addr >= base + NEXT.load(Ordering::Relaxed).min(CAP) as u64 {
        return None;
    }
    let k = (addr - base) as usize;
    struct Union {
        /// (first byte, range index), sorted
        starts: Vec<(usize, usize)>,
        /// per range: (unit, slot, block count)
        keys: Vec<(usize, u32, usize)>,
        ran: std::collections::HashSet<(usize, u32, usize, usize)>,
    }
    static U: std::sync::OnceLock<Union> = std::sync::OnceLock::new();
    let u = U.get_or_init(|| {
        let ranges = RANGES.lock().unwrap_or_else(|e| e.into_inner());
        let mut starts = Vec::with_capacity(ranges.len());
        let mut keys = Vec::with_capacity(ranges.len());
        let mut ran = std::collections::HashSet::new();
        for (ri, r) in ranges.iter().enumerate() {
            starts.push((r.first, ri));
            let n = r.pos.len();
            keys.push((r.unit, r.slot, n));
            for b in 0..n {
                if unsafe { std::ptr::read_volatile(t.add(r.first + b)) } != 0 {
                    ran.insert((r.unit, r.slot, n, b));
                }
            }
        }
        starts.sort_unstable();
        Union { starts, keys, ran }
    });
    let i = u.starts.partition_point(|s| s.0 <= k);
    if i == 0 {
        return Some(unsafe { std::ptr::read_volatile(t.add(k)) } != 0);
    }
    let (first, ri) = u.starts[i - 1];
    let (unit, slot, n) = u.keys[ri];
    let b = k - first;
    if b >= n {
        return Some(unsafe { std::ptr::read_volatile(t.add(k)) } != 0);
    }
    Some(u.ran.contains(&(unit, slot, n, b)))
}
