//! Block coverage of a traced run (the web trace host `makepad-web-trace`): which functions and
//! which of their basic blocks the program executed.
//!
//! On (`enable`, before the program is loaded), each freshly lowered basic block
//! gets a Coverage effect. It sets a bit in an embedder-owned atomic bitmap,
//! without aliasing program memory or preventing its load forwarding. Native
//! codegen expands the effect after optimisation, with no per-hit call. Inlined
//! bodies retain their probes. Blocks named by the same source position share
//! a bit; dominated duplicate probes disappear before optimisation. Tier-up
//! removes probes whose bits already read set, while unvisited positions
//! remain instrumented. On arm64 the trace host also patches covered native
//! probes to a single skip branch at frame boundaries, including final-tier
//! code. The code cache is off: the
//! bitmap addresses belong to this process (cache.rs).
//!
//! A block is named by the source position of its first instruction (RIR's per-inst `pos`), so the
//! record holds for another lowering of the same source (the wasm back end lowers for 32-bit
//! pointers); functions are named by `jit::slot_key` (def path and type arguments).

use crate::jit::Unit;
use relax_backend::rir::{Func, Inst};
use relax_session::sync::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static ON: AtomicBool = AtomicBool::new(false);

/// One source lowering of a slot. The bitmap allocation remains stable across
/// growth of the record and lives for the process, like the generated code.
struct Range {
    unit: usize,
    slot: u32,
    bits: Box<[AtomicU64]>,
    /// Blocks at the same source position name one observable coverage hit.
    bit: Vec<usize>,
    /// per block: source position ((file << 32) | offset), 0 none
    pos: Vec<u64>,
}

#[derive(Default)]
struct Record {
    ranges: Vec<Range>,
    by_slot: HashMap<(usize, u32), Vec<usize>>,
}
static RECORD: std::sync::LazyLock<Mutex<Record>> = std::sync::LazyLock::new(|| Mutex::new(Record::default()));

struct NativeProbe {
    unit: usize,
    pc: u64,
    word: u64,
    mask: u64,
    skip: u32,
}
static PROBES: Mutex<Vec<NativeProbe>> = Mutex::new(Vec::new());

pub(crate) fn forget(unit: *const Unit) {
    if !on() { return; }
    PROBES.lock().unwrap_or_else(|e| e.into_inner()).retain(|p| p.unit != unit as usize);
}

/// Remember patchable probes only after the code has been linked and written.
pub(crate) fn installed(unit: &Unit, base: u64, relocs: &[relax_backend::rir::RelocAt]) {
    if !on() { return; }
    let _k = crate::zone_alloc::kept();
    let mut probes = PROBES.lock().unwrap_or_else(|e| e.into_inner());
    for r in relocs {
        if let (relax_backend::rir::RelocKind::A64CoverageBit { bit, words }, relax_backend::rir::Reloc::Value(word)) = (r.kind, r.target) {
            probes.push(NativeProbe { unit: unit as *const Unit as usize, pc: base + r.at as u64, word, mask: 1 << bit, skip: 0x1400_0000 | words });
        }
    }
}

/// At a trace-host frame boundary, remove the native cost of covered probes,
/// including code that reached its final tier before its first hit. This never
/// waits for the compiler: an install in progress defers the sweep one frame.
///
/// # Safety
/// `unit` and its executable mappings must stay alive throughout this call.
pub unsafe fn retire_native(unit: *const Unit) {
    if !on() { return; }
    let Ok(mut probes) = PROBES.try_lock() else { return };
    probes.retain(|p| {
        if p.unit != unit as usize || (*(p.word as *const AtomicU64)).load(Ordering::Relaxed) & p.mask == 0 {
            return true;
        }
        // AArch64 permits concurrent replacement of an aligned NOP with B.
        // Only the first instruction changes; anyone already inside the old
        // probe can finish its idempotent atomic OR. fill_code writes through
        // the writable alias and flushes the executable instruction cache.
        crate::os::mem::fill_code(p.pc, 4, |bytes| {
            (&*(bytes.as_mut_ptr() as *const std::sync::atomic::AtomicU32)).store(p.skip, Ordering::Release);
        });
        false
    });
}

/// Turns coverage on for the program loaded after this call (the code cache with it off).
pub fn enable() {
    ON.store(true, Ordering::Release);
}

pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Instrument the source blocks, sharing bits when a slot is lowered again.
/// Storage grows with the traced program; no table capacity can discard hits.
pub fn instrument(up: *const Unit, slot: u32, f: &mut Func) {
    if f.blocks.is_empty() || !runtime_function(unsafe { &*up }, slot) {
        return;
    }
    let _k = crate::zone_alloc::kept();
    let pos: Vec<u64> = f.blocks.iter().map(|b| b.pos.first().copied().unwrap_or(b.term_pos)).collect();
    let mut record = RECORD.lock().unwrap_or_else(|e| e.into_inner());
    let key = (up as usize, slot);
    let found = record.by_slot.get(&key).and_then(|indices| indices.iter().copied().find(|&i| record.ranges[i].pos == pos));
    let index = found.unwrap_or_else(|| {
        let mut sites = HashMap::new();
        let bit = pos.iter().map(|&p| {
            let next = sites.len();
            *sites.entry(p).or_insert(next)
        }).collect();
        let bits = (0..sites.len().div_ceil(64)).map(|_| AtomicU64::new(0)).collect();
        let index = record.ranges.len();
        record.ranges.push(Range { unit: up as usize, slot, bits, bit, pos });
        record.by_slot.entry(key).or_default().push(index);
        index
    });
    let range = &record.ranges[index];
    for (bi, b) in f.blocks.iter_mut().enumerate() {
        let index = range.bit[bi];
        let bit = &range.bits[index / 64];
        let mask = 1u64 << (index % 64);
        if bit.load(Ordering::Relaxed) & mask != 0 {
            continue;
        }
        let positioned = b.pos.len() == b.insts.len();
        b.insts.insert(0, Inst::Coverage { word: bit as *const AtomicU64 as u64, mask, disabled: unsafe { &*up }.rs.coverage_disabled });
        if positioned {
            b.pos.insert(0, range.pos[bi]);
        }
    }
    drop(record);
    retire(f);
}

/// Initializer slots are an implementation detail of evaluation, not Rust
/// runtime functions. run_init may interpret the same item to data or execute
/// its lowered body (including after NeedsLoad); neither creates a runtime root.
pub(crate) fn runtime_function(u: &Unit, slot: u32) -> bool {
    use crate::program::DefKind;
    match u.fns[slot as usize].key {
        crate::jit::FnKey::Inst(d, _) => !matches!(u.prog.def(d).kind,
            DefKind::Const | DefKind::AssocConst | DefKind::AnonConst | DefKind::Static),
        crate::jit::FnKey::Plain(base) | crate::jit::FnKey::Sandbox(base) => runtime_function(u, base),
        _ => true,
    }
}

/// Retire already-covered probes before optimising a new tier, including
/// probes inherited from inline candidates. A racing first hit can only leave
/// an unnecessary probe; bits never clear, so no future coverage is lost.
pub fn retire(f: &mut Func) {
    if !on() {
        return;
    }
    for b in &mut f.blocks {
        let positioned = b.pos.len() == b.insts.len();
        let mut kept = 0;
        for k in 0..b.insts.len() {
            let covered = match b.insts[k] {
                Inst::Coverage { word, mask, .. } => {
                    // Only instrument() makes these effects. RECORD owns the
                    // aligned atomic words for the lifetime of every JIT tier.
                    unsafe { (*(word as *const AtomicU64)).load(Ordering::Relaxed) & mask != 0 }
                }
                _ => false,
            };
            if !covered {
                b.insts.swap(kept, k);
                if positioned { b.pos.swap(kept, k); }
                kept += 1;
            }
        }
        b.insts.truncate(kept);
        if positioned { b.pos.truncate(kept); }
    }
    // A source position often labels many lowering blocks (notably a macro
    // expansion and the comparison chain of a match). Once a dominating
    // probe sets its bit, another such probe on that path adds no coverage.
    // Remove it even in ahead-built code, before those probes obstruct
    // switch lowering or grow inline candidates. Branch siblings retain
    // their probes: neither necessarily runs before the other.
    if !f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::Coverage { .. }))) { return; }
    let (_, _, kids) = relax_backend::dom::dom_numbers(&relax_backend::dom::idoms(f));
    let mut active = HashSet::new();
    let mut undo = Vec::new();
    let mut work = vec![(0usize, true, 0usize)];
    while let Some((bi, enter, mark)) = work.pop() {
        if !enter {
            for key in undo.drain(mark..) { active.remove(&key); }
            continue;
        }
        work.push((bi, false, undo.len()));
        let b = &mut f.blocks[bi];
        let positioned = b.pos.len() == b.insts.len();
        let mut kept = 0;
        for k in 0..b.insts.len() {
            if let Inst::Coverage { word, mask, .. } = b.insts[k] {
                if !active.insert((word, mask)) { continue; }
                undo.push((word, mask));
            }
            b.insts.swap(kept, k);
            if positioned { b.pos.swap(kept, k); }
            kept += 1;
        }
        b.insts.truncate(kept);
        if positioned { b.pos.truncate(kept); }
        for &child in kids[bi].iter().rev() { work.push((child as usize, true, 0)); }
    }
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
    let record = RECORD.lock().unwrap_or_else(|e| e.into_inner());
    let mut by_slot: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    let mut out: Vec<FnCoverage> = Vec::new();
    let mut seen: Vec<std::collections::HashSet<(usize, usize)>> = Vec::new();
    for r in record.ranges.iter().filter(|r| r.unit == up as usize) {
        let hit = |b: usize| {
            let bit = r.bit[b];
            r.bits[bit / 64].load(Ordering::Relaxed) & (1 << (bit % 64)) != 0
        };
        if !hit(0) || !runtime_function(u, r.slot) {
            continue;
        }
        let lc = |p: u64| -> (String, usize, usize) {
            let (file, off) = (relax_backend::rir::pos_file(p) as usize, (p & 0xffff_ffff) as u32);
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

/// Snapshot the export's probes after all union bodies have been lowered. Other
/// runs use slot keys and source coordinates, never process-local bitmap addresses.
/// This does not mark their blocks as executed by the current run.
pub fn export_hits(u: &Unit, other: &HashMap<String, HashSet<(usize, usize)>>) -> HashMap<(u64, u64), bool> {
    let record = RECORD.lock().unwrap_or_else(|e| e.into_inner());
    let mut ran = HashSet::new();
    let mut keys = HashMap::new();
    for r in record.ranges.iter().filter(|r| r.unit == u as *const Unit as usize) {
        let key = keys.entry(r.slot).or_insert_with(|| crate::jit::slot_key(u, r.slot));
        let external = other.get(key);
        for (b, &pos) in r.pos.iter().enumerate() {
            let bit = r.bit[b];
            let lc = if pos == 0 { (0, 0) } else {
                u.prog.files.get(relax_backend::rir::pos_file(pos) as usize)
                    .map_or((0, 0), |f| f.line_col(pos as u32))
            };
            if r.bits[bit / 64].load(Ordering::Relaxed) & (1 << (bit % 64)) != 0
                || external.is_some_and(|hit| hit.contains(&lc)) {
                ran.insert((r.unit, r.slot, pos));
            }
        }
    }
    let mut hits = HashMap::new();
    for r in &record.ranges {
        for (b, &pos) in r.pos.iter().enumerate() {
            let bit = r.bit[b];
            let word = &r.bits[bit / 64] as *const AtomicU64 as u64;
            hits.insert((word, 1 << (bit % 64)), ran.contains(&(r.unit, r.slot, pos)));
        }
    }
    hits
}
