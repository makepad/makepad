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
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Loc {
    Reg(u8),
    Flt(u8),
    /// spill slot: byte offset into the spill area (multiple of 8)
    Stack(i32),
    None,
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
        Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::SlotAddr(d, _) | Inst::Addr(d, _) | Inst::FnAddr(d, _) => defs.push(*d),
        Inst::Mov(d, a) | Inst::INeg(_, d, a) | Inst::INot(_, d, a) | Inst::FUnary(_, _, d, a) | Inst::Conv(_, d, a) | Inst::IBinI(_, _, d, a, _) | Inst::ICmpI(_, _, d, a, _) => {
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
        Inst::LoadX(_, d, b, x, _, _) => {
            uses.push(*b);
            uses.push(*x);
            defs.push(*d);
        }
        Inst::StoreX(_, b, x, _, _, s) => {
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
        Inst::TlsAddr(d, _) => defs.push(*d),
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

pub fn term_uses(t: &Term, uses: &mut Vec<VReg>) {
    uses.clear();
    match t {
        Term::Branch(c, _, _) => uses.push(*c),
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
        match &f.blocks[b as usize].term {
            Term::Jump(t) => stack.push(*t),
            Term::Branch(_, t, e) => {
                stack.push(*t);
                stack.push(*e);
            }
            _ => {}
        }
    }
    r
}

pub fn allocate(f: &Func, cfg: &RegConfig) -> Alloc {
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
    let mut gen = vec![0u64; nb * words];
    let mut kill = vec![0u64; nb * words];
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut call_pos = Vec::new();
    let mut use_count = vec![0u32; nv as usize];
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        let g = &mut gen[bi * words..(bi + 1) * words];
        let k = &mut kill[bi * words..(bi + 1) * words];
        for (ii, i) in b.insts.iter().enumerate() {
            uses_defs(i, &mut uses, &mut defs);
            for u in &uses {
                use_count[u.0 as usize] += 1;
                if !bit_get(k, u.0) {
                    bit_set(g, u.0);
                }
            }
            for d in &defs {
                bit_set(k, d.0);
            }
            // a call that never returns (panic path: last inst before Unreachable) clobbers
            // nothing anyone reads afterwards: it does not count as a call position
            let noreturn = ii + 1 == b.insts.len() && matches!(b.term, Term::Unreachable);
            if let (Inst::Call(..), false) = (i, noreturn) {
                call_pos.push(block_start[bi] + 2 * ii as u32 + 1);
            }
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            use_count[u.0 as usize] += 1;
            if !bit_get(k, u.0) {
                bit_set(g, u.0);
            }
        }
    }
    // backward liveness to a fixpoint (reverse block order converges fast for forward CFGs)
    let mut live_in = vec![0u64; nb * words];
    let mut live_out = vec![0u64; nb * words];
    let mut out = vec![0u64; words];
    let mut changed = true;
    while changed {
        changed = false;
        let mut bi = nb;
        while bi > 0 {
            bi -= 1;
            if !reach[bi] {
                continue;
            }
            for w in 0..words {
                out[w] = 0;
            }
            match &f.blocks[bi].term {
                Term::Jump(t) => {
                    let t = *t as usize;
                    for w in 0..words {
                        out[w] |= live_in[t * words + w];
                    }
                }
                Term::Branch(_, t, e) => {
                    let (t, e) = (*t as usize, *e as usize);
                    for w in 0..words {
                        out[w] |= live_in[t * words + w] | live_in[e * words + w];
                    }
                }
                _ => {}
            }
            for w in 0..words {
                let newin = gen[bi * words + w] | (out[w] & !kill[bi * words + w]);
                if newin != live_in[bi * words + w] || out[w] != live_out[bi * words + w] {
                    changed = true;
                    live_in[bi * words + w] = newin;
                    live_out[bi * words + w] = out[w];
                }
            }
        }
    }
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
        for w in 0..words {
            let li = live_in[bi * words + w];
            let lo = live_out[bi * words + w];
            if li | lo == 0 {
                continue;
            }
            for k in 0..64 {
                let v = (w * 64 + k) as u32;
                if li & (1 << k) != 0 {
                    touch(v, block_start[bi], &mut start, &mut end);
                }
                if lo & (1 << k) != 0 {
                    touch(v, block_end[bi], &mut start, &mut end);
                }
            }
        }
        for (ii, i) in b.insts.iter().enumerate() {
            let p = block_start[bi] + 2 * ii as u32 + 1;
            uses_defs(i, &mut uses, &mut defs);
            for u in &uses {
                touch(u.0, p, &mut start, &mut end);
            }
            for d in &defs {
                touch(d.0, p + 1, &mut start, &mut end);
            }
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            touch(u.0, block_end[bi] - 1, &mut start, &mut end);
        }
    }
    // copy hints: `Mov(d, s)` where s dies at the move
    let mut hint = vec![u32::MAX; nv as usize];
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, i) in b.insts.iter().enumerate() {
            let p = block_start[bi] + 2 * ii as u32 + 1;
            // the result prefers the register of an operand that dies here (copies, and
            // two-address machines' first operand / either operand of commutative ops)
            let (d, a, b, comm) = match i {
                Inst::Mov(d, s2) | Inst::Conv(_, d, s2) | Inst::FUnary(_, _, d, s2) | Inst::INeg(_, d, s2) | Inst::INot(_, d, s2) | Inst::IBinI(_, _, d, s2, _) => (*d, *s2, None, false),
                Inst::IBin(op, _, d, x, y) => (*d, *x, Some(*y), matches!(op, IOp::Add | IOp::Mul | IOp::And | IOp::Or | IOp::Xor)),
                Inst::FBin(op, _, d, x, y) => (*d, *x, Some(*y), matches!(op, FOp::Add | FOp::Mul | FOp::Min | FOp::Max)),
                _ => continue,
            };
            if start[d.0 as usize] != p + 1 || f.vregs[d.0 as usize] != f.vregs[a.0 as usize] {
                continue;
            }
            if end[a.0 as usize] == p {
                hint[d.0 as usize] = a.0;
            } else if let Some(b) = b {
                if comm && end[b.0 as usize] == p {
                    hint[d.0 as usize] = b.0;
                }
            }
        }
    }
    // live ranges with holes (per block, built backwards from liveness)
    let ranges = build_ranges(f, &reach, &block_start, &block_end, &live_out, words);
    let mut order: Vec<u32> = Vec::new();
    for v in 0..nv {
        if !ranges[v as usize].is_empty() {
            order.push(v);
        }
    }
    order.sort_by(|a, b| ranges[*a as usize][0].0.cmp(&ranges[*b as usize][0].0));
    call_pos.sort();
    let crosses = |r: &Vec<(u32, u32)>| -> bool {
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
    // occupied ranges per physical register (ints 0..32, floats 32..64), sorted
    let mut occ: Vec<Vec<(u32, u32)>> = vec![Vec::new(); 64];
    let mut loc = vec![Loc::None; nv as usize];
    let mut used_callee_int = Vec::new();
    let mut used_callee_flt = Vec::new();
    let mut spill_bytes = 0u32;
    let mut needs_save: Vec<u32> = Vec::new();
    let mut cands: Vec<u8> = Vec::with_capacity(32);
    for &v in &order {
        let r = &ranges[v as usize];
        let is_int = f.vregs[v as usize] == Cls::I;
        let crossing = crosses(r);
        let (caller, callee) = if is_int { (&cfg.int_caller, &cfg.int_callee) } else { (&cfg.flt_caller, &cfg.flt_callee) };
        let base = if is_int { 0 } else { 32 };
        cands.clear();
        let h = hint[v as usize];
        if h != u32::MAX {
            match loc[h as usize] {
                Loc::Reg(x) if is_int => cands.push(x),
                Loc::Flt(x) if !is_int => cands.push(x),
                _ => {}
            }
        }
        if crossing {
            cands.extend_from_slice(callee);
            cands.extend_from_slice(caller);
        } else {
            cands.extend_from_slice(caller);
            cands.extend_from_slice(callee);
        }
        let mut chosen = None;
        for &c in cands.iter() {
            if crossing && !callee.contains(&c) && callee.len() > 0 && h != u32::MAX && cands[0] == c {
                // a hint into a caller-saved register across a call costs saves; let
                // a free callee-saved register win
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
        let l = match chosen {
            Some(c) => {
                insert_ranges(&mut occ[base + c as usize], r);
                if callee.contains(&c) {
                    let used = if is_int { &mut used_callee_int } else { &mut used_callee_flt };
                    if !used.contains(&c) {
                        used.push(c);
                    }
                } else if crossing {
                    needs_save.push(v);
                }
                if is_int {
                    Loc::Reg(c)
                } else {
                    Loc::Flt(c)
                }
            }
            None => {
                spill_bytes += 8;
                Loc::Stack(spill_bytes as i32)
            }
        };
        loc[v as usize] = l;
    }
    let mut saves: HashMap<u32, Vec<(VReg, i32)>> = HashMap::new();
    for v in needs_save {
        spill_bytes += 8;
        let slot = spill_bytes as i32;
        for (a, b) in &ranges[v as usize] {
            for &p in &call_pos {
                if p > *a && p < *b {
                    saves.entry(p).or_default().push((VReg(v), slot));
                }
            }
        }
    }
    Alloc { loc, used_callee_int, used_callee_flt, spill_bytes, block_start, block_end, saves, start, end, uses: use_count }
}

/// A move between locations (registers or spill/stack addresses as backends define them).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MLoc {
    R(u8),
    F(u8),
    /// memory (spill slot / staging); never a conflict source for register moves
    M(i32),
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
            (_, MLoc::M(_)) => from_mem.push(*m),
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
fn build_ranges(f: &Func, reach: &[bool], block_start: &[u32], block_end: &[u32], live_out: &[u64], words: usize) -> Vec<Vec<(u32, u32)>> {
    let nv = f.vregs.len();
    let mut ranges: Vec<Vec<(u32, u32)>> = vec![Vec::new(); nv];
    let mut cur_end = vec![u32::MAX; nv];
    let mut live: Vec<u32> = Vec::new();
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        live.clear();
        for w in 0..words {
            let lo = live_out[bi * words + w];
            if lo == 0 {
                continue;
            }
            for k in 0..64 {
                if lo & (1 << k) != 0 {
                    let v = (w * 64 + k) as u32;
                    cur_end[v as usize] = block_end[bi];
                    live.push(v);
                }
            }
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            if cur_end[u.0 as usize] == u32::MAX {
                cur_end[u.0 as usize] = block_end[bi] - 1;
                live.push(u.0);
            }
        }
        let mut ii = b.insts.len();
        while ii > 0 {
            ii -= 1;
            let p = block_start[bi] + 2 * ii as u32 + 1;
            uses_defs(&b.insts[ii], &mut uses, &mut defs);
            for d in &defs {
                let e = cur_end[d.0 as usize];
                if e != u32::MAX {
                    ranges[d.0 as usize].push((p + 1, e));
                    cur_end[d.0 as usize] = u32::MAX;
                } else {
                    ranges[d.0 as usize].push((p + 1, p + 1));
                }
            }
            for u in &uses {
                if cur_end[u.0 as usize] == u32::MAX {
                    cur_end[u.0 as usize] = p;
                    live.push(u.0);
                }
            }
        }
        for v in &live {
            let e = cur_end[*v as usize];
            if e != u32::MAX {
                ranges[*v as usize].push((block_start[bi], e));
                cur_end[*v as usize] = u32::MAX;
            }
        }
    }
    // params are defined at position 0 (entry block start): already covered by live-in
    for r in ranges.iter_mut() {
        r.sort();
        // merge touching ranges
        let mut out: Vec<(u32, u32)> = Vec::with_capacity(r.len());
        for x in r.iter() {
            if let Some(last) = out.last_mut() {
                if x.0 <= last.1 + 1 {
                    if x.1 > last.1 {
                        last.1 = x.1;
                    }
                    continue;
                }
            }
            out.push(*x);
        }
        *r = out;
    }
    ranges
}

fn overlaps(a: &[(u32, u32)], b: &[(u32, u32)]) -> bool {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].1 < b[j].0 {
            i += 1;
        } else if b[j].1 < a[i].0 {
            j += 1;
        } else {
            return true;
        }
    }
    false
}

fn insert_ranges(occ: &mut Vec<(u32, u32)>, r: &[(u32, u32)]) {
    occ.extend_from_slice(r);
    occ.sort();
}
