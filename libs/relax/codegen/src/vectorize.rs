//! Stream-map loop vectorisation: planning, legality, cost and emission.

use crate::loops::{head_pos, U64_TY};
use relax_backend::regalloc::{for_defs, for_uses};
use relax_backend::rir::*;

/// What a value of a stream loop is, per vreg, while the body is read symbolically.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Sv {
    /// not a stream-loop value
    No,
    /// `root + off`: the entry value of a loop input plus a constant (pointers and counters)
    Addr(VReg, i64),
    /// a 32-bit constant (its low 32 bits)
    K(u32),
    /// `(root + off) << shift`: a counter scaled to bytes
    Scaled(VReg, i64, u8),
    /// the address `base + ((counter + off) << shift)` of an indexed stream: (base, counter, off bytes, shift)
    Ptr(VReg, VReg, i64, u8),
    /// the vector value computed by operation `k` of the widened body
    Lane(usize),
}

/// The element type of a stream loop: what its loads and stores move and its operations compute.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Lane {
    /// u8, wrapping integer operations
    Byte,
    /// f32
    F32,
    /// 32-bit integers (wrapping operations)
    I32,
}

impl Lane {
    fn bytes(self) -> i64 {
        if self == Lane::Byte { 1 } else { 4 }
    }
}

/// Where a stream loop's memory access goes: the current element of a pointer that walks one element
/// per iteration (`iv` None), or an element of the array at `root` indexed by the counter `iv`;
/// `disp` bytes further on.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Stream {
    root: VReg,
    iv: Option<VReg>,
    sh: u8,
    disp: i64,
}

/// A loop bound: a loop-invariant register or a constant.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Bound {
    Reg(VReg),
    Imm(i64),
}

/// One operation of a widened body.
#[derive(Clone, Copy)]
enum WOp {
    /// a vector load of a stream
    Load(Stream),
    Bin(VX, usize, usize),
    Int(VX, usize, usize),
    Un(VX, usize),
    /// a loop-invariant scalar (the root vreg) in every lane
    Splat(VReg),
    /// a 32-bit constant in every lane
    Const(u32),
    /// lanes of the mask (first) picking the second's lane, else the third's
    Sel(usize, usize, usize),
    /// the lanes of a computed vector read as another kind (a float's bits)
    Same(usize, Wt),
}

/// What the lanes of a widened value hold.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Wt {
    /// floats
    F,
    /// 32-bit integers (also a float's bits)
    I,
    /// compare results: all ones (true) or zero per lane
    M,
    /// a constant, as either number kind
    Any,
}

/// The lane kind of widened value `k` of a loop of `kind` elements.
fn wtype(wops: &[WOp], vregs: &[Cls], kind: Lane, k: usize) -> Wt {
    match wops[k] {
        WOp::Load(_) => if kind == Lane::F32 { Wt::F } else { Wt::I },
        WOp::Bin(VX::FCmp(..), ..) => Wt::M,
        WOp::Bin(..) => Wt::F,
        WOp::Int(VX::ICmp(..), ..) => Wt::M,
        WOp::Int(VX::And | VX::Or | VX::Xor, a, b) => match wtype(wops, vregs, kind, a) {
            Wt::Any => wtype(wops, vregs, kind, b),
            t => t,
        },
        WOp::Int(..) => Wt::I,
        WOp::Un(VX::FNeg(_) | VX::FAbs(_) | VX::FSqrt(..), _) => Wt::F,
        WOp::Un(..) => Wt::I,
        WOp::Splat(r) => if vregs[r.0 as usize] == Cls::F32 { Wt::F } else { Wt::I },
        WOp::Const(_) => Wt::Any,
        WOp::Sel(_, a, b) => match wtype(wops, vregs, kind, a) {
            Wt::Any => wtype(wops, vregs, kind, b),
            t => t,
        },
        WOp::Same(_, t) => t,
    }
}

/// Widen straight-line element maps over pointer/count/index streams: a loop whose iteration loads
/// elements (u8, f32 or 32-bit integers) at the current position of one or more streams (a pointer
/// stepping one element per iteration, or `base[i + c]` of a counter `i` stepping by one),
/// computes on them with lane-wise operations and loop-invariant scalars, and stores one result
/// element at the current position of a stream. Streams advance by one element per iteration; the
/// loop's exits are guards on a down-counting length (`n != 0`), a pointer or counter reaching an end
/// (`p == end`), a counter below a bound (`i < n`) or a pointer being null (an iterator's niche test). The scalar loop remains the tail
/// and fallback, including its exact short-circuit exhaustion behavior. A vector iteration (one
/// 16-byte vector per stream; with AVX2 two side by side, which pair256 makes one 256-bit
/// vector, then one more when a vector's elements are left) is admitted only when every scalar
/// guard succeeds for all of its elements (the back edge's own test of its last element
/// excepted: made after the vector loop, on into the scalar loop or out of the loop); runtime
/// distance checks exclude dependencies within an iteration (equal pointers are OK), but for a
/// stack slot whose address goes nowhere else, which no other pointer reaches. A float loop's
/// lanes also compare, blend, and read a float's bits; a body that branches only to pick values
/// is widened as a copy with those branches folded into selects.
pub(crate) fn vectorize_stream_maps(options: &relax_session::Options, f: &mut Func) -> bool {
    stream_maps(options, f, None)
}

/// The stream-map vectoriser over every loop of `f`, or (`only`) over the loop at that head alone:
/// a copy whose value branches were folded, tried without a remark of its own (its checks and
/// verdict go to the remark of the loop it came from).
fn stream_maps(options: &relax_session::Options, f: &mut Func, only: Option<u32>) -> bool {
    if !f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::Store(Mem::Int(1, false) | Mem::Int(4, _) | Mem::F32, ..)))) {
        return false;
    }
    let reach = relax_backend::regalloc::reachable(f);
    let mut preds = vec![Vec::new(); f.blocks.len()];
    for (bi, b) in f.blocks.iter().enumerate() {
        if reach[bi] {
            for s in b.term.succs() {
                preds[s as usize].push(bi as u32);
            }
        }
    }
    let loops = relax_backend::regalloc::natural_loops(f, &reach, &preds, false);
    let mut changed = false;
    let mut transformed = vec![false; f.blocks.len()];
    let mut reads = None;
    for (head, members) in loops {
        if only.is_some_and(|h| h != head) { continue; }
        if members.iter().zip(&transformed).any(|(a, b)| *a && *b) { continue; }
        // makepad inspect remarks: the last check passed (stages 1..4) or vectorised
        if only.is_none() {
            relax_backend::remark::loop_try("stream-map", head_pos(f, head as usize));
        }
        let mut nonlinear = false;
        let Some(mut p) = plan(f, head, &members, &preds, &mut nonlinear) else {
            // a body that branches only to pick values (`if c { a } else { b }` of arithmetic):
            // every lane computes both arms anyway, so fold the member blocks in place and
            // restore them if the folded loop cannot be widened
            if nonlinear && only.is_none() && value_branch_loop(f, head, &members) {
                let nv = f.vregs.len();
                let nb = f.blocks.len();
                let saved: Vec<_> = members.iter().enumerate().filter(|(_, m)| **m).map(|(b, _)| (b, f.blocks[b].clone())).collect();
                if fold_value_branches(f, head, &members) && stream_maps(options, f, Some(head)) {
                    transformed.resize(f.blocks.len(), false);
                    for (done, member) in transformed.iter_mut().zip(&members) { *done |= *member; }
                    reads = None;
                    changed = true;
                } else {
                    // Folding only appends vregs and edits member blocks; a failed recursive
                    // attempt emits nothing and changes no other Func fields (including halves).
                    f.vregs.truncate(nv);
                    f.blocks.truncate(nb);
                    for (b, block) in saved { f.blocks[b] = block; }
                }
            }
            continue;
        };
        let span = if options.cpu.avx2 && p.kind != Lane::Byte { 32 } else { 16 };
        let Some(checks) = legal(f, &mut p, span, &reach, &preds, &mut reads) else { continue };
        let Some(shape) = cost(options, &p) else { continue };
        emit(f, p, checks, shape);
        for (done, member) in transformed.iter_mut().zip(&members) { *done |= *member; }
        reads = None;
        changed = true;
    }
    changed
}

/// The symbolic body and recurrences of one candidate, shared by all four phases.
struct Plan<'a> {
    head: u32,
    nv: usize,
    chain: Vec<u32>,
    members: &'a [bool],
    roots: Vec<VReg>,
    slots: Vec<(VReg, u32)>,
    defined: Vec<bool>,
    inputs: Vec<bool>,
    definitions: Vec<u32>,
    sv: Vec<Sv>,
    kind: Lane,
    loads: Vec<(Stream, usize)>,
    store: (Stream, usize),
    wops: Vec<WOp>,
    zero_guards: Vec<(VReg, i64)>,
    end_guards: Vec<(VReg, i64, VReg, i64)>,
    lt_guards: Vec<(VReg, i64, Bound, bool)>,
    before_last: (usize, usize, usize),
    last: u32,
    sz: i64,
    counts: Vec<VReg>,
    walkers: Vec<VReg>,
    ivs: Vec<VReg>,
}

/// The tail permitted by the static dependence and live-out checks.
struct Checks {
    tail: bool,
}

/// Vector width, elements per vector and element-to-byte shift for emission.
struct Shape {
    pair: bool,
    span: i64,
    lanes: i64,
    shift: i64,
}

/// Per block, the distinct registers read before that block defines them. Block stamps
/// reuse the definition and deduplication scratch across the function's instruction walk.
fn reads_before_defs(f: &Func) -> Vec<Vec<VReg>> {
    let nv = f.vregs.len();
    let mut defined_at = vec![usize::MAX; nv];
    let mut read_at = vec![usize::MAX; nv];
    let mut reads = Vec::with_capacity(f.blocks.len());
    let mut uses = Vec::new();
    for (bi, block) in f.blocks.iter().enumerate() {
        let mut before = Vec::new();
        for i in &block.insts {
            for_uses(i, |v| {
                let k = v.0 as usize;
                if k < nv && defined_at[k] != bi && read_at[k] != bi {
                    before.push(v);
                    read_at[k] = bi;
                }
            });
            for_defs(i, |v| if (v.0 as usize) < nv { defined_at[v.0 as usize] = bi });
        }
        uses.clear();
        relax_backend::regalloc::term_uses(&block.term, &mut uses);
        for &v in &uses {
            let k = v.0 as usize;
            if k < nv && defined_at[k] != bi && read_at[k] != bi {
                before.push(v);
                read_at[k] = bi;
            }
        }
        reads.push(before);
    }
    reads
}

/// Read a linear cycle symbolically and classify its streams, guards and recurrences.
/// `nonlinear` distinguishes the first rejection, which alone may try value-branch folding.
fn plan<'a>(f: &Func, head: u32, members: &'a [bool], preds: &[Vec<u32>], nonlinear: &mut bool) -> Option<Plan<'a>> {
    // Only a linear cycle: guards exit the loop, and the sole back edge follows
    // the store. No interior entries or nested control flow can be bypassed.
    let Some(chain) = linear_cycle(f, head, members, preds) else {
        *nonlinear = true;
        return None;
    };
    let nv = f.vregs.len();
    let mut defined = vec![false; nv];
    let mut inputs = vec![false; nv];
    let mut definitions = vec![0u32; nv];
    for &b in &chain {
        for i in &f.blocks[b as usize].insts {
            for_uses(i, |v| { if !defined[v.0 as usize] { inputs[v.0 as usize] = true; } });
            for_defs(i, |v| { defined[v.0 as usize] = true; definitions[v.0 as usize] += 1; });
        }
    }
    if definitions.iter().any(|n| *n > 1) { return None; }
    relax_backend::remark::loop_stage(1);
    // a slot's address the body makes (Alloc remat leaves it there) is invariant: a root
    // too, which the vector code makes for itself
    let slots: Vec<(VReg, u32)> = chain.iter().flat_map(|&b| &f.blocks[b as usize].insts).filter_map(|i| match *i {
        Inst::SlotAddr(d, sl) if !inputs[d.0 as usize] => Some((d, sl)),
        _ => None,
    }).collect();
    let roots: Vec<_> = (0..nv).filter(|v| inputs[*v]).map(|v| VReg(v as u32)).chain(slots.iter().map(|s| s.0)).collect();
    // Symbolically execute the body. Every address must be the current element of a
    // one-element-per-iteration recurrence; every length guard a -1 recurrence.
    let mut sv = vec![Sv::No; nv];
    for &v in &roots {
        sv[v.0 as usize] = Sv::Addr(v, 0);
    }
    let mut kind: Option<Lane> = None;
    // zero tests (a counter reaching zero, a pointer being null): (root, offset from its entry value)
    let mut zero_guards: Vec<(VReg, i64)> = Vec::new();
    // equality tests of two inputs: (a, offset, b, offset)
    let mut end_guards: Vec<(VReg, i64, VReg, i64)> = Vec::new();
    // order tests `a + off < bound` that keep the loop going: (a, off, bound, signed)
    let mut lt_guards: Vec<(VReg, i64, Bound, bool)> = Vec::new();
    let mut loads: Vec<(Stream, usize)> = Vec::new(); // (stream, op index)
    let mut wops: Vec<WOp> = Vec::new();
    let mut store: Option<(Stream, usize)> = None;
    // the operand of a lane operation: a computed vector or a loop-invariant scalar
    macro_rules! operand {
        ($v:expr, $k:expr) => {{
            match sv[$v.0 as usize] {
                Sv::Lane(k) => k,
                Sv::Addr(r, 0) if roots.contains(&r) && matches!(($k, f.vregs[r.0 as usize]), (Lane::I32, Cls::I) | (Lane::F32, Cls::F32)) => {
                    wops.push(WOp::Splat(r));
                    wops.len() - 1
                }
                Sv::K(c) if $k != Lane::Byte => {
                    wops.push(WOp::Const(c));
                    wops.len() - 1
                }
                _ => return None,
            }
        }};
    }
    // the operand of a lane operation on a float loop's lanes of kind `$t` (F, I or M): a
    // computed vector of that kind, a loop-invariant scalar or a constant (numbers only)
    macro_rules! lane_of {
        ($v:expr, $t:expr) => {{
            let cls = if $t == Wt::F { Cls::F32 } else { Cls::I };
            let k = match sv[$v.0 as usize] {
                Sv::Lane(k) => k,
                Sv::Addr(r, 0) if $t != Wt::M && roots.contains(&r) && f.vregs[r.0 as usize] == cls => {
                    wops.push(WOp::Splat(r));
                    wops.len() - 1
                }
                Sv::K(c) if $t != Wt::M => {
                    wops.push(WOp::Const(c));
                    wops.len() - 1
                }
                _ => return None,
            };
            let t = wtype(&wops, &f.vregs, Lane::F32, k);
            if t != $t && !(t == Wt::Any && $t != Wt::M) { return None; }
            k
        }};
    }
    // the stream an access `[p + off]` goes to
    macro_rules! stream_at {
        ($p:expr, $off:expr) => {{
            match sv[$p.0 as usize] {
                Sv::Addr(r, o) if roots.contains(&r) => o.checked_add($off as i64).filter(|d| d.abs() < 1 << 20).map(|disp| Stream { root: r, iv: None, sh: 0, disp }),
                Sv::Ptr(base, iv, o, sh) => o.checked_add($off as i64).filter(|d| d.abs() < 1 << 20).map(|disp| Stream { root: base, iv: Some(iv), sh, disp }),
                _ => None,
            }
        }};
    }
    let last = *chain.last().unwrap();
    let mut before_last = (0, 0, 0);
    for &b in &chain {
        if b == last { before_last = (zero_guards.len(), end_guards.len(), lt_guards.len()); }
        let block = &f.blocks[b as usize];
        if let Term::Branch(c, ..) = block.term {
            if !matches!(block.insts.last(), Some(Inst::ICmpI(Cond::Eq | Cond::Ne, false, d, _, 0) | Inst::ICmpI(Cond::Lt | Cond::Ge, _, d, _, _) | Inst::ICmp(Cond::Eq | Cond::Ne | Cond::Lt | Cond::Ge, _, d, _, _)) if *d == c) {
                return None;
            }
        }
        for i in &block.insts {
            match *i {
                // a float loop's lane test of a 32-bit value against zero (a float's sign bit)
                Inst::ICmpI(c @ (Cond::Eq | Cond::Ne), _, d, s, 0) if store.is_none() && kind == Some(Lane::F32) && matches!(sv[s.0 as usize], Sv::Lane(_)) && !matches!(block.term, Term::Branch(cv, ..) if cv == d) => {
                    let x = lane_of!(s, Wt::I);
                    wops.push(WOp::Const(0));
                    wops.push(WOp::Int(VX::ICmp(c, VW::S), x, wops.len() - 1));
                    sv[d.0 as usize] = Sv::Lane(wops.len() - 1);
                }
                Inst::Mov(d, s) => match sv[s.0 as usize] {
                    Sv::No => return None,
                    x => {
                        if f.vregs[d.0 as usize] != f.vregs[s.0 as usize] { return None; }
                        sv[d.0 as usize] = x;
                    }
                },
                Inst::IBinI(op @ (IOp::Add | IOp::Sub), it, d, s, k) if it == U64_TY => {
                    let Sv::Addr(r, off) = sv[s.0 as usize] else { return None };
                    let Some(off) = (if op == IOp::Add { off.checked_add(k) } else { off.checked_sub(k) }) else { return None };
                    sv[d.0 as usize] = Sv::Addr(r, off);
                }
                Inst::IBinI(IOp::Shl, it, d, s, k @ 1..=3) if it == U64_TY => {
                    let Sv::Addr(r, off) = sv[s.0 as usize] else { return None };
                    if !roots.contains(&r) { return None; }
                    sv[d.0 as usize] = Sv::Scaled(r, off, k as u8);
                }
                Inst::IBin(IOp::Add, it, d, a, b2) if it == U64_TY && (matches!(sv[a.0 as usize], Sv::Scaled(..)) || matches!(sv[b2.0 as usize], Sv::Scaled(..))) => {
                    let (Sv::Addr(base, 0), Sv::Scaled(iv, off, sh)) = (match (sv[a.0 as usize], sv[b2.0 as usize]) {
                        (x @ Sv::Addr(..), y @ Sv::Scaled(..)) => (x, y),
                        (y @ Sv::Scaled(..), x @ Sv::Addr(..)) => (x, y),
                        _ => return None,
                    }) else { return None };
                    let Some(disp) = off.checked_mul(1 << sh) else { return None };
                    if !roots.contains(&base) || base == iv { return None; }
                    sv[d.0 as usize] = Sv::Ptr(base, iv, disp, sh);
                }
                Inst::ICmp(c @ (Cond::Lt | Cond::Ge), sg, d, a, b2) => {
                    let Term::Branch(cv, t, e) = block.term else { return None };
                    if cv != d || (if c == Cond::Lt { !members[t as usize] || members[e as usize] } else { members[t as usize] || !members[e as usize] }) {
                        return None;
                    }
                    let (Sv::Addr(ra, oa), Sv::Addr(rb, 0)) = (sv[a.0 as usize], sv[b2.0 as usize]) else { return None };
                    if !roots.contains(&ra) || !roots.contains(&rb) { return None; }
                    lt_guards.push((ra, oa, Bound::Reg(rb), sg));
                }
                Inst::ICmpI(c @ (Cond::Lt | Cond::Ge), sg, d, a, k) => {
                    let Term::Branch(cv, t, e) = block.term else { return None };
                    if cv != d || (if c == Cond::Lt { !members[t as usize] || members[e as usize] } else { members[t as usize] || !members[e as usize] }) {
                        return None;
                    }
                    let Sv::Addr(ra, oa) = sv[a.0 as usize] else { return None };
                    if !roots.contains(&ra) { return None; }
                    lt_guards.push((ra, oa, Bound::Imm(k), sg));
                }
                Inst::ICmpI(c @ (Cond::Eq | Cond::Ne), false, d, s, 0) => {
                    let Term::Branch(cv, t, e) = block.term else { return None };
                    if cv != d || (if c == Cond::Ne { !members[t as usize] || members[e as usize] } else { members[t as usize] || !members[e as usize] }) {
                        return None;
                    }
                    let Sv::Addr(r, o) = sv[s.0 as usize] else { return None };
                    if !roots.contains(&r) { return None; }
                    zero_guards.push((r, o));
                }
                Inst::ICmp(c @ (Cond::Eq | Cond::Ne), false, d, a, b2) => {
                    let Term::Branch(cv, t, e) = block.term else { return None };
                    if cv != d || (if c == Cond::Ne { !members[t as usize] || members[e as usize] } else { members[t as usize] || !members[e as usize] }) {
                        return None;
                    }
                    let (Sv::Addr(ra, oa), Sv::Addr(rb, ob)) = (sv[a.0 as usize], sv[b2.0 as usize]) else { return None };
                    if !roots.contains(&ra) || !roots.contains(&rb) { return None; }
                    end_guards.push((ra, oa, rb, ob));
                }
                Inst::Iconst(d, k) if store.is_none() => sv[d.0 as usize] = Sv::K(k as u32),
                Inst::Fconst(d, bits, false) if store.is_none() => sv[d.0 as usize] = Sv::K(bits as u32),
                // a re-extension of a 32-bit value keeps its low 32 bits
                Inst::Conv(Conv::IntToInt(it), d, s) if store.is_none() && it.bits == 32 && matches!(sv[s.0 as usize], Sv::Lane(_)) && kind == Some(Lane::I32) => sv[d.0 as usize] = sv[s.0 as usize],
                Inst::Load(m, d, p, off) if store.is_none() => {
                    let k = match m { Mem::Int(1, false) => Lane::Byte, Mem::F32 => Lane::F32, Mem::Int(4, _) => Lane::I32, _ => return None };
                    if kind.is_some_and(|o| o != k) { return None; }
                    kind = Some(k);
                    let Some(st) = stream_at!(p, off) else { return None };
                    wops.push(WOp::Load(st));
                    loads.push((st, wops.len() - 1));
                    sv[d.0 as usize] = Sv::Lane(wops.len() - 1);
                }
                Inst::IBin(op @ (IOp::Add | IOp::Sub | IOp::And | IOp::Or | IOp::Xor | IOp::Mul), it, d, a, b2) if store.is_none() && matches!(kind, Some(Lane::Byte | Lane::I32)) && it.bits as i64 == 8 * kind.unwrap().bytes() => {
                    let k = kind.unwrap();
                    if k == Lane::Byte && op == IOp::Mul { return None; }
                    let (x, y) = (operand!(a, k), operand!(b2, k));
                    let w = if k == Lane::Byte { VW::B } else { VW::S };
                    let o = match op { IOp::Add => VX::Add(w), IOp::Sub => VX::Sub(w), IOp::Mul => VX::MulLo(w), IOp::And => VX::And, IOp::Or => VX::Or, _ => VX::Xor };
                    wops.push(WOp::Int(o, x, y));
                    sv[d.0 as usize] = Sv::Lane(wops.len() - 1);
                }
                Inst::IBinI(op @ (IOp::Add | IOp::Sub | IOp::And | IOp::Or | IOp::Xor | IOp::Mul), it, d, a, k) if store.is_none() && kind == Some(Lane::I32) && it.bits == 32 => {
                    let x = operand!(a, Lane::I32);
                    wops.push(WOp::Const(k as u32));
                    let c = wops.len() - 1;
                    let o = match op { IOp::Add => VX::Add(VW::S), IOp::Sub => VX::Sub(VW::S), IOp::Mul => VX::MulLo(VW::S), IOp::And => VX::And, IOp::Or => VX::Or, _ => VX::Xor };
                    wops.push(WOp::Int(o, x, c));
                    sv[d.0 as usize] = Sv::Lane(wops.len() - 1);
                }
                // 32-bit shifts by a constant below 32: the lane shift (immediate forms)
                Inst::IBinI(op @ (IOp::Shl | IOp::Shr), it, d, a, k) if store.is_none() && kind == Some(Lane::I32) && it.bits == 32 && (1..32).contains(&k) => {
                    let x = operand!(a, Lane::I32);
                    let o = match (op, it.signed) {
                        (IOp::Shl, _) => VX::ShlI(VW::S, k as u8),
                        (_, false) => VX::ShrUI(VW::S, k as u8),
                        _ => VX::ShrSI(VW::S, k as u8),
                    };
                    wops.push(WOp::Un(o, x));
                    sv[d.0 as usize] = Sv::Lane(wops.len() - 1);
                }
                Inst::FBin(op, false, d, a, b2) if store.is_none() && kind == Some(Lane::F32) => {
                    let o = match op { FOp::Add => VX::FAdd(VW::S, NanBits::Host), FOp::Sub => VX::FSub(VW::S, NanBits::Host), FOp::Mul => VX::FMul(VW::S, NanBits::Host), FOp::Div => VX::FDiv(VW::S, NanBits::Host), FOp::MinLt => VX::FMinLt(VW::S), FOp::MaxGt => VX::FMaxGt(VW::S), _ => return None };
                    let (x, y) = (lane_of!(a, Wt::F), lane_of!(b2, Wt::F));
                    wops.push(WOp::Bin(o, x, y));
                    sv[d.0 as usize] = Sv::Lane(wops.len() - 1);
                }
                Inst::FUnary(op, false, d, a) if store.is_none() && kind == Some(Lane::F32) => {
                    let o = match op { FUn::Neg => VX::FNeg(VW::S), FUn::Abs => VX::FAbs(VW::S), FUn::Sqrt => VX::FSqrt(VW::S, NanBits::Host), _ => return None };
                    let x = lane_of!(a, Wt::F);
                    wops.push(WOp::Un(o, x));
                    sv[d.0 as usize] = Sv::Lane(wops.len() - 1);
                }
                // a float loop's compares, their and / or / not, selects by them, and a float's
                // bits as an integer (`is_sign_negative`): masks and blends per lane
                Inst::FCmp(c, false, d, a, b2) if store.is_none() && kind == Some(Lane::F32) => {
                    let (x, y) = (lane_of!(a, Wt::F), lane_of!(b2, Wt::F));
                    wops.push(WOp::Bin(VX::FCmp(c, VW::S), x, y));
                    sv[d.0 as usize] = Sv::Lane(wops.len() - 1);
                }
                Inst::Conv(c @ (Conv::FToBits(false) | Conv::BitsToF(false)), d, s) if store.is_none() && kind == Some(Lane::F32) => {
                    let (from, to) = if matches!(c, Conv::FToBits(_)) { (Wt::F, Wt::I) } else { (Wt::I, Wt::F) };
                    let Sv::Lane(_) = sv[s.0 as usize] else { return None };
                    let x = lane_of!(s, from);
                    wops.push(WOp::Same(x, to));
                    sv[d.0 as usize] = Sv::Lane(wops.len() - 1);
                }
                Inst::IBin(op @ (IOp::And | IOp::Or | IOp::Xor), it, d, a, b2) if store.is_none() && kind == Some(Lane::F32) && matches!(it.bits, 8 | 32) => {
                    let t = if it.bits == 32 { Wt::I } else { Wt::M };
                    let (x, y) = (lane_of!(a, t), lane_of!(b2, t));
                    wops.push(WOp::Int(match op { IOp::And => VX::And, IOp::Or => VX::Or, _ => VX::Xor }, x, y));
                    sv[d.0 as usize] = Sv::Lane(wops.len() - 1);
                }
                Inst::IBinI(op @ (IOp::And | IOp::Or | IOp::Xor), it, d, a, k) if store.is_none() && kind == Some(Lane::F32) && (it.bits == 32 || it.bits == 8 && op == IOp::Xor && k == 1) => {
                    let (t, c) = if it.bits == 32 { (Wt::I, k as u32) } else { (Wt::M, u32::MAX) };
                    let x = lane_of!(a, t);
                    wops.push(WOp::Const(c));
                    wops.push(WOp::Int(match op { IOp::And => VX::And, IOp::Or => VX::Or, _ => VX::Xor }, x, wops.len() - 1));
                    sv[d.0 as usize] = Sv::Lane(wops.len() - 1);
                }
                Inst::Sel(d, c, a, b2) if store.is_none() && kind == Some(Lane::F32) => {
                    let m = lane_of!(c, Wt::M);
                    let t = if f.vregs[d.0 as usize] == Cls::F32 { Wt::F } else { Wt::I };
                    let (x, y) = (lane_of!(a, t), lane_of!(b2, t));
                    wops.push(WOp::Sel(m, x, y));
                    sv[d.0 as usize] = Sv::Lane(wops.len() - 1);
                }
                Inst::Store(m, p, off, s) if store.is_none() => {
                    let k = match m { Mem::Int(1, false) => Lane::Byte, Mem::F32 => Lane::F32, Mem::Int(4, _) => Lane::I32, _ => return None };
                    if kind != Some(k) { return None; }
                    let Some(st) = stream_at!(p, off) else { return None };
                    let Sv::Lane(w) = sv[s.0 as usize] else { return None };
                    if k == Lane::F32 && !matches!(wtype(&wops, &f.vregs, k, w), Wt::F | Wt::Any) { return None; }
                    store = Some((st, w));
                }
                Inst::SlotAddr(d, _) if slots.iter().any(|s| s.0 == d) => {}
                _ => return None,
            }
        }
        // the store ends the body: only its block's own exit test follows (`p == end` of the
        // next element), and the sole back edge leaves that block
        if store.is_some() && b != *chain.last().unwrap() { return None; }
    }
    let (Some((dst, value)), Some(kind)) = (store, kind) else { return None };
    if loads.is_empty() { return None; }
    relax_backend::remark::loop_stage(2);
    let sz = kind.bytes();
    let mut p = Plan {
        head, nv, chain, members, roots, slots, defined, inputs, definitions, sv, kind,
        loads, store: (dst, value), wops, zero_guards, end_guards, lt_guards,
        before_last, last, sz, counts: Vec::new(), walkers: Vec::new(), ivs: Vec::new(),
    };
    let streams: Vec<Stream> = p.loads.iter().map(|x| x.0).chain(std::iter::once(dst)).collect();
    // indexed streams scale by the element size
    if streams.iter().any(|s| s.iv.is_some() && 1i64 << s.sh != p.sz) { return None; }
    relax_backend::remark::loop_stage(3);
    let ptr_roots: Vec<VReg> = streams.iter().filter(|s| s.iv.is_none()).map(|s| s.root).collect();
    let base_roots: Vec<VReg> = streams.iter().filter(|s| s.iv.is_some()).map(|s| s.root).collect();
    let mut iv_roots: Vec<VReg> = streams.iter().filter_map(|s| s.iv).collect();
    iv_roots.extend(p.lt_guards.iter().map(|g| g.0));
    // an order test of a pointer or an equality test with the wrong roles: leave the loop
    if p.lt_guards.iter().any(|g| ptr_roots.contains(&g.0)) { return None; }
    let walks = |v: VReg| matches!(p.sv[v.0 as usize], Sv::Addr(r, s) if r == v && (s == p.sz || s == 1));
    for g in p.end_guards.iter_mut() {
        if walks(g.2) && !walks(g.0) {
            *g = (g.2, g.3, g.0, g.1);
        }
    }
    // The loop inputs: pointers that step one element per iteration, counters that step +1
    // (indices) or -1 (lengths), loop-invariant values (bases, bounds, end pointers, scalars
    // the body computes with).
    for &v in &p.roots {
        let fin = p.sv[v.0 as usize];
        let is_i = f.vregs[v.0 as usize] == Cls::I;
        let guarded_end = p.end_guards.iter().any(|g| g.0 == v);
        let guarded_zero = p.zero_guards.iter().any(|g| g.0 == v);
        match fin {
            Sv::Addr(r, -1) if r == v && is_i && guarded_zero && !ptr_roots.contains(&v) && !iv_roots.contains(&v) && !base_roots.contains(&v) => p.counts.push(v),
            Sv::Addr(r, step) if r == v && is_i && ptr_roots.contains(&v) && !iv_roots.contains(&v) && !base_roots.contains(&v) && step == p.sz => p.walkers.push(v),
            Sv::Addr(r, 1) if r == v && is_i && iv_roots.contains(&v) && !ptr_roots.contains(&v) && !base_roots.contains(&v) => p.ivs.push(v),
            // a pointer or index only the guards look at
            Sv::Addr(r, step) if r == v && is_i && step == p.sz && (guarded_end || guarded_zero) && !iv_roots.contains(&v) && !base_roots.contains(&v) => p.walkers.push(v),
            Sv::Addr(r, 1) if r == v && is_i && guarded_end && !base_roots.contains(&v) => p.ivs.push(v),
            // invariant: never defined in the loop (or a slot's address)
            Sv::Addr(r, 0) if r == v && (p.definitions[v.0 as usize] == 0 || p.slots.iter().any(|s| s.0 == v)) && !ptr_roots.contains(&v) && !iv_roots.contains(&v) => {}
            _ => return None,
        }
    }
    // a zero test of a counter or a pointer: fine; of an invariant it is a loop that never ends
    // or exits at once. Offsets: a counter is tested after its steps (<= 0), a pointer ahead
    for &(r, o) in &p.zero_guards {
        if !(p.counts.contains(&r) && o <= 0 || p.walkers.contains(&r) && o >= 0) { return None; }
    }
    // every end guard compares a walking pointer or index (ahead of its entry value, whole
    // elements) with an invariant
    for &(a, oa, b2, ob) in &p.end_guards {
        let step = if p.walkers.contains(&a) { p.sz } else { 1 };
        if !((p.walkers.contains(&a) || p.ivs.contains(&a)) && !p.walkers.contains(&b2) && !p.ivs.contains(&b2) && !p.counts.contains(&b2) && ob == 0 && oa >= 0 && oa % step == 0) { return None; }
    }
    for &(a, oa, b2, _) in &p.lt_guards {
        if !(p.ivs.contains(&a) && oa >= 0 && match b2 { Bound::Reg(r) => !p.walkers.contains(&r) && !p.ivs.contains(&r) && !p.counts.contains(&r), Bound::Imm(_) => true }) { return None; }
    }
    if p.counts.is_empty() && p.end_guards.is_empty() && p.lt_guards.is_empty() { return None; }
    relax_backend::remark::loop_stage(4);
    Some(p)
}

/// Check static dependence and live-out temporaries, then decide the scalar tail.
fn legal(f: &Func, p: &mut Plan<'_>, span: i64, reach: &[bool], preds: &[Vec<u32>], reads: &mut Option<Vec<Vec<VReg>>>) -> Option<Checks> {
    let Plan { nv, chain, members, defined, inputs, loads, store: (dst, _), last, before_last,
        zero_guards, end_guards, lt_guards, counts, walkers, sz, .. } = p;
    let (nv, last, before_last, sz) = (*nv, *last, *before_last, *sz);
    // dependencies between the store and the loads that a vector would reorder, when their
    // distance is known
    for l in loads.iter().map(|x| x.0) {
        if l.root == dst.root && l.iv == dst.iv && l.sh == dst.sh && dst.disp - l.disp > 0 && dst.disp - l.disp < span { return None; }
    }
    // The scalar temporaries are not produced by the widened body. All observable
    // loop-carried values must be precisely the pointer and length recurrences -- except
    // where the loop is left by a guard: the vector loop goes on only into the scalar one,
    // so a guard's exit always follows the scalar body up to that guard in the same
    // iteration, and what the body defined before it is current (a bounds check's panic
    // reading the index).
    let mut def_at = vec![u32::MAX; nv];
    for (k, &b) in chain.iter().enumerate() {
        for i in &f.blocks[b as usize].insts {
            for_defs(i, |v| def_at[v.0 as usize] = k as u32);
        }
    }
    let at_of = |b: u32| chain.iter().position(|&c| c == b).map(|k| k as u32);
    let reads = reads.get_or_insert_with(|| reads_before_defs(f));
    // a loop temporary read in `block` before it defines it again (but for those `current`)
    let reads_temporary = |block: usize, current: &dyn Fn(VReg) -> bool| {
        reads[block].iter().any(|v| (v.0 as usize) < nv && defined[v.0 as usize] && !inputs[v.0 as usize] && !current(*v))
    };
    for b in 0..f.blocks.len() {
        // (a block nothing reaches never runs: a bounds check's panic once the check folded)
        if b < members.len() && members[b] || b < reach.len() && !reach[b] { continue; }
        // left only from the chain: the earliest exit's position
        let from = preds.get(b).and_then(|ps| ps.iter().map(|&p| at_of(p)).try_fold(u32::MAX, |m, k| k.map(|k| m.min(k))));
        if reads_temporary(b, &|v| from.is_some_and(|k| k != u32::MAX && def_at[v.0 as usize] <= k)) { return None; }
    }
    // The back edge's own test (the last block's guard, after all of the iteration's work)
    // for the vector's last element is made after the vector loop, by `tail`: on into the
    // scalar loop, or out of it to where that test leaves -- when what the loop leaves to
    // reads none of its temporaries. Otherwise the vector loop always leaves at least one
    // element (that test's) to the scalar loop, which a rotated loop's head runs untested.
    let latch_exit = match f.blocks[last as usize].term {
        Term::Branch(_, t, e) => Some(if members[t as usize] { e } else { t }),
        _ => None,
    };
    let latch_exit = latch_exit.filter(|&x| !reads_temporary(x as usize, &|_| false));
    // That test (the last guard made) then counts one element more, the vector's last,
    // when it tests a recurrence a step ahead of the iteration's entry value.
    let mut tail = false;
    if latch_exit.is_some() {
        if zero_guards.len() > before_last.0 {
            let g = zero_guards.last_mut().unwrap();
            if counts.contains(&g.0) && g.1 <= -1 {
                g.1 += 1;
                tail = true;
            }
        } else if end_guards.len() > before_last.1 {
            let g = end_guards.last_mut().unwrap();
            let step = if walkers.contains(&g.0) { sz } else { 1 };
            if g.1 >= step {
                g.1 -= step;
                tail = true;
            }
        } else if lt_guards.len() > before_last.2 {
            let g = lt_guards.last_mut().unwrap();
            if g.1 >= 1 {
                g.1 -= 1;
                tail = true;
            }
        }
    }
    Some(Checks { tail })
}

/// Choose today's vector width and entry threshold; every legal plan has a shape.
fn cost(options: &relax_session::Options, p: &Plan<'_>) -> Option<Shape> {
    let lanes = 16 / p.sz;
    let shift = p.sz.trailing_zeros() as i64;
    // AVX2 (x64): an iteration takes two vectors per stream, side by side, which pair256
    // makes one 256-bit vector; one vector more when that many elements are left
    let pair = options.cpu.avx2 && p.kind != Lane::Byte;
    // the bytes of a stream an iteration reads or writes
    let span = if pair { 32 } else { 16 };
    Some(Shape { pair, span, lanes, shift })
}

/// Build the guards and vector blocks, preserving scalar allocation and block order.
fn emit(f: &mut Func, p: Plan<'_>, checks: Checks, shape: Shape) {
    let Plan { head, members, slots, sv, loads, store: (dst, value), wops,
        zero_guards, end_guards, lt_guards, last, sz, counts, walkers, ivs, .. } = p;
    let Checks { tail } = checks;
    let Shape { pair, span, lanes, shift } = shape;
    // entry: one guard's run length against a vector's elements, so a short run (the common
    // entry of a loop over a few elements) goes to the scalar loop after two instructions;
    // guard: every check (any one failing also leaves for the scalar loop); then the vector loop
    let entry = f.blocks.len() as u32;
    let guard = entry + 1;
    let pos = f.blocks[head as usize].term_pos;
    let mut pre = Vec::new();
    let long = f.vreg(Cls::I);
    if let Some(&(p, oa, e, _)) = end_guards.first() {
        // bytes (an index: elements) left to `e`, `oa` of them ahead of the entry value
        let step = if walkers.contains(&p) { sz } else { 1 };
        let left = f.vreg(Cls::I);
        pre.push(Inst::IBin(IOp::Sub, U64_TY, left, e, p));
        pre.push(Inst::ICmpI(Cond::Ge, false, long, left, (lanes + oa / step) * step));
    } else if let Some(&(c, o)) = zero_guards.iter().find(|g| counts.contains(&g.0)) {
        pre.push(Inst::ICmpI(Cond::Ge, false, long, c, lanes + (-o).max(0)));
    } else if let Some(&(iv, o, nb, _)) = lt_guards.first() {
        let n = match nb {
            Bound::Reg(r) => r,
            Bound::Imm(k) => {
                let n = f.vreg(Cls::I);
                pre.push(Inst::Iconst(n, k));
                n
            }
        };
        let left = f.vreg(Cls::I);
        pre.push(Inst::IBin(IOp::Sub, U64_TY, left, n, iv));
        pre.push(Inst::ICmpI(Cond::Ge, false, long, left, lanes + o.max(0)));
    } else {
        pre.push(Inst::Iconst(long, 1));
    }
    let trip = f.vreg(Cls::I);
    let mut init = vec![Inst::Iconst(trip, -1)];
    let min_into = |f: &mut Func, init: &mut Vec<Inst>, trip: VReg, c: VReg| {
        let less = f.vreg(Cls::I);
        init.push(Inst::ICmp(Cond::Lt, false, less, c, trip));
        init.push(Inst::Sel(trip, less, c, trip));
    };
    let ok = f.vreg(Cls::I);
    init.push(Inst::Iconst(ok, 1));
    // `ok &= cond` for a compare of `a` with a constant / register
    let and_ok = |f: &mut Func, init: &mut Vec<Inst>, ok: VReg, i: Inst| {
        let _ = f;
        let (Inst::ICmp(_, _, c, ..) | Inst::ICmpI(_, _, c, ..)) = i else { unreachable!() };
        init.push(i);
        init.push(Inst::IBin(IOp::And, U64_TY, ok, ok, c));
    };
    // a counter tested after `-o` steps must have them to give
    for &(c, o) in &zero_guards {
        if !counts.contains(&c) { continue; }
        if o < 0 {
            let (enough, left) = (f.vreg(Cls::I), f.vreg(Cls::I));
            and_ok(f, &mut init, ok, Inst::ICmpI(Cond::Ge, false, enough, c, -o));
            init.push(Inst::IBinI(IOp::Add, U64_TY, left, c, o));
            min_into(f, &mut init, trip, left);
        } else {
            min_into(f, &mut init, trip, c);
        }
    }
    // a pointer or index that reaches its end in whole elements (the scalar guard is `==`),
    // `oa` bytes (elements for an index) ahead of its entry value
    for &(p, oa, e, _) in &end_guards {
        let (step, sh) = if walkers.contains(&p) { (sz, shift) } else { (1, 0) };
        let (diff, cnt, rem, left) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        let (ord, aligned, enough) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        init.push(Inst::IBin(IOp::Sub, U64_TY, diff, e, p));
        and_ok(f, &mut init, ok, Inst::ICmp(Cond::Le, false, ord, p, e));
        if step > 1 {
            init.push(Inst::IBinI(IOp::And, U64_TY, rem, diff, step - 1));
            and_ok(f, &mut init, ok, Inst::ICmpI(Cond::Eq, false, aligned, rem, 0));
            init.push(Inst::IBinI(IOp::Shr, U64_TY, cnt, diff, sh));
        } else {
            init.push(Inst::Mov(cnt, diff));
        }
        let ahead = oa / step;
        if ahead > 0 {
            and_ok(f, &mut init, ok, Inst::ICmpI(Cond::Ge, false, enough, cnt, ahead));
            init.push(Inst::IBinI(IOp::Sub, U64_TY, left, cnt, ahead));
            min_into(f, &mut init, trip, left);
        } else {
            min_into(f, &mut init, trip, cnt);
        }
    }
    // an index below its bound: the loop goes on for `bound - index - o` more elements
    for &(iv, o, nb, signed) in &lt_guards {
        let n = match nb {
            Bound::Reg(r) => r,
            Bound::Imm(k) => {
                let n = f.vreg(Cls::I);
                init.push(Inst::Iconst(n, k));
                n
            }
        };
        let (ord, avail, enough, left) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        if signed {
            // both non-negative: the signed order is the unsigned one
            let (any, hi, pos0) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
            init.push(Inst::IBin(IOp::Or, U64_TY, any, iv, n));
            init.push(Inst::IBinI(IOp::Shr, U64_TY, hi, any, 63));
            and_ok(f, &mut init, ok, Inst::ICmpI(Cond::Eq, false, pos0, hi, 0));
        }
        and_ok(f, &mut init, ok, Inst::ICmp(Cond::Le, false, ord, iv, n));
        init.push(Inst::IBin(IOp::Sub, U64_TY, avail, n, iv));
        if o > 0 {
            and_ok(f, &mut init, ok, Inst::ICmpI(Cond::Ge, false, enough, avail, o));
            init.push(Inst::IBinI(IOp::Sub, U64_TY, left, avail, o));
            min_into(f, &mut init, trip, left);
        } else {
            min_into(f, &mut init, trip, avail);
        }
    }
    // A pointer guard may stop at zero. Cap the fast run before any pointer
    // wraps, even for RIR not originating from Rust's in-bounds pointer add (also a pointer
    // tested `o` bytes ahead of its entry value).
    for &p in &walkers {
        let ahead = zero_guards.iter().filter(|g| g.0 == p).map(|g| g.1).max().unwrap_or(0);
        let (until_zero, elems) = (f.vreg(Cls::I), f.vreg(Cls::I));
        init.push(Inst::INeg(U64_TY, until_zero, p));
        let from = if ahead > 0 {
            let (enough, left) = (f.vreg(Cls::I), f.vreg(Cls::I));
            and_ok(f, &mut init, ok, Inst::ICmpI(Cond::Ge, false, enough, until_zero, ahead));
            init.push(Inst::IBinI(IOp::Sub, U64_TY, left, until_zero, ahead));
            left
        } else {
            until_zero
        };
        if sz > 1 {
            init.push(Inst::IBinI(IOp::Shr, U64_TY, elems, from, shift));
        } else {
            init.push(Inst::Mov(elems, from));
        }
        min_into(f, &mut init, trip, elems);
    }
    let enough = f.vreg(Cls::I);
    and_ok(f, &mut init, ok, Inst::ICmpI(Cond::Ge, false, enough, trip, lanes));
    // the address of each stream's first element
    let mut eff: Vec<(Stream, VReg)> = Vec::new();
    let mut eff_scaled: Vec<(VReg, u8, VReg)> = Vec::new();
    let mut eff_of = |f: &mut Func, init: &mut Vec<Inst>, st: Stream| -> VReg {
        if let Some(e) = eff.iter().find(|e| e.0 == st) {
            return e.1;
        }
        let mut a = st.root;
        if let Some(iv) = st.iv {
            let t = match eff_scaled.iter().find(|x| x.0 == iv && x.1 == st.sh) {
                Some(x) => x.2,
                None => {
                    let t = f.vreg(Cls::I);
                    init.push(Inst::IBinI(IOp::Shl, U64_TY, t, iv, st.sh as i64));
                    eff_scaled.push((iv, st.sh, t));
                    t
                }
            };
            let u = f.vreg(Cls::I);
            init.push(Inst::IBin(IOp::Add, U64_TY, u, st.root, t));
            a = u;
        }
        if st.disp != 0 {
            let u = f.vreg(Cls::I);
            init.push(Inst::IBinI(IOp::Add, U64_TY, u, a, st.disp));
            a = u;
        }
        eff.push((st, a));
        a
    };
    // A store into a private stack slot and a load from any other pointer (or a load from
    // one and a store elsewhere) never overlap: no check between them
    let private = private_slots(f);
    let apart = |a: VReg, b: VReg| private.as_ref().is_some_and(|(der, private)| {
        let of = |v: VReg| der.get(v.0 as usize).copied().unwrap_or(u32::MAX);
        let (x, y) = (of(a), of(b));
        x != y && [x, y].iter().any(|&s| s < u32::MAX - 1 && private[s as usize])
    });
    for l in loads.iter().map(|x| x.0) {
        if l == dst || apart(l.root, dst.root) { continue; }
        let d_eff = eff_of(f, &mut init, dst);
        let s_eff = eff_of(f, &mut init, l);
        let (distance, far, same, safe) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        // Only forward dependencies matter: loads precede the store and a
        // destination behind the source cannot overwrite a later input.
        init.push(Inst::IBin(IOp::Sub, U64_TY, distance, d_eff, s_eff));
        init.push(Inst::ICmpI(Cond::Ge, false, far, distance, span));
        init.push(Inst::ICmp(Cond::Eq, false, same, d_eff, s_eff));
        init.push(Inst::IBin(IOp::Or, U64_TY, safe, far, same));
        init.push(Inst::IBin(IOp::And, U64_TY, ok, ok, safe));
    }
    // the invariant scalars and constants in every lane, made in `at`: each goes into a vector
    // once, however many operations read it (each copy would hold a register across the loop)
    let invariants = |f: &mut Func, at: &mut Vec<Inst>| -> Vec<Option<VReg>> {
        let mut inv: Vec<Option<VReg>> = vec![None; wops.len()];
        for (k, w) in wops.iter().enumerate() {
            let made = match *w {
                WOp::Splat(r) => wops[..k].iter().position(|x| matches!(*x, WOp::Splat(q) if q == r)),
                WOp::Const(c) => wops[..k].iter().position(|x| matches!(*x, WOp::Const(d) if d == c)),
                _ => continue,
            };
            inv[k] = Some(match made {
                Some(j) => inv[j].expect("made before"),
                None => {
                    let v = f.vreg(Cls::V128);
                    at.push(match *w {
                        WOp::Splat(r) => Inst::VSplat(v, r),
                        WOp::Const(c) => Inst::VConst(v, c),
                        _ => unreachable!(),
                    });
                    v
                }
            });
        }
        inv
    };
    // a vector iteration of `n` vectors per stream (the n vectors of a value made one after
    // another, each operation's in a row) over the invariants `inv` (None: its own, made
    // first), stepping the streams by as many elements; per iteration the addresses of the
    // indexed streams
    let iteration = |f: &mut Func, n: usize, inv: Option<&[Option<VReg>]>| -> Vec<Inst> {
        let mut wide = Vec::new();
        let own;
        let inv = match inv {
            Some(inv) => inv,
            None => {
                own = invariants(f, &mut wide);
                &own
            }
        };
        let mut scaled: Vec<(VReg, u8, VReg)> = Vec::new();
        let mut bases: Vec<(VReg, VReg, u8, VReg)> = Vec::new();
        let mut addr_of = |f: &mut Func, wide: &mut Vec<Inst>, st: Stream| -> VReg {
            let Some(iv) = st.iv else { return st.root };
            if let Some(b) = bases.iter().find(|b| b.0 == st.root && b.1 == iv && b.2 == st.sh) {
                return b.3;
            }
            let t = match scaled.iter().find(|x| x.0 == iv && x.1 == st.sh) {
                Some(x) => x.2,
                None => {
                    let t = f.vreg(Cls::I);
                    wide.push(Inst::IBinI(IOp::Shl, U64_TY, t, iv, st.sh as i64));
                    scaled.push((iv, st.sh, t));
                    t
                }
            };
            let u = f.vreg(Cls::I);
            wide.push(Inst::IBin(IOp::Add, U64_TY, u, st.root, t));
            bases.push((st.root, iv, st.sh, u));
            u
        };
        let mut vectors: Vec<Vec<VReg>> = Vec::new();
        for (k, w) in wops.iter().enumerate() {
            if let Some(v) = inv[k] {
                vectors.push(vec![v; n]);
                continue;
            }
            let vs: Vec<VReg> = (0..n).map(|_| f.vreg(Cls::V128)).collect();
            for (h, &v) in vs.iter().enumerate() {
                match *w {
                    WOp::Load(st) => {
                        let a = addr_of(f, &mut wide, st);
                        wide.push(Inst::VLoad(v, a, (st.disp + 16 * h as i64) as i32));
                    }
                    WOp::Bin(o, a, b2) | WOp::Int(o, a, b2) => wide.push(Inst::VX(o, v, vectors[a][h], vectors[b2][h])),
                    WOp::Un(o, a) => wide.push(Inst::VX(o, v, vectors[a][h], vectors[a][h])),
                    WOp::Sel(m, a, b2) => wide.push(Inst::VSel(v, vectors[m][h], vectors[a][h], vectors[b2][h])),
                    WOp::Same(a, _) => wide.push(Inst::Mov(v, vectors[a][h])),
                    WOp::Splat(_) | WOp::Const(_) => unreachable!("made once ahead"),
                }
            }
            vectors.push(vs);
        }
        let a = addr_of(f, &mut wide, dst);
        for h in 0..n {
            wide.push(Inst::VStore(a, (dst.disp + 16 * h as i64) as i32, vectors[value][h]));
        }
        let n = n as i64;
        for &p in &walkers { wide.push(Inst::IBinI(IOp::Add, U64_TY, p, p, 16 * n)); }
        for &c in &counts { wide.push(Inst::IBinI(IOp::Sub, U64_TY, c, c, lanes * n)); }
        for &i in &ivs { wide.push(Inst::IBinI(IOp::Add, U64_TY, i, i, lanes * n)); }
        wide.push(Inst::IBinI(IOp::Sub, U64_TY, trip, trip, lanes * n));
        wide
    };
    // (pair: the two-vector loop's invariants are made ahead of it, as 256-bit vectors; the
    // one vector after it makes its own, so that no 128-bit copy is held across the loop)
    let inv = invariants(f, &mut init);
    let (mut one, two) = if pair {
        (iteration(f, 1, None), Some(iteration(f, 2, Some(&inv))))
    } else {
        (iteration(f, 1, Some(&inv)), None)
    };
    // tail: the back edge's test of the vector's last element, its operands as they were
    // then (each root now, less the step it took since)
    let mut tested = Vec::new();
    let mut tail_term = None;
    let latch_targets = match f.blocks[last as usize].term {
        Term::Branch(_, t, e) => Some((t, e)),
        _ => None,
    };
    if let (true, Some((t, e))) = (tail, latch_targets) {
        let mut test = f.blocks[last as usize].insts.last().cloned().expect("a guard's test ends its block");
        let mut operands = Vec::new();
        for_uses(&test, |u| operands.push(u));
        let mut then: Vec<(VReg, VReg)> = Vec::new();
        for u in operands {
            let Sv::Addr(r, o) = sv[u.0 as usize] else { unreachable!("a guard tests recurrences and invariants") };
            let step = if walkers.contains(&r) { sz } else if ivs.contains(&r) { 1 } else if counts.contains(&r) { -1 } else { 0 };
            let w = if o == step {
                r
            } else {
                let w = f.vreg(Cls::I);
                tested.push(Inst::IBinI(IOp::Add, U64_TY, w, r, o - step));
                w
            };
            then.push((u, w));
        }
        let cond = f.vreg(Cls::I);
        let mut first = true;
        test.vregs_mut(|v| {
            if first {
                first = false;
                *v = cond;
            } else if let Some(&(_, w)) = then.iter().find(|x| x.0 == *v) {
                *v = w;
            }
        });
        tested.push(test);
        tail_term = Some(Term::Branch(cond, t, e));
    }
    // the blocks after the guard: one vector at a time while a vector's elements are left,
    // or (pair) two at a time while two vectors' are, then one when one vector's are; then
    // the tail or the scalar loop
    let mut made: Vec<(Vec<Inst>, Term)> = Vec::new();
    let at = entry + 2;
    match two {
        None => {
            let after = if tail_term.is_some() { at + 1 } else { head };
            let again = f.vreg(Cls::I);
            one.push(Inst::ICmpI(Cond::Ge, false, again, trip, lanes));
            made.push((one, Term::Branch(again, at, after)));
        }
        Some(mut two) => {
            // at: two vectors' elements left? at + 1: two at a time; at + 2: one vector's
            // left? at + 3: one vector
            let after = if tail_term.is_some() { at + 4 } else { head };
            let (first, again, rest) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
            made.push((vec![Inst::ICmpI(Cond::Ge, false, first, trip, 2 * lanes)], Term::Branch(first, at + 1, at + 3)));
            two.push(Inst::ICmpI(Cond::Ge, false, again, trip, 2 * lanes));
            made.push((two, Term::Branch(again, at + 1, at + 2)));
            made.push((vec![Inst::ICmpI(Cond::Ge, false, rest, trip, lanes)], Term::Branch(rest, at + 3, after)));
            made.push((one, Term::Jump(after)));
        }
    }
    if let Some(term) = tail_term {
        made.push((tested, term));
    }
    // the slots' addresses, made again ahead of the vector code
    for &(d, sl) in &slots {
        let a = f.vreg(Cls::I);
        for i in pre.iter_mut().chain(&mut init).chain(made.iter_mut().flat_map(|m| &mut m.0)) {
            i.vregs_mut(|v| if *v == d { *v = a });
        }
        pre.insert(0, Inst::SlotAddr(a, sl));
    }
    for (bi, block) in f.blocks.iter_mut().enumerate() {
        if bi >= members.len() || !members[bi] {
            block.term.map_succs(|s| if s == head { entry } else { s });
        }
    }
    f.blocks.push(Block { pos: vec![pos; pre.len()], insts: pre, term: Term::Branch(long, guard, head), term_pos: pos, count: -1.0 });
    f.blocks.push(Block { pos: vec![pos; init.len()], insts: init, term: Term::Branch(ok, at, head), term_pos: pos, count: -1.0 });
    for (insts, term) in made {
        f.blocks.push(Block { pos: vec![pos; insts.len()], insts, term, term_pos: pos, count: -1.0 });
    }
    relax_backend::remark::loop_done();
}

/// The stack slots no pointer reaches but those made from their own address: per value the slot
/// it points into (`slot_derivation`: u32::MAX none, u32::MAX - 1 several), and per slot whether
/// its address (and what is computed from it) goes only into loads', stores' and copies'
/// addresses and pure arithmetic -- never stored, returned, mixed with another slot's, a call's
/// argument or an effect's operand. None when the derivation does not settle.
fn private_slots(f: &Func) -> Option<(Vec<u32>, Vec<bool>)> {
    let (der, esc) = crate::opt::slot_derivation(f)?;
    let mut private: Vec<bool> = esc.iter().map(|e| !e).collect();
    for b in &f.blocks {
        for i in &b.insts {
            let access = matches!(i, Inst::Store(..) | Inst::StoreX(..) | Inst::VStore(..) | Inst::VStoreX(..) | Inst::Copy(..));
            if access || !relax_backend::passes::is_mem_effect(i) {
                continue;
            }
            for_uses(i, |v| {
                if let Some(&s) = der.get(v.0 as usize).filter(|s| **s < u32::MAX - 1) {
                    private[s as usize] = false;
                }
            });
        }
    }
    Some((der, private))
}

/// The blocks of the loop at `head` as one cycle from it, each entered only from the one before
/// (the head also from outside) and leaving the loop at most by its guard; None when the body
/// has other control flow.
fn linear_cycle(f: &Func, head: u32, members: &[bool], preds: &[Vec<u32>]) -> Option<Vec<u32>> {
    let mut chain = Vec::new();
    let mut bi = head;
    loop {
        if chain.contains(&bi) {
            if bi != head { return None; }
            break;
        }
        if bi != head && preds[bi as usize].iter().any(|p| !members[*p as usize]) {
            return None;
        }
        chain.push(bi);
        let inside: Vec<_> = f.blocks[bi as usize].term.succs().filter(|s| members.get(*s as usize).copied().unwrap_or(false)).collect();
        if inside.len() != 1 || !matches!(f.blocks[bi as usize].term, Term::Jump(_) | Term::Branch(..)) {
            return None;
        }
        bi = inside[0];
    }
    (chain.len() == members.iter().filter(|x| **x).count()).then_some(chain)
}

/// An instruction that may run where the code did not ask for it: one result, no memory access,
/// no trap (float arithmetic does not trap; integer division does).
fn speculatable(i: &Inst) -> bool {
    match i {
        Inst::Mov(..) | Inst::Iconst(..) | Inst::Fconst(..) | Inst::SlotAddr(..) | Inst::FBin(..) | Inst::FUnary(..) | Inst::FCmp(..) | Inst::ICmp(..) | Inst::ICmpI(..) | Inst::Sel(..) => true,
        Inst::IBin(op, ..) | Inst::IBinI(op, ..) => matches!(op, IOp::Add | IOp::Sub | IOp::Mul | IOp::And | IOp::Or | IOp::Xor | IOp::Shl | IOp::Shr),
        Inst::Conv(c, ..) => matches!(c, Conv::FToBits(_) | Conv::BitsToF(_) | Conv::IntToInt(_)),
        _ => false,
    }
}

/// Whether the loop at `head` could be a stream map but for branches that pick values: one store,
/// no calls or other writes, and some branch inside it whose two ways both stay in the loop.
fn value_branch_loop(f: &Func, head: u32, members: &[bool]) -> bool {
    let mut stores = 0;
    let mut inner = false;
    for (bi, b) in f.blocks.iter().enumerate() {
        if !members.get(bi).copied().unwrap_or(false) { continue; }
        for i in &b.insts {
            match i {
                Inst::Store(Mem::Int(1, false) | Mem::Int(4, _) | Mem::F32, ..) => stores += 1,
                Inst::Load(..) => {}
                i if speculatable(i) => {}
                _ => return false,
            }
        }
        if let Term::Branch(_, t, e) = b.term {
            inner |= t != e && t != head && e != head && members.get(t as usize).copied().unwrap_or(false) && members.get(e as usize).copied().unwrap_or(false);
        }
    }
    stores == 1 && inner
}

/// Folds each value branch of the loop at `head` into its block: a block ending in `branch c`
/// to one or two arms (blocks of `speculatable` instructions entered from it alone, both going
/// on to one join) runs both arms and picks the values they leave with `Sel(c, ..)`, then
/// jumps to the join, which joins it when it has no other way in. Repeated until no branch of
/// that shape is left. The arms' copies of a value one or both define (or that has definitions
/// elsewhere) are fresh registers, so each such value gets one definition, the select. Returns
/// whether anything folded.
fn fold_value_branches(f: &mut Func, head: u32, members: &[bool]) -> bool {
    let mut members = members.to_vec();
    let mut folded = false;
    loop {
        let n = f.blocks.len();
        let mut npred = vec![0u32; n];
        for b in &f.blocks {
            for t in b.term.succs() {
                npred[t as usize] += 1;
            }
        }
        let mut ndef = vec![0u32; f.vregs.len()];
        for p in &f.params {
            ndef[p.0 as usize] += 1;
        }
        for b in &f.blocks {
            for i in &b.insts {
                for_defs(i, |d| ndef[d.0 as usize] += 1);
            }
        }
        let inside = |b: u32| members.get(b as usize).copied().unwrap_or(false);
        // an arm: entered from its branch alone, speculatable, going on to one block
        let arm = |x: u32| -> Option<u32> {
            let b = &f.blocks[x as usize];
            match b.term {
                Term::Jump(j) if inside(x) && x != head && npred[x as usize] == 1 && b.insts.iter().all(speculatable) => Some(j),
                _ => None,
            }
        };
        let mut step: Option<(u32, VReg, Option<u32>, Option<u32>, u32)> = None; // (block, c, arm when c, arm when !c, join)
        for bi in 0..n as u32 {
            if !inside(bi) { continue; }
            let Term::Branch(c, t, e) = f.blocks[bi as usize].term else { continue };
            if t == e || !inside(t) || !inside(e) { continue; }
            step = match (arm(t), arm(e)) {
                (Some(j), Some(k)) if j == k && t != e => Some((bi, c, Some(t), Some(e), j)),
                (Some(j), _) if j == e => Some((bi, c, Some(t), None, e)),
                (_, Some(k)) if k == t => Some((bi, c, None, Some(e), t)),
                _ => None,
            };
            if step.is_some() { break; }
        }
        let Some((bi, c, when_c, when_not, join)) = step else { break };
        folded = true;
        let arms: Vec<u32> = when_c.iter().chain(when_not.iter()).copied().collect();
        // the values the arms define that get fresh copies in them and a select after
        let mut picked: Vec<VReg> = Vec::new();
        let mut in_arm: Vec<(u32, VReg)> = Vec::new();
        for &a in &arms {
            for i in &f.blocks[a as usize].insts {
                for_defs(i, |d| {
                    if !in_arm.contains(&(a, d)) { in_arm.push((a, d)); }
                });
            }
        }
        for &(a, d) in &in_arm {
            let both = in_arm.iter().any(|&(b, e)| b != a && e == d);
            if (both || ndef[d.0 as usize] > 1) && !picked.contains(&d) { picked.push(d); }
        }
        let mut insts = std::mem::take(&mut f.blocks[bi as usize].insts);
        let mut pos = std::mem::take(&mut f.blocks[bi as usize].pos);
        let term_pos = f.blocks[bi as usize].term_pos;
        // the branch's condition, kept when an arm redefines it
        let cond = if picked.contains(&c) {
            let k = f.vreg(f.vregs[c.0 as usize]);
            insts.push(Inst::Mov(k, c));
            pos.push(term_pos);
            k
        } else {
            c
        };
        // each arm's last copy of each picked value
        let mut last: Vec<(u32, VReg, VReg)> = Vec::new();
        for &a in &arms {
            let body = std::mem::take(&mut f.blocks[a as usize].insts);
            let at = std::mem::take(&mut f.blocks[a as usize].pos);
            let mut cur: Vec<(VReg, VReg)> = Vec::new();
            for (i, p) in body.into_iter().zip(at) {
                let mut d = None;
                for_defs(&i, |x| d = Some(x));
                let d = d.expect("a speculatable instruction has one result");
                let nd = if picked.contains(&d) { f.vreg(f.vregs[d.0 as usize]) } else { d };
                let mut j = i;
                let mut first = true;
                j.vregs_mut(|v| {
                    if first {
                        first = false;
                        *v = nd;
                    } else if let Some(&(_, w)) = cur.iter().find(|x| x.0 == *v) {
                        *v = w;
                    }
                });
                if nd != d {
                    cur.retain(|x| x.0 != d);
                    cur.push((d, nd));
                }
                insts.push(j);
                pos.push(p);
            }
            for (d, w) in cur {
                last.push((a, d, w));
            }
            f.blocks[a as usize].term = Term::Unreachable;
            members[a as usize] = false;
        }
        let value = |a: Option<u32>, d: VReg| a.and_then(|a| last.iter().find(|x| x.0 == a && x.1 == d)).map_or(d, |x| x.2);
        for &d in &picked {
            insts.push(Inst::Sel(d, cond, value(when_c, d), value(when_not, d)));
            pos.push(term_pos);
        }
        let b = &mut f.blocks[bi as usize];
        b.insts = insts;
        b.pos = pos;
        b.term = Term::Jump(join);
        // the join, entered from this block alone now, joins it
        let mut to = join;
        while to != head && members.get(to as usize).copied().unwrap_or(false) && to != bi && f.blocks.iter().map(|b| b.term.succs().filter(|s| *s == to).count()).sum::<usize>() == 1 {
            let tail = std::mem::replace(&mut f.blocks[to as usize], Block { insts: Vec::new(), pos: Vec::new(), term: Term::Unreachable, term_pos: 0, count: -1.0 });
            let b = &mut f.blocks[bi as usize];
            b.insts.extend(tail.insts);
            b.pos.extend(tail.pos);
            b.term = tail.term;
            b.term_pos = tail.term_pos;
            members[to as usize] = false;
            match b.term {
                Term::Jump(j) => to = j,
                _ => break,
            }
        }
    }
    folded
}

