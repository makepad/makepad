//! Target-independent register allocation for RIR: liveness, hull live intervals,
//! linear scan with copy hints, call-crossing handling, and a parallel-move
//! sequentializer. Backends (x64, arm64) pass a `RegConfig` and map `Loc`s to
//! their own registers.
//!
//! Positions: block b's instruction i uses its operands at `block_start[b] + 2i + 1`
//! and defines its result at `+ 2i + 2`; the terminator uses at `block_end[b] - 1`.

use crate::rir::*;
use std::collections::HashMap;

pub struct RegConfig {
    /// allocatable caller-saved integer registers (clobbered by calls)
    pub int_caller: Vec<u8>,
    /// allocatable callee-saved integer registers (saved once in the prologue)
    pub int_callee: Vec<u8>,
    pub flt_caller: Vec<u8>,
    pub flt_callee: Vec<u8>,
    /// indexed accesses ([base + idx << k + disp]) take a stack-slot base for free (x64 [rbp + ..]);
    /// without it (arm64) a slot address used by indexed accesses competes for a register
    pub slot_index_free: bool,
    /// registers an instruction overwrites while it runs (backend scratch use, e.g. x64 rcx for
    /// variable shifts): bit r = int register r, bit 32 + x = float register x. No value live at
    /// such an instruction (operands, result, or live across) is placed in them.
    pub clobbers: Option<fn(&relax_session::Options, &Inst) -> u64>,
    /// `Env::sym_class` (0: none): an address of a domain symbol is a thread-local block read
    /// and an add, not a constant, so it is never rematerialised
    pub sym_class: u64,
    /// which recomputed constants a store writes straight from the instruction (x64: imm32,
    /// sign-extended for 8 bytes; arm64: 0 from xzr): such stores need no register for them
    pub store_imm: fn(i64) -> bool,
    /// scalar float constants are recomputed at their uses instead of spilled (Remat::FConst)
    pub fconst_remat: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Loc {
    Reg(u8),
    Flt(u8),
    /// spill slot: byte offset into the spill area (multiple of 8)
    Stack(i32),
    None,
}

/// Set by drivers that compare builds (MAKEPAD_RUN_CODE_SIG): backends then report `alloc_sig`.
pub static SIG_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Hash of an allocation (every vreg's location, frame, saves): address-independent, so
/// back-end refactors can check equal output between builds.
pub fn alloc_sig(al: &Alloc) -> u64 {
    if !SIG_ON.load(std::sync::atomic::Ordering::Relaxed) {
        return 0;
    }
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |x: u64| h = (h ^ x).wrapping_mul(0x100_0000_01b3);
    for l in &al.loc {
        match *l {
            Loc::Reg(r) => mix(1 << 8 | r as u64),
            Loc::Flt(r) => mix(2 << 8 | r as u64),
            Loc::Stack(o) => mix(3 << 40 | o as u32 as u64),
            Loc::None => mix(4),
        }
    }
    mix(al.spill_bytes as u64);
    let mut saves: Vec<_> = al.saves.iter().collect();
    saves.sort_by_key(|x| *x.0);
    for (p, v) in saves {
        mix(*p as u64);
        for (r, o) in v {
            mix(r.0 as u64);
            mix(*o as u32 as u64);
        }
    }
    h
}

pub struct Alloc {
    pub loc: Vec<Loc>,
    pub used_callee_int: Vec<u8>,
    pub used_callee_flt: Vec<u8>,
    /// bytes of spill + save area (8 per slot)
    pub spill_bytes: u32,
    pub block_start: Vec<u32>,
    pub block_end: Vec<u32>,
    /// call position -> values in caller-saved registers that live across it, with
    /// their save slot offset
    pub saves: HashMap<u32, Vec<(VReg, i32)>>,
    pub start: Vec<u32>,
    pub end: Vec<u32>,
    /// number of uses per vreg
    pub uses: Vec<u32>,
    /// vregs without a location whose value the backend recomputes at each use: their single
    /// definition (Iconst / SlotAddr / Addr / FnAddr), which the backend then does not emit. A
    /// stack-slot address used only as a load/store base never takes a register; the others get
    /// one when one is free and are recomputed instead of spilled.
    pub remat: Vec<Remat>,
    /// spilled values held in a free register over a run of their uses in one block (no def in
    /// between, no call): (vreg, first use position, last use position, register). The backend
    /// loads the register before the first use; uses in the run read it; the slot stays the
    /// value's home (paths that read the slot directly remain right).
    pub windows: Vec<(u32, u32, u32, u8)>,
    /// windows entered with the value already in the register (region windows whose block's
    /// predecessors all hold it there): uses read the register, nothing is loaded
    pub chained: Vec<(u32, u32, u32, u8)>,
    /// (call position, vreg) saves whose slot already holds the value (saved at an earlier call
    /// on every path, not redefined since): no store
    pub save_skip: std::collections::HashSet<(u32, u32)>,
    /// (call position, vreg) restores nobody reads (the value is not used before the next call
    /// whose save skips its store, or before it dies): no load
    pub restore_skip: std::collections::HashSet<(u32, u32)>,
}

/// A value the backend recomputes at each use instead of keeping it (see Alloc::remat).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Remat {
    No,
    Const(i64),
    /// the address of stack slot n
    Slot(u32),
    /// a V128 constant: every lane `bits` (`Inst::VConst`): one movi / literal load at a use beats a
    /// spill store at the definition and a reload at each use
    VConst(u32),
    /// a scalar float constant (`Inst::Fconst`: bits, f64?) where the target loads one in a single
    /// instruction at a use (`RegConfig::fconst_remat`; arm64: fmov #imm or a literal load)
    FConst(u64, bool),
    /// a symbol's value (relocated: loaded from the address pool)
    Addr(Sym),
    /// a function's table thunk
    FnAddr(u32),
    /// base + k: an address derived from a value that is never redefined and is live wherever
    /// this one is (e.g. `&self.table` from `self`); folds into the displacement of the
    /// accesses based on it. Never a call argument or returned value (an argument move may
    /// already have overwritten the base's register).
    Offset(VReg, i32),
}

impl Remat {
    #[inline]
    pub fn is_some(&self) -> bool {
        *self != Remat::No
    }
}

impl Alloc {
    #[inline]
    pub fn inst_pos(&self, b: usize, i: usize) -> u32 {
        self.block_start[b] + 2 * i as u32 + 1
    }
}

fn bit_set(s: &mut [u64], i: u32) {
    s[(i >> 6) as usize] |= 1 << (i & 63);
}
fn bit_get(s: &[u64], i: u32) -> bool {
    s[(i >> 6) as usize] & (1 << (i & 63)) != 0
}

/// Uses and defs of one instruction.
pub fn uses_defs(i: &Inst, uses: &mut Vec<VReg>, defs: &mut Vec<VReg>) {
    uses.clear();
    defs.clear();
    match i {
        Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::SlotAddr(d, _) | Inst::Addr(d, _) | Inst::FnAddr(d, _) | Inst::VConst(d, _) => defs.push(*d),
        Inst::Mov(d, a) | Inst::INeg(_, d, a) | Inst::INot(_, d, a) | Inst::IBit(_, d, a) | Inst::FUnary(_, _, d, a) | Inst::Conv(_, d, a) | Inst::IBinI(_, _, d, a, _) | Inst::ICmpI(_, _, d, a, _) => {
            uses.push(*a);
            defs.push(*d);
        }
        Inst::IBin(_, _, d, a, b) | Inst::ICmp(_, _, d, a, b) | Inst::FBin(_, _, d, a, b) | Inst::FCmp(_, _, d, a, b) => {
            uses.push(*a);
            uses.push(*b);
            defs.push(*d);
        }
        Inst::Load(_, d, b, _) => {
            uses.push(*b);
            defs.push(*d);
        }
        Inst::Store(_, b, _, s) => {
            uses.push(*b);
            uses.push(*s);
        }
        Inst::LoadX(_, d, b, x, _, _) | Inst::VLoadX(d, b, x, _, _) => {
            uses.push(*b);
            uses.push(*x);
            defs.push(*d);
        }
        Inst::StoreX(_, b, x, _, _, s) | Inst::VStoreX(b, x, _, _, s) => {
            uses.push(*b);
            uses.push(*x);
            uses.push(*s);
        }
        Inst::Call(c, a, r) => {
            if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                uses.push(*v);
            }
            uses.extend_from_slice(a);
            defs.extend_from_slice(r);
        }
        Inst::Copy(a, b, _) => {
            uses.push(*a);
            uses.push(*b);
        }
        Inst::Fence(..) => {}
        Inst::FpEnv(true, v) => uses.push(*v),
        Inst::FpEnv(false, v) => defs.push(*v),
        Inst::Cpuid(ds, a, b) => { uses.push(*a); uses.push(*b); defs.extend_from_slice(ds); }
        Inst::Xgetbv(d, a) => { uses.push(*a); defs.push(*d); }
        Inst::TlsAddr(d, _) => defs.push(*d),
        Inst::VGather(d, a, b) | Inst::VTbl(d, a, b, _) | Inst::VShuffle(d, a, b, _) => {
            uses.push(*a);
            uses.push(*b);
            defs.push(*d);
        }
        Inst::VX(op, d, a, b) => {
            uses.push(*a);
            if op.arity() == 2 {
                uses.push(*b);
            }
            defs.push(*d);
        }
        Inst::VSplat(d, a) | Inst::VExtract(d, a, _) | Inst::VLoad(d, a, _) | Inst::VAny(d, a) => {
            uses.push(*a);
            defs.push(*d);
        }
        Inst::VX3(_, d, m, a, b) | Inst::VSel(d, m, a, b) | Inst::VFma(_, _, d, m, a, b) => {
            uses.push(*m);
            uses.push(*a);
            uses.push(*b);
            defs.push(*d);
        }
        Inst::VInsert(d, v, _, s) => {
            uses.push(*v);
            uses.push(*s);
            defs.push(*d);
        }
        Inst::VStore(b, _, s) => {
            uses.push(*b);
            uses.push(*s);
        }
        Inst::Sel(d, c, a, b) => {
            uses.push(*c);
            uses.push(*a);
            uses.push(*b);
            defs.push(*d);
        }
        Inst::FMulAdd(_, _, _, d, a, b, c) => {
            uses.push(*a);
            uses.push(*b);
            uses.push(*c);
            defs.push(*d);
        }
        Inst::AtomicLoad(_, _, d, a) => {
            uses.push(*a);
            defs.push(*d);
        }
        Inst::AtomicStore(_, _, a, s) => {
            uses.push(*a);
            uses.push(*s);
        }
        Inst::AtomicRmw(_, _, _, d, a, s) => {
            uses.push(*a);
            uses.push(*s);
            defs.push(*d);
        }
        Inst::AtomicCas(_, _, _, d, a, e, n) => {
            uses.push(*a);
            uses.push(*e);
            uses.push(*n);
            defs.push(*d);
        }
    }
}

/// The instruction's used vregs in uses_defs order, without a buffer.
#[inline]
#[allow(unused_variables)]
pub fn for_uses(i: &Inst, mut uses: impl FnMut(VReg)) {
    match i {
        Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::SlotAddr(d, _) | Inst::Addr(d, _) | Inst::FnAddr(d, _) | Inst::VConst(d, _) => {}
        Inst::Mov(d, a) | Inst::INeg(_, d, a) | Inst::INot(_, d, a) | Inst::IBit(_, d, a) | Inst::FUnary(_, _, d, a) | Inst::Conv(_, d, a) | Inst::IBinI(_, _, d, a, _) | Inst::ICmpI(_, _, d, a, _) => {
            uses(*a);
        }
        Inst::IBin(_, _, d, a, b) | Inst::ICmp(_, _, d, a, b) | Inst::FBin(_, _, d, a, b) | Inst::FCmp(_, _, d, a, b) => {
            uses(*a);
            uses(*b);
        }
        Inst::Load(_, d, b, _) => {
            uses(*b);
        }
        Inst::Store(_, b, _, s) => {
            uses(*b);
            uses(*s);
        }
        Inst::LoadX(_, d, b, x, _, _) | Inst::VLoadX(d, b, x, _, _) => {
            uses(*b);
            uses(*x);
        }
        Inst::StoreX(_, b, x, _, _, s) | Inst::VStoreX(b, x, _, _, s) => {
            uses(*b);
            uses(*x);
            uses(*s);
        }
        Inst::Call(c, a, r) => {
            if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                uses(*v);
            }
            a.iter().for_each(|v| uses(*v));
        }
        Inst::Copy(a, b, _) => {
            uses(*a);
            uses(*b);
        }
        Inst::Fence(..) | Inst::FpEnv(false, _) => {}
        Inst::FpEnv(true, v) => uses(*v),
        Inst::Cpuid(_, a, b) => { uses(*a); uses(*b); }
        Inst::Xgetbv(_, a) => uses(*a),
        Inst::TlsAddr(d, _) => {}
        Inst::VGather(d, a, b) | Inst::VTbl(d, a, b, _) | Inst::VShuffle(d, a, b, _) => {
            uses(*a);
            uses(*b);
        }
        Inst::VX(op, d, a, b) => {
            uses(*a);
            if op.arity() == 2 {
                uses(*b);
            }
        }
        Inst::VSplat(d, a) | Inst::VExtract(d, a, _) | Inst::VLoad(d, a, _) | Inst::VAny(d, a) => {
            uses(*a);
        }
        Inst::VX3(_, d, m, a, b) | Inst::VSel(d, m, a, b) | Inst::VFma(_, _, d, m, a, b) => {
            uses(*m);
            uses(*a);
            uses(*b);
        }
        Inst::VInsert(d, v, _, s) => {
            uses(*v);
            uses(*s);
        }
        Inst::VStore(b, _, s) => {
            uses(*b);
            uses(*s);
        }
        Inst::Sel(d, c, a, b) => {
            uses(*c);
            uses(*a);
            uses(*b);
        }
        Inst::FMulAdd(_, _, _, d, a, b, c) => {
            uses(*a);
            uses(*b);
            uses(*c);
        }
        Inst::AtomicLoad(_, _, d, a) => {
            uses(*a);
        }
        Inst::AtomicStore(_, _, a, s) => {
            uses(*a);
            uses(*s);
        }
        Inst::AtomicRmw(_, _, _, d, a, s) => {
            uses(*a);
            uses(*s);
        }
        Inst::AtomicCas(_, _, _, d, a, e, n) => {
            uses(*a);
            uses(*e);
            uses(*n);
        }
    }
}

/// The instruction's defined vregs in uses_defs order, without a buffer.
#[inline]
pub fn for_defs(i: &Inst, mut defs: impl FnMut(VReg)) {
    match i {
        Inst::Store(..) | Inst::StoreX(..) | Inst::Copy(..) | Inst::Fence(..) | Inst::VStore(..) | Inst::VStoreX(..) | Inst::AtomicStore(..) | Inst::FpEnv(true, _) => {}
        Inst::FpEnv(false, d) => defs(*d),
        Inst::Cpuid(ds, _, _) => { for d in ds { defs(*d); } }
        Inst::Xgetbv(d, _) => defs(*d),
        Inst::Call(_, _, r) => r.iter().for_each(|v| defs(*v)),
        Inst::Iconst(d, _)
        | Inst::Fconst(d, _, _)
        | Inst::SlotAddr(d, _)
        | Inst::Addr(d, _)
        | Inst::FnAddr(d, _)
        | Inst::VConst(d, _)
        | Inst::TlsAddr(d, _)
        | Inst::Mov(d, _)
        | Inst::INeg(_, d, _)
        | Inst::INot(_, d, _)
        | Inst::IBit(_, d, _)
        | Inst::FUnary(_, _, d, _)
        | Inst::Conv(_, d, _)
        | Inst::IBinI(_, _, d, _, _)
        | Inst::ICmpI(_, _, d, _, _)
        | Inst::IBin(_, _, d, _, _)
        | Inst::ICmp(_, _, d, _, _)
        | Inst::FBin(_, _, d, _, _)
        | Inst::FCmp(_, _, d, _, _)
        | Inst::Load(_, d, _, _)
        | Inst::LoadX(_, d, _, _, _, _)
        | Inst::VLoadX(d, _, _, _, _)
        | Inst::VGather(d, _, _)
        | Inst::VTbl(d, _, _, _)
        | Inst::VShuffle(d, _, _, _)
        | Inst::VX(_, d, _, _)
        | Inst::VX3(_, d, _, _, _)
        | Inst::VSplat(d, _)
        | Inst::VExtract(d, _, _)
        | Inst::VLoad(d, _, _)
        | Inst::VAny(d, _)
        | Inst::VSel(d, _, _, _)
        | Inst::VFma(_, _, d, _, _, _)
        | Inst::VInsert(d, _, _, _)
        | Inst::Sel(d, _, _, _)
        | Inst::FMulAdd(_, _, _, d, _, _, _)
        | Inst::AtomicLoad(_, _, d, _)
        | Inst::AtomicRmw(_, _, _, d, _, _)
        | Inst::AtomicCas(_, _, _, d, _, _, _) => defs(*d),
    }
}

pub fn term_uses(t: &Term, uses: &mut Vec<VReg>) {
    uses.clear();
    match t {
        Term::Branch(c, _, _) | Term::Switch(c, _) => uses.push(*c),
        Term::Ret(v) => uses.extend_from_slice(v),
        _ => {}
    }
}

/// Blocks reachable from the entry.
pub fn reachable(f: &Func) -> Vec<bool> {
    let mut r = vec![false; f.blocks.len()];
    let mut stack = vec![0u32];
    while let Some(b) = stack.pop() {
        if r[b as usize] {
            continue;
        }
        r[b as usize] = true;
        stack.extend(f.blocks[b as usize].term.succs());
    }
    r
}

/// The reachable instructions' operands, decoded once for all of the allocator's walks:
/// per reachable block (in order) one entry per instruction, then one for the terminator.
struct OpTab {
    ops: Vec<u32>,
    /// (offset into ops, uses, defs)
    ent: Vec<(u32, u32, u32)>,
    /// per block: its first entry (u32::MAX: unreachable)
    first: Vec<u32>,
}

impl OpTab {
    fn new(f: &Func, reach: &[bool]) -> OpTab {
        let n: usize = f.blocks.iter().map(|b| b.insts.len() + 1).sum();
        let mut t = OpTab { ops: Vec::with_capacity(n * 3), ent: Vec::with_capacity(n), first: vec![u32::MAX; f.blocks.len()] };
        let mut uses = Vec::new();
        let mut defs = Vec::new();
        for (bi, b) in f.blocks.iter().enumerate() {
            if !reach[bi] {
                continue;
            }
            t.first[bi] = t.ent.len() as u32;
            for i in &b.insts {
                uses_defs(i, &mut uses, &mut defs);
                t.ent.push((t.ops.len() as u32, uses.len() as u32, defs.len() as u32));
                t.ops.extend(uses.iter().map(|v| v.0));
                t.ops.extend(defs.iter().map(|v| v.0));
            }
            term_uses(&b.term, &mut uses);
            t.ent.push((t.ops.len() as u32, uses.len() as u32, 0));
            t.ops.extend(uses.iter().map(|v| v.0));
        }
        t
    }
    #[inline]
    fn uses(&self, k: usize) -> &[u32] {
        let (o, nu, _) = self.ent[k];
        &self.ops[o as usize..(o + nu) as usize]
    }
    #[inline]
    fn defs(&self, k: usize) -> &[u32] {
        let (o, nu, nd) = self.ent[k];
        &self.ops[(o + nu) as usize..(o + nu + nd) as usize]
    }
}

/// Every vreg's live ranges, flat: vreg v's sorted, merged ranges (inclusive position
/// pairs) are `r[off[v]..off[v + 1]]`.
struct Ranges {
    r: Vec<(u32, u32)>,
    off: Vec<u32>,
}

impl Ranges {
    #[inline]
    fn of(&self, v: u32) -> &[(u32, u32)] {
        &self.r[self.off[v as usize] as usize..self.off[v as usize + 1] as usize]
    }
}

/// MAKEPAD_RUN_RA_PRIO: allocation order by weighted references (default: the values whose spilling
/// costs most go first; live-range splitting keeps long ones from blocking whole functions),
/// `len` (per position of live range), `sqrt` (per square root of the length)
fn ra_prio(options: &relax_session::Options) -> u8 {
    options.ra_prio as u8
}

fn ra_order_by_start(options: &relax_session::Options) -> bool {
    options.ra_order
}

/// Any bit set in positions a..=b
#[inline]
fn bits_any(o: &[u64], a: u32, b: u32) -> bool {
    let (wa, wb) = ((a / 64) as usize, (b / 64) as usize);
    let ma = !0u64 << (a % 64);
    let mb = !0u64 >> (63 - b % 64);
    if wa == wb {
        return o[wa] & ma & mb != 0;
    }
    if o[wa] & ma != 0 || o[wb] & mb != 0 {
        return true;
    }
    o[wa + 1..wb].iter().any(|w| *w != 0)
}

/// Sets positions a..=b
#[inline]
fn bits_fill(o: &mut [u64], a: u32, b: u32) {
    let (wa, wb) = ((a / 64) as usize, (b / 64) as usize);
    let ma = !0u64 << (a % 64);
    let mb = !0u64 >> (63 - b % 64);
    if wa == wb {
        o[wa] |= ma & mb;
        return;
    }
    o[wa] |= ma;
    o[wb] |= mb;
    for w in &mut o[wa + 1..wb] {
        *w = !0;
    }
}

/// Which vregs can be live across a block boundary: referenced in more than one block, or
/// used before (or without) a definition in their block, or a parameter. Every other vreg is
/// local to one block (defined there before each use), is in no block's live-in or live-out
/// set, and needs no place in them: block liveness over the globals alone is exact and takes
/// O(globals x blocks) bits instead of O(vregs x blocks) (a 10k-vreg, 8k-block fn split for
/// O3 held hundreds of MB of dense sets). Returns (vreg -> global index or u32::MAX,
/// global index -> vreg).
fn global_vregs(f: &Func, reach: &[bool], tab: &OpTab) -> (Vec<u32>, Vec<u32>) {
    let nv = f.vregs.len();
    let mut home = vec![u32::MAX; nv];
    let mut defd = vec![u32::MAX; nv];
    let mut glob = vec![false; nv];
    for p in &f.params {
        if (p.0 as usize) < nv {
            glob[p.0 as usize] = true;
        }
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        let bu = bi as u32;
        let k0 = tab.first[bi] as usize;
        for ii in 0..=b.insts.len() {
            for &u in tab.uses(k0 + ii) {
                let v = u as usize;
                if (home[v] != u32::MAX && home[v] != bu) || defd[v] != bu {
                    glob[v] = true;
                }
                home[v] = bu;
            }
            if ii < b.insts.len() {
                for &d in tab.defs(k0 + ii) {
                    let v = d as usize;
                    if home[v] != u32::MAX && home[v] != bu {
                        glob[v] = true;
                    }
                    home[v] = bu;
                    defd[v] = bu;
                }
            }
        }
    }
    let mut v2g = vec![u32::MAX; nv];
    let mut g2v = Vec::new();
    for v in 0..nv {
        if glob[v] {
            v2g[v] = g2v.len() as u32;
            g2v.push(v as u32);
        }
    }
    (v2g, g2v)
}

/// Block live-in (or live-out) sets over a function's global vregs (global_vregs).
pub struct LiveSets {
    words: usize,
    v2g: Vec<u32>,
    g2v: Vec<u32>,
    sets: Vec<u64>,
}

impl LiveSets {
    /// vreg `v` is in block `b`'s set
    #[inline]
    fn has(&self, b: usize, v: u32) -> bool {
        match self.v2g.get(v as usize) {
            Some(&g) if g != u32::MAX => self.sets[b * self.words + (g as usize >> 6)] & (1u64 << (g & 63)) != 0,
            _ => false,
        }
    }
    /// `f(vreg)` for every vreg in block `b`'s set, ascending
    #[inline]
    fn each(&self, b: usize, mut f: impl FnMut(u32)) {
        for w in 0..self.words {
            let mut m = self.sets[b * self.words + w];
            while m != 0 {
                let k = m.trailing_zeros() as usize;
                m &= m - 1;
                f(self.g2v[w * 64 + k]);
            }
        }
    }
}

/// Live-in and live-out sets of `f` over its global vregs (gen/kill built here).
fn live_sets(f: &Func, reach: &[bool], tab: &OpTab, want_out: bool) -> (LiveSets, LiveSets) {
    let (v2g, g2v) = global_vregs(f, reach, tab);
    let words = (g2v.len() + 63) / 64;
    let nb = f.blocks.len();
    let mut gen = vec![0u64; nb * words];
    let mut kill = vec![0u64; nb * words];
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        let k0 = tab.first[bi] as usize;
        let g = &mut gen[bi * words..(bi + 1) * words];
        let k = &mut kill[bi * words..(bi + 1) * words];
        for ii in 0..=b.insts.len() {
            for &u in tab.uses(k0 + ii) {
                let x = v2g[u as usize];
                if x != u32::MAX && !bit_get(k, x) {
                    bit_set(g, x);
                }
            }
            if ii < b.insts.len() {
                for &d in tab.defs(k0 + ii) {
                    let x = v2g[d as usize];
                    if x != u32::MAX {
                        bit_set(k, x);
                    }
                }
            }
        }
    }
    let (li, lo) = solve_liveness(f, reach, words, &gen, &kill, want_out);
    drop((gen, kill));
    (LiveSets { words, v2g: v2g.clone(), g2v: g2v.clone(), sets: li }, LiveSets { words, v2g, g2v, sets: lo })
}

/// Backward liveness (bitsets of `words` per block) to its least fixpoint, by a worklist:
/// a block is revisited only when a successor's live-in grew. Returns live-in and (if
/// asked) live-out per block; unreachable blocks stay empty.
#[inline(never)]
pub fn solve_liveness(f: &Func, reach: &[bool], words: usize, gen: &[u64], kill: &[u64], want_out: bool) -> (Vec<u64>, Vec<u64>) {
    let nb = f.blocks.len();
    let mut live_in = vec![0u64; nb * words];
    if words == 0 {
        return (live_in, Vec::new());
    }
    // every edge forward (no loop): one sweep from the last block is the fixpoint
    if f.blocks.iter().enumerate().all(|(bi, b)| b.term.succs().all(|t| t as usize > bi)) {
        let mut live_out = if want_out { vec![0u64; nb * words] } else { Vec::new() };
        let mut out = vec![0u64; words];
        for bi in (0..nb).rev() {
            if !reach[bi] {
                continue;
            }
            out.iter_mut().for_each(|w| *w = 0);
            for t in f.blocks[bi].term.succs() {
                let t = t as usize;
                for w in 0..words {
                    out[w] |= live_in[t * words + w];
                }
            }
            for w in 0..words {
                live_in[bi * words + w] = gen[bi * words + w] | (out[w] & !kill[bi * words + w]);
            }
            if want_out {
                live_out[bi * words..(bi + 1) * words].copy_from_slice(&out);
            }
        }
        return (live_in, live_out);
    }
    // predecessors (reachable ones), flat
    let mut off = vec![0u32; nb + 1];
    for (bi, b) in f.blocks.iter().enumerate() {
        if reach[bi] {
            for t in b.term.succs() {
                off[t as usize + 1] += 1;
            }
        }
    }
    for i in 0..nb {
        off[i + 1] += off[i];
    }
    let mut fill = off.clone();
    let mut preds = vec![0u32; off[nb] as usize];
    for (bi, b) in f.blocks.iter().enumerate() {
        if reach[bi] {
            for t in b.term.succs() {
                preds[fill[t as usize] as usize] = bi as u32;
                fill[t as usize] += 1;
            }
        }
    }
    // Visit successors before predecessors even after loop rotation / cold layout.
    // Reverse numeric order repeatedly propagates through backward, non-loop edges.
    let mut queued = vec![false; nb];
    let mut work: Vec<u32> = Vec::with_capacity(nb);
    let mut dfs = vec![(0u32, false)];
    while let Some((b, done)) = dfs.pop() {
        if done {
            work.push(b);
        } else if reach[b as usize] && !queued[b as usize] {
            queued[b as usize] = true;
            dfs.push((b, true));
            for t in f.blocks[b as usize].term.succs() {
                if !queued[t as usize] {
                    dfs.push((t, false));
                }
            }
        }
    }
    work.reverse();
    let mut out = vec![0u64; words];
    while let Some(b) = work.pop() {
        let bi = b as usize;
        queued[bi] = false;
        out.iter_mut().for_each(|w| *w = 0);
        for t in f.blocks[bi].term.succs() {
            let t = t as usize;
            for (o, i) in out.iter_mut().zip(&live_in[t * words..(t + 1) * words]) {
                *o |= *i;
            }
        }
        let mut grew = false;
        let g = &gen[bi * words..(bi + 1) * words];
        let k = &kill[bi * words..(bi + 1) * words];
        let dst = &mut live_in[bi * words..(bi + 1) * words];
        for (((dst, g), k), o) in dst.iter_mut().zip(g).zip(k).zip(&out) {
            let newin = *g | (*o & !*k);
            grew |= newin != *dst;
            *dst = newin;
        }
        if grew {
            for &p in &preds[off[bi] as usize..off[bi + 1] as usize] {
                if !queued[p as usize] {
                    queued[p as usize] = true;
                    work.push(p);
                }
            }
        }
    }
    let mut live_out = Vec::new();
    if want_out {
        live_out = vec![0u64; nb * words];
        for bi in 0..nb {
            if !reach[bi] {
                continue;
            }
            for t in f.blocks[bi].term.succs() {
                let t = t as usize;
                for w in 0..words {
                    live_out[bi * words + w] |= live_in[t * words + w];
                }
            }
        }
    }
    (live_in, live_out)
}

/// The callee-saved registers asm! blocks write (outputs, clobbers): the prologue saves them as
/// it saves the ones values were given.
fn asm_callee_writes(f: &Func, cfg: &RegConfig, int: &mut Vec<u8>, flt: &mut Vec<u8>) {
    for b in &f.blocks {
        for i in &b.insts {
            let Inst::Call(Callee::Asm(a), _, _) = i else { continue };
            let mut m = a.writes;
            while m != 0 {
                let bit = m.trailing_zeros() as u8;
                m &= m - 1;
                let (r, set, used) = if bit < 32 { (bit, &cfg.int_callee, &mut *int) } else { (bit - 32, &cfg.flt_callee, &mut *flt) };
                if set.contains(&r) && !used.contains(&r) {
                    used.push(r);
                }
            }
        }
    }
}

pub fn allocate(options: &relax_session::Options, f: &Func, cfg: &RegConfig) -> Alloc {
    allocate_with(options, f, cfg, true, None)
}

/// `allocate`; `thorough` adds copy coalescing (allocate_split adds live-range splitting).
pub fn allocate_with(options: &relax_session::Options, f: &Func, cfg: &RegConfig, thorough: bool, freq: Option<&[f32]>) -> Alloc {
    allocate_low(options, f, cfg, thorough, freq, None, None)
}

/// allocate_with, where the vregs flagged in `low` go last (they take a register only if one is
/// left: passed-through loop pieces)
/// `origin`: for a function `split_loops` made, each loop piece's original vreg (u32::MAX for
/// the others).
fn allocate_low(options: &relax_session::Options, f: &Func, cfg: &RegConfig, thorough: bool, freq: Option<&[f32]>, low: Option<&[bool]>, origin: Option<&[u32]>) -> Alloc {
    let nv = f.vregs.len() as u32;
    let words = ((nv + 63) / 64) as usize;
    let nb = f.blocks.len();
    let reach = reachable(f);
    let mut block_start = Vec::with_capacity(nb);
    let mut block_end = Vec::with_capacity(nb);
    let mut pos = 0u32;
    for b in &f.blocks {
        block_start.push(pos);
        pos += 2 * (b.insts.len() as u32 + 1);
        block_end.push(pos);
        pos += 2;
    }
    let tab = OpTab::new(f, &reach);
    let mut call_pos = Vec::new();
    let mut use_count = vec![0u32; nv as usize];
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        let k0 = tab.first[bi] as usize;
        for (ii, i) in b.insts.iter().enumerate() {
            for &u in tab.uses(k0 + ii) {
                use_count[u as usize] += 1;
            }
            // a call that never returns (panic path: last inst before Unreachable) clobbers
            // nothing anyone reads afterwards: it does not count as a call position
            let noreturn = ii + 1 == b.insts.len() && matches!(b.term, Term::Unreachable);
            if let (Inst::Call(..), false) = (i, noreturn) {
                call_pos.push(block_start[bi] + 2 * ii as u32 + 1);
            }
        }
        for &u in tab.uses(k0 + b.insts.len()) {
            use_count[u as usize] += 1;
        }
    }
    let _ = words;
    let (live_in, live_out) = live_sets(f, &reach, &tab, true);
    let mut start = vec![u32::MAX; nv as usize];
    let mut end = vec![0u32; nv as usize];
    fn touch(v: u32, p: u32, start: &mut [u32], end: &mut [u32]) {
        let i = v as usize;
        if p < start[i] {
            start[i] = p;
        }
        if p > end[i] {
            end[i] = p;
        }
    }
    for p in &f.params {
        touch(p.0, 0, &mut start, &mut end);
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        live_in.each(bi, |v| touch(v, block_start[bi], &mut start, &mut end));
        live_out.each(bi, |v| touch(v, block_end[bi], &mut start, &mut end));
        let k0 = tab.first[bi] as usize;
        for ii in 0..b.insts.len() {
            let p = block_start[bi] + 2 * ii as u32 + 1;
            for &u in tab.uses(k0 + ii) {
                touch(u, p, &mut start, &mut end);
            }
            for &d in tab.defs(k0 + ii) {
                touch(d, p + 1, &mut start, &mut end);
            }
        }
        for &u in tab.uses(k0 + b.insts.len()) {
            touch(u, block_end[bi] - 1, &mut start, &mut end);
        }
    }
    // live ranges with holes (per block, built backwards from liveness)
    drop(live_in);
    let ranges = build_ranges(f, &reach, &tab, &block_start, &block_end, &live_out);
    drop(live_out);
    let wts = block_weights(options, f, &reach, freq);
    let aff = affinities(f, &reach, &block_start, &ranges, &wts);
    let (mut remat, base_only) = remat_values(options, f, &reach, cfg);
    // base + k is recomputed at its uses: the base must be live (hold its one value) at each of
    // them -- not over the whole range of the value (the cold-call split leaves the base a hole
    // at a call the value lives across)
    if remat.iter().any(|r| matches!(r, Remat::Offset(..))) {
        let mut ok = vec![true; nv as usize];
        let live_at = |b: u32, p: u32| ranges.of(b).iter().any(|&(a, z)| a <= p && p <= z);
        for (bi, b) in f.blocks.iter().enumerate() {
            if !reach[bi] {
                continue;
            }
            let k0 = tab.first[bi] as usize;
            for k in 0..=b.insts.len() {
                let p = if k < b.insts.len() { block_start[bi] + 2 * k as u32 + 1 } else { block_end[bi] - 1 };
                for &u in tab.uses(k0 + k) {
                    if let Remat::Offset(base, _) = remat[u as usize] {
                        if !live_at(base.0, p) {
                            ok[u as usize] = false;
                        }
                    }
                }
            }
        }
        for v in 0..nv as usize {
            if !ok[v] {
                remat[v] = Remat::No;
            }
        }
    }
    // copy coalescing: vregs joined by a copy (or an operation whose operand dies there) whose
    // live ranges never meet become one allocation unit, strongest pairs first. The copies
    // between them then cost nothing, whatever order the units are placed in.
    let is_low = |v: u32| low.map_or(false, |lw| lw.get(v as usize).copied().unwrap_or(false));
    let mut parent: Vec<u32> = (0..nv).collect();
    let mut merged: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
    if thorough && coalesce_on(options) {
        fn find(p: &mut [u32], mut x: u32) -> u32 {
            while p[x as usize] != x {
                let g = p[p[x as usize] as usize];
                p[x as usize] = g;
                x = g;
            }
            x
        }
        let mut pairs: Vec<(f32, u32, u32)> = Vec::new();
        for v in 0..nv {
            for &(u, w) in aff.of(v) {
                if v < u {
                    pairs.push((w, v, u));
                }
            }
        }
        let equal = equal_copies(options, f, &reach, &tab, &block_start, &ranges, &wts, &remat);
        for &(w, a, b) in &equal {
            if !pairs.iter().any(|x| x.1 == a && x.2 == b) {
                pairs.push((w, a, b));
            }
        }
        let equal: std::collections::HashSet<(u32, u32)> = equal.iter().map(|x| (x.1, x.2)).collect();
        pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        for (_, a, b) in pairs {
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra == rb || f.vregs[ra as usize] != f.vregs[rb as usize] || remat[a as usize].is_some() || remat[b as usize].is_some() || remat[ra as usize].is_some() || remat[rb as usize].is_some() || is_low(a) || is_low(b) {
                continue;
            }
            let x = merged.get(&ra).map(|v| v.as_slice()).unwrap_or(ranges.of(ra));
            let y = merged.get(&rb).map(|v| v.as_slice()).unwrap_or(ranges.of(rb));
            // overlapping ranges join only for a copy of one value (equal_copies) between two
            // values not yet in a unit: wherever both are live they hold the same bits
            let same = (ra, rb) == (a, b) && !merged.contains_key(&ra) && !merged.contains_key(&rb) && equal.contains(&(a, b));
            if x.is_empty() || y.is_empty() || (ranges_meet(x, y) && !same) {
                continue;
            }
            let u = union_ranges(x, y);
            merged.remove(&rb);
            merged.insert(ra, u);
            parent[rb as usize] = ra;
        }
        for v in 0..nv {
            let r = find(&mut parent, v);
            parent[v as usize] = r;
        }
    }
    // merged units' ranges by index (no hashing in the hot paths below)
    let mut merged_ix: Vec<u32> = vec![u32::MAX; nv as usize];
    let merged_list: Vec<Vec<(u32, u32)>> = {
        let mut l = Vec::with_capacity(merged.len());
        for (k, v) in merged.drain() {
            merged_ix[k as usize] = l.len() as u32;
            l.push(v);
        }
        l
    };
    // a unit that finds no register is dissolved back into its members (aggressive coalescing
    // must not turn values that fit apart into one that spills)
    let dissolved: Vec<std::cell::Cell<bool>> = (0..nv).map(|_| std::cell::Cell::new(false)).collect();
    let rng = |v: u32| -> &[(u32, u32)] {
        let k = merged_ix[v as usize];
        if k == u32::MAX || dissolved[v as usize].get() {
            ranges.of(v)
        } else {
            &merged_list[k as usize]
        }
    };
    let mut members: HashMap<u32, Vec<u32>> = HashMap::new();
    for v in 0..nv {
        if parent[v as usize] != v {
            members.entry(parent[v as usize]).or_default().push(v);
        }
    }
    let unit_members = |v: u32| -> Option<&Vec<u32>> {
        if merged_ix[v as usize] != u32::MAX && !dissolved[v as usize].get() {
            members.get(&v)
        } else {
            None
        }
    };
    let mut order: Vec<u32> = Vec::new();
    for v in 0..nv {
        if parent[v as usize] == v && !rng(v).is_empty() && !base_only[v as usize] {
            order.push(v);
        }
    }
    // priority: spill weight = loop-weighted references per position of live range, so a
    // short temporary never loses its register to a long-lived value with few uses (the
    // allocator has no eviction: what is placed first keeps its register)
    let (refs, lens) = spill_weights(f, &reach, &tab, &ranges, &wts);
    let mut weight = vec![0f32; nv as usize];
    for &v in &order {
        let (mut r, mut l) = (refs[v as usize], lens[v as usize]);
        if let Some(m) = if merged_ix[v as usize] != u32::MAX { members.get(&v) } else { None } {
            for &u in m {
                r += refs[u as usize];
                l += lens[u as usize];
            }
        }
        // recomputing costs about what a reload does, without the store: half the weight
        weight[v as usize] = prio(options, r, l) * if remat[v as usize].is_some() { 0.5 } else { 1.0 };
        if low.map_or(false, |lw| lw.get(v as usize).copied().unwrap_or(false)) {
            weight[v as usize] *= 1e-6;
        }
    }
    let first: Vec<u32> = (0..nv).map(|v| rng(v).first().map_or(0, |x| x.0)).collect();
    if ra_order_by_start(options) {
        order.sort_by_key(|a| first[*a as usize]);
    } else {
        order.sort_by(|a, b| weight[*b as usize].partial_cmp(&weight[*a as usize]).unwrap_or(std::cmp::Ordering::Equal).then(first[*a as usize].cmp(&first[*b as usize])));
    }
    call_pos.sort();
    // weight of each call's block (call_pos order)
    let call_w: Vec<f32> = call_pos.iter().map(|&p| wts[block_start.partition_point(|&s| s <= p) - 1]).collect();
    // summed weight of the calls strictly inside a value's ranges
    let cross_w = |r: &[(u32, u32)]| -> f32 {
        let mut w = 0.0;
        for (a, b) in r {
            let from = call_pos.partition_point(|p| *p <= *a);
            for k in from..call_pos.len() {
                if call_pos[k] >= *b {
                    break;
                }
                w += call_w[k];
            }
        }
        w
    };
    let entry_w = wts.first().copied().unwrap_or(1.0).max(1e-6);
    let crosses = |r: &[(u32, u32)]| -> bool {
        for (a, b) in r {
            // first call strictly after a
            let mut lo = 0usize;
            let mut hi = call_pos.len();
            while lo < hi {
                let mid = (lo + hi) / 2;
                if call_pos[mid] <= *a {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            if lo < call_pos.len() && call_pos[lo] < *b {
                return true;
            }
        }
        false
    };
    // occupied ranges per physical register (ints 0..32, floats 32..64): disjoint,
    // keyed by start
    // one bit per position (allocated on a register's first use)
    let pos_words = (pos as usize + 64) / 64;
    let mut occ: Vec<Vec<u64>> = (0..64).map(|_| Vec::new()).collect();
    if let Some(cl) = cfg.clobbers {
        for (bi, b) in f.blocks.iter().enumerate() {
            if !reach[bi] {
                continue;
            }
            for (ii, i) in b.insts.iter().enumerate() {
                let mut m = cl(options, i);
                if m == 0 {
                    continue;
                }
                let p = block_start[bi] + 2 * ii as u32 + 1;
                // an asm! block's registers also hold its results until they are moved out: no
                // result of it lives in another one's register (positions p..=p+1)
                let w = if matches!(i, Inst::Call(Callee::Asm(_), _, _)) { 2 } else { 1 };
                while m != 0 {
                    let r = m.trailing_zeros() as usize;
                    m &= m - 1;
                    if occ[r].is_empty() {
                        occ[r].resize(pos_words, 0);
                    }
                    bits_fill(&mut occ[r], p, p + w);
                }
            }
        }
    }
    let overlaps = |o: &Vec<u64>, r: &[(u32, u32)]| -> bool { !o.is_empty() && r.iter().any(|&(a, b)| bits_any(o, a, b)) };
    let mut loc = vec![Loc::None; nv as usize];
    let mut used_callee_int = Vec::new();
    let mut used_callee_flt = Vec::new();
    let mut spill_bytes = 0u32;
    let mut needs_save: Vec<u32> = Vec::new();
    let mut cands: Vec<u8> = Vec::with_capacity(32);
    let mut partners: Vec<(u32, f32)> = Vec::new();
    let no_callee: Vec<u8> = Vec::new();
    let mut oi = 0;
    while oi < order.len() {
        let v = order[oi];
        oi += 1;
        let r = rng(v);
        let is_int = f.vregs[v as usize] == Cls::I;
        let crossing = crosses(r);
        // crossing only calls that run rarely next to the function's entry (error paths of a
        // `Result` fn, a slow path): a caller-saved register saved around those calls costs less
        // than a callee-saved one, whose push / pop runs on every call of the function
        let cold_cross = crossing && cross_w(r) < 0.5 * entry_w;
        // a recomputed value never pays for a call: no callee-saved register (push / pop on
        // every entry) and no saves around calls; it is recomputed after them instead
        if crossing && remat[v as usize].is_some() {
            loc[v as usize] = Loc::None;
            if let Some(m) = unit_members(v) {
                for &u in m {
                    loc[u as usize] = Loc::None;
                }
            }
            continue;
        }
        let (caller, callee) = if is_int { (&cfg.int_caller, &cfg.int_callee) } else { (&cfg.flt_caller, &cfg.flt_callee) };
        // a vector crossing a call goes to a caller-saved register (saved/restored around
        // the call in 16 bytes): arm64's callee-saved d8-d15 keep only the low 64 bits
        let vec_cross = crossing && f.vregs[v as usize].is_vec();
        let callee = if vec_cross { &no_callee } else { callee };
        let base = if is_int { 0 } else { 32 };
        cands.clear();
        // registers of already allocated copy partners first (strongest affinity first): the
        // copies between them then vanish
        let mut n_aff = 0;
        let mv = unit_members(v);
        partners.clear();
        partners.extend_from_slice(aff.of(v));
        if let Some(m) = mv {
            for &u in m {
                partners.extend_from_slice(aff.of(u));
            }
            partners.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        }
        for &(u, _) in &partners {
            let u = parent[u as usize];
            let x = match loc[u as usize] {
                Loc::Reg(x) if is_int => x,
                Loc::Flt(x) if !is_int && !(vec_cross && cfg.flt_callee.contains(&x)) => x,
                _ => continue,
            };
            if !cands.contains(&x) {
                cands.push(x);
                n_aff += 1;
            }
        }
        if crossing && !cold_cross {
            cands.extend_from_slice(callee);
            cands.extend_from_slice(caller);
        } else {
            cands.extend_from_slice(caller);
            cands.extend_from_slice(callee);
        }
        // no partner placed yet: look ahead to the strongest partner and prefer a register that is
        // free over its ranges too (a short temporary placed first must not take a register its
        // loop variable cannot share)
        let ahead = if n_aff == 0 {
            partners.iter().map(|x| parent[x.0 as usize]).find(|u| *u != v && loc[*u as usize] == Loc::None && f.vregs[*u as usize] == f.vregs[v as usize])
        } else {
            None
        };
        if let Some(u) = ahead {
            let ru = rng(u);
            if let Some(k) = cands.iter().position(|&c| !overlaps(&occ[base + c as usize], r) && !overlaps(&occ[base + c as usize], ru) && !(crossing && !cold_cross && !callee.is_empty() && !callee.contains(&c))) {
                let c = cands.remove(k);
                cands.insert(0, c);
            }
        }
        let mut chosen = None;
        for (k, &c) in cands.iter().enumerate() {
            if k < n_aff && crossing && !cold_cross && !callee.contains(&c) && callee.len() > 0 {
                // a partner's caller-saved register across a call costs saves; let a free
                // callee-saved register win
                let mut callee_free = false;
                for &k in callee.iter() {
                    if !overlaps(&occ[base + k as usize], r) {
                        callee_free = true;
                    }
                }
                if callee_free {
                    continue;
                }
            }
            if !overlaps(&occ[base + c as usize], r) {
                chosen = Some(c);
                break;
            }
        }
        if chosen.is_none() {
            if let Some(m) = unit_members(v) {
                // dissolve: the root and its members take their turns now, heaviest first
                let mut parts: Vec<u32> = m.clone();
                parts.push(v);
                dissolved[v as usize].set(true);
                parts.sort_by(|a, b| prio(options, refs[*b as usize], lens[*b as usize]).partial_cmp(&prio(options, refs[*a as usize], lens[*a as usize])).unwrap_or(std::cmp::Ordering::Equal));
                let parts: Vec<u32> = parts.into_iter().filter(|u| !ranges.of(*u).is_empty()).collect();
                order.splice(oi..oi, parts);
                continue;
            }
        }
        let l = match chosen {
            Some(c) => {
                let o = &mut occ[base + c as usize];
                if o.is_empty() {
                    o.resize(pos_words, 0);
                }
                for &(a, b) in r {
                    bits_fill(o, a, b);
                }
                if callee.contains(&c) {
                    let used = if is_int { &mut used_callee_int } else { &mut used_callee_flt };
                    if !used.contains(&c) {
                        used.push(c);
                    }
                } else if crossing {
                    needs_save.push(v);
                    if let Some(m) = unit_members(v) {
                        needs_save.extend_from_slice(m);
                    }
                }
                if is_int {
                    Loc::Reg(c)
                } else {
                    Loc::Flt(c)
                }
            }
            None if remat[v as usize].is_some() => Loc::None,
            None => {
                // vectors: 16-aligned offsets (backends address slots downwards)
                spill_bytes = grow_spill(spill_bytes, f.vregs[v as usize]);
                Loc::Stack(spill_bytes as i32)
            }
        };
        loc[v as usize] = l;
        if let Some(m) = unit_members(v) {
            for &u in m {
                loc[u as usize] = l;
            }
        }
    }
    // MAKEPAD_RUN_RA_TRACE=<fn suffix>: why each value spilled (weight, length, unit) and which
    // allocated values hold each register over its ranges
    if trace_on(f) {
        for v in 0..nv as usize {
            if !matches!(loc[v], Loc::Stack(_)) {
                continue;
            }
            let is_int = f.vregs[v] == Cls::I;
            let r = rng(v as u32);
            let len: u32 = r.iter().map(|(a, b)| b - a + 1).sum();
            let unit = unit_members(v as u32).map(|m| m.len()).unwrap_or(0);
            let mut by: Vec<String> = Vec::new();
            let regs: Vec<u8> = if is_int { cfg.int_caller.iter().chain(cfg.int_callee.iter()).copied().collect() } else { cfg.flt_caller.iter().chain(cfg.flt_callee.iter()).copied().collect() };
            for c in regs {
                let at = if is_int { Loc::Reg(c) } else { Loc::Flt(c) };
                let hold: Vec<String> = (0..nv as usize).filter(|&u| loc[u] == at && ranges_meet(rng(u as u32), r)).map(|u| format!("v{}({:.3})", u, weight[u])).collect();
                by.push(format!("{}{}:{}", if is_int { "r" } else { "f" }, c, hold.join("+")));
            }
            eprintln!("ra-spill {} v{} w={:.3} refs={:.2} len={} unit={} ranges={} | {}", f.name, v, weight[v], refs[v], len, unit, r.len(), by.join(" "));
        }
    }
    // spill slots shared along copies: a copy between two spilled values whose live ranges
    // never meet puts both in one slot, so the copy is no code at all (call-crossing splits of a
    // value that spilled anyway were a load + store per copy: shape's apply_forward moved values
    // between slots around every call)
    if thorough && slot_share_on(options) {
        let mut pairs: Vec<(f32, i32, i32)> = Vec::new();
        for (bi, b) in f.blocks.iter().enumerate() {
            if !reach[bi] {
                continue;
            }
            for i in &b.insts {
                if let Inst::Mov(d, s2) = i {
                    let (cd, cs) = (f.vregs[d.0 as usize], f.vregs[s2.0 as usize]);
                    if cd.spill_bytes() != cs.spill_bytes() {
                        continue;
                    }
                    if let (Loc::Stack(a), Loc::Stack(b2)) = (loc[d.0 as usize], loc[s2.0 as usize]) {
                        if a != b2 {
                            pairs.push((wts[bi], a, b2));
                        }
                    }
                }
            }
        }
        if !pairs.is_empty() {
            pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
            // ranges per involved slot (every vreg placed there)
            let mut slot_r: HashMap<i32, Vec<(u32, u32)>> = HashMap::new();
            for &(_, a, b2) in &pairs {
                slot_r.entry(a).or_default();
                slot_r.entry(b2).or_default();
            }
            for v in 0..nv as usize {
                if let Loc::Stack(o) = loc[v] {
                    if let Some(e) = slot_r.get_mut(&o) {
                        let u = union_ranges(e, ranges.of(v as u32));
                        *e = u;
                    }
                }
            }
            let mut to: HashMap<i32, i32> = HashMap::new();
            let root = |to: &HashMap<i32, i32>, mut o: i32| {
                while let Some(&n) = to.get(&o) {
                    o = n;
                }
                o
            };
            for (_, a, b2) in pairs {
                let (ra, rb) = (root(&to, a), root(&to, b2));
                if ra == rb {
                    continue;
                }
                let (x, y) = (&slot_r[&ra], &slot_r[&rb]);
                if ranges_meet(x, y) {
                    continue;
                }
                let u = union_ranges(x, y);
                slot_r.insert(ra, u);
                to.insert(rb, ra);
            }
            if !to.is_empty() {
                for l in loc.iter_mut() {
                    if let Loc::Stack(o) = *l {
                        let r = root(&to, o);
                        if r != o {
                            *l = Loc::Stack(r);
                        }
                    }
                }
            }
        }
    }
    // a loop piece that still spills uses its original's slot: the copies between them become
    // no-ops (both hold the same value wherever both are live: the piece lives inside its loop
    // only, where the original is dead when the piece is redefined, as every exit copies back)
    // only between slots no other vreg uses (coalescing may have put several vregs in one slot,
    // and another member of the original's unit can be live where the piece is)
    if let Some(origin) = origin {
        let mut slot_users: HashMap<i32, u32> = HashMap::new();
        for l in &loc {
            if let Loc::Stack(o) = l {
                *slot_users.entry(*o).or_insert(0) += 1;
            }
        }
        for v in 0..f.vregs.len() {
            let o = origin[v];
            if o != u32::MAX {
                let mut root = o;
                while origin[root as usize] != u32::MAX {
                    root = origin[root as usize];
                }
                if let (Loc::Stack(y), Loc::Stack(x)) = (loc[v], loc[root as usize]) {
                    if !f.vregs[v].is_vec() && slot_users[&x] == 1 && slot_users[&y] == 1 {
                        loc[v] = Loc::Stack(x);
                    }
                }
            }
        }
    }
    // spill slots shared by live range: values that are never live at once take one slot.
    // Every spilled value had a slot of its own, so baseline frames carried one 8-byte cell per
    // spill (Scope's recursive type parser: ~800 bytes of parse_type's frame)
    if slot_share_on(options) {
        spill_bytes = pack_spill_slots(f, &reach, &block_start, &mut loc, &ranges, &start);
    }
    for v in 0..nv as usize {
        if !matches!(loc[v], Loc::None) {
            remat[v] = Remat::No;
        }
    }
    let mut saves: HashMap<u32, Vec<(VReg, i32)>> = HashMap::new();
    for v in needs_save {
        spill_bytes = grow_spill(spill_bytes, f.vregs[v as usize]);
        let slot = spill_bytes as i32;
        for (a, b) in ranges.of(v) {
            // the calls strictly inside (a, b) (call_pos is sorted)
            let from = call_pos.partition_point(|p| *p <= *a);
            for &p in &call_pos[from..] {
                if p >= *b {
                    break;
                }
                saves.entry(p).or_default().push((VReg(v), slot));
            }
        }
    }
    let (save_skip, restore_skip) = if thorough && !crate::stats::opt_off(options, "-saveskip") { redundant_saves(f, &reach, &tab, &block_start, &saves, &remat) } else { Default::default() };
    // region windows (O3): a spilled int value used in many blocks once each (a loop-carried
    // value whose register some short temporary holds somewhere in the loop) gets a register
    // over the blocks where one is free over its whole live part: blocks it enters live and does
    // not redefine, with no call over it for a caller-saved register. Such a block loads the value
    // at its start only when a predecessor is not in the region (the slot stays its home, as for
    // windows); one whose predecessors all hold it in the register loads nothing.
    let mut chained: Vec<(u32, u32, u32, u8)> = Vec::new();
    let mut region_windows: Vec<(u32, u32, u32, u8)> = Vec::new();
    let mut in_region: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    if thorough && windows_on(options) && !crate::stats::opt_off(options, "-regionwin") {
        region_windows_alloc(f, &reach, &tab, &block_start, &block_end, &ranges, &wts, &loc, &remat, &call_pos, cfg, &used_callee_int, &mut occ, pos_words, &mut region_windows, &mut chained, &mut in_region);
    }
    // spill windows (O3): a spilled int value's runs of >= 2 uses inside a block take a
    // register free over the run in the final occupancy -- one load instead of one per use.
    // Recomputed constants and addresses too (one recomputation instead of one per use), except
    // as the value of a store that writes the constant directly (RegConfig::store_imm)
    let mut windows: Vec<(u32, u32, u32, u8)> = Vec::new();
    if thorough && windows_on(options) {
        let regs: Vec<u8> = cfg.int_caller.iter().chain(used_callee_int.iter()).copied().collect();
        let callee_set = used_callee_int.clone();
        let simple = |i: &Inst| matches!(i, Inst::IBin(..) | Inst::IBinI(..) | Inst::ICmp(..) | Inst::ICmpI(..) | Inst::Load(..) | Inst::Store(..) | Inst::LoadX(..) | Inst::StoreX(..) | Inst::Conv(..) | Inst::Mov(..));
        // blocks heaviest first: hot runs get the free registers first
        let mut bo: Vec<usize> = (0..nb).filter(|b| reach[*b]).collect();
        bo.sort_by(|a, b| wts[*b].partial_cmp(&wts[*a]).unwrap_or(std::cmp::Ordering::Equal));
        for bi in bo {
            let b = &f.blocks[bi];
            let n = b.insts.len();
            if n < 4 {
                continue;
            }
            let lim = n - 2;
            // per spilled vreg: use instruction indices, in order; a def or a call ends a run
            let mut runs: HashMap<u32, Vec<usize>> = HashMap::new();
            let mut done: Vec<(u32, Vec<usize>)> = Vec::new();
            for ii in 0..lim {
                let i = &b.insts[ii];
                if matches!(i, Inst::Call(..)) {
                    for (v, r) in runs.drain() {
                        done.push((v, r));
                    }
                    continue;
                }
                let mut ds = Vec::new();
                for_defs(i, |d| ds.push(d.0));
                if simple(i) {
                    let stored = match i {
                        Inst::Store(_, _, _, s2) | Inst::StoreX(_, _, _, _, _, s2) => match remat[s2.0 as usize] {
                            Remat::Const(k) if loc[s2.0 as usize] == Loc::None && (cfg.store_imm)(k) => Some(s2.0),
                            _ => None,
                        },
                        _ => None,
                    };
                    for_uses(i, |u| {
                        let uv = u.0 as usize;
                        let rm = loc[uv] == Loc::None && matches!(remat[uv], Remat::Const(_) | Remat::Addr(_) | Remat::FnAddr(_)) && stored != Some(u.0);
                        if (matches!(loc[uv], Loc::Stack(_)) || rm) && f.vregs[uv] == Cls::I && !ds.contains(&u.0) {
                            let e = runs.entry(u.0).or_default();
                            if e.last() != Some(&ii) {
                                e.push(ii);
                            }
                        }
                    });
                }
                for d in ds {
                    if let Some(r) = runs.remove(&d) {
                        done.push((d, r));
                    }
                }
            }
            for (v, r) in runs.drain() {
                done.push((v, r));
            }
            done.sort_by_key(|x| (std::cmp::Reverse(x.1.len()), x.0));
            for (v, us) in done {
                if us.len() < 2 || in_region.contains(&(v, bi as u32)) {
                    continue;
                }
                let a = block_start[bi] + 2 * us[0] as u32; // just before the first use
                let z = block_start[bi] + 2 * *us.last().unwrap() as u32 + 1;
                // the register must also be free over the previous instruction: the emitters defer
                // a folded load / fused compare into the next instruction, which then reads operands
                // whose ranges ended at the previous one -- after this window's load at its start
                let a0 = if us[0] > 0 { a - 2 } else { a };
                for &c in &regs {
                    let o = &mut occ[c as usize];
                    if !o.is_empty() && bits_any(o, a0, z) {
                        continue;
                    }
                    // no call strictly inside for a caller-saved register (runs hold no calls, but
                    // be safe)
                    if !callee_set.contains(&c) && call_pos.iter().any(|p| *p > a && *p < z) {
                        continue;
                    }
                    if o.is_empty() {
                        o.resize(pos_words, 0);
                    }
                    bits_fill(o, a0, z);
                    windows.push((v, a + 1, z, c));
                    break;
                }
            }
        }
    }
    windows.extend(region_windows);
    asm_callee_writes(f, cfg, &mut used_callee_int, &mut used_callee_flt);
    Alloc { loc, used_callee_int, used_callee_flt, spill_bytes, block_start, block_end, saves, start, end, uses: use_count, remat, windows, chained, save_skip, restore_skip }
}

/// Saves around calls that need no store or no load (Alloc::save_skip / restore_skip): forward,
/// a value's save slot is clean after a save until the value is redefined (paths meet by
/// intersection); backward, a restore is needed only when the register is read before it is
/// redefined -- by a use or by a later save that stores (paths meet by union). A value live
/// across a run of calls (lookup_local: five values around 18 calls) is then stored once and
/// loaded only where it is used. A use of a recomputed base + k (Remat::Offset) reads its base's
/// register.
fn redundant_saves(f: &Func, reach: &[bool], tab: &OpTab, block_start: &[u32], saves: &HashMap<u32, Vec<(VReg, i32)>>, remat: &[Remat]) -> (std::collections::HashSet<(u32, u32)>, std::collections::HashSet<(u32, u32)>) {
    let mut skip_s = std::collections::HashSet::new();
    let mut skip_r = std::collections::HashSet::new();
    if saves.is_empty() {
        return (skip_s, skip_r);
    }
    // the saved values, densely
    let mut ix: HashMap<u32, usize> = HashMap::new();
    for l in saves.values() {
        for (v, _) in l {
            let n = ix.len();
            ix.entry(v.0).or_insert(n);
        }
    }
    let k = ix.len();
    let w = (k + 63) / 64;
    let nb = f.blocks.len();
    let preds = preds_of(f, reach);
    let set = |s: &mut [u64], i: usize| s[i >> 6] |= 1 << (i & 63);
    let clr = |s: &mut [u64], i: usize| s[i >> 6] &= !(1u64 << (i & 63));
    let get = |s: &[u64], i: usize| s[i >> 6] >> (i & 63) & 1 != 0;
    // forward: clean save slots
    let mut out = vec![vec![!0u64; w]; nb];
    let mut changed = true;
    while changed {
        changed = false;
        for bi in 0..nb {
            if !reach[bi] {
                continue;
            }
            let mut st = if bi == 0 || preds[bi].is_empty() {
                vec![0u64; w]
            } else {
                let mut x = vec![!0u64; w];
                for &p in &preds[bi] {
                    for j in 0..w {
                        x[j] &= out[p as usize][j];
                    }
                }
                x
            };
            let b = &f.blocks[bi];
            let k0 = tab.first[bi] as usize;
            for ii in 0..b.insts.len() {
                let p = block_start[bi] + 2 * ii as u32 + 1;
                if let Some(l) = saves.get(&p) {
                    for (v, _) in l {
                        set(&mut st, ix[&v.0]);
                    }
                }
                for &d in tab.defs(k0 + ii) {
                    if let Some(&i) = ix.get(&d) {
                        clr(&mut st, i);
                    }
                }
            }
            if st != out[bi] {
                out[bi] = st;
                changed = true;
            }
        }
    }
    // the stores the clean state makes redundant
    for bi in 0..nb {
        if !reach[bi] {
            continue;
        }
        let mut st = if bi == 0 || preds[bi].is_empty() {
            vec![0u64; w]
        } else {
            let mut x = vec![!0u64; w];
            for &p in &preds[bi] {
                for j in 0..w {
                    x[j] &= out[p as usize][j];
                }
            }
            x
        };
        let b = &f.blocks[bi];
        let k0 = tab.first[bi] as usize;
        for ii in 0..b.insts.len() {
            let p = block_start[bi] + 2 * ii as u32 + 1;
            if let Some(l) = saves.get(&p) {
                for (v, _) in l {
                    let i = ix[&v.0];
                    if get(&st, i) {
                        skip_s.insert((p, v.0));
                    }
                    set(&mut st, i);
                }
            }
            for &d in tab.defs(k0 + ii) {
                if let Some(&i) = ix.get(&d) {
                    clr(&mut st, i);
                }
            }
        }
    }
    // backward: registers read before redefinition (a recomputed base + k reads its base, and
    // that base may be one too)
    let read = |st: &mut [u64], mut u: u32| loop {
        if let Some(&i) = ix.get(&u) {
            set(st, i);
        }
        match remat[u as usize] {
            Remat::Offset(b, _) => u = b.0,
            _ => break,
        }
    };
    let mut inn = vec![vec![0u64; w]; nb];
    let walk = |bi: usize, inn: &Vec<Vec<u64>>, skip_r: &mut Option<&mut std::collections::HashSet<(u32, u32)>>| -> Vec<u64> {
        let b = &f.blocks[bi];
        let mut st = vec![0u64; w];
        for s2 in b.term.succs() {
            for j in 0..w {
                st[j] |= inn[s2 as usize][j];
            }
        }
        let k0 = tab.first[bi] as usize;
        for &u in tab.uses(k0 + b.insts.len()) {
            read(&mut st, u);
        }
        for ii in (0..b.insts.len()).rev() {
            let p = block_start[bi] + 2 * ii as u32 + 1;
            let l = saves.get(&p);
            if let Some(l) = l {
                for (v, _) in l {
                    let i = ix[&v.0];
                    if !get(&st, i) {
                        if let Some(r) = skip_r.as_mut() {
                            r.insert((p, v.0));
                        }
                    }
                    clr(&mut st, i);
                }
            }
            for &d in tab.defs(k0 + ii) {
                if let Some(&i) = ix.get(&d) {
                    clr(&mut st, i);
                }
            }
            if let Some(l) = l {
                for (v, _) in l {
                    if !skip_s.contains(&(p, v.0)) {
                        set(&mut st, ix[&v.0]);
                    }
                }
            }
            for &u in tab.uses(k0 + ii) {
                read(&mut st, u);
            }
        }
        st
    };
    let mut changed = true;
    while changed {
        changed = false;
        for bi in (0..nb).rev() {
            if !reach[bi] {
                continue;
            }
            let st = walk(bi, &inn, &mut None);
            if st != inn[bi] {
                inn[bi] = st;
                changed = true;
            }
        }
    }
    for bi in 0..nb {
        if reach[bi] {
            walk(bi, &inn, &mut Some(&mut skip_r));
        }
    }
    (skip_s, skip_r)
}

/// Region windows (see allocate_low): per spilled int value, heaviest first, the register whose
/// region -- blocks the value enters live, does not define, and holds the register free over --
/// saves the most weighted loads; a block of the region loads at its start unless every
/// predecessor is in the region too (`chained`: the register already holds the value).
#[allow(clippy::too_many_arguments)]
fn region_windows_alloc(
    f: &Func,
    reach: &[bool],
    tab: &OpTab,
    block_start: &[u32],
    block_end: &[u32],
    ranges: &Ranges,
    wts: &[f32],
    loc: &[Loc],
    remat: &[Remat],
    call_pos: &[u32],
    cfg: &RegConfig,
    used_callee_int: &[u8],
    occ: &mut [Vec<u64>],
    pos_words: usize,
    loading: &mut Vec<(u32, u32, u32, u8)>,
    chained: &mut Vec<(u32, u32, u32, u8)>,
    in_region: &mut std::collections::HashSet<(u32, u32)>,
) {
    let nv = f.vregs.len();
    let nb = f.blocks.len();
    let cand: Vec<bool> = (0..nv).map(|v| matches!(loc[v], Loc::Stack(_)) && f.vregs[v] == Cls::I && !remat[v].is_some()).collect();
    if !cand.iter().any(|x| *x) {
        return;
    }
    // per candidate: blocks it is used in (count) and blocks that define it
    let mut uses_in: HashMap<u32, HashMap<u32, u32>> = HashMap::new();
    let mut def_in: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        let k0 = tab.first[bi] as usize;
        for k in 0..=b.insts.len() {
            for &u in tab.uses(k0 + k) {
                if cand[u as usize] {
                    *uses_in.entry(u).or_default().entry(bi as u32).or_insert(0) += 1;
                }
            }
            if k < b.insts.len() {
                for &d in tab.defs(k0 + k) {
                    if cand[d as usize] {
                        def_in.insert((d, bi as u32));
                    }
                }
            }
        }
    }
    // blocks the emitters may skip (only moves, then a jump: x64 / arm64 jump threading sends
    // their predecessors straight on) or that start no code are never in a region
    let skippable: Vec<bool> = f.blocks.iter().map(|b| b.insts.is_empty() || (matches!(b.term, Term::Jump(_)) && b.insts.iter().all(|i| matches!(i, Inst::Mov(..))))).collect();
    let preds = preds_of(f, reach);
    // a predecessor ending in an invoke reaches its successors (the landing pad) after a call
    let invoke_pred: Vec<bool> = (0..nb).map(|b| preds[b].iter().any(|&p| matches!(f.blocks[p as usize].term, Term::Invoke(..)))).collect();
    let regs: Vec<u8> = cfg.int_caller.iter().chain(used_callee_int.iter()).copied().collect();
    let mut order: Vec<u32> = (0..nv as u32).filter(|v| cand[*v as usize] && uses_in.get(v).map_or(false, |m| m.len() >= 2)).collect();
    let heat = |v: u32| -> f32 { uses_in[&v].iter().map(|(b, n)| wts[*b as usize] * *n as f32).sum() };
    order.sort_by(|a, b| heat(*b).partial_cmp(&heat(*a)).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(b)));
    let block_of = |p: u32| block_start.partition_point(|&s| s <= p) - 1;
    for v in order {
        let rv = ranges.of(v);
        // per block the value enters live: its live pieces there
        let mut pieces: Vec<(u32, Vec<(u32, u32)>)> = Vec::new();
        for &(a, z) in rv {
            let mut p = a;
            loop {
                let b = block_of(p);
                let e = z.min(block_end[b]);
                match pieces.last_mut() {
                    Some((lb, ps)) if *lb == b as u32 => ps.push((p, e)),
                    _ => pieces.push((b as u32, vec![(p, e)])),
                }
                if e >= z || b + 1 >= nb {
                    break;
                }
                p = block_start[b + 1];
            }
        }
        let live_in = |b: u32, ps: &Vec<(u32, u32)>| ps.first().map_or(false, |x| x.0 == block_start[b as usize]);
        let elig: Vec<(u32, &Vec<(u32, u32)>)> = pieces.iter().filter(|(b, ps)| reach[*b as usize] && !skippable[*b as usize] && live_in(*b, ps) && !def_in.contains(&(v, *b))).map(|(b, ps)| (*b, ps)).collect();
        if elig.len() < 2 {
            continue;
        }
        let used = &uses_in[&v];
        let mut best: Option<(f32, u8, Vec<(u32, bool)>)> = None;
        for &c in &regs {
            let callee = used_callee_int.contains(&c);
            let o = &occ[c as usize];
            let mut set: Vec<u32> = Vec::new();
            for (b, ps) in &elig {
                let free = o.is_empty() || !ps.iter().any(|&(a, z)| bits_any(o, a, z));
                let call = !callee && ps.iter().any(|&(a, z)| {
                    let k = call_pos.partition_point(|q| *q < a);
                    k < call_pos.len() && call_pos[k] <= z
                });
                if free && !call {
                    set.push(*b);
                }
            }
            if set.len() < 2 {
                continue;
            }
            // drop blocks that only load for nothing (no use, and a load needed) until stable
            loop {
                let inset = |b: u32| set.binary_search(&b).is_ok();
                let needs = |b: u32| invoke_pred[b as usize] || preds[b as usize].is_empty() || preds[b as usize].iter().any(|p| !inset(*p));
                let drop: Vec<u32> = set.iter().copied().filter(|b| !used.contains_key(b) && needs(*b)).collect();
                if drop.is_empty() {
                    break;
                }
                set.retain(|b| !drop.contains(b));
            }
            let inset = |b: u32| set.binary_search(&b).is_ok();
            let mut gain = 0f32;
            let mut plan: Vec<(u32, bool)> = Vec::new();
            for &b in &set {
                let load = invoke_pred[b as usize] || preds[b as usize].is_empty() || preds[b as usize].iter().any(|p| !inset(*p));
                // without the region: one load per block that uses it (windows join a block's uses)
                let before = if used.contains_key(&b) { 1.0 } else { 0.0 };
                gain += wts[b as usize] * (before - if load { 1.0 } else { 0.0 });
                plan.push((b, load));
            }
            if gain > 0.0 && best.as_ref().map_or(true, |x| gain > x.0) {
                best = Some((gain, c, plan));
            }
        }
        let Some((gain, c, plan)) = best else { continue };
        if trace_on(f) {
            let bs: Vec<String> = plan.iter().map(|(b, l)| format!("b{}{}", b, if *l { "L" } else { "" })).collect();
            eprintln!("ra-region {} v{} r{} gain {:.0}: {}", f.name, v, c, gain, bs.join(" "));
        }
        let o = &mut occ[c as usize];
        if o.is_empty() {
            o.resize(pos_words, 0);
        }
        for (b, load) in plan {
            let ps = &pieces.iter().find(|x| x.0 == b).unwrap().1;
            for &(a, z) in ps.iter() {
                bits_fill(o, a, z);
            }
            let a = block_start[b as usize] + 1;
            let z = ps.last().unwrap().1;
            if load {
                loading.push((v, a, z, c));
            } else {
                chained.push((v, a, z, c));
            }
            in_region.insert((v, b));
        }
    }
}

/// Baseline-tier allocation (cold code, compiled once before it is known to matter; hot code
/// tiers up): one interval per vreg -- the hull of its live positions, no range pieces -- placed
/// by a linear scan in start order; when no register is free, the interval that ends last spills.
/// Positions follow the reverse post order, so without a loop every execution passes them in
/// ascending order and a value lives between its first and its last reference (no liveness sets);
/// with a loop, block liveness extends the hulls. No copy affinities (a copy's result takes its
/// source's register when the source ends there), coalescing, spill weights or windows. Saves
/// around calls follow the hulls: a crossing value whose saves would cost more than its slot spills.
fn allocate_baseline(options: &relax_session::Options, f: &Func, cfg: &RegConfig) -> Alloc {
    let nv = f.vregs.len();
    let nb = f.blocks.len();
    // depth-first from the entry: post order, and whether an edge closes a loop (its target is
    // still on the search stack)
    let mut state = vec![0u8; nb];
    let mut post: Vec<u32> = Vec::with_capacity(nb);
    let mut cyclic = false;
    let mut stack: Vec<(u32, u32)> = vec![(0, 0)];
    state[0] = 1;
    while let Some(top) = stack.last_mut() {
        let (b, k) = *top;
        match f.blocks[b as usize].term.succs().nth(k as usize) {
            Some(t) => {
                top.1 += 1;
                match state[t as usize] {
                    0 => {
                        state[t as usize] = 1;
                        stack.push((t, 0));
                    }
                    1 => cyclic = true,
                    _ => {}
                }
            }
            None => {
                state[b as usize] = 2;
                post.push(b);
                stack.pop();
            }
        }
    }
    let reach: Vec<bool> = state.iter().map(|s| *s != 0).collect();
    // positions in reverse post order: every path runs through ascending positions (the backend
    // reads positions only as keys of its own blocks, in any block order)
    let mut block_start = vec![0u32; nb];
    let mut block_end = vec![0u32; nb];
    let mut pos = 0u32;
    for b in post.iter().rev().copied().chain((0..nb as u32).filter(|b| !reach[*b as usize])) {
        block_start[b as usize] = pos;
        pos += 2 * (f.blocks[b as usize].insts.len() as u32 + 1);
        block_end[b as usize] = pos;
        pos += 2;
    }
    #[inline]
    fn touch(start: &mut [u32], end: &mut [u32], v: u32, p: u32) {
        let i = v as usize;
        if p < start[i] {
            start[i] = p;
        }
        if p > end[i] {
            end[i] = p;
        }
    }
    let mut start = vec![u32::MAX; nv];
    let mut end = vec![0u32; nv];
    let mut use_count = vec![0u32; nv];
    let mut hint = vec![u32::MAX; nv];
    // definitions per vreg (uses are added below)
    let mut refs = vec![0u32; nv];
    let mut call_pos: Vec<u32> = Vec::new();
    // (call position, a result of the call), ascending
    let mut call_res: Vec<(u32, u32)> = Vec::new();
    // per physical register (ints 0..32, floats 32..64): positions of instructions that clobber it
    let mut clob: Vec<Vec<u32>> = (0..64).map(|_| Vec::new()).collect();
    for p in &f.params {
        touch(&mut start, &mut end, p.0, 0);
    }
    // ascending positions: call_pos, call_res and clob come out sorted
    for &bi in post.iter().rev() {
        let bi = bi as usize;
        let b = &f.blocks[bi];
        for (ii, i) in b.insts.iter().enumerate() {
            let p = block_start[bi] + 2 * ii as u32 + 1;
            for_uses(i, |u| {
                use_count[u.0 as usize] += 1;
                touch(&mut start, &mut end, u.0, p);
            });
            for_defs(i, |d| {
                refs[d.0 as usize] += 1;
                touch(&mut start, &mut end, d.0, p + 1)
            });
            // copy hint: the result takes its source's register when the source ends here
            let src = match i {
                Inst::Mov(d, a) | Inst::Conv(_, d, a) | Inst::FUnary(_, _, d, a) | Inst::INeg(_, d, a) | Inst::INot(_, d, a) | Inst::IBit(_, d, a) | Inst::IBinI(_, _, d, a, _) | Inst::IBin(_, _, d, a, _) | Inst::FBin(_, _, d, a, _) => Some((*d, *a)),
                _ => None,
            };
            if let Some((d, a)) = src {
                if d != a && hint[d.0 as usize] == u32::MAX {
                    hint[d.0 as usize] = a.0;
                }
            }
            // a call that never returns (panic path: last inst before Unreachable) clobbers
            // nothing anyone reads afterwards: it does not count as a call position
            if matches!(i, Inst::Call(..)) && !(ii + 1 == b.insts.len() && matches!(b.term, Term::Unreachable)) {
                call_pos.push(p);
                for_defs(i, |d| call_res.push((p, d.0)));
            }
            if let Some(cl) = cfg.clobbers {
                let mut m = cl(options, i);
                while m != 0 {
                    let r = m.trailing_zeros() as usize;
                    m &= m - 1;
                    clob[r].push(p);
                }
            }
        }
        let p = block_end[bi] - 1;
        match &b.term {
            Term::Branch(c, _, _) | Term::Switch(c, _) => {
                use_count[c.0 as usize] += 1;
                touch(&mut start, &mut end, c.0, p);
            }
            Term::Ret(vs) => {
                for v in vs {
                    use_count[v.0 as usize] += 1;
                    touch(&mut start, &mut end, v.0, p);
                }
            }
            _ => {}
        }
    }
    // a loop: values live around it are live where they are not referenced
    if cyclic {
        let tab = OpTab::new(f, &reach);
        let (live_in, live_out) = live_sets(f, &reach, &tab, true);
        for &bi in &post {
            let bi = bi as usize;
            live_in.each(bi, |v| touch(&mut start, &mut end, v, block_start[bi]));
            live_out.each(bi, |v| touch(&mut start, &mut end, v, block_end[bi]));
        }
    }
    let (mut remat, base_only) = remat_values(options, f, &reach, cfg);
    // base + k: only inside the base's hull (it holds its one value there)
    for v in 0..nv {
        if let Remat::Offset(b, _) = remat[v] {
            let b = b.0 as usize;
            if start[v] != u32::MAX && !(start[b] <= start[v] && end[v] <= end[b]) {
                remat[v] = Remat::No;
            }
        }
    }
    let mut order: Vec<u64> = (0..nv).filter(|&v| start[v] != u32::MAX && !base_only[v]).map(|v| (start[v] as u64) << 32 | v as u64).collect();
    order.sort_unstable();
    // a call strictly inside (s, e)
    let crosses = |s: u32, e: u32| -> bool {
        let k = call_pos.partition_point(|&p| p <= s);
        k < call_pos.len() && call_pos[k] < e
    };
    // an instruction clobbering register r runs over positions p..=p+1
    let clobbered = |r: usize, s: u32, e: u32| -> bool {
        let l = &clob[r];
        if l.is_empty() {
            return false;
        }
        let k = l.partition_point(|&c| c + 1 < s);
        k < l.len() && l[k] <= e
    };
    // per physical register: the vreg holding it, and up to which position
    let mut owner = [u32::MAX; 64];
    let mut busy = [0u32; 64];
    let mut loc = vec![Loc::None; nv];
    // spilled: Stack(0) until slots are packed below
    let spill = |v: usize, remat: &[Remat]| if remat[v].is_some() { Loc::None } else { Loc::Stack(0) };
    let mut used_callee_int: Vec<u8> = Vec::new();
    let mut used_callee_flt: Vec<u8> = Vec::new();
    let no_callee: Vec<u8> = Vec::new();
    let mut cands: Vec<u8> = Vec::with_capacity(32);
    for &k in &order {
        let v = (k & 0xffff_ffff) as usize;
        let (s, e) = (start[v], end[v]);
        let is_int = f.vregs[v] == Cls::I;
        let crossing = crosses(s, e);
        let (caller, callee) = if is_int { (&cfg.int_caller, &cfg.int_callee) } else { (&cfg.flt_caller, &cfg.flt_callee) };
        // a vector crossing a call goes to a caller-saved register (saved/restored around the call
        // in 16 bytes): arm64's callee-saved d8-d15 keep only the low 64 bits
        let callee = if crossing && f.vregs[v].is_vec() { &no_callee } else { callee };
        let base = if is_int { 0 } else { 32 };
        cands.clear();
        if crossing {
            cands.extend_from_slice(callee);
            cands.extend_from_slice(caller);
        } else {
            cands.extend_from_slice(caller);
            cands.extend_from_slice(callee);
        }
        // a value in a caller-saved register is stored and reloaded around each call it crosses:
        // where that costs more than its slot does (a load per use, a store per def), it spills
        let ncalls = if crossing { call_pos.partition_point(|&p| p < e) - call_pos.partition_point(|&p| p <= s) } else { 0 };
        let caller_ok = 2 * ncalls as u32 <= refs[v] + use_count[v];
        let usable = |c: u8| (caller_ok || callee.contains(&c)) && !clobbered(base + c as usize, s, e);
        let free = |c: u8| {
            let r = base + c as usize;
            (owner[r] == u32::MAX || busy[r] < s) && usable(c)
        };
        let hinted = match hint[v] {
            h if h == u32::MAX || f.vregs[h as usize] != f.vregs[v] => None,
            h => match loc[h as usize] {
                Loc::Reg(c) | Loc::Flt(c) if cands.contains(&c) && (!crossing || callee.is_empty() || callee.contains(&c)) && free(c) => Some(c),
                _ => None,
            },
        };
        let mut chosen = hinted.or_else(|| cands.iter().copied().find(|&c| free(c)));
        if chosen.is_none() && !remat[v].is_some() {
            // the interval ending last gives up its register (spilled everywhere)
            let mut best: Option<(u8, u32)> = None;
            for &c in &cands {
                let r = base + c as usize;
                if busy[r] > e && best.map_or(true, |b| busy[r] > b.1) && usable(c) {
                    best = Some((c, busy[r]));
                }
            }
            if let Some((c, _)) = best {
                let w = owner[base + c as usize] as usize;
                loc[w] = spill(w, &remat);
                chosen = Some(c);
            }
        }
        loc[v] = match chosen {
            Some(c) => {
                owner[base + c as usize] = v as u32;
                busy[base + c as usize] = e;
                if callee.contains(&c) {
                    let used = if is_int { &mut used_callee_int } else { &mut used_callee_flt };
                    if !used.contains(&c) {
                        used.push(c);
                    }
                }
                if is_int {
                    Loc::Reg(c)
                } else {
                    Loc::Flt(c)
                }
            }
            None => spill(v, &remat),
        };
    }
    // spill slots shared by values whose intervals never meet (start order: a slot is free once
    // its last value ended)
    let mut spill_bytes = 0u32;
    if order.iter().any(|&k| matches!(loc[(k & 0xffff_ffff) as usize], Loc::Stack(_))) {
        let share = slot_share_on(options);
        // (offset, 16-byte slot, end of its last value)
        let mut slots: Vec<(i32, u32, u32)> = Vec::new();
        for &k in &order {
            let v = (k & 0xffff_ffff) as usize;
            if !matches!(loc[v], Loc::Stack(_)) {
                continue;
            }
            let wide = f.vregs[v].spill_bytes();
            let found = if share { slots.iter_mut().find(|x| x.1 == wide && x.2 < start[v]) } else { None };
            let o = match found {
                Some(x) => {
                    x.2 = end[v];
                    x.0
                }
                None => {
                    spill_bytes = grow_spill(spill_bytes, f.vregs[v]);
                    slots.push((spill_bytes as i32, wide, end[v]));
                    spill_bytes as i32
                }
            };
            loc[v] = Loc::Stack(o);
        }
    }
    // values in caller-saved registers across calls: a save slot each, stored / reloaded around
    // every call inside their interval -- except a call that defines the value (an interval with
    // a hole there, in a loop: the reload would overwrite the call's result)
    let mut saves: HashMap<u32, Vec<(VReg, i32)>> = HashMap::new();
    for v in 0..nv {
        let saved = match loc[v] {
            Loc::Reg(c) => !cfg.int_callee.contains(&c),
            Loc::Flt(c) => f.vregs[v].is_vec() || !cfg.flt_callee.contains(&c),
            _ => false,
        };
        if !saved || !crosses(start[v], end[v]) {
            continue;
        }
        let mut slot = 0;
        let from = call_pos.partition_point(|&p| p <= start[v]);
        for &p in &call_pos[from..] {
            if p >= end[v] {
                break;
            }
            if call_res.binary_search(&(p, v as u32)).is_ok() {
                continue;
            }
            if slot == 0 {
                spill_bytes = grow_spill(spill_bytes, f.vregs[v]);
                slot = spill_bytes as i32;
            }
            saves.entry(p).or_default().push((VReg(v as u32), slot));
        }
    }
    for v in 0..nv {
        if !matches!(loc[v], Loc::None) {
            remat[v] = Remat::No;
        }
    }
    asm_callee_writes(f, cfg, &mut used_callee_int, &mut used_callee_flt);
    Alloc { loc, used_callee_int, used_callee_flt, spill_bytes, block_start, block_end, saves, start, end, uses: use_count, remat, windows: Vec::new(), chained: Vec::new(), save_skip: Default::default(), restore_skip: Default::default() }
}

/// Rematerialisable values: one definition by a constant-like instruction, not parameters.
/// Call arguments and returned values among them are recomputed straight into their argument /
/// result register (MLoc::K). Returns (per vreg its recomputation, whether it is a stack-slot
/// address used only as a memory base: such a value never takes a register).
fn remat_values(options: &relax_session::Options, f: &Func, reach: &[bool], cfg: &RegConfig) -> (Vec<Remat>, Vec<bool>) {
    let nv = f.vregs.len();
    let mut remat: Vec<Remat> = vec![Remat::No; nv];
    let mut base_only = vec![false; nv];
    if remat_on(options) {
        let mut ndef = vec![0u32; nv];
        let mut bad = vec![false; nv];
        let mut nonbase = vec![false; nv];
        for p in &f.params {
            bad[p.0 as usize] = true;
        }
        let mut cand: Vec<(u32, Remat)> = Vec::new();
        // per vreg: (constant, number of Iconst definitions, all the same constant)
        let mut kdefs: Vec<(i64, u32, bool)> = vec![(0, 0, true); nv];
        let mut movs: Vec<(u32, u32)> = Vec::new();
        // call arguments and returned values (no Remat::Offset for them)
        let mut argret = vec![false; nv];
        for (bi, b) in f.blocks.iter().enumerate() {
            if !reach[bi] {
                continue;
            }
            for i in &b.insts {
                for_defs(i, |d| ndef[d.0 as usize] += 1);
                match i {
                    Inst::Iconst(d, k) => {
                        cand.push((d.0, Remat::Const(*k)));
                        let e = &mut kdefs[d.0 as usize];
                        if e.1 == 0 {
                            e.0 = *k;
                        } else if e.0 != *k {
                            e.2 = false;
                        }
                        e.1 += 1;
                    }
                    Inst::SlotAddr(d, n) => cand.push((d.0, Remat::Slot(*n))),
                    Inst::Addr(d, a) if crate::rir::SymClass::of(cfg.sym_class, a.id) != crate::rir::SymClass::Domain => cand.push((d.0, Remat::Addr(*a))),
                    Inst::FnAddr(d, n) => cand.push((d.0, Remat::FnAddr(*n))),
                    Inst::VConst(d, bits) => cand.push((d.0, Remat::VConst(*bits))),
                    Inst::Fconst(d, bits, w) if cfg.fconst_remat => cand.push((d.0, Remat::FConst(*bits, *w))),
                    Inst::IBinI(IOp::Add, IntTy { bits: 64, .. }, d, b2, k) if *k == *k as i32 as i64 => cand.push((d.0, Remat::Offset(*b2, *k as i32))),
                    Inst::Mov(d, s2) => movs.push((d.0, s2.0)),
                    Inst::Call(c, a, _) => {
                        a.iter().for_each(|v| argret[v.0 as usize] = true);
                        if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                            nonbase[v.0 as usize] = true;
                        }
                    }
                    _ => {}
                }
                // uses other than as a memory base
                match i {
                    Inst::Load(_, _, _, _) => {}
                    Inst::Call(_, a, _) => a.iter().for_each(|v| nonbase[v.0 as usize] = true),
                    Inst::Store(_, _, _, s2) => nonbase[s2.0 as usize] = true,
                    Inst::LoadX(_, _, b2, x, _, _) => {
                        nonbase[x.0 as usize] = true;
                        if !cfg.slot_index_free {
                            nonbase[b2.0 as usize] = true;
                        }
                    }
                    Inst::StoreX(_, b2, x, _, _, s2) => {
                        nonbase[x.0 as usize] = true;
                        nonbase[s2.0 as usize] = true;
                        if !cfg.slot_index_free {
                            nonbase[b2.0 as usize] = true;
                        }
                    }
                    _ => for_uses(i, |u| nonbase[u.0 as usize] = true),
                }
            }
            match &b.term {
                Term::Ret(vs) => vs.iter().for_each(|v| {
                    nonbase[v.0 as usize] = true;
                    argret[v.0 as usize] = true;
                }),
                Term::Branch(c, _, _) | Term::Switch(c, _) => nonbase[c.0 as usize] = true,
                _ => {}
            }
        }
        let is_param = |v: VReg| f.params.contains(&v);
        // round trips `t = v .. v = t` (t defined once, by that copy: the cold-call and loop
        // splits' save / restore pairs) give v back the value it had: per v, how many such defs
        let mut copy_of = vec![u32::MAX; nv];
        for &(d, s2) in &movs {
            if ndef[d as usize] == 1 {
                copy_of[d as usize] = s2;
            }
        }
        let mut round = vec![0u32; nv];
        for &(d, s2) in &movs {
            if copy_of[s2 as usize] == d {
                round[d as usize] += 1;
            }
        }
        // an Offset base holds one value for the whole function: a parameter no instruction
        // redefines, or a value with one definition (round trips aside)
        let fixed = |v: VReg| {
            let n = ndef[v.0 as usize] - round[v.0 as usize];
            f.vregs[v.0 as usize] == Cls::I && if is_param(v) { n == 0 } else { n == 1 }
        };
        for (d, i) in cand {
            let d = d as usize;
            if let Remat::Offset(b2, _) = i {
                if argret[d] || !fixed(b2) || b2.0 as usize == d {
                    continue;
                }
            }
            // one definition, or every definition the same constant (built on several paths)
            let same_const = matches!(i, Remat::Const(_)) && kdefs[d].2 && kdefs[d].1 == ndef[d];
            let cls_ok = match i {
                Remat::VConst(_) => f.vregs[d] == Cls::V128,
                Remat::FConst(_, w) => f.vregs[d] == if w { Cls::F64 } else { Cls::F32 },
                _ => f.vregs[d] == Cls::I,
            };
            if (ndef[d] == 1 || same_const) && !bad[d] && cls_ok {
                base_only[d] = !nonbase[d] && matches!(i, Remat::Slot(..));
                remat[d] = i;
            }
        }
        // a single-def copy of such a value is the same value (chains: a few rounds)
        for _ in 0..3 {
            let mut more = false;
            for &(d, s2) in &movs {
                let (d, s2) = (d as usize, s2 as usize);
                if !remat[d].is_some() && remat[s2].is_some() && ndef[d] == 1 && !bad[d] && f.vregs[d] == Cls::I && !(argret[d] && matches!(remat[s2], Remat::Offset(..))) {
                    remat[d] = remat[s2];
                    base_only[d] = !nonbase[d] && matches!(remat[d], Remat::Slot(..));
                    more = true;
                }
            }
            if !more {
                break;
            }
        }
    }
    (remat, base_only)
}

/// How often each block runs, relative: with measured frequencies (O3: samples of the code
/// the block's instructions came from, 0..=1 of the hottest block) mostly those, a sliver of loop
/// depth for blocks no sample hit; without, 8^loop depth (capped).
fn block_weights(options: &relax_session::Options, f: &Func, reach: &[bool], freq: Option<&[f32]>) -> Vec<f32> {
    let depth = loop_depths(f, reach);
    match (freq, freq_mode(options)) {
        (Some(fr), 0) if fr.len() == f.blocks.len() => depth.iter().zip(fr).map(|(d, x)| 0.02 * 8f32.powi((*d).min(2) as i32) + 64.0 * x).collect(),
        (Some(fr), 2) if fr.len() == f.blocks.len() => {
            // static estimate as the base, samples raise a block up to 8x: tier-up happens after a
            // handful of samples (glyph_cache: 9 of 176 blocks sampled, values 0.5 / 1.0), too
            // few to rank the blocks of a hot loop on their own
            let st = static_weights(options, f, reach, &depth);
            st.iter().zip(fr).map(|(s, x)| s * (1.0 + 7.0 * x)).collect()
        }
        (_, 0) => depth.iter().map(|d| 8f32.powi((*d).min(depth_cap(options)) as i32)).collect(),
        _ => static_weights(options, f, reach, &depth),
    }
}

/// MAKEPAD_RUN_FREQ_MODE: how block weights use sampled frequencies -- sampled (0: samples dominate),
/// static (1: loop depth + cold paths, samples ignored), blend (2, default: static x up to 8 by samples)
fn freq_mode(options: &relax_session::Options) -> u8 {
    options.freq_mode as u8
}

/// Static block weights: 8^loop depth (capped), panic / unwind paths (blocks that can only end in
/// Unreachable or Resume, and landing pads) ~0.
fn static_weights(options: &relax_session::Options, f: &Func, reach: &[bool], depth: &[u32]) -> Vec<f32> {
    let nb = f.blocks.len();
    let mut cold = vec![false; nb];
    for b in &f.blocks {
        if let Term::Invoke(_, u) = b.term {
            cold[u as usize] = true;
        }
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        if matches!(b.term, Term::Unreachable | Term::Resume) {
            cold[bi] = true;
        }
    }
    // a block whose every successor is cold is cold (back to front until nothing changes)
    loop {
        let mut changed = false;
        for bi in (0..nb).rev() {
            if cold[bi] || !reach[bi] {
                continue;
            }
            let mut n = 0;
            let mut all = true;
            for s in f.blocks[bi].term.succs() {
                n += 1;
                if !cold[s as usize] {
                    all = false;
                }
            }
            if n > 0 && all {
                cold[bi] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    (0..nb).map(|b| if cold[b] { 0.001 } else { 8f32.powi(depth[b].min(depth_cap(options)) as i32) }).collect()
}

/// Loop depth per block: the number of loops holding it. Loops from a depth-first search: an
/// edge to a block still on the search stack closes a loop over the blocks that reach its source
/// without passing the header (the natural loop, for reducible flow). Block order is no guide:
/// rotated loops keep hot blocks after their latch. Linear in the blocks plus the loop bodies.
#[inline(never)]
pub fn loop_depths(f: &Func, reach: &[bool]) -> Vec<u32> {
    let nb = f.blocks.len();
    let mut depth = vec![0u32; nb];
    // no edge to an earlier or the same block: no loop (every loop has one)
    if f.blocks.iter().enumerate().all(|(bi, b)| !reach[bi] || b.term.succs().all(|t| t as usize > bi)) {
        return depth;
    }
    // iterative DFS: state 0 new, 1 on stack, 2 done; latches per header
    let mut state = vec![0u8; nb];
    let mut latches: Vec<(u32, u32)> = Vec::new();
    let mut stack: Vec<(u32, u32)> = vec![(0, 0)];
    while let Some(&mut (b, ref mut k)) = stack.last_mut() {
        if *k == 0 {
            state[b as usize] = 1;
        }
        let mut it = f.blocks[b as usize].term.succs();
        let mut next = None;
        let mut idx = 0u32;
        while let Some(t) = it.next() {
            if idx == *k {
                next = Some(t);
                break;
            }
            idx += 1;
        }
        match next {
            Some(t) => {
                *k += 1;
                match state[t as usize] {
                    0 => stack.push((t, 0)),
                    1 => latches.push((t, b)),
                    _ => {}
                }
            }
            None => {
                state[b as usize] = 2;
                stack.pop();
            }
        }
    }
    if latches.is_empty() {
        return depth;
    }
    // predecessors, flat (off[b]..off[b + 1])
    let mut off = vec![0u32; nb + 1];
    for (bi, b) in f.blocks.iter().enumerate() {
        if reach[bi] {
            for t in b.term.succs() {
                off[t as usize + 1] += 1;
            }
        }
    }
    for i in 0..nb {
        off[i + 1] += off[i];
    }
    let mut fill = off.clone();
    let mut pr = vec![0u32; off[nb] as usize];
    for (bi, b) in f.blocks.iter().enumerate() {
        if reach[bi] {
            for t in b.term.succs() {
                pr[fill[t as usize] as usize] = bi as u32;
                fill[t as usize] += 1;
            }
        }
    }
    latches.sort_unstable();
    let mut stamp = vec![u32::MAX; nb];
    let mut work: Vec<u32> = Vec::new();
    let mut k = 0;
    while k < latches.len() {
        let h = latches[k].0;
        let mark = k as u32;
        stamp[h as usize] = mark;
        depth[h as usize] += 1;
        while k < latches.len() && latches[k].0 == h {
            work.push(latches[k].1);
            k += 1;
        }
        while let Some(x) = work.pop() {
            if stamp[x as usize] == mark {
                continue;
            }
            stamp[x as usize] = mark;
            depth[x as usize] += 1;
            for &p in &pr[off[x as usize] as usize..off[x as usize + 1] as usize] {
                if stamp[p as usize] != mark {
                    work.push(p);
                }
            }
        }
    }
    depth
}

/// Copy affinities: for every `Mov(d, s)` of one class, d and s prefer one register (weighted by
/// 8^loop depth); an operation's result prefers an operand that dies there (two-address machines:
/// the first operand, or either operand of a commutative op). Per vreg, strongest first.
struct Aff {
    e: Vec<(u32, f32)>,
    off: Vec<u32>,
}

impl Aff {
    #[inline]
    fn of(&self, v: u32) -> &[(u32, f32)] {
        &self.e[self.off[v as usize] as usize..self.off[v as usize + 1] as usize]
    }
}

/// Does a range of `v` end exactly at position p (v dies at the instruction using it at p)?
#[inline(never)]
fn dies_at(ranges: &Ranges, v: u32, p: u32) -> bool {
    let r = ranges.of(v);
    let k = r.partition_point(|x| x.1 < p);
    k < r.len() && r[k].1 == p && r[k].0 <= p
}

#[inline(never)]
fn affinities(f: &Func, reach: &[bool], block_start: &[u32], ranges: &Ranges, wts: &[f32]) -> Aff {
    let nv = f.vregs.len();
    let mut edges: Vec<(u32, u32, f32)> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        let w = wts[bi];
        for (ii, i) in b.insts.iter().enumerate() {
            let p = block_start[bi] + 2 * ii as u32 + 1;
            match i {
                Inst::Mov(d, s2) => {
                    if d != s2 && f.vregs[d.0 as usize] == f.vregs[s2.0 as usize] {
                        edges.push((d.0, s2.0, w));
                        edges.push((s2.0, d.0, w));
                    }
                    continue;
                }
                // fused multiply-add accumulates in place on both targets (arm64 fmla/fmls
                // vd += vn*vm, x64 vfmadd231 d += a*b): the result prefers the addend's register
                Inst::VFma(_, _, d, _, _, c) => {
                    if d != c && dies_at(ranges, c.0, p) {
                        edges.push((d.0, c.0, w));
                        edges.push((c.0, d.0, w));
                    }
                    continue;
                }
                _ => {}
            }
            let (d, a, b2, comm) = match i {
                Inst::Conv(_, d, s2) | Inst::FUnary(_, _, d, s2) | Inst::INeg(_, d, s2) | Inst::INot(_, d, s2) | Inst::IBit(_, d, s2) | Inst::IBinI(_, _, d, s2, _) => (*d, *s2, None, false),
                Inst::IBin(op, _, d, x, y) => (*d, *x, Some(*y), matches!(op, IOp::Add | IOp::Mul | IOp::And | IOp::Or | IOp::Xor)),
                Inst::FBin(op, _, d, x, y) => (*d, *x, Some(*y), matches!(op, FOp::Add | FOp::Mul | FOp::Min | FOp::Max)),
                _ => continue,
            };
            if f.vregs[d.0 as usize] != f.vregs[a.0 as usize] {
                continue;
            }
            if d != a && dies_at(ranges, a.0, p) {
                edges.push((d.0, a.0, w));
                edges.push((a.0, d.0, w));
            } else if let Some(b2) = b2 {
                if comm && d != b2 && dies_at(ranges, b2.0, p) {
                    edges.push((d.0, b2.0, w));
                    edges.push((b2.0, d.0, w));
                }
            }
        }
    }
    // merge duplicates, strongest first per vreg
    edges.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut m: Vec<(u32, u32, f32)> = Vec::with_capacity(edges.len());
    for e in edges {
        match m.last_mut() {
            Some(l) if l.0 == e.0 && l.1 == e.1 => l.2 += e.2,
            _ => m.push(e),
        }
    }
    m.sort_by(|a, b| a.0.cmp(&b.0).then(b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal)));
    let mut off = vec![0u32; nv + 1];
    for e in &m {
        off[e.0 as usize + 1] += 1;
    }
    for i in 0..nv {
        off[i + 1] += off[i];
    }
    Aff { e: m.iter().map(|e| (e.1, e.2)).collect(), off }
}

/// Copies whose two values may share a register although both stay live: `d = s` (a `Mov`, or an
/// integer cast the backends emit as a plain move -- upper bits nobody reads, or a source already
/// zero above the target width; the condition both backends' `Conv::IntToInt` use) where d has no
/// other definition and s is not redefined anywhere d is live. Wherever both are live they hold the
/// same bits, so they do not interfere (LLVM's coalescer joins such copies by value number). A cast
/// of a value that stays live (`entry as u8` as a shift count, `entry` read again after) then needs
/// no copy. (weight, lower vreg, higher vreg).
#[allow(clippy::too_many_arguments)]
fn equal_copies(options: &relax_session::Options, f: &Func, reach: &[bool], tab: &OpTab, block_start: &[u32], ranges: &Ranges, wts: &[f32], remat: &[Remat]) -> Vec<(f32, u32, u32)> {
    let nv = f.vregs.len();
    let mut cand: Vec<(u32, u32, f32, u8)> = Vec::new(); // (d, s, weight, cast width or 0 for a Mov)
    let mut any_cast = false;
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        for i in &b.insts {
            let (d, s2, cast) = match *i {
                Inst::Mov(d, s2) => (d, s2, 0),
                // (bits | 0x80 for a signed target)
                Inst::Conv(Conv::IntToInt(it), d, s2) => (d, s2, it.bits | if it.signed { 0x80 } else { 0 }),
                _ => continue,
            };
            if d == s2 || f.vregs[d.0 as usize] != f.vregs[s2.0 as usize] || f.vregs[d.0 as usize].is_vec() || remat[d.0 as usize].is_some() || remat[s2.0 as usize].is_some() {
                continue;
            }
            any_cast |= cast != 0;
            cand.push((d.0, s2.0, wts[bi], cast));
        }
    }
    if cand.is_empty() {
        return Vec::new();
    }
    let (low, zero) = if any_cast {
        let low = crate::passes::low_bits_only(options, f);
        let zero = crate::passes::zero_above(options, f, &low);
        (low, zero)
    } else {
        (Vec::new(), Vec::new())
    };
    // definition positions of every vreg (params at 0)
    let mut ndef = vec![0u32; nv];
    let mut defs_at: Vec<Vec<u32>> = vec![Vec::new(); nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
        defs_at[p.0 as usize].push(0);
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        let k0 = tab.first[bi] as usize;
        for ii in 0..b.insts.len() {
            for &d in tab.defs(k0 + ii) {
                ndef[d as usize] += 1;
                defs_at[d as usize].push(block_start[bi] + 2 * ii as u32 + 2);
            }
        }
    }
    let mut out = Vec::new();
    for (d, s2, w, cast) in cand {
        if ndef[d as usize] != 1 {
            continue;
        }
        // a cast is a plain move: its upper bits nobody reads, or its source zero above fewer bits
        if cast != 0 && !(low[d as usize] || crate::passes::cast_is_move(zero[s2 as usize], IntTy { bits: cast & 0x7f, signed: cast & 0x80 != 0 })) {
            continue;
        }
        let rd = ranges.of(d);
        if defs_at[s2 as usize].iter().any(|&q| rd.iter().any(|&(a, b)| a < q && q <= b)) {
            continue;
        }
        out.push((w, d.min(s2), d.max(s2)));
    }
    out
}

/// Spill weight of every vreg: its uses and defs, each weighted 8^loop depth, divided by the
/// length of its live ranges.
#[inline(never)]
fn spill_weights(f: &Func, reach: &[bool], tab: &OpTab, ranges: &Ranges, wts: &[f32]) -> (Vec<f32>, Vec<u32>) {
    let nv = f.vregs.len();
    let mut refs = vec![0f32; nv];
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        let w = wts[bi];
        let k0 = tab.first[bi] as usize;
        for k in k0..k0 + b.insts.len() {
            for &v in tab.uses(k).iter().chain(tab.defs(k).iter()) {
                refs[v as usize] += w;
            }
        }
        for &v in tab.uses(k0 + b.insts.len()) {
            refs[v as usize] += w;
        }
    }
    let mut lens = vec![0u32; nv];
    for v in 0..nv {
        for (a, b) in ranges.of(v as u32) {
            lens[v] += b.saturating_sub(*a) + 1;
        }
    }
    (refs, lens)
}

/// Allocation priority from weighted references and live-range length (MAKEPAD_RUN_RA_PRIO).
fn prio(options: &relax_session::Options, refs: f32, len: u32) -> f32 {
    match ra_prio(options) {
        1 => refs,
        2 => refs / (len.max(1) as f32).sqrt(),
        _ => refs / len.max(1) as f32,
    }
}

/// A move between locations (registers or spill/stack addresses as backends define them).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MLoc {
    R(u8),
    F(u8),
    /// memory (spill slot / staging); never a conflict source for register moves
    M(i32),
    /// (source only) vreg n recomputed into the destination (Alloc::remat): reads no register,
    /// so it is emitted with the moves from memory, after every register move
    K(u32),
}

/// Orders a set of simultaneous moves `(dst, src)` into a sequence, breaking cycles with
/// the scratch registers. Moves to memory come first (they only read registers),
/// moves from memory last (their destinations must be read first).
pub fn seq_moves(moves: &[(MLoc, MLoc)], int_scratch: u8, flt_scratch: u8) -> Vec<(MLoc, MLoc)> {
    let mut out = Vec::new();
    let mut pending: Vec<(MLoc, MLoc)> = Vec::new();
    let mut from_mem: Vec<(MLoc, MLoc)> = Vec::new();
    for m in moves {
        if m.0 == m.1 {
            continue;
        }
        match (m.0, m.1) {
            (MLoc::M(_), _) => out.push(*m),
            (_, MLoc::M(_) | MLoc::K(_)) => from_mem.push(*m),
            _ => pending.push(*m),
        }
    }
    while !pending.is_empty() {
        let mut done = None;
        for i in 0..pending.len() {
            let d = pending[i].0;
            let mut blocked = false;
            for j in 0..pending.len() {
                if j != i && pending[j].1 == d {
                    blocked = true;
                    break;
                }
            }
            if !blocked {
                done = Some(i);
                break;
            }
        }
        match done {
            Some(i) => {
                out.push(pending[i]);
                pending.remove(i);
            }
            None => {
                // cycle: save the first destination's current value to scratch
                let d = pending[0].0;
                let tmp = match d {
                    MLoc::F(_) => MLoc::F(flt_scratch),
                    _ => MLoc::R(int_scratch),
                };
                out.push((tmp, d));
                for m in pending.iter_mut() {
                    if m.1 == d {
                        m.1 = tmp;
                    }
                }
            }
        }
    }
    out.extend(from_mem);
    out
}

/// Live ranges per vreg as sorted, disjoint [start, end] position pairs.
#[inline(never)]
fn build_ranges(f: &Func, reach: &[bool], tab: &OpTab, block_start: &[u32], block_end: &[u32], live_out: &LiveSets) -> Ranges {
    let nv = f.vregs.len();
    // (vreg, start, end) pieces, sorted and merged per vreg below
    let mut pieces: Vec<(u32, u32, u32)> = Vec::with_capacity(tab.ops.len());
    let mut cur_end = vec![u32::MAX; nv];
    let mut live: Vec<u32> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        live.clear();
        live_out.each(bi, |v| {
            cur_end[v as usize] = block_end[bi];
            live.push(v);
        });
        let k0 = tab.first[bi] as usize;
        for &u in tab.uses(k0 + b.insts.len()) {
            if cur_end[u as usize] == u32::MAX {
                cur_end[u as usize] = block_end[bi] - 1;
                live.push(u);
            }
        }
        let mut ii = b.insts.len();
        while ii > 0 {
            ii -= 1;
            let p = block_start[bi] + 2 * ii as u32 + 1;
            for &d in tab.defs(k0 + ii) {
                let e = cur_end[d as usize];
                if e != u32::MAX {
                    pieces.push((d, p + 1, e));
                    cur_end[d as usize] = u32::MAX;
                } else {
                    pieces.push((d, p + 1, p + 1));
                }
            }
            for &u in tab.uses(k0 + ii) {
                if cur_end[u as usize] == u32::MAX {
                    cur_end[u as usize] = p;
                    live.push(u);
                }
            }
        }
        for v in &live {
            let e = cur_end[*v as usize];
            if e != u32::MAX {
                pieces.push((*v, block_start[bi], e));
                cur_end[*v as usize] = u32::MAX;
            }
        }
    }
    // params are defined at position 0 (entry block start): already covered by live-in
    // bucket the pieces by vreg (counting sort), then order each vreg's few pieces by start:
    // a global sort of all pieces was a top O3 cost (most of it on Windows)
    let mut cnt = vec![0u32; nv + 1];
    for p in &pieces {
        cnt[p.0 as usize + 1] += 1;
    }
    for v in 0..nv {
        cnt[v + 1] += cnt[v];
    }
    let mut fill = cnt.clone();
    let mut by: Vec<(u32, u32)> = vec![(0, 0); pieces.len()];
    for p in &pieces {
        let at = &mut fill[p.0 as usize];
        by[*at as usize] = (p.1, p.2);
        *at += 1;
    }
    for v in 0..nv {
        let seg = &mut by[cnt[v] as usize..cnt[v + 1] as usize];
        if seg.len() > 1 {
            if seg.len() <= 16 {
                // insertion sort: pieces arrive nearly ordered (blocks ascending)
                for i in 1..seg.len() {
                    let mut j = i;
                    while j > 0 && seg[j - 1] > seg[j] {
                        seg.swap(j - 1, j);
                        j -= 1;
                    }
                }
            } else {
                seg.sort_unstable();
            }
        }
    }
    let mut r: Vec<(u32, u32)> = Vec::with_capacity(pieces.len());
    let mut off = vec![0u32; nv + 1];
    let mut k = 0;
    for v in 0..nv {
        off[v] = r.len() as u32;
        let first = r.len();
        while k < cnt[v + 1] as usize {
            let x = by[k];
            k += 1;
            // merge touching ranges
            if r.len() > first {
                let last = r.last_mut().unwrap();
                if x.0 <= last.1 + 1 {
                    if x.1 > last.1 {
                        last.1 = x.1;
                    }
                    continue;
                }
            }
            r.push(x);
        }
    }
    off[nv] = r.len() as u32;
    Ranges { r, off }
}

// ------------------------------------------------------------ live-range splitting at loops

/// MAKEPAD_RUN_NO_THROUGH=1: no passed-through loop pieces (split_loops)
fn through_on(options: &relax_session::Options) -> bool {
    !options.no_through
}

/// MAKEPAD_RUN_NO_WINDOWS=1: no spill windows (Alloc::windows)
fn windows_on(options: &relax_session::Options) -> bool {
    !options.no_windows
}

fn split_on(options: &relax_session::Options) -> bool {
    !options.no_split
}

thread_local! {
    /// the last allocate_split on this thread, when pass timing is on: ns of [first allocation,
    /// cold-call split + its allocation, loop split + its allocations], then vregs / blocks in,
    /// vregs / blocks of the split copy, values spilled (a slow O3 build's split)
    static RA_STATS: std::cell::Cell<[u64; 8]> = const { std::cell::Cell::new([0; 8]) };
}

pub fn ra_stats() -> [u64; 8] {
    RA_STATS.with(|c| c.get())
}

fn ra_stat(i: usize, v: u64) {
    RA_STATS.with(|c| {
        let mut a = c.get();
        a[i] = v;
        c.set(a);
    });
}

/// `allocate`, then live-range splitting where it spilled: a value the first allocation put in a
/// stack slot that is used or defined inside a loop gets a loop-local copy (a fresh vreg standing
/// for it inside the loop: copied in on entry, copied back on the exits where the original is
/// live), so the allocator can give the loop piece a register while the rest stays in memory
/// (LLVM's region splitting around loops, done as renaming before a second allocation). Loops are
/// split innermost first; pieces that still spill share the original's slot (the copies between
/// them are then no-ops). Returns the function the allocation is for when it differs from `f`.
/// `freq`: measured block frequencies of `f` (see block_weights), if any.
pub fn allocate_split(options: &relax_session::Options, f: &Func, cfg: &RegConfig, thorough: bool, freq: Option<&[f32]>) -> (Option<Func>, Alloc) {
    let t0 = crate::stats::pass_clock();
    let (g, al) = allocate_split_inner(options, f, cfg, thorough, freq, t0);
    if t0 != 0 {
        ra_stat(3, f.vregs.len() as u64);
        ra_stat(4, f.blocks.len() as u64);
        let h = g.as_ref().unwrap_or(f);
        ra_stat(5, h.vregs.len() as u64);
        ra_stat(6, h.blocks.len() as u64);
        ra_stat(7, al.loc.iter().filter(|l| matches!(l, Loc::Stack(_))).count() as u64);
    }
    (g, al)
}

fn allocate_split_inner(options: &relax_session::Options, f: &Func, cfg: &RegConfig, thorough: bool, freq: Option<&[f32]>, t0: u64) -> (Option<Func>, Alloc) {
    let al = if !thorough && f.baseline && !options.no_quick_ra { allocate_baseline(options, f, cfg) } else { allocate_with(options, f, cfg, thorough, freq) };
    let t1 = crate::stats::pass_clock();
    if t0 != 0 {
        ra_stat(0, t1 - t0);
        ra_stat(1, 0);
        ra_stat(2, 0);
    }
    // Quick allocation is complete. Avoid the spill vector and loop-splitting
    // setup (including its copied frequencies) for every O1 function.
    if !thorough {
        return (None, al);
    }
    if let (true, Some(fr)) = (thorough && split_on(options), freq) {
        // spilled values that are not recomputed instead
        let cand: Vec<bool> = al.loc.iter().zip(&al.remat).map(|(l, r)| matches!(l, Loc::Stack(_)) && !r.is_some()).collect();
        if cand.iter().any(|x| *x) {
            if let Some(g) = split_cold_calls(f, fr, &cand) {
                let al2 = allocate_with(options, &g, cfg, thorough, freq);
                let t2 = crate::stats::pass_clock();
                if t0 != 0 {
                    ra_stat(1, t2 - t1);
                }
                if trace_on(f) {
                    let sp: Vec<String> = (0..f.vregs.len()).filter(|v| cand[*v]).map(|v| format!("v{}{}", v, match al2.loc[v] { Loc::Reg(r) => format!("=r{}", r), Loc::Stack(_) => "=S".to_string(), _ => "=-".to_string() })).collect();
                    eprintln!("cold-split {}: first-pass spills now: {}", f.name, sp.join(" "));
                }
                let (h, al3) = allocate_split_loops(options, &g, al2, cfg, thorough, freq);
                if t0 != 0 {
                    ra_stat(2, crate::stats::pass_clock() - t2);
                }
                return (Some(h.unwrap_or(g)), al3);
            }
        }
    }
    let r = allocate_split_loops(options, f, al, cfg, thorough, freq);
    if t0 != 0 {
        ra_stat(2, crate::stats::pass_clock() - t1);
    }
    r
}

/// Values live across a call in a cold block of a loop (one the sampler never hit while it hit
/// the loop): `t = v` before the call (the call reads t),
/// `v = t` after it. v then has a hole at the call and can stay in a caller-saved register through
/// the hot part of the loop; t crosses the call (a spill slot or a callee-saved register): the
/// save / restore happens on the cold path only. (Without this, every value live across such a
/// call -- an Rc drop_slow, a Vec grow inlined into the loop -- needs a callee-saved register for
/// the whole loop or spills: x64 has five.) Returns the rewritten copy, or None.
fn trace_on(f: &Func) -> bool {
    match std::env::var("MAKEPAD_RUN_RA_TRACE") {
        Ok(w) => f.name.ends_with(w.as_str()),
        Err(_) => false,
    }
}

/// The most values the live-range splits (cold calls, loops) may add to a function of `nv`.
#[allow(non_snake_case)]
fn SPLIT_GROWTH_MAX(nv: usize) -> usize {
    nv + 1024
}

fn split_cold_calls(f: &Func, freq: &[f32], cand: &[bool]) -> Option<Func> {
    let reach = reachable(f);
    let preds = preds_of(f, &reach);
    let loops = natural_loops(f, &reach, &preds, true);
    if loops.is_empty() {
        return None;
    }
    let fr = |b: usize| freq.get(b).copied().unwrap_or(0.0);
    let mut cold = vec![false; f.blocks.len()];
    // per cold block: the values its loop reads in hot blocks (worth a register there)
    let mut hot_use: Vec<Option<Vec<u64>>> = vec![None; f.blocks.len()];
    let words0 = (f.vregs.len() + 63) / 64;
    let mut any = false;
    for bi in 0..f.blocks.len() {
        if !reach[bi] || !f.blocks[bi].insts.iter().any(|i| matches!(i, Inst::Call(..))) {
            continue;
        }
        // the innermost loop holding it (loops are sorted smallest first)
        if let Some((_, m)) = loops.iter().find(|(_, m)| m[bi]) {
            let hot = m.iter().enumerate().filter(|(_, x)| **x).map(|(b, _)| fr(b)).fold(0.0f32, f32::max);
            if trace_on(f) {
                eprintln!("cold-split {}: call block b{} freq {} loop hot {} term {:?}", f.name, bi, fr(bi), hot, f.blocks[bi].term);
            }
            // never sampled: a call block's own samples are few even when its callee is hot (the
            // sampler counts the callee), so "rarely sampled" is not "cold" (the Splash VM's
            // run_core -> opcode() call saved 7 values per op when this was < 1/16 of the hottest)
            if hot > 0.0 && fr(bi) == 0.0 {
                let mut hu = vec![0u64; words0];
                for (b, x) in m.iter().enumerate() {
                    // sampled at all (samples are sparse: a block of the hot path can hold few)
                    if *x && fr(b) > 0.0 {
                        for i in &f.blocks[b].insts {
                            for_uses(i, |u| hu[u.0 as usize >> 6] |= 1u64 << (u.0 & 63));
                        }
                        let mut tb = f.blocks[b].term.clone();
                        term_vregs_mut(&mut tb, |u| hu[u.0 as usize >> 6] |= 1u64 << (u.0 & 63));
                    }
                }
                hot_use[bi] = Some(hu);
                cold[bi] = true;
                any = true;
            }
        }
    }
    if !any {
        return None;
    }
    let live_in = live_in_sets(f, &reach);
    let words = words0;
    let nv0 = f.vregs.len();
    let mut g = f.clone();
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        if !cold[bi] {
            continue;
        }
        // live after each instruction, walking back from the block's end
        let mut live = vec![0u64; words];
        for s in f.blocks[bi].term.succs() {
            if reach[s as usize] {
                live_in.each(s as usize, |v| live[v as usize >> 6] |= 1u64 << (v & 63));
            }
        }
        let mut tb = f.blocks[bi].term.clone();
        term_vregs_mut(&mut tb, |r| {
            if (r.0 as usize) < nv0 {
                live[r.0 as usize >> 6] |= 1u64 << (r.0 & 63);
            }
        });
        let b = &f.blocks[bi];
        let n = b.insts.len();
        // (call index, values live across it)
        let mut sites: Vec<(usize, Vec<VReg>)> = Vec::new();
        for ii in (0..n).rev() {
            let i = &b.insts[ii];
            // an invoke's call ends its block (and may unwind to a pad that reads v): left alone
            let noreturn_or_invoke = ii + 1 == n && matches!(b.term, Term::Unreachable | Term::Invoke(..));
            let mut defs = Vec::new();
            for_defs(i, |d| defs.push(d));
            if let (Inst::Call(..), false) = (i, noreturn_or_invoke) {
                let mut across = Vec::new();
                let (mut n_live, mut n_cand) = (0, 0);
                for w in 0..words {
                    let mut m = live[w];
                    while m != 0 {
                        let v = (w * 64 + m.trailing_zeros() as usize) as u32;
                        m &= m - 1;
                        let hu = hot_use[bi].as_ref().unwrap();
                        n_live += 1;
                        if cand[v as usize] {
                            n_cand += 1;
                        }
                        if cand[v as usize] && hu[v as usize >> 6] & (1u64 << (v & 63)) != 0 && !defs.iter().any(|d| d.0 == v) && !f.vregs[v as usize].is_vec() {
                            across.push(VReg(v));
                        }
                    }
                }
                if trace_on(f) {
                    eprintln!("cold-split {} nv {}: b{} @{} freq {}: live across {}, spilled {}, hot-used {}", f.name, f.vregs.len(), bi, ii, fr(bi), n_live, n_cand, across.len());
                }
                if !across.is_empty() {
                    sites.push((ii, across));
                }
            }
            for d in &defs {
                live[d.0 as usize >> 6] &= !(1u64 << (d.0 & 63));
            }
            for_uses(i, |u| live[u.0 as usize >> 6] |= 1u64 << (u.0 & 63));
        }
        if sites.is_empty() {
            continue;
        }
        // sites are back to front: inserting at a later index keeps the earlier ones valid
        for (ii, across) in sites {
            let p = g.blocks[bi].pos[ii];
            let mut before = Vec::new();
            let mut after = Vec::new();
            let mut call = g.blocks[bi].insts[ii].clone();
            for v in across {
                let t = g.vreg(f.vregs[v.0 as usize]);
                before.push(Inst::Mov(t, v));
                after.push(Inst::Mov(v, t));
                if let Inst::Call(_, args, _) = &mut call {
                    for a in args.iter_mut() {
                        if *a == v {
                            *a = t;
                        }
                    }
                }
            }
            let blk = &mut g.blocks[bi];
            blk.insts[ii] = call;
            let (nb_, na) = (before.len(), after.len());
            blk.insts.splice(ii + 1..ii + 1, after);
            blk.pos.splice(ii + 1..ii + 1, std::iter::repeat(p).take(na));
            blk.insts.splice(ii..ii, before);
            blk.pos.splice(ii..ii, std::iter::repeat(p).take(nb_));
            changed = true;
        }
        // a copy pair per value per cold call: a big loop with many cold calls made
        // thousands (Stage's video_edit store worker: 10.6k vregs -> 69k after both splits,
        // 7.4 s of allocation, 1.8 MB of code): past half again the function's values the
        // split costs more than its spills, and the first allocation stands
        if g.vregs.len() - nv0 > SPLIT_GROWTH_MAX(nv0) / 2 {
            return None;
        }
    }
    if changed {
        Some(g)
    } else {
        None
    }
}

fn allocate_split_loops(options: &relax_session::Options, f: &Func, al: Alloc, cfg: &RegConfig, thorough: bool, freq: Option<&[f32]>) -> (Option<Func>, Alloc) {
    let (h, al, _, _) = allocate_split_loops_inner(options, f, al, cfg, thorough, freq);
    (h, al)
}

fn allocate_split_loops_inner(options: &relax_session::Options, f: &Func, al: Alloc, cfg: &RegConfig, thorough: bool, freq: Option<&[f32]>) -> (Option<Func>, Alloc, Vec<bool>, Option<Vec<f32>>) {
    // spilled values and recomputed constants (a loop piece of a constant keeps it in a register
    // across that loop)
    let spilled: Vec<bool> = al.loc.iter().zip(&al.remat).map(|(l, r)| matches!(l, Loc::Stack(_)) || r.is_some()).collect();
    if !thorough || !split_on(options) || !spilled.iter().any(|x| *x) {
        return (None, al, Vec::new(), freq.map(|x| x.to_vec()));
    }
    let mut g = f.clone();
    let inreg: Vec<bool> = al.loc.iter().map(|l| matches!(l, Loc::Reg(_) | Loc::Flt(_))).collect();
    // with sampled frequencies, only loops the sampler hit are split: the second allocation is
    // most of thorough allocation's cost (back-end replay: 87 of +144 ms per 1k fns), and a loop
    // that never ran hot gains nothing from it
    let hot: Option<Vec<bool>> = freq.map(|fr| (0..f.blocks.len()).map(|b| fr.get(b).copied().unwrap_or(0.0) > 0.0).collect());
    let (origin, src, through) = match split_loops(&mut g, &spilled, if through_on(options) { Some(&inreg) } else { None }, hot.as_deref()) {
        Some(o) => o,
        None => return (None, al, Vec::new(), freq.map(|x| x.to_vec())),
    };
    // the made blocks run about as often as the block they stand for
    let gfreq: Option<Vec<f32>> = freq.map(|fr| src.iter().map(|b| fr.get(*b as usize).copied().unwrap_or(0.0)).collect());
    let gfreq = gfreq.as_deref();
    let mut al2 = allocate_low(options, &g, cfg, true, gfreq, Some(&through), Some(&origin));
    // a piece that did not get a register only adds copies: it goes back into its original
    // (renamed, the copies between them vanish) and the allocation runs once more
    let n_orig = f.vregs.len();
    let mut back: Vec<u32> = (0..g.vregs.len() as u32).collect();
    let mut any_back = false;
    for v in n_orig..g.vregs.len() {
        // a passed-through piece is meant to be in memory
        if matches!(al2.loc[v], Loc::Stack(_)) && !through[v] {
            let mut root = origin[v];
            while origin[root as usize] != u32::MAX {
                root = origin[root as usize];
            }
            back[v] = root;
            any_back = true;
        }
    }
    if any_back {
        for b in g.blocks.iter_mut() {
            for i in b.insts.iter_mut() {
                i.vregs_mut(|r| r.0 = back[r.0 as usize]);
            }
            term_vregs_mut(&mut b.term, |r| r.0 = back[r.0 as usize]);
            let mut k = 0;
            let keep: Vec<bool> = b.insts.iter().map(|i| !matches!(i, Inst::Mov(d, s2) if d == s2)).collect();
            b.insts.retain(|_| {
                k += 1;
                keep[k - 1]
            });
            let mut k2 = 0;
            b.pos.retain(|_| {
                k2 += 1;
                keep[k2 - 1]
            });
        }
        if back[n_orig..].iter().enumerate().all(|(k, b)| *b as usize != n_orig + k) {
            // every piece went back: the original allocation stands
            return (None, al, Vec::new(), freq.map(|x| x.to_vec()));
        }
        al2 = allocate_low(options, &g, cfg, true, gfreq, Some(&through), Some(&origin));
    }
    if let Ok(w) = std::env::var("MAKEPAD_RUN_RA_TRACE") {
        if f.name.ends_with(w.as_str()) {
            let n1 = al.loc.iter().filter(|l| matches!(l, Loc::Stack(_))).count();
            let n2 = al2.loc.iter().filter(|l| matches!(l, Loc::Stack(_))).count();
            eprintln!("ra {}: spilled {} -> {} of {} vregs ({} pieces)", f.name, n1, n2, g.vregs.len(), g.vregs.len() - f.vregs.len());
            for v in f.vregs.len()..g.vregs.len() {
                eprintln!("  piece v{} of v{}: {:?}", v, origin[v], al2.loc[v]);
            }
            let reach = reachable(&g);
            let li = live_in_sets(&g, &reach);
            let depth = loop_depths(&g, &reach);
            for bi in 0..g.blocks.len() {
                if !reach[bi] || depth[bi] == 0 {
                    continue;
                }
                let mut ints = Vec::new();
                for v in 0..g.vregs.len() {
                    if li.has(bi, v as u32) && g.vregs[v] == Cls::I {
                        ints.push(format!("v{}{}", v, match al2.loc[v] { Loc::Reg(r) => format!("=r{}", r), Loc::Stack(_) => "=S".to_string(), _ => String::new() }));
                    }
                }
                let calls = g.blocks[bi].insts.iter().filter(|i| matches!(i, Inst::Call(..))).count();
                eprintln!("  b{} depth {} calls {} live-in ints {}: {}", bi, depth[bi], calls, ints.len(), ints.join(" "));
            }
        }
    }
    let gf: Option<Vec<f32>> = gfreq.map(|x| x.to_vec());
    (Some(g), al2, through, gf)
}

/// Natural loops (back edge b -> h with h dominating b): (header, member blocks), innermost
/// (smallest) first. Loops headed by the entry block or entered through an unwind edge are left out.
#[inline(never)]
pub fn natural_loops(f: &Func, reach: &[bool], preds: &[Vec<u32>], all: bool) -> Vec<(u32, Vec<bool>)> {
    let idom = crate::dom::idoms(f);
    let (pre, post, _) = crate::dom::dom_numbers(&idom);
    let dominates = |a: u32, b: u32| -> bool {
        pre[a as usize] <= pre[b as usize] && post[b as usize] <= post[a as usize]
    };
    let nb = f.blocks.len();
    let mut by_header: Vec<(u32, Vec<bool>)> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        for h in b.term.succs() {
            if !dominates(h, bi as u32) || (h == 0 && !all) {
                continue;
            }
            let k = match by_header.iter().position(|x| x.0 == h) {
                Some(k) => k,
                None => {
                    let mut m = vec![false; nb];
                    m[h as usize] = true;
                    by_header.push((h, m));
                    by_header.len() - 1
                }
            };
            let mut work = vec![bi as u32];
            while let Some(x) = work.pop() {
                if by_header[k].1[x as usize] {
                    continue;
                }
                by_header[k].1[x as usize] = true;
                for &p in &preds[x as usize] {
                    if reach[p as usize] {
                        work.push(p);
                    }
                }
            }
        }
    }
    // entered through an unwind edge: no place for entry copies
    by_header.retain(|(h, m)| {
        all || !f.blocks.iter().enumerate().any(|(bi, b)| reach[bi] && !m[bi] && matches!(b.term, Term::Invoke(_, u) if u == *h))
    });
    by_header.sort_by_key(|(_, m)| m.iter().filter(|x| **x).count());
    by_header
}

#[inline(never)]
pub fn preds_of(f: &Func, reach: &[bool]) -> Vec<Vec<u32>> {
    let mut p = vec![Vec::new(); f.blocks.len()];
    for (bi, b) in f.blocks.iter().enumerate() {
        if reach[bi] {
            for t in b.term.succs() {
                p[t as usize].push(bi as u32);
            }
        }
    }
    p
}

/// Live-in sets per block (bitsets of `words`).
#[inline(never)]
fn live_in_sets(f: &Func, reach: &[bool]) -> LiveSets {
    let tab = OpTab::new(f, reach);
    live_sets(f, reach, &tab, false).0
}

pub fn term_vregs_mut(t: &mut Term, mut fv: impl FnMut(&mut VReg)) {
    match t {
        Term::Branch(c, _, _) | Term::Switch(c, _) => fv(c),
        Term::Ret(vs) => vs.iter_mut().for_each(fv),
        _ => {}
    }
}

/// Splits the spilled values' live ranges at loops (see `allocate_split`). Returns, per vreg of
/// the new function, the vreg it was split from (u32::MAX: an original), or None if nothing changed.
/// One liveness solution serves every loop: splitting an inner loop moves an original's uses and
/// defs to that loop's entry and exit copies, which leaves its liveness at every block outside the
/// loop as it was; blocks made here answer liveness through the block they stand in front of.
#[inline(never)]
/// Also (with `inreg`, the values the first allocation gave a register): values live through a loop
/// with spilled references inside it but never referenced there themselves get a piece too -- saved
/// at the entry, restored at the exits, dead inside -- so the registers they held through the loop
/// go to the loop's own values (a long value that is only passed through keeps the hot block's
/// temporaries in memory otherwise). Returns (origin, source block, through-piece flags).
fn split_loops(f: &mut Func, spilled: &[bool], inreg: Option<&[bool]>, hot: Option<&[bool]>) -> Option<(Vec<u32>, Vec<u32>, Vec<bool>)> {
    let reach0 = reachable(f);
    let mut preds = preds_of(f, &reach0);
    let mut loops = natural_loops(f, &reach0, &preds, false);
    if trace_on(f) {
        for (hd, m) in &loops {
            let hb = m.iter().enumerate().any(|(b, x)| *x && hot.map_or(true, |h| h.get(b).copied().unwrap_or(false)));
            eprintln!("split-loop {} loop h{} blocks {} hot {}", f.name, hd, m.iter().filter(|x| **x).count(), hb);
        }
    }
    if let Some(h) = hot {
        loops.retain(|(_, m)| m.iter().enumerate().any(|(b, x)| *x && h.get(b).copied().unwrap_or(false)));
    }
    if loops.is_empty() {
        return None;
    }
    let live_in = live_in_sets(f, &reach0);
    let nb0 = f.blocks.len();
    let n_orig = f.vregs.len();
    let mut origin: Vec<u32> = vec![u32::MAX; n_orig];
    // loop membership as loop indices, and each loop's blocks (blocks made below join them)
    let mut member: Vec<Vec<u16>> = vec![Vec::new(); nb0];
    let mut blocks_of: Vec<Vec<u32>> = vec![Vec::new(); loops.len()];
    for (li, (_, m)) in loops.iter().enumerate() {
        for (bi, x) in m.iter().enumerate() {
            if *x {
                member[bi].push(li as u16);
                blocks_of[li].push(bi as u32);
            }
        }
    }
    // a made block's liveness: that of the block it leads to (alias), less the originals it defines
    let mut alias: Vec<u32> = (0..nb0 as u32).collect();
    // (a made block can lead to another made block: an exit of an inner loop into the entry
    // block made for an outer one -- follow the chain to an original block)
    let live = |f: &Func, alias: &[u32], b: u32, v: u32| -> bool {
        let mut b = b;
        for _ in 0..f.blocks.len() + 1 {
            if (b as usize) < nb0 {
                // an inner loop's exit copies spliced at the start of this block (a target
                // entered only from that loop) define or read the original first: `live_in` is
                // the original function's. An outer loop sharing the exit edge must not copy its
                // own (stale) piece back over the value the inner copy wrote.
                for i in &f.blocks[b as usize].insts {
                    let Inst::Mov(d, s2) = i else { break };
                    if s2.0 == v {
                        return true;
                    }
                    if d.0 == v {
                        return false;
                    }
                }
                return v < n_orig as u32 && live_in.has(b as usize, v);
            }
            for i in &f.blocks[b as usize].insts {
                if let Inst::Mov(d, s2) = i {
                    if s2.0 == v {
                        return true;
                    }
                    if d.0 == v {
                        return false;
                    }
                }
            }
            b = alias[b as usize];
        }
        true
    };
    // new blocks: placed before (entry copies) or after (exit copies) an existing block in the
    // final order: (anchor block, after?, block)
    let mut placed: Vec<(u32, bool, u32)> = Vec::new();
    let mut changed = false;
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut seen = vec![false; n_orig];
    let mut defd = vec![false; n_orig];
    let mut refall = vec![false; n_orig];
    let mut through_pieces: Vec<u32> = Vec::new();
    for li in 0..loops.len() {
        let header = loops[li].0;
        let inl = |b: u32, member: &Vec<Vec<u16>>| member[b as usize].contains(&(li as u16));
        // spilled originals referenced inside the loop (pieces made for inner loops are reached
        // through their originals' copies, which are split here too)
        let mut refd: Vec<u32> = Vec::new();
        for &bi in &blocks_of[li] {
            let blk = &f.blocks[bi as usize];
            for i in &blk.insts {
                uses_defs(i, &mut uses, &mut defs);
                for v in uses.iter().chain(defs.iter()) {
                    let v = v.0 as usize;
                    if v < n_orig {
                        refall[v] = true;
                    }
                    if v < n_orig && spilled[v] && !seen[v] {
                        seen[v] = true;
                        refd.push(v as u32);
                    }
                }
                for d in &defs {
                    if (d.0 as usize) < n_orig {
                        defd[d.0 as usize] = true;
                    }
                }
            }
            term_uses(&blk.term, &mut uses);
            for v in &uses {
                let v = v.0 as usize;
                if v < n_orig {
                    refall[v] = true;
                }
                if v < n_orig && spilled[v] && !seen[v] {
                    seen[v] = true;
                    refd.push(v as u32);
                }
            }
        }
        // exit edges (from a member to a non-member) and entering blocks
        let mut exits: Vec<(u32, u32, bool)> = Vec::new(); // (from, to, unwind edge)
        for &bi in &blocks_of[li] {
            let t = &f.blocks[bi as usize].term;
            let unwind_to = if let Term::Invoke(_, u) = t { Some(*u) } else { None };
            for s2 in t.succs() {
                if !inl(s2, &member) {
                    exits.push((bi, s2, unwind_to == Some(s2)));
                }
            }
        }
        let entries: Vec<u32> = preds[header as usize].iter().copied().filter(|p| !inl(*p, &member)).collect();
        let mut split: Vec<(u32, u32, bool, Vec<u32>)> = Vec::new(); // (v, piece, entry copy, exits needing a copy back)
        for &v in &refd {
            let entry = live(f, &alias, header, v);
            let mut back: Vec<u32> = Vec::new();
            let mut ok = true;
            if defd[v as usize] {
                for (k, &(_, to, unwind)) in exits.iter().enumerate() {
                    if live(f, &alias, to, v) {
                        if unwind {
                            ok = false;
                        }
                        back.push(k as u32);
                    }
                }
            }
            if !ok {
                continue;
            }
            let c = f.vregs[v as usize];
            let piece = f.vreg(c).0;
            origin.push(v);
            split.push((v, piece, entry, back));
        }
        // values passed through in registers while the loop's own values spill: up to as many as
        // the loop has spilled values referenced
        if let (Some(inreg), false) = (inreg, refd.is_empty()) {
            let mut k = refd.len();
            for v in 0..n_orig as u32 {
                if k == 0 {
                    break;
                }
                if !inreg[v as usize] || refall[v as usize] || f.vregs[v as usize].is_vec() || !live(f, &alias, header, v) {
                    continue;
                }
                let mut back: Vec<u32> = Vec::new();
                let mut ok = true;
                for (e, &(_, to, unwind)) in exits.iter().enumerate() {
                    if live(f, &alias, to, v) {
                        if unwind {
                            ok = false;
                        }
                        back.push(e as u32);
                    }
                }
                if !ok {
                    continue;
                }
                let c = f.vregs[v as usize];
                let piece = f.vreg(c).0;
                origin.push(v);
                through_pieces.push(piece);
                split.push((v, piece, true, back));
                k -= 1;
            }
        }
        // (see split_cold_calls): too many pieces, no split
        if f.vregs.len() - n_orig > SPLIT_GROWTH_MAX(n_orig) {
            return None;
        }
        for &bi in &blocks_of[li] {
            for i in &f.blocks[bi as usize].insts {
                for_uses(i, |u| {
                    if (u.0 as usize) < n_orig {
                        refall[u.0 as usize] = false;
                    }
                });
                for_defs(i, |d| {
                    if (d.0 as usize) < n_orig {
                        refall[d.0 as usize] = false;
                    }
                });
            }
            let mut tu = Vec::new();
            term_uses(&f.blocks[bi as usize].term, &mut tu);
            for u in tu {
                if (u.0 as usize) < n_orig {
                    refall[u.0 as usize] = false;
                }
            }
        }
        for &v in &refd {
            seen[v as usize] = false;
            defd[v as usize] = false;
        }
        for &bi in &blocks_of[li] {
            for i in &f.blocks[bi as usize].insts {
                for_defs(i, |d| {
                    if (d.0 as usize) < n_orig {
                        defd[d.0 as usize] = false;
                    }
                });
            }
        }
        if trace_on(f) {
            let sp: Vec<String> = split.iter().map(|x| format!("v{}->v{}{}", x.0, x.1, if through_pieces.contains(&x.1) { "t" } else { "" })).collect();
            eprintln!("split-loop {} h{} blocks {} exits {} refd {}: {}", f.name, header, blocks_of[li].len(), exits.len(), refd.len(), sp.join(" "));
        }
        if split.is_empty() {
            continue;
        }
        changed = true;
        // rename inside the loop
        let map: HashMap<u32, u32> = split.iter().map(|x| (x.0, x.1)).collect();
        for &bi in &blocks_of[li] {
            let b = &mut f.blocks[bi as usize];
            for i in b.insts.iter_mut() {
                i.vregs_mut(|r| {
                    if let Some(&n) = map.get(&r.0) {
                        r.0 = n;
                    }
                });
            }
            term_vregs_mut(&mut b.term, |r| {
                if let Some(&n) = map.get(&r.0) {
                    r.0 = n;
                }
            });
        }
        // entry copies
        let ins: Vec<Inst> = split.iter().filter(|x| x.2).map(|x| Inst::Mov(VReg(x.1), VReg(x.0))).collect();
        if !ins.is_empty() {
            let tp = f.blocks[header as usize].term_pos;
            if entries.len() == 1 && matches!(f.blocks[entries[0] as usize].term, Term::Jump(_)) {
                let b = &mut f.blocks[entries[0] as usize];
                let p = b.term_pos;
                for i in ins {
                    b.insts.push(i);
                    b.pos.push(p);
                }
            } else {
                let nbk = f.blocks.len() as u32;
                let n = ins.len();
                f.blocks.push(Block { insts: ins, pos: vec![tp; n], term: Term::Jump(header), term_pos: tp });
                alias.push(header);
                // member of the loops holding the header and every entering block, except this one
                let mut m: Vec<u16> = member[header as usize].iter().copied().filter(|x| *x != li as u16).collect();
                for &e in &entries {
                    m.retain(|x| member[e as usize].contains(x));
                }
                for &x in &m {
                    blocks_of[x as usize].push(nbk);
                }
                member.push(m);
                for &e in &entries {
                    f.blocks[e as usize].term.map_succs(|s2| if s2 == header { nbk } else { s2 });
                }
                preds[header as usize].retain(|p| inl(*p, &member));
                preds[header as usize].push(nbk);
                preds.push(entries.clone());
                placed.push((header, false, nbk));
            }
        }
        // exit copies: per exit edge, the values live at its target and defined in the loop
        let mut per_edge: Vec<Vec<Inst>> = vec![Vec::new(); exits.len()];
        for (v, piece, _, back) in &split {
            for &k in back {
                per_edge[k as usize].push(Inst::Mov(VReg(*v), VReg(*piece)));
            }
        }
        // a target entered only from this loop (normal edges) takes the copies at its start
        let mut done_target: Vec<u32> = Vec::new();
        for (k, &(from, to, _)) in exits.iter().enumerate() {
            if per_edge[k].is_empty() {
                continue;
            }
            let only_from_loop = preds[to as usize].iter().all(|&p| inl(p, &member) && !matches!(f.blocks[p as usize].term, Term::Invoke(_, u) if u == to));
            let same_copies = exits.iter().enumerate().filter(|(_, e2)| e2.1 == to).all(|(j, _)| per_edge[j] == per_edge[k]);
            if only_from_loop && same_copies && !matches!(f.blocks[from as usize].term, Term::Invoke(_, u) if u == to) {
                if done_target.contains(&to) {
                    continue;
                }
                done_target.push(to);
                let b = &mut f.blocks[to as usize];
                let p = b.pos.first().copied().unwrap_or(b.term_pos);
                let n = per_edge[k].len();
                b.insts.splice(0..0, per_edge[k].iter().cloned());
                b.pos.splice(0..0, std::iter::repeat(p).take(n));
                continue;
            }
            let nbk = f.blocks.len() as u32;
            let tp = f.blocks[to as usize].term_pos;
            let n = per_edge[k].len();
            f.blocks.push(Block { insts: per_edge[k].clone(), pos: vec![tp; n], term: Term::Jump(to), term_pos: tp });
            alias.push(to);
            let m: Vec<u16> = member[from as usize].iter().copied().filter(|x| member[to as usize].contains(x)).collect();
            for &x in &m {
                blocks_of[x as usize].push(nbk);
            }
            member.push(m);
            // retarget only this edge (an Invoke's normal edge or a branch arm)
            match &mut f.blocks[from as usize].term {
                Term::Jump(t) => *t = nbk,
                Term::Branch(_, t, e2) => {
                    if *t == to {
                        *t = nbk;
                    }
                    if *e2 == to {
                        *e2 = nbk;
                    }
                }
                Term::Invoke(t, _) => *t = nbk,
                sw @ Term::Switch(..) => sw.map_succs(|s2| if s2 == to { nbk } else { s2 }),
                _ => {}
            }
            if let Some(q) = preds[to as usize].iter().position(|p| *p == from) {
                preds[to as usize].remove(q);
            }
            preds[to as usize].push(nbk);
            preds.push(vec![from]);
            placed.push((from, true, nbk));
        }
    }
    if !changed {
        return None;
    }
    // per final block, the original block whose frequency it has (made blocks: the one they lead to)
    let mut src: Vec<u32> = alias.clone();
    // final order: entry blocks right before their header, exit blocks right after their source,
    // so layout (fall-through, back edges in block order) stays as it was
    if !placed.is_empty() {
        let nb = f.blocks.len();
        let n_old = nb - placed.len();
        let mut before: Vec<Vec<u32>> = vec![Vec::new(); nb];
        let mut after: Vec<Vec<u32>> = vec![Vec::new(); nb];
        for &(a2, aft, nbk) in &placed {
            if aft {
                after[a2 as usize].push(nbk);
            } else {
                before[a2 as usize].push(nbk);
            }
        }
        let mut order: Vec<u32> = Vec::with_capacity(nb);
        fn emit(b: u32, before: &[Vec<u32>], after: &[Vec<u32>], order: &mut Vec<u32>) {
            for &x in &before[b as usize] {
                emit(x, before, after, order);
            }
            order.push(b);
            for &x in &after[b as usize] {
                emit(x, before, after, order);
            }
        }
        for b in 0..n_old as u32 {
            emit(b, &before, &after, &mut order);
        }
        debug_assert_eq!(order.len(), nb);
        let mut newidx = vec![0u32; nb];
        for (k, &b) in order.iter().enumerate() {
            newidx[b as usize] = k as u32;
        }
        let mut old: Vec<Option<Block>> = std::mem::take(&mut f.blocks).into_iter().map(Some).collect();
        for &b in &order {
            let mut blk = old[b as usize].take().unwrap();
            blk.term.map_succs(|s2| newidx[s2 as usize]);
            f.blocks.push(blk);
        }
        src = order.iter().map(|b| alias[*b as usize]).collect();
    }
    let mut through = vec![false; f.vregs.len()];
    for p in through_pieces {
        through[p as usize] = true;
    }
    Some((origin, src, through))
}

fn coalesce_on(options: &relax_session::Options) -> bool {
    !options.no_coalesce
}

/// Do two sorted, disjoint range lists share a position?
#[inline(never)]
fn ranges_meet(x: &[(u32, u32)], y: &[(u32, u32)]) -> bool {
    let (mut i, mut j) = (0, 0);
    while i < x.len() && j < y.len() {
        if x[i].1 < y[j].0 {
            i += 1;
        } else if y[j].1 < x[i].0 {
            j += 1;
        } else {
            return true;
        }
    }
    false
}

/// Union of two sorted, disjoint range lists (touching ranges merged).
#[inline(never)]
fn union_ranges(x: &[(u32, u32)], y: &[(u32, u32)]) -> Vec<(u32, u32)> {
    // a merge of the two sorted lists (no sort: this ran per coalescing step and showed up as
    // a top O3 cost on Windows)
    let mut out: Vec<(u32, u32)> = Vec::with_capacity(x.len() + y.len());
    let (mut i, mut j) = (0, 0);
    while i < x.len() || j < y.len() {
        let r = if j >= y.len() || (i < x.len() && x[i] <= y[j]) {
            i += 1;
            x[i - 1]
        } else {
            j += 1;
            y[j - 1]
        };
        match out.last_mut() {
            Some(l) if r.0 <= l.1 + 1 => l.1 = l.1.max(r.1),
            _ => out.push(r),
        }
    }
    out
}

/// Repacks the spill slots (`Loc::Stack` offsets) so that slots whose values cannot be live at
/// once share one offset: a slot's occupancy is its values' live ranges (with holes) plus every
/// position one of them is defined at (a dead definition still stores); slots are placed in
/// order of their first position into the first offset whose occupancy they do not meet.
/// V128 slots (16 bytes, 16-aligned) and 8-byte slots are packed separately. Returns the new
/// spill area size.
/// The spill area grown by one slot of class `c` (vectors at 16-aligned offsets).
fn grow_spill(bytes: u32, c: Cls) -> u32 {
    match c.spill_bytes() {
        8 => bytes + 8,
        n => (bytes + 15) / 16 * 16 + n,
    }
}

fn pack_spill_slots(f: &Func, reach: &[bool], block_start: &[u32], loc: &mut [Loc], ranges: &Ranges, start: &[u32]) -> u32 {
    // per old offset: occupancy, its slot size (Cls::spill_bytes)
    let mut occ: HashMap<i32, (Vec<(u32, u32)>, u32)> = HashMap::new();
    for v in 0..loc.len() {
        if let Loc::Stack(o) = loc[v] {
            let e = occ.entry(o).or_insert((Vec::new(), 8));
            // a value no reachable code touches: occupies everything
            let r: Vec<(u32, u32)> = if start[v] == u32::MAX { vec![(0, u32::MAX)] } else { ranges.of(v as u32).to_vec() };
            e.0 = union_ranges(&e.0, &r);
            e.1 = e.1.max(f.vregs[v].spill_bytes());
        }
    }
    if occ.len() < 2 {
        return occ.keys().map(|&o| o as u32).max().unwrap_or(0);
    }
    for p in &f.params {
        if let Loc::Stack(o) = loc[p.0 as usize] {
            let e = occ.get_mut(&o).unwrap();
            e.0 = union_ranges(&e.0, &[(0, 0)]);
        }
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        for (ii, i) in b.insts.iter().enumerate() {
            let p = block_start[bi] + 2 * ii as u32 + 2;
            for_defs(i, |d| {
                if let Loc::Stack(o) = loc[d.0 as usize] {
                    let e = occ.get_mut(&o).unwrap();
                    e.0 = union_ranges(&e.0, &[(p, p)]);
                }
            });
        }
    }
    let mut old: Vec<(u32, i32, u32, Vec<(u32, u32)>)> = occ.into_iter().map(|(o, (r, w))| (r.first().map_or(0, |x| x.0), o, w, r)).collect();
    old.sort_unstable_by_key(|x| (x.0, x.1));
    let mut bytes = 0u32;
    let mut to: HashMap<i32, i32> = HashMap::new();
    for wide in [32u32, 16, 8] {
        // (offset, union of the occupancies placed there)
        let mut slots: Vec<(i32, Vec<(u32, u32)>)> = Vec::new();
        for (_, o, w, r) in &old {
            if *w != wide {
                continue;
            }
            // linear in the slots so far: beyond a bound the first free one is not searched for
            let found = if slots.len() <= 256 { slots.iter_mut().find(|s| !ranges_meet(&s.1, r)) } else { None };
            match found {
                Some(s) => {
                    s.1 = union_ranges(&s.1, r);
                    to.insert(*o, s.0);
                }
                None => {
                    bytes = if wide == 8 { bytes + 8 } else { (bytes + 15) / 16 * 16 + wide };
                    slots.push((bytes as i32, r.clone()));
                    to.insert(*o, bytes as i32);
                }
            }
        }
    }
    for l in loc.iter_mut() {
        if let Loc::Stack(o) = *l {
            *l = Loc::Stack(to[&o]);
        }
    }
    bytes
}

/// MAKEPAD_RUN_NO_SLOT_SHARE=1: every spilled value keeps a slot of its own
fn slot_share_on(options: &relax_session::Options) -> bool {
    !options.no_slot_share
}

fn remat_on(options: &relax_session::Options) -> bool {
    !options.no_remat
}


/// Constants and loops (backend preparation, target rules in `fits`):
/// - an operation on a constant vreg whose immediate form the target encodes takes that form
///   (`IBin` / `ICmp` with a single-def `Iconst` operand -> `IBinI` / `ICmpI`);
/// - immediates it cannot encode, inside loops, become registers: `IBinI` / `ICmpI` there turn
///   into `IBin` / `ICmp` on a constant vreg;
/// - constants used inside loops are defined once outside them, one vreg per value and place:
///   at the entry, or with `near` at the nearest dominator of the loop that is in no loop (the
///   block before the loop nest; a function whose loops sit in rarely taken arms, such as a
///   tokenizer's `next`, then builds none of them on the common path).
/// The allocator then keeps such a constant in a register across the loop, or recomputes it at
/// the use when none is free (what the backend did at every use before).
/// - with `floats`, float constants defined inside loops move out too (one vreg per value and
///   width): for targets that build them with instructions or a literal-pool load at each
///   definition (arm64 `fmov #imm`, `ldr s / d, =lit`), not for those that fold them into the
///   using instruction's memory operand (x64).
/// With measured block frequencies (`freq`, O3) a constant stays where it is when its block runs
/// no more often than the home block (a loop that rarely goes round: a state machine's `loop`
/// around its whole body would otherwise build every constant of every arm per call).
/// Returns the rewritten copy, or None when there is nothing to do.
pub fn hoist_loop_consts(f: &Func, fits: &dyn Fn(&Inst) -> bool, floats: bool, near: bool, freq: Option<&[f32]>) -> Option<Func> {
    let reach = reachable(f);
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
    }
    for p in &f.params {
        ndef[p.0 as usize] += 2;
    }
    let mut cval: Vec<Option<i64>> = vec![None; nv];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Iconst(d, k) = i {
                if ndef[d.0 as usize] == 1 {
                    cval[d.0 as usize] = Some(*k);
                }
            }
        }
    }
    let depth = loop_depths(f, &reach);
    let has_loop = depth.iter().any(|d| *d > 0);
    // where a constant of a block in a loop is defined: the entry, or (near) its nearest
    // dominator in no loop (it dominates every use of a value defined in that block)
    let idom = if near && has_loop { crate::dom::idoms(f) } else { Vec::new() };
    let home = |bi: usize| -> u32 {
        if idom.is_empty() {
            return 0;
        }
        let mut t = bi as u32;
        while depth[t as usize] > 0 && t != 0 {
            t = idom[t as usize];
        }
        t
    };
    let mut changed = false;
    let mut g = f.clone();
    // (value, home block) -> vreg
    let mut consts: Vec<(i64, u32, VReg)> = Vec::new();
    // float constants by (bits, f64?)
    let mut fconsts: Vec<((u64, bool), u32, VReg)> = Vec::new();
    let mut moved = vec![false; nv];
    let mut moved_home = vec![0u32; nv];
    for bi in 0..g.blocks.len() {
        if !reach[bi] {
            continue;
        }
        let h = if depth[bi] > 0 { home(bi) } else { 0 };
        let iterates = match freq {
            Some(fr) if fr.len() == f.blocks.len() => fr[bi] > 1.5 * fr[h as usize],
            _ => true,
        };
        let in_loop = depth[bi] > 0 && iterates;
        for k in 0..g.blocks[bi].insts.len() {
            let i = g.blocks[bi].insts[k].clone();
            // immediate forms the target encodes
            let as_imm = match i {
                Inst::IBin(op, it, d, a, c) if cval[c.0 as usize].is_some() && !matches!(op, IOp::Div | IOp::Rem | IOp::MulHi) => Some(Inst::IBinI(op, it, d, a, cval[c.0 as usize].unwrap())),
                Inst::IBin(op @ (IOp::Add | IOp::Mul | IOp::And | IOp::Or | IOp::Xor), it, d, c, a) if cval[c.0 as usize].is_some() && cval[a.0 as usize].is_none() => Some(Inst::IBinI(op, it, d, a, cval[c.0 as usize].unwrap())),
                Inst::ICmp(cc, sg, d, a, c) if cval[c.0 as usize].is_some() => Some(Inst::ICmpI(cc, sg, d, a, cval[c.0 as usize].unwrap())),
                _ => None,
            };
            if let Some(ii) = as_imm {
                if fits(&ii) {
                    g.blocks[bi].insts[k] = ii;
                    changed = true;
                    continue;
                }
            }
            if !in_loop {
                continue;
            }
            match i {
                // a constant defined in the loop: defined outside it instead
                Inst::Iconst(d, v) if ndef[d.0 as usize] == 1 => {
                    moved[d.0 as usize] = true;
                    moved_home[d.0 as usize] = h;
                    if !consts.iter().any(|x| x.0 == v && x.1 == h) {
                        consts.push((v, h, d));
                    }
                    changed = true;
                }
                Inst::Fconst(d, bits, w) if floats && ndef[d.0 as usize] == 1 => {
                    moved[d.0 as usize] = true;
                    moved_home[d.0 as usize] = h;
                    if !fconsts.iter().any(|x| x.0 == (bits, w) && x.1 == h) {
                        fconsts.push(((bits, w), h, d));
                    }
                    changed = true;
                }
                Inst::IBinI(op, it, d, a, v) if !fits(&i) => {
                    let c = const_vreg(&mut g, &mut consts, v, h);
                    g.blocks[bi].insts[k] = Inst::IBin(op, it, d, a, c);
                    changed = true;
                }
                Inst::ICmpI(cc, sg, d, a, v) if !fits(&i) => {
                    let c = const_vreg(&mut g, &mut consts, v, h);
                    g.blocks[bi].insts[k] = Inst::ICmp(cc, sg, d, a, c);
                    changed = true;
                }
                _ => {}
            }
        }
    }
    if !changed {
        return None;
    }
    if has_loop && moved.iter().any(|x| *x) {
        // moved constants: the loop's defs go; other vregs of the same value and home read the one defined there
        let mut map: Vec<u32> = (0..g.vregs.len() as u32).collect();
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Iconst(d, v) = i {
                    if moved[d.0 as usize] {
                        let h = moved_home[d.0 as usize];
                        map[d.0 as usize] = consts.iter().find(|x| x.0 == *v && x.1 == h).unwrap().2 .0;
                    }
                }
                if let Inst::Fconst(d, bits, w) = i {
                    if moved[d.0 as usize] {
                        let h = moved_home[d.0 as usize];
                        map[d.0 as usize] = fconsts.iter().find(|x| x.0 == (*bits, *w) && x.1 == h).unwrap().2 .0;
                    }
                }
            }
        }
        for b in g.blocks.iter_mut() {
            let keep: Vec<bool> = b.insts.iter().map(|i| !matches!(i, Inst::Iconst(d, _) | Inst::Fconst(d, _, _) if (d.0 as usize) < nv && moved[d.0 as usize])).collect();
            let mut k = 0;
            b.insts.retain(|_| {
                k += 1;
                keep[k - 1]
            });
            let mut k2 = 0;
            b.pos.retain(|_| {
                k2 += 1;
                keep[k2 - 1]
            });
            for i in b.insts.iter_mut() {
                i.vregs_mut(|r| r.0 = map[r.0 as usize]);
            }
            term_vregs_mut(&mut b.term, |r| r.0 = map[r.0 as usize]);
        }
    }
    // the definitions at the start of their home blocks (in first-seen order)
    let mut homes: Vec<u32> = consts.iter().map(|x| x.1).chain(fconsts.iter().map(|x| x.1)).collect();
    homes.sort_unstable();
    homes.dedup();
    for hb in homes {
        let b = &mut g.blocks[hb as usize];
        let p0 = b.pos.first().copied().unwrap_or(b.term_pos);
        let defs: Vec<Inst> = consts.iter().filter(|x| x.1 == hb).map(|(v, _, c)| Inst::Iconst(*c, *v)).chain(fconsts.iter().filter(|x| x.1 == hb).map(|((bits, w), _, c)| Inst::Fconst(*c, *bits, *w))).collect();
        let n = defs.len();
        b.insts.splice(0..0, defs);
        b.pos.splice(0..0, std::iter::repeat(p0).take(n));
    }
    Some(g)
}

fn const_vreg(g: &mut Func, consts: &mut Vec<(i64, u32, VReg)>, v: i64, home: u32) -> VReg {
    match consts.iter().find(|x| x.0 == v && x.1 == home) {
        Some(x) => x.2,
        None => {
            let c = g.vreg(Cls::I);
            consts.push((v, home, c));
            c
        }
    }
}

fn depth_cap(options: &relax_session::Options) -> u32 {
    options.ra_depthcap
}

/// Indexed accesses of one base and index in a block, some with displacements (unrolled copies:
/// [b + i*4 + 4], [b + i*4 + 8], ...): one address `p = b + (i << s)` before the first and plain
/// accesses `[p + off]` after it -- for targets whose indexed form takes no displacement (arm64
/// otherwise adds base and index again for each). Returns the rewritten copy, or None.
pub fn share_index_bases(f: &Func) -> Option<Func> {
    let mut g: Option<Func> = None;
    for bi in 0..f.blocks.len() {
        let b = &f.blocks[bi];
        // (base, idx, sh) -> [inst index], current run (reset when base or idx is redefined)
        let mut groups: Vec<((VReg, VReg, u8), Vec<usize>)> = Vec::new();
        let mut open: Vec<usize> = Vec::new(); // indices into groups still valid
        for (k, i) in b.insts.iter().enumerate() {
            let key = match i {
                Inst::LoadX(_, _, base, x, sh, _) | Inst::StoreX(_, base, x, sh, _, _) => Some((*base, *x, *sh)),
                _ => None,
            };
            if let Some(key) = key {
                match open.iter().find(|gi| groups[**gi].0 == key) {
                    Some(&gi) => groups[gi].1.push(k),
                    None => {
                        groups.push((key, vec![k]));
                        open.push(groups.len() - 1);
                    }
                }
            }
            // a def of a base or index closes its groups (after this instruction: a load's own
            // destination may be its index)
            let mut defs = Vec::new();
            for_defs(i, |d| defs.push(d));
            open.retain(|gi| {
                let (bs, x, _) = groups[*gi].0;
                !defs.contains(&bs) && !defs.contains(&x)
            });
        }
        let worth: Vec<&((VReg, VReg, u8), Vec<usize>)> = groups
            .iter()
            .filter(|(_, ks)| ks.len() >= 2 && ks.iter().any(|k| matches!(b.insts[*k], Inst::LoadX(_, _, _, _, _, o) | Inst::StoreX(_, _, _, _, o, _) if o != 0)))
            .collect();
        if worth.is_empty() {
            continue;
        }
        let gf = g.get_or_insert_with(|| f.clone());
        // the accesses are rewritten in place first, the address definitions spliced in after,
        // back to front (groups interleave: a load and a store run of one copy loop; splicing a
        // group's definition first moved the other group's later accesses off their indices)
        let mut plan: Vec<(usize, (VReg, VReg, u8), Vec<usize>)> = worth.iter().map(|(key, ks)| (ks[0], *key, ks.clone())).collect();
        plan.sort_by(|a, b2| b2.0.cmp(&a.0));
        let mut splices: Vec<(usize, Vec<Inst>)> = Vec::new();
        for (first, (base, x, sh), ks) in plan {
            let p = gf.vreg(Cls::I);
            let mut ins: Vec<Inst> = Vec::new();
            if sh > 0 {
                let s = gf.vreg(Cls::I);
                ins.push(Inst::IBinI(IOp::Shl, IntTy { bits: 64, signed: false }, s, x, sh as i64));
                ins.push(Inst::IBin(IOp::Add, IntTy { bits: 64, signed: false }, p, base, s));
            } else {
                ins.push(Inst::IBin(IOp::Add, IntTy { bits: 64, signed: false }, p, base, x));
            }
            let blk = &mut gf.blocks[bi];
            for k in &ks {
                blk.insts[*k] = match blk.insts[*k] {
                    Inst::LoadX(m, d, _, _, _, o) => Inst::Load(m, d, p, o),
                    Inst::StoreX(m, _, _, _, o, v) => Inst::Store(m, p, o, v),
                    ref other => other.clone(),
                };
            }
            splices.push((first, ins));
        }
        // plan is sorted by first index, descending
        let blk = &mut gf.blocks[bi];
        for (first, ins) in splices {
            let ps = blk.pos[first];
            let n = ins.len();
            blk.insts.splice(first..first, ins);
            blk.pos.splice(first..first, std::iter::repeat(ps).take(n));
        }
    }
    g
}

/// An integer compare whose result a select in the same block reads moves down to just before
/// that select, when the instructions in between neither read its result nor redefine its
/// operands: the backend then takes the select's condition from the compare's flags (x64 cmovcc
/// right after cmp + setcc) instead of testing the materialised bool again after other
/// arithmetic clobbered the flags (stable partition: `c = a < b; p = end - 8; d = c ? x : p`).
/// Returns the rewritten copy, or None.
/// Reads of a spilled integer value that a register copy of it can serve: after `u = v` (one of
/// them in a stack slot, the other in a register) a later read of the spilled one in the same
/// block -- neither redefined since, no call between, and the register one still read later in the
/// block (so its register holds it there) -- reads the register instead of loading the slot.
/// `mov r14, [rbp-0x70]` then `cmp r10, [rbp-0x70]` (html's position after its copy) becomes
/// `cmp r14, ...`. Keyed by (use position, vreg). MAKEPAD_RUN_OPT=-copyreuse: none (bisection).
pub fn copy_reuse(options: &relax_session::Options, f: &Func, al: &Alloc) -> HashMap<(u32, u32), u8> {
    let mut out = HashMap::new();
    if crate::stats::opt_off(options, "-copyreuse") {
        return out;
    }
    let reach = reachable(f);
    let (mut us, mut ds) = (Vec::new(), Vec::new());
    let mut tu = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] || !b.insts.iter().any(|i| matches!(i, Inst::Mov(..))) {
            continue;
        }
        // last read of each vreg in the block (the terminator reads at index n)
        let mut last: HashMap<u32, usize> = HashMap::new();
        for (ii, i) in b.insts.iter().enumerate() {
            uses_defs(i, &mut us, &mut ds);
            for u in &us {
                last.insert(u.0, ii);
            }
        }
        term_uses(&b.term, &mut tu);
        for u in &tu {
            last.insert(u.0, b.insts.len());
        }
        // spilled vreg -> (its register copy, that register)
        let mut have: Vec<(u32, u32, u8)> = Vec::new();
        for (ii, i) in b.insts.iter().enumerate() {
            uses_defs(i, &mut us, &mut ds);
            if !have.is_empty() {
                for u in &us {
                    if let Some(&(_, c, r)) = have.iter().find(|h| h.0 == u.0) {
                        if last.get(&c).map_or(false, |&l| l >= ii) {
                            out.insert((al.inst_pos(bi, ii), u.0), r);
                        }
                    }
                }
                if matches!(i, Inst::Call(..)) {
                    have.clear();
                }
                have.retain(|h| !ds.iter().any(|d| d.0 == h.0 || d.0 == h.1));
            }
            if let Inst::Mov(d, s2) = *i {
                if d != s2 && f.vregs[d.0 as usize] == Cls::I && f.vregs[s2.0 as usize] == Cls::I {
                    match (al.loc[d.0 as usize], al.loc[s2.0 as usize]) {
                        (Loc::Reg(r), Loc::Stack(_)) => have.push((s2.0, d.0, r)),
                        (Loc::Stack(_), Loc::Reg(r)) => have.push((d.0, s2.0, r)),
                        _ => {}
                    }
                }
            }
        }
    }
    out
}

pub fn sink_compares(f: &Func) -> Option<Func> {
    let mut g: Option<Func> = None;
    for bi in 0..f.blocks.len() {
        let n = f.blocks[bi].insts.len();
        let mut i = 0;
        while i < n {
            let b = g.as_ref().map_or(&f.blocks[bi], |x| &x.blocks[bi]);
            let c = match b.insts[i] {
                Inst::Sel(_, c, _, _) => c,
                _ => {
                    i += 1;
                    continue;
                }
            };
            // the compare defining c earlier in this block
            let j = match (0..i).rev().find(|k| {
                let mut d = false;
                for_defs(&b.insts[*k], |x| d |= x == c);
                d
            }) {
                Some(j) if j + 1 < i => j,
                _ => {
                    i += 1;
                    continue;
                }
            };
            let ops: Vec<VReg> = match b.insts[j] {
                Inst::ICmp(_, _, d, x, y) if d == c => vec![x, y],
                Inst::ICmpI(_, _, d, x, _) if d == c => vec![x],
                _ => {
                    i += 1;
                    continue;
                }
            };
            let blocked = (j + 1..i).any(|k| {
                let mut bad = false;
                for_uses(&b.insts[k], |u| bad |= u == c);
                for_defs(&b.insts[k], |d| bad |= d == c || ops.contains(&d));
                bad
            });
            if blocked {
                i += 1;
                continue;
            }
            let gf = g.get_or_insert_with(|| f.clone());
            let blk = &mut gf.blocks[bi];
            let inst = blk.insts.remove(j);
            let p = blk.pos.remove(j);
            blk.insts.insert(i - 1, inst);
            blk.pos.insert(i - 1, p);
            i += 1;
        }
    }
    g
}

/// The blocks whose start reads the running thread's domain base for a function that names domain
/// symbols (`SymClass::Domain`), in block order: per block that names one, that block lifted to the
/// shallowest loop depth on its chain of dominators (the base is the same on every iteration, so
/// it is read outside the loops that hold the uses), minus those another read dominates. Paths that
/// never name a domain symbol never read the base: html_parse's `process_entity` names an entity
/// table only after it has seen a `&`, and a `Result` fn names its error statics only on the
/// error returns (read_pb_varint read the base on its hot 1-byte path, at the uses' common
/// dominator). Empty: the function names no domain symbol.
pub fn domain_base_blocks(f: &Func, sym_class: u64, reach: &[bool]) -> Vec<usize> {
    if sym_class == 0 {
        return Vec::new();
    }
    let uses: Vec<usize> = f
        .blocks
        .iter()
        .enumerate()
        .filter(|(bi, b)| reach[*bi] && b.insts.iter().any(|i| matches!(i, Inst::Addr(_, s) if SymClass::of(sym_class, s.id) == SymClass::Domain)))
        .map(|(bi, _)| bi)
        .collect();
    if uses.is_empty() {
        return Vec::new();
    }
    let idom = crate::dom::idoms(f);
    // the dominator chain of a block, block first, entry last
    let chain = |mut b: usize| {
        let mut c = vec![b];
        while b != 0 {
            b = idom[b] as usize;
            c.push(b);
        }
        c
    };
    let depth = loop_depths(f, reach);
    let mut reads: Vec<usize> = Vec::new();
    let mut chains: Vec<Vec<usize>> = Vec::new();
    for &u in &uses {
        // from the use's block up to the entry: the shallowest loop depth, the latest on a tie
        let c = chain(u);
        let mut best = c[0];
        for &b in &c {
            if depth[b] < depth[best] {
                best = b;
            }
        }
        if !reads.contains(&best) {
            reads.push(best);
            chains.push(chain(best));
        }
    }
    // a read another read dominates (on its dominator chain) is redundant
    let keep: Vec<bool> = (0..reads.len()).map(|k| !(0..reads.len()).any(|j| j != k && chains[k][1..].contains(&reads[j]))).collect();
    let mut out: Vec<usize> = reads.iter().zip(&keep).filter(|x| *x.1).map(|x| *x.0).collect();
    out.sort_unstable();
    out
}
