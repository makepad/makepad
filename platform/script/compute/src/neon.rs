//! NEON ×4 backend: a kernel's element loop compiled to run four elements
//! per iteration (masked SPMD), bit-identical to the scalar backend and the
//! interpreter.
//!
//! Every AIR value lives in a 128-bit vector register, one element per
//! lane, holding exactly the bits the scalar backend would hold for that
//! element (bools included: 0/1 words, compared as raw words). Lane-wise
//! IEEE ops are the scalar ops (no fusion but AIR's explicit `Fma`, the
//! same FPCR), integer division
//! and non-power-of-two wraps run per lane through the scalar sequences,
//! and every memory access clamps per lane exactly like scalar code.
//!
//! Control flow is analysed once for **uniformity**: a value is uniform
//! when every lane is known to hold the same bits (constants, ctx words,
//! loads at uniform addresses, ops of uniform values). A branch on a
//! uniform condition is a real branch; a branch on a varying one runs both
//! sides under an execution mask (`v28`), merging the masks afterwards. A
//! loop whose lanes can leave at different iterations keeps per-loop
//! `break` and `continue` masks; a loop whose exits are uniform is a plain
//! loop. Variables written while not every lane runs are blended into
//! their old value; stores write only the running lanes, in lane order.
//!
//! The element loop itself is recognised in the shape `lower_kernel`
//! emits: its counter holds the lanes' element offsets `[i, i+1, i+2,
//! i+3]` and steps by 4, and its `i >= n` exit is uniform because the
//! runtime calls this code with `n` a multiple of 4 (the tail runs on the
//! scalar code). Four elements run at once, so only kernels proven
//! element-local and non-overlapping may use it: the caller checks
//! `parallel_safe` and the output capacity first. [`compile`] declines
//! (returns None) whatever it does not vectorize, and the scalar code runs
//! instead: audio I/O, state, f64, reductions (a ctx word both read and
//! written per element), large frames, or any other program shape.

use crate::arm64::{allocate_with, enc::*, Code, Ent, Loc};
use crate::ir::{Bin, Block, Cmp, Op, Program, Region, Stmt, Un, Val, Var};
use crate::spmd::{analyse, bool_masks, id, shape, steps, supported, Info, Shape};
use std::collections::{HashMap, HashSet};

/// Vector register pool (v28 is the execution mask, v29..v31 scratch).
const V_POOL: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 8, 9, 10, 11, 12, 13, 14, 15];
/// Loop counters (x9..x14 and x15..x17 are codegen scratch).
const G_POOL: &[u8] = &[19, 20, 21, 22, 23, 24, 25, 26, 27, 28];
const VM: u8 = 28;
const VS0: u8 = 31;
const VS1: u8 = 30;
const VS2: u8 = 29;

// =========================================================================
// Vector encodings (4S unless noted)
// =========================================================================

mod v {
    pub fn r3(base: u32, d: u8, n: u8, m: u8) -> u32 {
        base | (m as u32) << 16 | (n as u32) << 5 | d as u32
    }
    pub fn r2(base: u32, d: u8, n: u8) -> u32 {
        base | (n as u32) << 5 | d as u32
    }
    pub const FADD: u32 = 0x4E20_D400;
    pub const FSUB: u32 = 0x4EA0_D400;
    pub const FMUL: u32 = 0x6E20_DC00;
    pub const FDIV: u32 = 0x6E20_FC00;
    /// vd += vn * vm / vd -= vn * vm (fused).
    pub const FMLA: u32 = 0x4E20_CC00;
    pub const FMLS: u32 = 0x4EA0_CC00;
    /// vd += vn * vm (i32 lanes).
    pub const MLA: u32 = 0x4EA0_9400;
    pub const FCMEQ: u32 = 0x4E20_E400;
    pub const FCMGE: u32 = 0x6E20_E400;
    pub const FCMGT: u32 = 0x6EA0_E400;
    pub const ADD: u32 = 0x4EA0_8400;
    pub const SUB: u32 = 0x6EA0_8400;
    pub const MUL: u32 = 0x4EA0_9C00;
    pub const AND: u32 = 0x4E20_1C00;
    pub const ORR: u32 = 0x4EA0_1C00;
    pub const EOR: u32 = 0x6E20_1C00;
    pub const BIC: u32 = 0x4E60_1C00;
    /// d = d ? n : m (bitwise).
    pub const BSL: u32 = 0x6E60_1C00;
    /// d = m ? n : d (bitwise).
    pub const BIT: u32 = 0x6EA0_1C00;
    /// d = m ? d : n (bitwise).
    pub const BIF: u32 = 0x6EE0_1C00;
    pub const CMEQ: u32 = 0x6EA0_8C00;
    pub const CMGT: u32 = 0x4EA0_3400;
    pub const CMGE: u32 = 0x4EA0_3C00;
    pub const CMTST: u32 = 0x4EA0_8C00;
    pub const SSHL: u32 = 0x4EA0_4400;
    pub const USHL: u32 = 0x6EA0_4400;
    pub const FABS: u32 = 0x4EA0_F800;
    pub const FNEG: u32 = 0x6EA0_F800;
    pub const FSQRT: u32 = 0x6EA1_F800;
    pub const FRINTM: u32 = 0x4E21_9800;
    pub const FRINTP: u32 = 0x4EA1_8800;
    pub const FRINTZ: u32 = 0x4EA1_9800;
    pub const FRINTA: u32 = 0x6E21_8800;
    pub const FCVTZS: u32 = 0x4EA1_B800;
    pub const SCVTF: u32 = 0x4E21_D800;
    pub const NEG: u32 = 0x6EA0_B800;
    pub const CMEQ0: u32 = 0x4EA0_9800;
    pub const NOT: u32 = 0x6E20_5800;
    pub const USHR31: u32 = 0x6F21_0400;
    /// Shifts by an immediate (1..=31; shl 0..=31).
    pub fn sshr(d: u8, n: u8, sh: u32) -> u32 {
        0x4F00_0400 | (64 - sh) << 16 | (n as u32) << 5 | d as u32
    }
    pub fn ushr(d: u8, n: u8, sh: u32) -> u32 {
        0x6F00_0400 | (64 - sh) << 16 | (n as u32) << 5 | d as u32
    }
    pub fn shl(d: u8, n: u8, sh: u32) -> u32 {
        0x4F00_5400 | (32 + sh) << 16 | (n as u32) << 5 | d as u32
    }
    pub const UMAXV: u32 = 0x6EB0_A800;
    pub const UMINV: u32 = 0x6EB1_A800;
    /// eor3 vd.16b, vn, vm, va (SHA3: every M-series core).
    pub fn eor3(d: u8, n: u8, m: u8, a: u8) -> u32 {
        0xCE00_0000 | (m as u32) << 16 | (a as u32) << 10 | (n as u32) << 5 | d as u32
    }
    /// By-element multiplies: vd = vn * vm.s[lane] (and accumulating).
    pub const FMUL_E: u32 = 0x4F80_9000;
    pub const FMLA_E: u32 = 0x4F80_1000;
    pub const FMLS_E: u32 = 0x4F80_5000;
    pub const MUL_E: u32 = 0x4F80_8000;
    pub const MLA_E: u32 = 0x6F80_0000;
    pub fn by_elem(base: u32, d: u8, n: u8, m: u8, lane: u32) -> u32 {
        base | (lane & 1) << 21 | (m as u32) << 16 | (lane >> 1) << 11 | (n as u32) << 5 | d as u32
    }
    /// zip1/zip2 of 32-bit lanes and of 64-bit halves.
    pub const ZIP1S: u32 = 0x4E80_3800;
    pub const ZIP2S: u32 = 0x4E80_7800;
    pub const ZIP1D: u32 = 0x4EC0_3800;
    pub const ZIP2D: u32 = 0x4EC0_7800;
    /// tbl vd.16b, {vn.16b .. vn+3.16b} / {vn, vn+1}, vm.16b
    pub const TBL4: u32 = 0x4E00_6000;
    pub const TBL2: u32 = 0x4E00_2000;
    /// ld2/ld3/ld4 {vt.4s ..}, [xn] (de-interleaving).
    pub const LD2: u32 = 0x4C40_8800;
    pub const LD3: u32 = 0x4C40_4800;
    pub const LD4: u32 = 0x4C40_0800;
    /// dup vd.4s, vn.s[0]
    pub fn dup_lane0(d: u8, n: u8) -> u32 {
        0x4E04_0400 | (n as u32) << 5 | d as u32
    }
    /// smull vd.2d, vn.2s, vm.2s / smull2 (upper halves).
    pub const SMULL: u32 = 0x0EA0_C000;
    pub const SMULL2: u32 = 0x4EA0_C000;
    pub const UZP2: u32 = 0x4E80_5800;
    pub const CMLT0: u32 = 0x4EA0_A800;
    pub const DUP_W: u32 = 0x4E04_0C00;
    pub fn ins_w(d: u8, lane: u32, n: u8) -> u32 {
        0x4E00_1C00 | ((lane << 3) | 4) << 16 | (n as u32) << 5 | d as u32
    }
    pub fn umov_w(d: u8, n: u8, lane: u32) -> u32 {
        0x0E00_3C00 | ((lane << 3) | 4) << 16 | (n as u32) << 5 | d as u32
    }
    pub fn mov(d: u8, n: u8) -> u32 {
        r3(ORR, d, n, n)
    }
    pub fn movi0(d: u8) -> u32 {
        0x6F00_E400 | d as u32
    }
    /// ldr/str q, [xn, #imm] (imm a multiple of 16 below 65536).
    pub fn ldst_q(load: bool, t: u8, n: u8, off: u32) -> u32 {
        (if load { 0x3DC0_0000 } else { 0x3D80_0000 }) | (off / 16) << 10 | (n as u32) << 5 | t as u32
    }
    /// ld1 {vt.s}[lane], [xn], xm (post-index by xm).
    pub fn ld1_lane_post(t: u8, lane: u32, n: u8, m: u8) -> u32 {
        0x0DC0_8000 | (lane >> 1) << 30 | (m as u32) << 16 | (lane & 1) << 12 | (n as u32) << 5 | t as u32
    }
    /// st1 {vt.s}[lane], [xn], xm (post-index by xm).
    pub fn st1_lane_post(t: u8, lane: u32, n: u8, m: u8) -> u32 {
        0x0D80_8000 | (lane >> 1) << 30 | (m as u32) << 16 | (lane & 1) << 12 | (n as u32) << 5 | t as u32
    }
    /// ld1r {vt.4s}, [xn]
    pub fn ld1r(t: u8, n: u8) -> u32 {
        0x4D40_C800 | (n as u32) << 5 | t as u32
    }
}

// Scalar helpers not in `enc`.
fn sdiv(d: u8, n: u8, m: u8) -> u32 {
    SDIV | (m as u32) << 16 | (n as u32) << 5 | d as u32
}
/// d = a - n * m
fn msub(d: u8, n: u8, m: u8, a: u8) -> u32 {
    0x1B00_8000 | (m as u32) << 16 | (a as u32) << 10 | (n as u32) << 5 | d as u32
}
/// add xd, xn, xm (64-bit)
fn add_x(d: u8, n: u8, m: u8) -> u32 {
    0x8B00_0000 | (m as u32) << 16 | (n as u32) << 5 | d as u32
}
/// add xd, xn, xm, lsl #sh (64-bit)
fn add_x_lsl(d: u8, n: u8, m: u8, sh: u32) -> u32 {
    0x8B00_0000 | (m as u32) << 16 | sh << 10 | (n as u32) << 5 | d as u32
}
/// add xd, xn|sp, #imm (64-bit, imm < 4096)
fn add_xi(d: u8, n: u8, imm: u32) -> u32 {
    0x9100_0000 | imm << 10 | (n as u32) << 5 | d as u32
}
/// sub xd, xn, #imm (64-bit)
fn sub_xi(d: u8, n: u8, imm: u32) -> u32 {
    0xD100_0000 | imm << 10 | (n as u32) << 5 | d as u32
}
/// cmp xn, xm (64-bit)
fn cmp_x(n: u8, m: u8) -> u32 {
    0xEB00_001F | (m as u32) << 16 | (n as u32) << 5
}
/// csel xd, xn, xm, cond (64-bit)
fn csel_x(d: u8, n: u8, m: u8, cond: u8) -> u32 {
    0x9A80_0000 | (m as u32) << 16 | (cond as u32) << 12 | (n as u32) << 5 | d as u32
}
/// ldr/str w, [xn, xm, lsl #2]
fn ldst_w_reg(load: bool, t: u8, n: u8, m: u8) -> u32 {
    (if load { 0xB860_7800 } else { 0xB820_7800 }) | (m as u32) << 16 | (n as u32) << 5 | t as u32
}
/// ldr/str w, [xn] (no offset)
fn ldst_w(load: bool, t: u8, n: u8) -> u32 {
    (if load { 0xB940_0000 } else { 0xB900_0000 }) | (n as u32) << 5 | t as u32
}

// =========================================================================
// NEON-specific analysis (the shared part is in crate::spmd)
// =========================================================================

/// The small table (base word, 8 or 16 words) whose varying reads in the
/// element body are the most (at least two), with offsets proven inside it.
fn small_table(body: &Block, info: &Info, bounds: &[Option<u32>]) -> Option<(u32, u32)> {
    fn walk(b: &Block, info: &Info, bounds: &[Option<u32>], n: &mut HashMap<(u32, u32), u32>) {
        for s in b {
            match s {
                Stmt::Def(_, Op::Load { region: Region::Shared, base, extent, off: Some(o) })
                    if (*extent == 8 || *extent == 16) && info.vval[o.0 as usize] && bounds[o.0 as usize].is_some_and(|x| x < *extent) =>
                {
                    *n.entry((*base, *extent)).or_default() += 1;
                }
                Stmt::If(_, t, e) => {
                    walk(t, info, bounds, n);
                    walk(e, info, bounds, n);
                }
                Stmt::Loop { body, .. } => walk(body, info, bounds, n),
                _ => {}
            }
        }
    }
    let mut n = HashMap::new();
    walk(body, info, bounds, &mut n);
    n.into_iter().filter(|(_, c)| *c >= 2).max_by_key(|((b, e), c)| (*c, std::cmp::Reverse(*b), *e)).map(|(k, _)| k)
}

/// A copy of `p` for register allocation without the element counter's
/// step (`Set(i, next)` and next's definition when nothing else reads it),
/// and next's value id.
fn without_step(p: &Program, sh: &Shape) -> (Program, Option<u32>) {
    let mut q = p.clone();
    let Some(Stmt::Loop { body, .. }) = q.body.last_mut() else { return (q, None) };
    let Some(Stmt::Set(i, next)) = body.last().cloned() else { return (q, None) };
    if i != sh.i || use_counts(p)[next.0 as usize] != 1 {
        return (q, None);
    }
    body.pop();
    body.retain(|s| !matches!(s, Stmt::Def(v, _) if *v == next));
    (q, Some(next.0))
}

/// The negated constants c of right-shift amounts x + c the emitter's
/// pair fusion will subtract from (most used first).
fn shift_constants(p: &Program, bounds: &[Option<u32>]) -> Vec<u32> {
    let uses = use_counts(p);
    let mut consts: HashMap<u32, i32> = HashMap::new();
    let mut count: HashMap<u32, u32> = HashMap::new();
    fn walk(b: &Block, uses: &[u32], bounds: &[Option<u32>], consts: &mut HashMap<u32, i32>, count: &mut HashMap<u32, u32>) {
        for (k, s) in b.iter().enumerate() {
            match s {
                Stmt::Def(v, Op::ConstI(c)) => {
                    consts.insert(v.0, *c);
                }
                Stmt::Def(t, Op::Bin(Bin::AddI, _, kk)) => {
                    if let (Some(c), Some(Stmt::Def(_, Op::Bin(Bin::ShrUI | Bin::ShrI, _, amt)))) = (consts.get(&kk.0), b.get(k + 1)) {
                        if amt == t && uses[t.0 as usize] == 1 && bounds[t.0 as usize].is_some_and(|m| m < 32) {
                            *count.entry(c.wrapping_neg() as u32).or_default() += 1;
                        }
                    }
                }
                Stmt::If(_, t, e) => {
                    walk(t, uses, bounds, consts, count);
                    walk(e, uses, bounds, consts, count);
                }
                Stmt::Loop { body, .. } => walk(body, uses, bounds, consts, count),
                _ => {}
            }
        }
    }
    walk(&p.body, &uses, bounds, &mut consts, &mut count);
    let mut v: Vec<(u32, u32)> = count.into_iter().filter(|(_, n)| *n >= 2).collect();
    v.sort_by_key(|(b, n)| (std::cmp::Reverse(*n), *b));
    v.into_iter().map(|(b, _)| b).collect()
}

/// How many times each value is read.
fn use_counts(p: &Program) -> Vec<u32> {
    let mut n = vec![0u32; p.vals.len()];
    fn walk(b: &Block, n: &mut Vec<u32>) {
        for s in b {
            let us: Vec<Val> = match s {
                Stmt::Def(_, op) => crate::ir::op_uses(op),
                Stmt::Set(_, x) => vec![*x],
                Stmt::Store { off, val, .. } => off.iter().copied().chain([*val]).collect(),
                Stmt::Out { idx, val, .. } => vec![*idx, *val],
                Stmt::If(c, t, e) => {
                    walk(t, n);
                    walk(e, n);
                    vec![*c]
                }
                Stmt::Loop { body, .. } => {
                    walk(body, n);
                    vec![]
                }
                Stmt::CallHost { args, slices, .. } => args.iter().copied().chain(slices.iter().flat_map(|x| [x.off, x.len])).collect(),
                _ => vec![],
            };
            for u in us {
                n[u.0 as usize] += 1;
            }
        }
    }
    walk(&p.body, &mut n);
    n
}

/// Picks constants read only as multiplicands (either side of MulF/MulI,
/// a or b of a fused multiply-add), most used first, up to four packed
/// registers taken from the end of `pool`: value -> (register, lane), and
/// each register's four words for the prologue.
fn pack_constants(p: &Program, pool: &mut Vec<u8>) -> (HashMap<u32, (u8, u32, u32)>, Vec<(u8, [u32; 4])>) {
    let mut konst: HashMap<u32, u32> = HashMap::new();
    let mut ok: HashMap<u32, u32> = HashMap::new();
    let mut bad: HashSet<u32> = HashSet::new();
    fn walk(b: &Block, konst: &mut HashMap<u32, u32>, ok: &mut HashMap<u32, u32>, bad: &mut HashSet<u32>) {
        for s in b {
            let (mults, others): (Vec<Val>, Vec<Val>) = match s {
                Stmt::Def(v, op) => {
                    match *op {
                        Op::ConstF(x) => {
                            konst.insert(v.0, x.to_bits());
                        }
                        Op::ConstI(x) => {
                            konst.insert(v.0, x as u32);
                        }
                        _ => {}
                    }
                    match *op {
                        Op::Bin(Bin::MulF | Bin::MulI, a, b) => (vec![a, b], vec![]),
                        Op::Fma(_, a, b, c) => (vec![a, b], vec![c]),
                        _ => (vec![], crate::ir::op_uses(op)),
                    }
                }
                Stmt::Set(_, x) => (vec![], vec![*x]),
                Stmt::Store { off, val, .. } => (vec![], off.iter().copied().chain([*val]).collect()),
                Stmt::Out { idx, val, .. } => (vec![], vec![*idx, *val]),
                Stmt::If(c, t, e) => {
                    walk(t, konst, ok, bad);
                    walk(e, konst, ok, bad);
                    (vec![], vec![*c])
                }
                Stmt::Loop { body, .. } => {
                    walk(body, konst, ok, bad);
                    (vec![], vec![])
                }
                Stmt::CallHost { args, slices, .. } => (vec![], args.iter().copied().chain(slices.iter().flat_map(|x| [x.off, x.len])).collect()),
                _ => (vec![], vec![]),
            };
            for m in mults {
                *ok.entry(m.0).or_default() += 1;
            }
            for o in others {
                bad.insert(o.0);
            }
        }
    }
    walk(&p.body, &mut konst, &mut ok, &mut bad);
    let mut cands: Vec<(u32, u32)> = ok.into_iter().filter(|(v, _)| konst.contains_key(v) && !bad.contains(v)).collect();
    // A register saves three when it holds four; pack in fours only.
    cands.sort_by_key(|(v, n)| (std::cmp::Reverse(*n), *v));
    let regs = (cands.len() / 4).min(4);
    let mut packed = HashMap::new();
    let mut packs = Vec::new();
    for r in 0..regs {
        let Some(reg) = pool.pop() else { break };
        let mut words = [0u32; 4];
        for l in 0..4 {
            let (v, _) = cands[r * 4 + l];
            words[l] = konst[&v];
            packed.insert(v, (reg, l as u32, konst[&v]));
        }
        packs.push((reg, words));
    }
    (packed, packs)
}

// =========================================================================
// Code generation
// =========================================================================

#[derive(Clone, Copy)]
enum Fix {
    B,
    BCond,
    Cbz,
    /// ldr q, <literal>: the target is a constant pool entry.
    Lit,
}

struct LoopCtx {
    masked: bool,
    top: usize,
    exit: usize,
    iter_end: usize,
    /// Mask slots (byte offsets from sp) of a masked loop.
    brk: u32,
    cont: u32,
}

struct Em {
    info: Info,
    code: Vec<u32>,
    locs: HashMap<Ent, Loc>,
    labels: Vec<Option<usize>>,
    fixups: Vec<(usize, usize, Fix)>,
    loops: Vec<LoopCtx>,
    loop_id: u32,
    /// Byte offsets from sp: the mask slots and the per-lane frame.
    mask_base: u32,
    frame_base: u32,
    /// Construct depth, for mask slots.
    depth: u32,
    /// A 16-byte slot saving the mask around a four-register load.
    group_slot: u32,
    /// A small table (base word, words: 8 or 16) held in v24.. for the
    /// whole call: lane reads from it are one TBL.
    table: Option<(u32, u32)>,
    /// Elements per iteration (4, or 8 for a widened program).
    lanes: u32,
    /// Where the last label was bound (a join: registers may differ).
    last_bind: Option<usize>,
    /// Uses of every value (for fusing a definition into its one user).
    uses: Vec<u32>,
    /// The element counter's IR step value (not emitted).
    step_def: Option<u32>,
    /// Bool values held as lane masks (all ones / zero) rather than the
    /// 0/1 words the scalar code holds (see `bool_masks`).
    mask: Vec<bool>,
    /// Bool variables that hold masks.
    var_mask: Vec<bool>,
    /// Every value's type.
    p_vals: Vec<crate::ir::Ty>,
    /// Constants the emitter itself needs in the loop, splatted once into
    /// registers taken from the pool: (bits, register).
    resident: Vec<(u32, u8)>,
    /// Constants used only as multiplicands, packed four to a register:
    /// value -> (register, lane), read with by-element FMUL/FMLA/MUL/MLA.
    packed: HashMap<u32, (u8, u32, u32)>,
    /// The execution mask is statically all lanes.
    full: bool,
    i: Var,
    /// Proven upper bounds of offsets (as in the scalar backend).
    bounds: Vec<Option<u32>>,
    /// Per value: lanes are `lane0 + l * step` (i32, from the element index).
    step: Vec<Option<i64>>,
    /// ConstI values.
    consts: Vec<Option<i32>>,
    /// The constant pool: four words per entry, and the label of each entry.
    pool: Vec<([u32; 4], usize)>,
}

impl Em {
    fn e(&mut self, w: u32) {
        self.code.push(w);
    }

    fn label(&mut self) -> usize {
        self.labels.push(None);
        self.labels.len() - 1
    }

    fn bind(&mut self, l: usize) {
        self.labels[l] = Some(self.code.len());
        self.last_bind = Some(self.code.len());
    }

    fn jump(&mut self, kind: Fix, word: u32, l: usize) {
        self.fixups.push((self.code.len(), l, kind));
        self.e(word);
    }

    fn mov_imm(&mut self, d: u8, v: u32) {
        self.e(movz(d, v & 0xFFFF, 0));
        if v >> 16 != 0 {
            self.e(movk(d, v >> 16, 1));
        }
    }

    /// x(d) = sp + byte offset.
    fn sp_addr(&mut self, d: u8, off: u32) {
        if off < 4096 {
            self.e(add_xi(d, 31, off));
        } else {
            self.mov_imm(d, off);
            // add xd, sp, xd (extended register, UXTX)
            self.e(0x8B20_63E0 | (d as u32) << 16 | d as u32);
        }
    }

    fn ldst_sp_q(&mut self, load: bool, t: u8, off: u32) {
        if off < 65536 {
            self.e(v::ldst_q(load, t, 31, off));
        } else {
            self.sp_addr(9, off);
            self.e(v::ldst_q(load, t, 9, 0));
        }
    }

    fn loc(&self, e: Ent) -> Loc {
        self.locs[&e]
    }

    /// The vector register holding `e` (a spilled one loads into `scratch`).
    fn src(&mut self, e: Ent, scratch: u8) -> u8 {
        match self.loc(e) {
            Loc::Reg(r) => r,
            Loc::Stack(off) => {
                // The instruction just emitted stored (or loaded) this slot
                // from (into) the same register, and nothing branches in
                // between: the register already holds it.
                if off < 65536 && self.last_bind != Some(self.code.len()) {
                    if let Some(&last) = self.code.last() {
                        if last == v::ldst_q(false, scratch, 31, off) || last == v::ldst_q(true, scratch, 31, off) {
                            return scratch;
                        }
                    }
                }
                self.ldst_sp_q(true, scratch, off);
                scratch
            }
        }
    }

    /// d = m ? x : y (m a mask register), in one BSL/BIT/BIF when d is
    /// one of the three, else a copy of the mask and BSL.
    fn select_into(&mut self, d: u8, m: u8, x: u8, y: u8) {
        if d == m {
            self.e(v::r3(v::BSL, d, x, y));
        } else if d == y {
            self.e(v::r3(v::BIT, d, x, m));
        } else if d == x {
            self.e(v::r3(v::BIF, d, y, m));
        } else {
            self.e(v::mov(d, m));
            self.e(v::r3(v::BSL, d, x, y));
        }
    }

    fn is_bool(&self, v: Val) -> bool {
        self.p_vals[v.0 as usize] == crate::ir::Ty::Bool
    }

    /// `v` in a register as the scalar code's word when `word` is set (a
    /// mask becomes 0/1 in `scratch`), else as held.
    fn vsrc_word(&mut self, v: Val, scratch: u8, word: bool) -> u8 {
        let r = self.vsrc(v, scratch);
        if word && self.mask[v.0 as usize] {
            self.e(v::r2(v::USHR31, scratch, r));
            return scratch;
        }
        r
    }

    fn vsrc(&mut self, v: Val, scratch: u8) -> u8 {
        if let Some(&(_, _, bits)) = self.packed.get(&v.0) {
            // A packed constant read other than as a multiplicand.
            self.splat(scratch, bits);
            return scratch;
        }
        self.src(Ent::Val(v.0), scratch)
    }

    fn dst(&self, e: Ent) -> u8 {
        match self.loc(e) {
            Loc::Reg(r) => r,
            Loc::Stack(_) => VS0,
        }
    }

    fn done(&mut self, e: Ent, r: u8) {
        if let Loc::Stack(off) = self.loc(e) {
            self.ldst_sp_q(false, r, off);
        }
    }

    fn uniform(&self, v: Val) -> bool {
        !self.info.vval[v.0 as usize]
    }

    /// w(d) = lane `lane` of value v.
    fn lane(&mut self, d: u8, v: Val, lane: u32) {
        let r = self.vsrc(v, VS1);
        self.e(v::umov_w(d, r, lane));
    }

    /// Branches to `l` when the mask register `m` has no lane set.
    fn cbz_none(&mut self, m: u8, l: usize) {
        self.e(v::r2(v::UMAXV, VS2, m));
        self.e(fp2(FMOV_WS, 15, VS2));
        self.jump(Fix::Cbz, 0x3400_0000 | 15, l);
    }

    fn slot(&self, k: u32) -> u32 {
        self.mask_base + (2 * self.depth + k) * 16
    }

    // -- memory ---------------------------------------------------------------

    /// The offset proven below `extent` (no clamp needed).
    fn proven(&self, off: Val, extent: u32) -> bool {
        self.bounds[off.0 as usize].is_some_and(|b| b < extent)
    }

    /// x(d) = the clamped word index of a non-buffer region access for one
    /// lane (or the uniform offset): base + min(off, extent - 1), with the
    /// offset vector in register `ro`.
    fn region_index(&mut self, d: u8, base: u32, extent: u32, off: Option<(Val, u8)>, lane: u32) {
        match off {
            None => self.mov_imm(d, base),
            Some((o, ro)) => {
                self.e(v::umov_w(d, ro, lane));
                if !self.proven(o, extent) {
                    self.mov_imm(17, extent - 1);
                    self.e(cmp(d, 17));
                    self.e(csel(d, d, 17, LS));
                }
                if base > 0 {
                    if base < 4096 {
                        self.e(add_imm(d, d, base));
                    } else {
                        self.mov_imm(17, base);
                        self.e(ADD | 17 << 16 | (d as u32) << 5 | d as u32);
                    }
                }
            }
        }
    }

    /// Host buffer k: x10 = its base pointer, x11 = its last word index.
    fn buf_regs(&mut self, k: u8) {
        self.e(ldr_x(10, 3, k as u32 * 16));
        self.e(ldr_x(11, 3, k as u32 * 16 + 8));
        self.e(sub_xi(11, 11, 1));
    }

    /// x12 = min(base + lane offset, len - 1), in 64 bits (no wrap).
    fn buf_index(&mut self, base: u32, ro: Option<u8>, lane: u32) {
        match ro {
            Some(ro) => self.e(v::umov_w(12, ro, lane)),
            None => self.e(mov(12, 31)),
        }
        if base > 0 {
            if base < 4096 {
                self.e(add_xi(12, 12, base));
            } else {
                self.mov_imm(13, base);
                self.e(add_x(12, 12, 13));
            }
        }
        self.e(cmp_x(12, 11));
        self.e(csel_x(12, 12, 11, LS));
    }

    /// For an offset whose lanes are `o, o+1, o+2, o+3` (words `step`
    /// apart per lane with `step` accesses grouped): branches to `slow`
    /// unless all `4 * width` words from base + o lie inside the buffer;
    /// else x12 = base + o. Needs buf_regs.
    fn contiguous_or(&mut self, base: u32, ro: u8, width: u32, slow: usize) {
        self.e(v::umov_w(12, ro, 0));
        // x13 = base + o + 4 width - 1 (64 bits): the last word touched.
        let span = base as u64 + 4 * width as u64 - 1;
        if span > i32::MAX as u64 {
            // Past any buffer (bound buffers hold fewer than 2^31 words).
            self.jump(Fix::B, 0x1400_0000, slow);
            return;
        }
        if span < 4096 {
            self.e(add_xi(13, 12, span as u32));
        } else {
            self.mov_imm(13, span as u32);
            self.e(add_x(13, 12, 13));
        }
        self.e(cmp_x(13, 11));
        // b.hi slow: past the last word (lanes would clamp one by one).
        self.jump(Fix::BCond, 0x5400_0000 | 8, slow);
        if base > 0 {
            if base < 4096 {
                self.e(add_xi(12, 12, base));
            } else {
                self.mov_imm(13, base);
                self.e(add_x(12, 12, 13));
            }
        }
    }

    /// For an offset whose lanes are `o + l * step` (step > 1): branches to
    /// `slow` unless every lane's word lies inside the buffer; else x9 = the
    /// address of lane 0's word and x13 = step * 4 (the four lanes are then
    /// one lane load or store each, post-indexed). Needs buf_regs.
    fn strided_or(&mut self, base: u32, ro: u8, step: u32, slow: usize) {
        self.e(v::umov_w(12, ro, 0));
        // The last lane's word, in 64 bits: lanes increase, so the last one
        // inside means all are (a lane that wrapped 32 bits is past any
        // buffer and takes the slow path).
        let span = base as u64 + 3 * step as u64;
        if span > i32::MAX as u64 {
            self.jump(Fix::B, 0x1400_0000, slow);
            return;
        }
        if span < 4096 {
            self.e(add_xi(13, 12, span as u32));
        } else {
            self.mov_imm(13, span as u32);
            self.e(add_x(13, 12, 13));
        }
        self.e(cmp_x(13, 11));
        self.jump(Fix::BCond, 0x5400_0000 | 8, slow);
        if base > 0 {
            if base < 4096 {
                self.e(add_xi(12, 12, base));
            } else {
                self.mov_imm(13, base);
                self.e(add_x(12, 12, 13));
            }
        }
        self.e(add_x_lsl(9, 10, 12, 2));
        self.mov_imm(13, step * 4);
    }

    /// A proven step (lanes `o + l * step`) of a varying offset, step > 1.
    fn stride_of(&self, off: Option<Val>) -> Option<u32> {
        let s = self.step[off?.0 as usize]?;
        (s > 1 && s <= 1 << 16).then_some(s as u32)
    }

    fn load(&mut self, dst: Ent, region: Region, base: u32, extent: u32, off: Option<Val>) {
        let d = self.dst(dst);
        let uniform = off.is_none_or(|o| self.uniform(o));
        let ro = off.map(|o| self.vsrc(o, VS1));
        match region {
            Region::Buf(k) => {
                self.buf_regs(k);
                if uniform {
                    self.buf_index(base, ro, 0);
                    self.e(ldst_w_reg(true, 14, 10, 12));
                    self.e(v::r2(v::DUP_W, d, 14));
                } else {
                    let ro = ro.unwrap();
                    let step1 = off.is_some_and(|o| self.step[o.0 as usize] == Some(1));
                    let (slow, end) = (self.label(), self.label());
                    if step1 {
                        // Four consecutive words: one vector load.
                        self.contiguous_or(base, ro, 1, slow);
                        self.e(add_x_lsl(9, 10, 12, 2));
                        self.e(v::ldst_q(true, VS2, 9, 0));
                        self.jump(Fix::B, 0x1400_0000, end);
                    } else if let Some(st) = self.stride_of(off) {
                        // Records `st` words apart, all inside: a lane load each.
                        self.strided_or(base, ro, st, slow);
                        for l in 0..4 {
                            self.e(v::ld1_lane_post(VS2, l, 9, 13));
                        }
                        self.jump(Fix::B, 0x1400_0000, end);
                    }
                    self.bind(slow);
                    for l in 0..4 {
                        self.buf_index(base, Some(ro), l);
                        self.e(ldst_w_reg(true, 14, 10, 12));
                        self.e(v::ins_w(VS2, l, 14));
                    }
                    self.bind(end);
                    self.e(v::mov(d, VS2));
                }
            }
            Region::Frame => {
                if uniform {
                    self.frame_q_addr(base, extent, off.zip(ro));
                    self.e(v::ldst_q(true, d, 9, 0));
                } else {
                    for l in 0..4 {
                        self.frame_lane_addr(base, extent, off.zip(ro), l);
                        self.e(ldst_w(true, 14, 9));
                        self.e(v::ins_w(VS2, l, 14));
                    }
                    self.e(v::mov(d, VS2));
                }
            }
            Region::Shared if !uniform && self.table.is_some_and(|(tb, w)| base >= tb && base + extent <= tb + w && base < tb + w) && self.proven(off.unwrap(), extent) => {
                // Word o of the table in v24..: byte indices 4o + 0..3.
                let ro = ro.unwrap();
                let (tb, words) = self.table.unwrap();
                self.e(v::r3(v::MUL, VS2, ro, 22));
                self.e(v::r3(v::ADD, VS2, VS2, 23));
                if base > tb {
                    self.splat(VS0, (base - tb).wrapping_mul(0x0404_0404));
                    self.e(v::r3(v::ADD, VS2, VS2, VS0));
                }
                let tbl = if words == 16 { v::TBL4 } else { v::TBL2 };
                self.e(v::r3(tbl, d, 24, VS2));
            }
            Region::Ctx | Region::Shared => {
                let rb = if region == Region::Ctx { 0 } else { 2 };
                if uniform {
                    self.region_index(12, base, extent, off.zip(ro), 0);
                    // x9 = xb + x12 * 4; ld1r
                    self.e(add_x_lsl(9, rb, 12, 2));
                    self.e(v::ld1r(d, 9));
                } else {
                    let (o, ro) = (off.unwrap(), ro.unwrap());
                    // x9 = xb + base * 4, then per lane [x9 + clamp(o) * 4].
                    if base * 4 < 4096 {
                        self.e(add_xi(9, rb, base * 4));
                    } else {
                        self.mov_imm(9, base * 4);
                        self.e(add_x(9, rb, 9));
                    }
                    let proven = self.proven(o, extent);
                    if !proven {
                        self.mov_imm(17, extent - 1);
                    }
                    for l in 0..4 {
                        self.e(v::umov_w(12, ro, l));
                        if !proven {
                            self.e(cmp(12, 17));
                            self.e(csel(12, 12, 17, LS));
                        }
                        self.e(ldst_w_reg(true, 14, 9, 12));
                        self.e(v::ins_w(VS2, l, 14));
                    }
                    self.e(v::mov(d, VS2));
                }
            }
            Region::State => unreachable!("declined"),
        }
        self.done(dst, d);
    }

    /// x9 = the address of the q word (4 lanes) of a frame access, with
    /// lane l's offset (a uniform offset: any lane).
    fn frame_q_addr(&mut self, base: u32, extent: u32, off: Option<(Val, u8)>) {
        self.frame_q_addr_lane(base, extent, off, 0);
    }

    fn frame_q_addr_lane(&mut self, base: u32, extent: u32, off: Option<(Val, u8)>, l: u32) {
        self.region_index(12, base, extent, off, l);
        self.sp_addr(9, self.frame_base);
        self.e(add_x_lsl(9, 9, 12, 4));
    }

    /// x9 = the address of lane l's word of a frame access (word w of lane
    /// l lives at frame + 16 w + 4 l).
    fn frame_lane_addr(&mut self, base: u32, extent: u32, off: Option<(Val, u8)>, l: u32) {
        self.frame_q_addr_lane(base, extent, off, l);
        if l > 0 {
            self.e(add_xi(9, 9, 4 * l));
        }
    }

    /// Skips to `skip` when lane l is not running (nothing when the mask is
    /// statically full).
    fn lane_guard(&mut self, l: u32, skip: usize) {
        if !self.full {
            self.e(v::umov_w(15, VM, l));
            self.jump(Fix::Cbz, 0x3400_0000 | 15, skip);
        }
    }

    fn store(&mut self, region: Region, base: u32, extent: u32, off: Option<Val>, val: Val) {
        let uniform = off.is_none_or(|o| self.uniform(o));
        // Memory holds the scalar code's words (a mask stores as 0/1).
        let rv = self.vsrc_word(val, VS0, true);
        let ro = off.map(|o| self.vsrc(o, VS1));
        match region {
            Region::Frame if uniform => {
                self.frame_q_addr(base, extent, off.zip(ro));
                self.store_q(rv, 9);
            }
            _ => {
                let end = self.label();
                if let Region::Buf(k) = region {
                    self.buf_regs(k);
                    if !uniform && off.is_some_and(|o| self.step[o.0 as usize] == Some(1)) {
                        // Four consecutive words: one vector store.
                        let slow = self.label();
                        self.contiguous_or(base, ro.unwrap(), 1, slow);
                        self.e(add_x_lsl(9, 10, 12, 2));
                        self.store_q(rv, 9);
                        self.jump(Fix::B, 0x1400_0000, end);
                        self.bind(slow);
                    } else if let (false, true, Some(st)) = (uniform, self.full, self.stride_of(off)) {
                        // Records `st` words apart, all inside, every lane
                        // running: a lane store each, in lane order.
                        let slow = self.label();
                        self.strided_or(base, ro.unwrap(), st, slow);
                        for l in 0..4 {
                            self.e(v::st1_lane_post(rv, l, 9, 13));
                        }
                        self.jump(Fix::B, 0x1400_0000, end);
                        self.bind(slow);
                    }
                }
                // Per lane, in lane order: a word two lanes store ends as the
                // later element's, as in scalar order.
                for l in 0..4 {
                    let skip = self.label();
                    self.lane_guard(l, skip);
                    self.e(v::umov_w(14, rv, l));
                    match region {
                        Region::Buf(_) => {
                            self.buf_index(base, ro, l);
                            self.e(ldst_w_reg(false, 14, 10, 12));
                        }
                        Region::Frame => {
                            self.frame_lane_addr(base, extent, off.zip(ro), l);
                            self.e(ldst_w(false, 14, 9));
                        }
                        Region::Ctx => {
                            self.region_index(12, base, extent, off.zip(ro), l);
                            self.e(ldst_w_reg(false, 14, 0, 12));
                        }
                        // Shared tables are read-only in kernels; state is declined.
                        Region::Shared | Region::State => unreachable!("validated / declined"),
                    }
                    self.bind(skip);
                }
                self.bind(end);
            }
        }
    }

    /// Stores the running lanes of `r` to the q word at [x(a)] (the others
    /// keep their memory).
    fn store_q(&mut self, r: u8, a: u8) {
        if self.full {
            self.e(v::ldst_q(false, r, a, 0));
        } else {
            self.e(v::ldst_q(true, VS2, a, 0));
            self.e(v::r3(v::BIT, VS2, r, VM));
            self.e(v::ldst_q(false, VS2, a, 0));
        }
    }

    // -- statements -------------------------------------------------------------

    fn block(&mut self, b: &Block) {
        let mut k = 0;
        while k < b.len() {
            if let Some(n) = self.compare_branch(&b[k..]) {
                k += n;
                continue;
            }
            let n = match match self.tbl_group(&b[k..]) {
                0 => self.gather_group(&b[k..]),
                n => n,
            } {
                0 => match self.store_group(&b[k..]) {
                    0 => self.pair(&b[k..]),
                    n => n,
                },
                n => n,
            };
            if n > 0 {
                k += n;
                continue;
            }
            let s = &b[k];
            self.stmt(s);
            if self.info.escapes.contains(&id(s)) {
                self.full = false;
            }
            k += 1;
        }
    }

    /// A compare of two uniform lanes whose only use is the uniform `if`
    /// right after it (the element loop's exit and cancel tests, param
    /// tests): the compare is made on lane 0 into the flags (FCMP, or CMP /
    /// CBZ from GPRs) and branches, instead of a lane mask moved to a GPR.
    fn compare_branch(&mut self, b: &[Stmt]) -> Option<usize> {
        let (Some(Stmt::Def(c, op)), Some(s @ Stmt::If(ci, t, e))) = (b.first(), b.get(1)) else { return None };
        if ci != c || self.uses[c.0 as usize] != 1 || self.info.div_if.contains(&id(s)) || self.packed.contains_key(&c.0) {
            return None;
        }
        let (float, cc, x, y) = match *op {
            Op::CmpF(cc, x, y) => (true, cc, x, y),
            Op::CmpI(cc, x, y) if !self.is_bool(x) => (false, cc, x, y),
            _ => return None,
        };
        let else_l = self.label();
        let end_l = self.label();
        // The condition code of `x cc y` (signed ints; floats false on NaN),
        // inverted: branch to else when it does not hold.
        let inv: u8 = match (cc, float) {
            (Cmp::Lt, false) => GE,
            (Cmp::Le, false) => GT,
            (Cmp::Gt, false) => LE,
            (Cmp::Ge, false) => LT,
            (Cmp::Lt, true) => 5,  // PL
            (Cmp::Le, true) => 8,  // HI
            (Cmp::Gt, true) => LE,
            (Cmp::Ge, true) => LT,
            (Cmp::Eq, _) => NE,
            (Cmp::Ne, _) => EQ,
        };
        if float {
            let rx = self.vsrc(x, VS0);
            let ry = if y == x { rx } else { self.vsrc(y, VS1) };
            self.e(fcmp(rx, ry));
            self.jump(Fix::BCond, 0x5400_0000 | inv as u32, else_l);
        } else if self.consts[y.0 as usize] == Some(0) && matches!(cc, Cmp::Eq | Cmp::Ne) {
            let rx = self.vsrc(x, VS0);
            self.e(v::umov_w(9, rx, 0));
            // cbnz (x == 0 fails) / cbz (x != 0 fails) to else.
            let op = if cc == Cmp::Eq { 0x3500_0000 } else { 0x3400_0000 };
            self.jump(Fix::Cbz, op | 9, else_l);
        } else {
            let rx = self.vsrc(x, VS0);
            self.e(v::umov_w(9, rx, 0));
            let ry = self.vsrc(y, VS1);
            self.e(v::umov_w(10, ry, 0));
            self.e(cmp(9, 10));
            self.jump(Fix::BCond, 0x5400_0000 | inv as u32, else_l);
        }
        let full = self.full;
        self.block(t);
        if !e.is_empty() {
            self.jump(Fix::B, 0x1400_0000, end_l);
        }
        self.bind(else_l);
        self.full = full;
        self.block(e);
        self.bind(end_l);
        self.full = full;
        if self.info.escapes.contains(&id(s)) {
            self.full = false;
        }
        Some(2)
    }

    /// Two adjacent definitions whose first has no other use, fused into
    /// fewer instructions: `(a ^ b) ^ c` is one EOR3; a right shift by
    /// `x + c` (proven below 32) shifts by `-c - x` computed with one SUB
    /// (a right shift is a left shift by the negated amount). Returns 2
    /// when it emitted both, else 0.
    fn pair(&mut self, b: &[Stmt]) -> usize {
        let (Some(Stmt::Def(t, op1)), Some(Stmt::Def(v, op2))) = (b.first(), b.get(1)) else { return 0 };
        if self.uses[t.0 as usize] != 1 || self.packed.contains_key(&t.0) {
            return 0;
        }
        match (*op1, *op2) {
            (Op::Bin(Bin::XorI, a, a2), Op::Bin(Bin::XorI, x, y)) if (x == *t) != (y == *t) => {
                let c = if x == *t { y } else { x };
                let ra = self.vsrc(a, VS0);
                let rb = self.vsrc(a2, VS1);
                let rc = self.vsrc(c, VS2);
                let d = self.dst(Ent::Val(v.0));
                self.e(v::eor3(d, ra, rb, rc));
                self.done(Ent::Val(v.0), d);
                2
            }
            (Op::Bin(Bin::AddI, x0, k), Op::Bin(sh @ (Bin::ShrUI | Bin::ShrI), s0, amt))
                if amt == *t && s0 != *t && self.consts[k.0 as usize].is_some() && self.bounds[t.0 as usize].is_some_and(|m| m < 32) =>
            {
                let c = self.consts[k.0 as usize].unwrap();
                let rx = self.vsrc(x0, VS0);
                let neg = c.wrapping_neg() as u32;
                let kreg = match self.resident.iter().find(|(b, _)| *b == neg) {
                    Some((_, r)) => *r,
                    None => {
                        self.splat(VS2, neg);
                        VS2
                    }
                };
                self.e(v::r3(v::SUB, VS2, kreg, rx));
                let rs = self.vsrc(s0, VS1);
                let d = self.dst(Ent::Val(v.0));
                self.e(v::r3(if sh == Bin::ShrUI { v::USHL } else { v::SSHL }, d, rs, VS2));
                self.done(Ent::Val(v.0), d);
                2
            }
            _ => 0,
        }
    }

    /// Consecutive stores of words `base .. base + n` (n >= 2) of a host
    /// buffer at one varying offset (a record's fields, an emitted record):
    /// when every lane's words lie inside the buffer (checked at run time
    /// on the largest lane offset), each running lane writes its n words
    /// with post-indexed lane stores, lane after lane; lanes' records never
    /// overlap without clamping, so the words end as in statement order.
    /// Otherwise the stores one by one. Returns the stores handled.
    fn store_group(&mut self, b: &[Stmt]) -> usize {
        let Some(Stmt::Store { region: Region::Buf(kb), base, off: Some(o), .. }) = b.first() else { return 0 };
        let (kb, base, o) = (*kb, *base, *o);
        if self.uniform(o) {
            return 0;
        }
        let mut n = 1;
        while n < b.len() {
            match &b[n] {
                Stmt::Store { region: Region::Buf(r), base: bn, off: Some(on), .. } if *r == kb && *on == o && *bn == base + n as u32 => n += 1,
                _ => break,
            }
        }
        let last = base as u64 + n as u64 - 1;
        if n < 2 || last >= 1 << 31 {
            return 0;
        }
        let (slow, end) = (self.label(), self.label());
        self.buf_regs(kb);
        let ro = self.vsrc(o, VS1);
        // Every lane inside: max(offset) + last <= len - 1 (64 bits).
        self.e(v::r2(v::UMAXV, VS2, ro));
        self.e(fp2(FMOV_WS, 12, VS2));
        if last < 4096 {
            self.e(add_xi(13, 12, last as u32));
        } else {
            self.mov_imm(13, last as u32);
            self.e(add_x(13, 12, 13));
        }
        self.e(cmp_x(13, 11));
        self.jump(Fix::BCond, 0x5400_0000 | 8, slow);
        for l in 0..4 {
            let skip = self.label();
            self.lane_guard(l, skip);
            self.e(v::umov_w(12, ro, l));
            if base > 0 {
                if base < 4096 {
                    self.e(add_xi(12, 12, base));
                } else {
                    self.mov_imm(13, base);
                    self.e(add_x(12, 12, 13));
                }
            }
            self.e(add_x_lsl(9, 10, 12, 2));
            for s in &b[..n] {
                let Stmt::Store { val, .. } = s else { unreachable!() };
                let rv = self.vsrc_word(*val, VS0, true);
                self.e(v::st1_lane_post(rv, l, 9, 31));
            }
            self.bind(skip);
        }
        self.jump(Fix::B, 0x1400_0000, end);
        self.bind(slow);
        for s in &b[..n] {
            self.stmt(s);
        }
        self.bind(end);
        n
    }

    /// Consecutive reads of the resident small table at one varying offset
    /// (a row: `GRAD2[h]`, `GRAD2[h + 1]` once peeled): the byte index is
    /// made once and stepped by 4 bytes per word, one TBL per read.
    fn tbl_group(&mut self, b: &[Stmt]) -> usize {
        let Some((tb, words)) = self.table else { return 0 };
        let inside = |base: u32, extent: u32| base >= tb && base < tb + words && base + extent <= tb + words;
        let Some(Stmt::Def(_, Op::Load { region: Region::Shared, base, extent, off: Some(o) })) = b.first() else { return 0 };
        let (base0, o) = (*base, *o);
        if self.uniform(o) || !inside(base0, *extent) || !self.proven(o, *extent) {
            return 0;
        }
        let mut n = 1;
        while n < b.len() {
            match &b[n] {
                Stmt::Def(_, Op::Load { region: Region::Shared, base, extent, off: Some(on) }) if *on == o && *base == base0 + n as u32 && inside(*base, *extent) && self.proven(o, *extent) => n += 1,
                _ => break,
            }
        }
        if n < 2 {
            return 0;
        }
        let ro = self.vsrc(o, VS1);
        self.e(v::r3(v::MUL, VS2, ro, 22));
        self.e(v::r3(v::ADD, VS2, VS2, 23));
        if base0 > tb {
            self.splat(VS0, (base0 - tb).wrapping_mul(0x0404_0404));
            self.e(v::r3(v::ADD, VS2, VS2, VS0));
        }
        let tbl = if words == 16 { v::TBL4 } else { v::TBL2 };
        for (m, s) in b[..n].iter().enumerate() {
            let Stmt::Def(v, _) = s else { unreachable!() };
            if m > 0 {
                self.e(v::r3(v::ADD, VS2, VS2, 22));
            }
            let e = Ent::Val(v.0);
            let d = self.dst(e);
            self.e(v::r3(tbl, d, 24, VS2));
            self.done(e, d);
        }
        n
    }

    /// Consecutive loads of words `base .. base + n` (n = 2..4) of a table
    /// at one varying offset (a vec2, vec3 or vec4 row read per lane): when
    /// the lanes read consecutive rows (offsets o, o + n, o + 2n, o + 3n,
    /// checked at run time), one de-interleaving ld2/ld3/ld4 of the 4n
    /// words; otherwise the loads one by one. Only offsets proven inside
    /// the table (no clamp) take it. Returns the loads handled (0: none).
    fn gather_group(&mut self, b: &[Stmt]) -> usize {
        let Some(Stmt::Def(_, Op::Load { region: Region::Shared, base, extent, off: Some(o) })) = b.first() else { return 0 };
        let (base, extent, o) = (*base, *extent, *o);
        if self.uniform(o) || !self.proven(o, extent) {
            return 0;
        }
        let mut n = 1;
        while n < 4 && n < b.len() {
            match &b[n] {
                Stmt::Def(_, Op::Load { region: Region::Shared, base: bn, extent: en, off: Some(on) }) if *bn == base + n as u32 && *on == o && self.proven(o, *en) => n += 1,
                _ => break,
            }
        }
        if n < 2 {
            return 0;
        }
        let (slow, end) = (self.label(), self.label());
        let ro = self.vsrc(o, VS1);
        // Lanes o0 + l * n?
        self.e(v::dup_lane0(VS2, ro));
        self.e(v::r3(v::SUB, VS2, ro, VS2));
        let nn = n as u32;
        self.vconst(VS0, [0, nn, 2 * nn, 3 * nn]);
        self.e(v::r3(v::CMEQ, VS2, VS2, VS0));
        self.e(v::r2(v::UMINV, VS2, VS2));
        self.e(fp2(FMOV_WS, 15, VS2));
        self.jump(Fix::Cbz, 0x3400_0000 | 15, slow);
        // x9 = shared + (base + o0) * 4
        self.e(v::umov_w(12, ro, 0));
        if base * 4 < 4096 {
            self.e(add_xi(9, 2, base * 4));
        } else {
            self.mov_imm(9, base * 4);
            self.e(add_x(9, 2, 9));
        }
        self.e(add_x_lsl(9, 9, 12, 2));
        let first = 32 - nn as u8;
        if n == 4 {
            // v28 (the mask) is the first register of the four.
            self.ldst_sp_q(false, VM, self.group_slot);
        }
        self.e(match n {
            2 => v::LD2,
            3 => v::LD3,
            _ => v::LD4,
        } | 9 << 5 | first as u32);
        for (m, s) in b[..n].iter().enumerate() {
            let Stmt::Def(v, _) = s else { unreachable!() };
            let t = first + m as u8;
            let e = Ent::Val(v.0);
            match self.loc(e) {
                Loc::Reg(d) => self.e(v::mov(d, t)),
                Loc::Stack(_) => self.done(e, t),
            }
        }
        if n == 4 {
            self.ldst_sp_q(true, VM, self.group_slot);
        }
        self.jump(Fix::B, 0x1400_0000, end);
        self.bind(slow);
        let dests: Vec<Option<u8>> = b[..n]
            .iter()
            .map(|s| match s {
                Stmt::Def(v, _) => match self.loc(Ent::Val(v.0)) {
                    Loc::Reg(d) if d < VM => Some(d),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        if n == 4 && dests.iter().all(|d| d.is_some()) {
            // Scattered vec4 rows: one q load per lane, then a 4x4
            // transpose into the four components.
            let d: Vec<u8> = dests.into_iter().map(|d| d.unwrap()).collect();
            let ro = self.vsrc(o, VS1);
            for l in 0..4 {
                self.e(v::umov_w(12 + l as u8, ro, l));
            }
            self.ldst_sp_q(false, VM, self.group_slot);
            for l in 0..4u8 {
                self.e(add_x_lsl(9 + (l > 0) as u8 * 0, 2, 12 + l, 2));
                // x9 = shared + o_l * 4; the row is at + base * 4.
                if base * 4 < 4096 {
                    self.e(add_xi(9, 9, base * 4));
                } else {
                    self.mov_imm(10, base * 4);
                    self.e(add_x(9, 9, 10));
                }
                self.e(v::ldst_q(true, VM + l, 9, 0));
            }
            self.e(v::r3(v::ZIP1S, d[0], VM, VM + 1));
            self.e(v::r3(v::ZIP2S, d[1], VM, VM + 1));
            self.e(v::r3(v::ZIP1S, VM, VM + 2, VM + 3));
            self.e(v::r3(v::ZIP2S, VM + 1, VM + 2, VM + 3));
            self.e(v::r3(v::ZIP1D, VM + 2, d[0], VM));
            self.e(v::r3(v::ZIP2D, VM + 3, d[0], VM));
            self.e(v::r3(v::ZIP1D, d[2], d[1], VM + 1));
            self.e(v::r3(v::ZIP2D, d[3], d[1], VM + 1));
            self.e(v::mov(d[0], VM + 2));
            self.e(v::mov(d[1], VM + 3));
            self.ldst_sp_q(true, VM, self.group_slot);
        } else {
            for s in &b[..n] {
                self.stmt(s);
            }
        }
        self.bind(end);
        n
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Def(v, op) => self.def(*v, op),
            Stmt::Set(var, v) => {
                let dst = Ent::Var(var.0);
                if *var == self.i {
                    // The element counter steps by the elements of one
                    // iteration (4, or 8 for two groups).
                    let r = self.src(dst, VS0);
                    self.splat(VS2, self.lanes);
                    let d = self.dst(dst);
                    self.e(v::r3(v::ADD, d, r, VS2));
                    self.done(dst, d);
                    return;
                }
                // A raw bool variable takes a mask as its 0/1 word.
                let word = self.is_bool(*v) && !self.var_mask[var.0 as usize];
                let r = self.vsrc_word(*v, VS1, word);
                if self.full && matches!(self.loc(dst), Loc::Stack(_)) {
                    // A spilled variable takes the value straight from r.
                    self.done(dst, r);
                    return;
                }
                if self.full {
                    let d = self.dst(dst);
                    if d != r {
                        self.e(v::mov(d, r));
                    }
                    self.done(dst, d);
                } else {
                    // Only the running lanes take the new value.
                    let d = self.src(dst, VS0);
                    self.e(v::r3(v::BIT, d, r, VM));
                    self.done(dst, d);
                }
            }
            Stmt::Store { region, base, extent, off, val } => self.store(*region, *base, *extent, *off, *val),
            Stmt::Out { .. } => unreachable!("declined"),
            Stmt::If(c, t, e) => {
                if !self.info.div_if.contains(&id(s)) {
                    self.lane(9, *c, 0);
                    let else_l = self.label();
                    let end_l = self.label();
                    self.jump(Fix::Cbz, 0x3400_0000 | 9, else_l);
                    let full = self.full;
                    self.block(t);
                    if !e.is_empty() {
                        self.jump(Fix::B, 0x1400_0000, end_l);
                    }
                    self.bind(else_l);
                    self.full = full;
                    self.block(e);
                    self.bind(end_l);
                    self.full = full;
                    return;
                }
                // Divergent: both sides under masks, then the union.
                let (else_slot, then_slot) = (self.slot(0), self.slot(1));
                self.depth += 1;
                let rc = self.vsrc(*c, VS1);
                if self.mask[c.0 as usize] {
                    self.e(v::mov(VS2, rc));
                } else {
                    self.e(v::r3(v::CMTST, VS2, rc, rc));
                }
                self.e(v::r3(v::BIC, VS1, VM, VS2));
                self.ldst_sp_q(false, VS1, else_slot);
                self.e(v::r3(v::AND, VM, VM, VS2));
                let full = self.full;
                self.full = false;
                let skip_t = self.label();
                self.cbz_none(VM, skip_t);
                self.block(t);
                self.bind(skip_t);
                self.ldst_sp_q(false, VM, then_slot);
                self.ldst_sp_q(true, VM, else_slot);
                let skip_e = self.label();
                self.cbz_none(VM, skip_e);
                self.full = false;
                self.block(e);
                self.bind(skip_e);
                self.ldst_sp_q(true, VS2, then_slot);
                self.e(v::r3(v::ORR, VM, VM, VS2));
                self.depth -= 1;
                self.full = full;
            }
            Stmt::Loop { cap, body } => {
                let lid = self.loop_id;
                self.loop_id += 1;
                let ce = Ent::Counter(lid);
                let masked = self.info.masked.contains(&id(s));
                let top = self.label();
                let exit = self.label();
                let iter_end = self.label();
                let (brk, cont) = (self.slot(0), self.slot(1));
                if masked {
                    self.depth += 1;
                    self.e(v::movi0(VS2));
                    self.ldst_sp_q(false, VS2, brk);
                }
                let full = self.full;
                // A plain loop's mask shrinks only through masked exits to
                // outer loops; with none inside, it keeps the entry mask.
                let has_masked_exit = self.info.escapes.contains(&id(s));
                self.full = full && !masked && !has_masked_exit;
                let d = self.gdst(ce);
                self.e(mov(d, 31));
                self.gdone(ce, d);
                self.bind(top);
                let rc = self.gsrc(ce, 9);
                if *cap < 4096 {
                    self.e(cmp_imm(rc, *cap));
                } else {
                    self.mov_imm(10, *cap);
                    self.e(cmp(rc, 10));
                }
                self.jump(Fix::BCond, 0x5400_0000 | HS as u32, exit);
                if masked || has_masked_exit {
                    self.cbz_none(VM, exit);
                }
                let d = self.gdst(ce);
                self.e(add_imm(d, rc, 1));
                self.gdone(ce, d);
                if masked {
                    self.e(v::movi0(VS2));
                    self.ldst_sp_q(false, VS2, cont);
                }
                self.loops.push(LoopCtx { masked, top, exit, iter_end, brk, cont });
                self.block(body);
                self.loops.pop();
                self.bind(iter_end);
                if masked {
                    self.ldst_sp_q(true, VS2, cont);
                    self.e(v::r3(v::ORR, VM, VM, VS2));
                    // A continued lane may be the only one left running.
                    self.full = false;
                }
                self.jump(Fix::B, 0x1400_0000, top);
                self.bind(exit);
                if masked {
                    self.ldst_sp_q(true, VS2, brk);
                    self.e(v::r3(v::ORR, VM, VM, VS2));
                    self.depth -= 1;
                }
                self.full = full;
            }
            Stmt::CallHost { .. } => unreachable!("declined"),
            Stmt::Call { .. } => unreachable!("flattened"),
            Stmt::Break(d) | Stmt::Continue(d) => {
                let is_break = matches!(s, Stmt::Break(_));
                let t = self.loops.len() - 1 - *d as usize;
                let (masked, top, exit, iter_end, brk, cont) = {
                    let l = &self.loops[t];
                    (l.masked, l.top, l.exit, l.iter_end, l.brk, l.cont)
                };
                let slot = if is_break { brk } else { cont };
                if masked {
                    self.ldst_sp_q(true, VS2, slot);
                    self.e(v::r3(v::ORR, VS2, VS2, VM));
                    self.ldst_sp_q(false, VS2, slot);
                }
                if self.info.jump.contains(&id(s)) {
                    // Every running lane leaves together. A masked loop may
                    // still hold continued lanes: they go on at the
                    // iteration end (breaking lanes are parked in `brk`).
                    let to = match (is_break, masked) {
                        (_, true) => {
                            if is_break {
                                self.e(v::movi0(VM));
                            }
                            iter_end
                        }
                        (true, false) => exit,
                        (false, false) => top,
                    };
                    self.jump(Fix::B, 0x1400_0000, to);
                } else {
                    // These lanes stop running until their loop's exit or
                    // next iteration.
                    self.e(v::movi0(VM));
                    self.full = false;
                }
            }
        }
    }

    fn gsrc(&mut self, e: Ent, scratch: u8) -> u8 {
        match self.loc(e) {
            Loc::Reg(r) => r,
            Loc::Stack(off) => {
                self.sp_addr(scratch, off);
                self.e(ldst_w(true, scratch, scratch));
                scratch
            }
        }
    }

    fn gdst(&self, e: Ent) -> u8 {
        match self.loc(e) {
            Loc::Reg(r) => r,
            Loc::Stack(_) => 16,
        }
    }

    fn gdone(&mut self, e: Ent, r: u8) {
        if let Loc::Stack(off) = self.loc(e) {
            self.sp_addr(17, off);
            self.e(ldst_w(false, r, 17));
        }
    }

    // -- ops ------------------------------------------------------------------------

    fn def(&mut self, v: Val, op: &Op) {
        if self.step_def == Some(v.0) {
            return;
        }
        if self.packed.contains_key(&v.0) {
            // Lives in a lane of a packed register (loaded in the prologue).
            return;
        }
        let dst = Ent::Val(v.0);
        match *op {
            Op::ConstF(x) => self.konst(dst, x.to_bits()),
            Op::ConstI(x) => self.konst(dst, x as u32),
            Op::ConstB(x) => self.konst(dst, if self.mask[v.0 as usize] { if x { u32::MAX } else { 0 } } else { x as u32 }),
            Op::Get(var) => {
                let r = self.src(Ent::Var(var.0), VS1);
                let d = self.dst(dst);
                if d != r {
                    self.e(v::mov(d, r));
                }
                self.done(dst, d);
            }
            Op::Un(u, a) => {
                let ra = self.vsrc(a, VS1);
                let d = self.dst(dst);
                match u {
                    Un::BitsFI | Un::BitsIF => {
                        if d != ra {
                            self.e(v::mov(d, ra));
                        }
                    }
                    Un::NotB => {
                        // A mask: NOT of a mask, or raw bits == 0 (as the
                        // scalar code tests them).
                        if self.mask[a.0 as usize] {
                            self.e(v::r2(v::NOT, d, ra));
                        } else {
                            self.e(v::r2(v::CMEQ0, d, ra));
                        }
                        if !self.mask[v.0 as usize] {
                            self.e(v::r2(v::USHR31, d, d));
                        }
                    }
                    _ => {
                        let base = match u {
                            Un::NegF => v::FNEG,
                            Un::AbsF => v::FABS,
                            Un::SqrtF => v::FSQRT,
                            Un::FloorF => v::FRINTM,
                            Un::CeilF => v::FRINTP,
                            Un::TruncF => v::FRINTZ,
                            Un::RoundF => v::FRINTA,
                            Un::F2I => v::FCVTZS,
                            Un::I2F => v::SCVTF,
                            Un::NegI => v::NEG,
                            _ => unreachable!("f64 is declined"),
                        };
                        self.e(v::r2(base, d, ra));
                    }
                }
                self.done(dst, d);
            }
            Op::Bin(b @ (Bin::MulF | Bin::MulI), x, y) if self.packed.contains_key(&x.0) != self.packed.contains_key(&y.0) => {
                let (x, y) = if self.packed.contains_key(&y.0) { (x, y) } else { (y, x) };
                let (pr, pl, _) = self.packed[&y.0];
                let rx = self.vsrc(x, VS0);
                let d = self.dst(dst);
                let base = if b == Bin::MulF { v::FMUL_E } else { v::MUL_E };
                self.e(v::by_elem(base, d, rx, pr, pl));
                self.done(dst, d);
            }
            Op::Bin(b, x, y) => {
                // And/or of bools: masks when both are, else 0/1 words.
                let raw = matches!(b, Bin::AndB | Bin::OrB) && !self.mask[v.0 as usize];
                let rx = self.vsrc_word(x, VS0, raw);
                let ry = if y == x { rx } else { self.vsrc_word(y, VS1, raw) };
                match b {
                    Bin::DivI | Bin::RemI if self.consts[y.0 as usize].is_some_and(|c| c > 1 && (c as u32).is_power_of_two()) => {
                        // By 2^k: q = (x + ((x >> 31) >>> (32 - k))) >> k
                        // (rounds toward zero, as SDIV); r = x - (q << k).
                        let k = (self.consts[y.0 as usize].unwrap() as u32).trailing_zeros();
                        self.e(v::sshr(VS2, rx, 31));
                        self.e(v::ushr(VS2, VS2, 32 - k));
                        self.e(v::r3(v::ADD, VS2, rx, VS2));
                        self.e(v::sshr(VS2, VS2, k));
                        if b == Bin::RemI {
                            self.e(v::shl(VS2, VS2, k));
                            self.e(v::r3(v::SUB, VS2, rx, VS2));
                        }
                        let d = self.dst(dst);
                        self.e(v::mov(d, VS2));
                        self.done(dst, d);
                        return;
                    }
                    Bin::DivI | Bin::RemI if self.consts[y.0 as usize].is_some_and(|c| c > 1) => {
                        // By a constant: the magic multiplier (as SDIV rounds).
                        let c = self.consts[y.0 as usize].unwrap() as u32;
                        self.magic_div(rx, c);
                        if b == Bin::RemI {
                            self.splat(VS1, c);
                            self.e(v::r3(v::MUL, VS2, VS2, VS1));
                            self.e(v::r3(v::SUB, VS2, rx, VS2));
                        }
                        let d = self.dst(dst);
                        self.e(v::mov(d, VS2));
                        self.done(dst, d);
                        return;
                    }
                    Bin::DivI | Bin::RemI => {
                        for l in 0..4 {
                            self.e(v::umov_w(10, rx, l));
                            self.e(v::umov_w(11, ry, l));
                            self.e(sdiv(12, 10, 11));
                            if b == Bin::RemI {
                                self.e(msub(12, 12, 11, 10));
                            }
                            self.e(v::ins_w(VS2, l, 12));
                        }
                        let d = self.dst(dst);
                        self.e(v::mov(d, VS2));
                        self.done(dst, d);
                        return;
                    }
                    Bin::MinF | Bin::MaxF => {
                        // a < b ? a : b  /  a > b ? a : b (false on NaN: b).
                        let d = self.dst(dst);
                        let m = if d != rx && d != ry { d } else { VS2 };
                        if b == Bin::MinF {
                            self.e(v::r3(v::FCMGT, m, ry, rx));
                        } else {
                            self.e(v::r3(v::FCMGT, m, rx, ry));
                        }
                        self.select_into(d, m, rx, ry);
                        self.done(dst, d);
                        return;
                    }
                    Bin::ShlI | Bin::ShrI | Bin::ShrUI if self.consts[y.0 as usize].is_some() => {
                        // A constant amount (mod 32): an immediate shift.
                        let k = self.consts[y.0 as usize].unwrap() as u32 & 31;
                        let d = self.dst(dst);
                        if k == 0 {
                            if d != rx {
                                self.e(v::mov(d, rx));
                            }
                        } else {
                            self.e(match b {
                                Bin::ShlI => v::shl(d, rx, k),
                                Bin::ShrI => v::sshr(d, rx, k),
                                _ => v::ushr(d, rx, k),
                            });
                        }
                        self.done(dst, d);
                        return;
                    }
                    Bin::ShlI | Bin::ShrI | Bin::ShrUI => {
                        // Amount mod 32 (unless proven below 32); right
                        // shifts are negative left shifts.
                        let amt = if self.bounds[y.0 as usize].is_some_and(|m| m < 32) {
                            ry
                        } else {
                            self.splat(VS2, 31);
                            self.e(v::r3(v::AND, VS2, ry, VS2));
                            VS2
                        };
                        if b != Bin::ShlI {
                            self.e(v::r2(v::NEG, VS2, amt));
                        } else if amt != VS2 {
                            self.e(v::mov(VS2, amt));
                        }
                        let d = self.dst(dst);
                        self.e(v::r3(if b == Bin::ShrUI { v::USHL } else { v::SSHL }, d, rx, VS2));
                        self.done(dst, d);
                        return;
                    }
                    _ => {}
                }
                let base = match b {
                    Bin::AddF => v::FADD,
                    Bin::SubF => v::FSUB,
                    Bin::MulF => v::FMUL,
                    Bin::DivF => v::FDIV,
                    Bin::AddI => v::ADD,
                    Bin::SubI => v::SUB,
                    Bin::MulI => v::MUL,
                    Bin::AndI | Bin::AndB => v::AND,
                    Bin::OrI | Bin::OrB => v::ORR,
                    Bin::XorI => v::EOR,
                    _ => unreachable!("f64 is declined"),
                };
                let d = self.dst(dst);
                self.e(v::r3(base, d, rx, ry));
                self.done(dst, d);
            }
            Op::CmpF(cc, x, y) | Op::CmpI(cc, x, y) => {
                let float = matches!(op, Op::CmpF(..));
                // Bools compare as masks when both are and the compare is an
                // equality (true held as all ones orders below false), else
                // as 0/1 words.
                let both = self.mask[x.0 as usize] && self.mask[y.0 as usize] && matches!(cc, Cmp::Eq | Cmp::Ne);
                let raw = !float && self.is_bool(x) && !both;
                let rx = self.vsrc_word(x, VS0, raw);
                let ry = self.vsrc_word(y, VS1, raw);
                let (gt, ge, eq) = if float { (v::FCMGT, v::FCMGE, v::FCMEQ) } else { (v::CMGT, v::CMGE, v::CMEQ) };
                match cc {
                    Cmp::Lt => self.e(v::r3(gt, VS2, ry, rx)),
                    Cmp::Le => self.e(v::r3(ge, VS2, ry, rx)),
                    Cmp::Gt => self.e(v::r3(gt, VS2, rx, ry)),
                    Cmp::Ge => self.e(v::r3(ge, VS2, rx, ry)),
                    Cmp::Eq | Cmp::Ne => {
                        self.e(v::r3(eq, VS2, rx, ry));
                        if cc == Cmp::Ne {
                            self.e(v::r2(v::NOT, VS2, VS2));
                        }
                    }
                }
                let d = self.dst(dst);
                if self.mask[v.0 as usize] {
                    if d != VS2 {
                        // The compare straight into d (it reads before writing).
                        let n = self.code.len();
                        let last = self.code[n - 1];
                        if last & 0x1F == VS2 as u32 && cc != Cmp::Ne {
                            self.code[n - 1] = (last & !0x1F) | d as u32;
                        } else {
                            self.e(v::mov(d, VS2));
                        }
                    }
                } else {
                    self.e(v::r2(v::USHR31, d, VS2));
                }
                self.done(dst, d);
            }
            Op::Sel(c, x, y) => {
                // The selector: a mask as it is, raw bits made one.
                let mut rc = self.vsrc(c, VS2);
                if !self.mask[c.0 as usize] {
                    self.e(v::r3(v::CMTST, VS2, rc, rc));
                    rc = VS2;
                }
                // A bool select of raw words takes masks as their 0/1 word.
                let raw = self.is_bool(v) && !self.mask[v.0 as usize];
                let rx = self.vsrc_word(x, VS0, raw);
                let ry = self.vsrc_word(y, VS1, raw);
                let d = self.dst(dst);
                self.select_into(d, rc, rx, ry);
                self.done(dst, d);
            }
            Op::Wrap(x, len) => {
                let rx = self.vsrc(x, VS0);
                if len.is_power_of_two() {
                    self.splat(VS2, len - 1);
                    let d = self.dst(dst);
                    self.e(v::r3(v::AND, d, rx, VS2));
                    self.done(dst, d);
                } else if len < 1 << 31 {
                    // q = x / len by the magic multiplier (as SDIV rounds),
                    // r = x - q * len, then r < 0 ? r + len : r.
                    self.magic_div(rx, len);
                    self.splat(VS1, len);
                    self.e(v::r3(v::MUL, VS2, VS2, VS1));
                    self.e(v::r3(v::SUB, VS2, rx, VS2));
                    let d = self.dst(dst);
                    self.e(v::r2(v::CMLT0, d, VS2));
                    self.e(v::r3(v::AND, d, d, VS1));
                    self.e(v::r3(v::ADD, d, VS2, d));
                    self.done(dst, d);
                } else {
                    // r = x - (x / len) * len; r < 0 ? r + len : r
                    self.mov_imm(11, len);
                    for l in 0..4 {
                        self.e(v::umov_w(10, rx, l));
                        self.e(sdiv(12, 10, 11));
                        self.e(msub(12, 12, 11, 10));
                        self.e(ADD | 11 << 16 | 12 << 5 | 13);
                        self.e(cmp_imm(12, 0));
                        self.e(csel(12, 13, 12, LT));
                        self.e(v::ins_w(VS2, l, 12));
                    }
                    let d = self.dst(dst);
                    self.e(v::mov(d, VS2));
                    self.done(dst, d);
                }
            }
            Op::Load { region, base, extent, off } => self.load(dst, region, base, extent, off),
            Op::FrameCount => {
                let d = self.dst(dst);
                self.e(v::r2(v::DUP_W, d, 4));
                self.done(dst, d);
            }
            Op::BufLen(k) => {
                self.e(ldst_imm(false, true, 9, 3, k as u32 * 16 + 8));
                let d = self.dst(dst);
                self.e(v::r2(v::DUP_W, d, 9));
                self.done(dst, d);
            }
            Op::Fma(k, a, b, c) => {
                // acc = c (negated for a*b - c), then fmla/fmls acc, a, b
                // (by element when a multiplicand is a packed constant).
                let (a, b) = if self.packed.contains_key(&a.0) { (b, a) } else { (a, b) };
                let pk = self.packed.get(&b.0).map(|&(r, l, _)| (r, l)).filter(|_| !self.packed.contains_key(&a.0));
                let ra = self.vsrc(a, VS0);
                let rb = if pk.is_some() {
                    0
                } else if b == a {
                    ra
                } else {
                    self.vsrc(b, VS1)
                };
                let d = self.dst(dst);
                // A spilled or packed addend loads straight into the
                // accumulator when the result register is free for it.
                let direct = d != ra && (pk.is_some() || d != rb) && d != VS0 && (self.packed.contains_key(&c.0) || matches!(self.loc(Ent::Val(c.0)), Loc::Stack(_)));
                let rc = self.vsrc(c, if direct { d } else { VS2 });
                let acc = if d != ra && (pk.is_some() || d != rb) { d } else { VS2 };
                if k == crate::ir::Fma::Sub {
                    self.e(v::r2(v::FNEG, acc, rc));
                } else if acc != rc {
                    self.e(v::mov(acc, rc));
                }
                match pk {
                    Some((pr, pl)) => {
                        let op = match k {
                            crate::ir::Fma::SubFrom => v::FMLS_E,
                            crate::ir::Fma::MulAddI => v::MLA_E,
                            _ => v::FMLA_E,
                        };
                        self.e(v::by_elem(op, acc, ra, pr, pl));
                    }
                    None => {
                        let op = match k {
                            crate::ir::Fma::SubFrom => v::FMLS,
                            crate::ir::Fma::MulAddI => v::MLA,
                            _ => v::FMLA,
                        };
                        self.e(v::r3(op, acc, ra, rb));
                    }
                }
                if acc != d {
                    self.e(v::mov(d, acc));
                }
                self.done(dst, d);
            }
            Op::ConstD(_) | Op::CmpD(..) | Op::In { .. } => unreachable!("declined"),
        }
    }

    /// VS2 = x / d per lane (truncating, as SDIV) for a constant d in
    /// 2..2^31, by the magic multiplier; `rx` is kept (clobbers VS1).
    fn magic_div(&mut self, rx: u8, d: u32) {
        let (m, sh) = crate::ir::magic_s32(d);
        self.splat(VS1, m as u32);
        self.e(v::r3(v::SMULL, VS2, rx, VS1));
        self.e(v::r3(v::SMULL2, VS1, rx, VS1));
        self.e(v::r3(v::UZP2, VS2, VS2, VS1));
        if m < 0 {
            self.e(v::r3(v::ADD, VS2, VS2, rx));
        }
        if sh > 0 {
            self.e(v::sshr(VS2, VS2, sh));
        }
        self.e(v::ushr(VS1, rx, 31));
        self.e(v::r3(v::ADD, VS2, VS2, VS1));
    }

    fn konst(&mut self, dst: Ent, bits: u32) {
        let d = self.dst(dst);
        self.splat(d, bits);
        self.done(dst, d);
    }

    /// v(d) = bits in every lane: one load from the constant pool.
    fn splat(&mut self, d: u8, bits: u32) {
        if bits == 0 {
            self.e(v::movi0(d));
            return;
        }
        self.vconst(d, [bits; 4]);
    }

    /// v(d) = four words (lane 0 first): one load from the constant pool.
    fn vconst(&mut self, d: u8, words: [u32; 4]) {
        let l = match self.pool.iter().find(|(b, _)| *b == words) {
            Some((_, l)) => *l,
            None => {
                let l = self.label();
                self.pool.push((words, l));
                l
            }
        };
        // ldr qd, <literal>
        self.jump(Fix::Lit, 0x9C00_0000 | d as u32, l);
    }

    /// Appends the constant pool (16-byte aligned) after the code.
    fn emit_pool(&mut self) {
        while self.code.len() % 4 != 0 {
            self.e(0xD503_201F); // nop
        }
        for (words, l) in std::mem::take(&mut self.pool) {
            self.bind(l);
            for w in words {
                self.e(w);
            }
        }
    }

    fn patch(&mut self) -> bool {
        for &(at, l, kind) in &self.fixups {
            let Some(target) = self.labels[l] else { return false };
            let delta = target as i64 - at as i64;
            let w = &mut self.code[at];
            match kind {
                Fix::B => {
                    if delta.abs() >= 1 << 25 {
                        return false;
                    }
                    *w |= (delta as u32) & 0x03FF_FFFF;
                }
                Fix::BCond | Fix::Cbz | Fix::Lit => {
                    if delta.abs() >= 1 << 18 {
                        return false;
                    }
                    *w |= ((delta as u32) & 0x7FFFF) << 5;
                }
            }
        }
        true
    }
}

/// Deepest nesting of divergent ifs and masked loops (mask slot pairs).
fn mask_depth(info: &Info, b: &Block) -> u32 {
    b.iter()
        .map(|s| match s {
            Stmt::If(_, t, e) => info.div_if.contains(&id(s)) as u32 + mask_depth(info, t).max(mask_depth(info, e)),
            Stmt::Loop { body, .. } => info.masked.contains(&id(s)) as u32 + mask_depth(info, body),
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}

/// Compiles a kernel program's element loop to NEON ×4 code, or declines.
/// The code runs `n` elements (a multiple of 4) from ctx's element base,
/// with the scalar code's ABI; the caller runs the remainder on scalar
/// code, and uses this only for element-local, non-overlapping kernels
/// whose written buffers hold every element's records.
pub fn compile(p: &Program) -> Option<Code> {
    compile_words(p, 4).and_then(|(w, _)| Code::new(&w))
}

/// Size of the vector code: (instructions, spill bytes), or None when
/// declined (for diagnostics and `check()`).
pub fn stats(p: &Program) -> Option<(usize, u32)> {
    compile_words(p, 4).map(|(w, s)| (w.len(), s))
}

fn compile_words(p: &Program, lanes: u32) -> Option<(Vec<u32>, u32)> {
    // Functions are inlined: native code is one body.
    let flat = crate::ir::flat(p);
    let p = &*flat;
    let sh = shape(p)?;
    let Stmt::Loop { body, .. } = sh.element else { return None };
    if !supported(p, body) {
        return None;
    }
    let info = analyse(p, &sh)?;
    let bounds = crate::arm64::bounds(p);
    let table = small_table(body, &info, &bounds);
    // A resident table takes v24.. and keeps its byte-index constants in
    // v22 (0x04040404) and v23 (0x03020100).
    let mut pool: Vec<u8> = V_POOL.iter().copied().filter(|r| table.is_none_or(|(_, w)| !(22..24 + w / 4).contains(&(*r as u32)))).collect();
    // Constants used only as multiplicands, four to a register (taken from
    // the pool's end): each saves three registers.
    let (packed, packs) = pack_constants(p, &mut pool);
    // The negated constants of shift amounts x + c (one SUB from them per
    // shift): at most two, each a register from the pool's end.
    let resident: Vec<(u32, u8)> = shift_constants(p, &bounds).into_iter().take(2).filter_map(|c| pool.pop().map(|r| (c, r))).collect();
    // No divergent branch or masked loop: the mask register is never read
    // (every lane always runs), so it holds values too. (A four-register
    // row load saves and restores v28 whatever it holds.)
    if mask_depth(&info, &p.body) == 0 && info.masked.is_empty() && info.escapes.is_empty() {
        pool.push(VM);
    }
    // The counter's step is emitted from the counter itself: its IR value
    // (iv + 1) is never computed, so the allocator must not keep iv live to
    // the end of the element for it.
    let (alloc_view, step_def) = without_step(p, &sh);
    let (bmask, bvar) = bool_masks(p);
    let alloc = allocate_with(
        &alloc_view,
        |e| match e {
            Ent::Counter(_) => 0,
            // Packed constants take no register or slot of their own.
            Ent::Val(v) if packed.contains_key(&v) => 2,
            _ => 1,
        },
        &[G_POOL, &pool],
        |e| match e {
            Ent::Counter(_) => 4,
            _ => 16,
        },
        true,
    );
    let spill = (alloc.spill_bytes + 15) & !15;
    // The mask slots, then one slot for gather groups.
    let masks = (mask_depth(&info, &p.body) + 1) * 2 * 16 + 16;
    let frame = p.frame_words * 16;
    let total = spill + masks + frame;
    // Spill slots and mask slots use scaled 12-bit q offsets.
    if spill + masks >= 65536 || total > 1 << 20 {
        return None;
    }
    let i = sh.i;
    let prelude_len = sh.prelude.len();
    let (step, consts) = steps(p, &info, i);
    let mut em = Em {
        info,
        code: Vec::new(),
        locs: alloc.locs,
        labels: Vec::new(),
        fixups: Vec::new(),
        loops: Vec::new(),
        loop_id: 0,
        mask_base: spill,
        frame_base: spill + masks,
        depth: 0,
        group_slot: spill + masks - 16,
        table,
        lanes,
        packed,
        uses: use_counts(p),
        last_bind: None,
        step_def,
        mask: bmask,
        var_mask: bvar,
        p_vals: p.vals.clone(),
        resident: resident.clone(),
        full: true,
        i,
        bounds,
        step,
        consts,
        pool: Vec::new(),
    };
    // Prologue: frame record, x19..x28, all of q8..q15.
    em.e(0xA9BF_7BFD); // stp x29, x30, [sp, #-16]!
    em.e(0x9100_03FD); // mov x29, sp
    for (a, b) in [(19u32, 20u32), (21, 22), (23, 24), (25, 26), (27, 28)] {
        em.e(0xA9BF_0000 | b << 10 | 31 << 5 | a);
    }
    for (a, b) in [(8u32, 9u32), (10, 11), (12, 13), (14, 15)] {
        // stp qa, qb, [sp, #-32]!
        em.e(0xADBF_0000 | b << 10 | 31 << 5 | a);
    }
    let sub_sp = |em: &mut Em, add: bool| {
        if total == 0 {
            return;
        }
        if total < 4096 {
            em.e((if add { 0x9100_03FF } else { 0xD100_03FF }) | total << 10);
        } else {
            em.mov_imm(16, total);
            em.e((if add { 0x8B20_63FF } else { 0xCB20_63FF }) | 16 << 16);
        }
    };
    sub_sp(&mut em, false);
    // A small table into v24.. (it lies inside the shared tables: its
    // loads' extent is its size).
    if let Some((base, words)) = table {
        em.mov_imm(9, base * 4);
        em.e(add_x(9, 2, 9));
        for k in 0..words / 4 {
            em.e(v::ldst_q(true, 24 + k as u8, 9, 16 * k));
        }
        em.splat(22, 0x0404_0404);
        em.splat(23, 0x0302_0100);
    }
    for (r, words) in &packs {
        em.vconst(*r, *words);
    }
    for (bits, r) in &resident {
        em.splat(*r, *bits);
    }
    // The execution mask: every lane.
    em.e(v::movi0(VM));
    em.e(v::r2(v::NOT, VM, VM));
    for s in &p.body[..prelude_len] {
        if let Stmt::Set(var, _) = s {
            if *var == i {
                // Lanes' element offsets [0, 1, 2, 3].
                let d = em.dst(Ent::Var(i.0));
                for l in 0..4 {
                    em.e(movz(9, l, 0));
                    em.e(v::ins_w(d, l, 9));
                }
                em.done(Ent::Var(i.0), d);
                continue;
            }
        }
        em.stmt(s);
    }
    em.stmt(sh.element);
    // The run's counted work: the lanes' counts, summed, into its ctx word.
    if let Some((var, word)) = sh.post {
        let r = em.src(Ent::Var(var.0), VS0);
        em.e(0x4EB1_B800 | (r as u32) << 5 | VS0 as u32); // addv s31, vR.4s
        em.e(v::umov_w(9, VS0, 0));
        em.e(0xB900_0000 | word << 10 | 9); // str w9, [x0, #4 * word]
    }
    sub_sp(&mut em, true);
    for (a, b) in [(14u32, 15u32), (12, 13), (10, 11), (8, 9)] {
        // ldp qa, qb, [sp], #32
        em.e(0xACC1_0000 | b << 10 | 31 << 5 | a);
    }
    for (a, b) in [(27u32, 28u32), (25, 26), (23, 24), (21, 22), (19, 20)] {
        // ldp xa, xb, [sp], #16
        em.e(0xA8C1_0000 | b << 10 | 31 << 5 | a);
    }
    em.e(0xA8C1_7BFD); // ldp x29, x30, [sp], #16
    em.e(RET);
    em.emit_pool();
    if !em.patch() {
        return None;
    }
    Some((em.code, spill))
}

#[cfg(test)]
mod tests {
    #[test]
    fn encodings() {
        // Spot checks against the architecture manual.
        assert_eq!(super::v::r3(super::v::FADD, 0, 1, 2), 0x4E22_D420);
        assert_eq!(super::v::umov_w(9, 28, 3), 0x0E1C_3F89);
        assert_eq!(super::v::ins_w(29, 2, 12), 0x4E14_1D9D);
    }
}
