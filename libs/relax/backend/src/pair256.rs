//! x64 AVX: 256-bit vectors out of pairs of 128-bit halves. core::arch's `__m256` intrinsics,
//! the per-chunk lowering of std::simd's 32-byte vectors and the loop vectoriser compute a
//! 256-bit value as two V128 halves. After optimisation (before register allocation) this pass
//! finds the pairs of identical V128 instructions on the two halves of one value -- the halves'
//! loads and stores 16 bytes apart, then the ops whose operands are such pairs -- and makes each
//! pair one instruction on a `Cls::V256` vreg (one ymm register instead of two xmm). Nothing
//! before this pass sees V256, and every rewrite keeps the meaning of the V128 code:
//!
//! - the fused instruction stands where the earlier of the two was (lowering emits a value's low
//!   half chain, then its high half chain); the later one moves up to it only when nothing between
//!   reads or writes its result, defines its operands (other than high halves fused before it)
//!   or (for memory instructions) touches memory in a conflicting way;
//! - a vreg pair fuses only when every definition of both halves is such an instruction pair;
//! - a read of a fused half by an unfused instruction gets the half out first (`VX::Half`), an
//!   operand pair that is not fused is joined (`VX::Join`), a vreg read as both halves (a splat
//!   used for both) gets a 256-bit copy after each of its definitions; each is checked to see the
//!   same values as the original reads;
//! - the function is rewritten only when fewer instructions result.

use crate::passes::is_mem_effect;
use crate::regalloc::{term_uses, uses_defs};
use crate::rir::*;
use std::collections::{HashMap, HashSet};

/// What this CPU has for 256-bit vectors (AVX is required to run the pass at all).
#[derive(Clone, Copy)]
pub struct Caps {
    /// vbroadcastss from a register
    pub avx2: bool,
    /// vfmadd*ps ymm
    pub fma: bool,
}

/// Lane-wise ops with a VEX.256 form here (the x64 emitter's `y_vx`).
pub fn op256(op: VX) -> bool {
    use VW::*;
    match op {
        VX::FAdd(w, n) | VX::FSub(w, n) | VX::FMul(w, n) | VX::FDiv(w, n) => matches!(w, S | D) && n != NanBits::Neon,
        VX::FMinLt(w) | VX::FMaxGt(w) | VX::FCmp(_, w) => matches!(w, S | D),
        VX::And | VX::Or | VX::Xor | VX::AndNot => true,
        _ => false,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Load(VReg),
    Store(VReg),
    /// indexed: (base, index, shift)
    LoadX(VReg, VReg, u8),
    StoreX(VReg, VReg, u8),
    Vx(VX),
    Fma(bool, bool),
    Mov,
}

#[derive(Clone, Copy)]
struct Shape {
    kind: Kind,
    off: i32,
    dest: Option<VReg>,
    ops: [VReg; 3],
    n: usize,
}

fn shape(f: &Func, i: &Inst, caps: Caps) -> Option<Shape> {
    let v = |r: VReg| f.vregs[r.0 as usize] == Cls::V128;
    let z = VReg(0);
    match *i {
        Inst::VLoad(d, b, o) if v(d) && o <= i32::MAX - 16 => Some(Shape { kind: Kind::Load(b), off: o, dest: Some(d), ops: [z; 3], n: 0 }),
        Inst::VStore(b, o, s) if v(s) && o <= i32::MAX - 16 => Some(Shape { kind: Kind::Store(b), off: o, dest: None, ops: [s, z, z], n: 1 }),
        Inst::VLoadX(d, b, x, sh, o) if v(d) && o <= i32::MAX - 16 => Some(Shape { kind: Kind::LoadX(b, x, sh), off: o, dest: Some(d), ops: [z; 3], n: 0 }),
        Inst::VStoreX(b, x, sh, o, s) if v(s) && o <= i32::MAX - 16 => Some(Shape { kind: Kind::StoreX(b, x, sh), off: o, dest: None, ops: [s, z, z], n: 1 }),
        Inst::VX(op, d, a, b) if op256(op) && v(d) && v(a) && v(b) => Some(Shape { kind: Kind::Vx(op), off: 0, dest: Some(d), ops: [a, b, z], n: 2 }),
        Inst::VFma(np, na, d, a, b, c) if caps.fma && v(d) => Some(Shape { kind: Kind::Fma(np, na), off: 0, dest: Some(d), ops: [a, b, c], n: 3 }),
        Inst::Mov(d, a) if v(d) && v(a) && d != a => Some(Shape { kind: Kind::Mov, off: 0, dest: Some(d), ops: [a, z, z], n: 1 }),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct IPair {
    b: u32,
    lo: u32,
    hi: u32,
    on: bool,
    /// positions between the two whose definitions the moved instruction needs first (see `movable`)
    needs: [u32; 6],
    nneeds: u8,
}

/// An operand pair of a fused instruction that is not a fused vreg pair.
#[derive(Clone, Copy, PartialEq)]
enum Odd {
    /// the same vreg read for both halves
    Same(VReg),
    /// two equal broadcasts: both halves the splat of one scalar, or one constant
    Splat(VReg),
    Const(u32),
    /// two unrelated values joined
    Join(VReg, VReg),
}

struct Cx {
    sh: Vec<Vec<Option<Shape>>>,
    /// per vreg: (block, inst) of each definition
    defs: Vec<Vec<(u32, u32)>>,
    /// per vreg: (block, inst) of each use (inst = block length for the terminator)
    uses: Vec<Vec<(u32, u32)>>,
    /// single-def VSplat / VConst values: what they broadcast
    bcast: HashMap<u32, Odd>,
}

const WINDOW: usize = 64;

impl Cx {
    fn odd(&self, a: VReg, b: VReg) -> Odd {
        if a == b {
            return Odd::Same(a);
        }
        match (self.bcast.get(&a.0), self.bcast.get(&b.0)) {
            (Some(x), Some(y)) if x == y => *x,
            _ => Odd::Join(a, b),
        }
    }

    /// An address as (root vreg, offset): single-definition `x + k` chains folded.
    fn addr(&self, f: &Func, mut base: VReg, mut off: i64) -> (VReg, i64) {
        for _ in 0..8 {
            let d = &self.defs[base.0 as usize];
            if d.len() != 1 {
                break;
            }
            match f.blocks[d[0].0 as usize].insts[d[0].1 as usize] {
                Inst::IBinI(IOp::Add, _, _, r, k) => {
                    base = r;
                    off += k;
                }
                Inst::IBinI(IOp::Sub, _, _, r, k) => {
                    base = r;
                    off -= k;
                }
                _ => break,
            }
        }
        (base, off)
    }

    /// Whether the instruction at `q` may move up to `p` (same block, p < q): None if not, else
    /// the positions between that define its V128 operands or read its V128 result's old value
    /// (each must be the later half of a pair fused before `p`).
    fn movable(f: &Func, b: u32, p: u32, q: u32) -> Option<Vec<u32>> {
        let insts = &f.blocks[b as usize].insts;
        let m = &insts[q as usize];
        let (mut mu, mut md) = (Vec::new(), Vec::new());
        uses_defs(m, &mut mu, &mut md);
        let load = matches!(m, Inst::VLoad(..) | Inst::VLoadX(..));
        let store = matches!(m, Inst::VStore(..) | Inst::VStoreX(..));
        let (mut u, mut d) = (Vec::new(), Vec::new());
        let mut needs = Vec::new();
        for k in p + 1..q {
            let i = &insts[k as usize];
            uses_defs(i, &mut u, &mut d);
            if d.iter().any(|x| md.contains(x)) {
                return None;
            }
            // a read of the old value of its result, or a definition of an operand: fine when
            // that instruction is itself the later half of a pair fused before `p`
            if u.iter().any(|x| md.contains(x)) {
                if md.iter().any(|x| f.vregs[x.0 as usize] != Cls::V128) {
                    return None;
                }
                needs.push(k);
            }
            for x in d.iter().filter(|x| mu.contains(x)) {
                if f.vregs[x.0 as usize] != Cls::V128 {
                    return None;
                }
                needs.push(k);
            }
            if (load || store) && is_mem_effect(i) {
                return None;
            }
            if store && matches!(i, Inst::Load(..) | Inst::LoadX(..) | Inst::VLoad(..) | Inst::VLoadX(..) | Inst::VGather(..) | Inst::VTbl(..)) {
                return None;
            }
        }
        Some(needs)
    }
}

/// Fuses V128 pairs into V256 instructions; returns whether `f` changed.
pub fn pair256(f: &mut Func, caps: Caps) -> bool {
    if !f.vregs.iter().any(|c| *c == Cls::V128) {
        return false;
    }
    let nv = f.vregs.len();
    let mut cx = Cx { sh: Vec::new(), defs: vec![Vec::new(); nv], uses: vec![Vec::new(); nv], bcast: HashMap::new() };
    let mut any = false;
    let (mut u, mut d) = (Vec::new(), Vec::new());
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut row = Vec::with_capacity(b.insts.len());
        for (k, i) in b.insts.iter().enumerate() {
            let s = shape(f, i, caps);
            any |= matches!(s, Some(Shape { kind: Kind::Load(_) | Kind::Store(_) | Kind::LoadX(..) | Kind::StoreX(..), .. }));
            row.push(s);
            uses_defs(i, &mut u, &mut d);
            for x in &u {
                cx.uses[x.0 as usize].push((bi as u32, k as u32));
            }
            for x in &d {
                cx.defs[x.0 as usize].push((bi as u32, k as u32));
            }
        }
        term_uses(&b.term, &mut u);
        for x in &u {
            cx.uses[x.0 as usize].push((bi as u32, b.insts.len() as u32));
        }
        cx.sh.push(row);
    }
    if !any {
        return false;
    }
    for v in 0..nv {
        if cx.defs[v].len() != 1 || f.vregs[v] != Cls::V128 {
            continue;
        }
        let (b, k) = cx.defs[v][0];
        match f.blocks[b as usize].insts[k as usize] {
            Inst::VSplat(_, s) if cx.defs[s.0 as usize].len() == 1 && (caps.avx2 || f.vregs[s.0 as usize] == Cls::F32) => {
                cx.bcast.insert(v as u32, Odd::Splat(s));
            }
            Inst::VConst(_, k) => {
                cx.bcast.insert(v as u32, Odd::Const(k));
            }
            _ => {}
        }
    }

    // ---- candidates: anchors (loads/stores 16 bytes apart), then ops over known pairs
    let mut pairs: Vec<IPair> = Vec::new();
    let mut paired: HashSet<(u32, u32)> = HashSet::new();
    let mut pair_at: HashMap<(u32, u32), usize> = HashMap::new();
    // vreg -> its pair (lo, hi)
    let mut vp: HashMap<u32, (VReg, VReg)> = HashMap::new();
    loop {
        let mut changed = false;
        for bi in 0..f.blocks.len() {
            let n = cx.sh[bi].len();
            for i in 0..n {
                let Some(si) = cx.sh[bi][i] else { continue };
                if paired.contains(&(bi as u32, i as u32)) {
                    continue;
                }
                for j in i + 1..n.min(i + 1 + WINDOW) {
                    let Some(sj) = cx.sh[bi][j] else { continue };
                    let same_kind = match (si.kind, sj.kind) {
                        (Kind::Load(_), Kind::Load(_)) | (Kind::Store(_), Kind::Store(_)) => true,
                        (x, y) => x == y,
                    };
                    if !same_kind || paired.contains(&(bi as u32, j as u32)) {
                        continue;
                    }
                    // Some(true): i is the low half
                    let mut lo_i: Option<bool> = None;
                    let mut ok = true;
                    match (si.kind, sj.kind) {
                        (Kind::LoadX(..) | Kind::StoreX(..), _) => {
                            if sj.off == si.off + 16 {
                                lo_i = Some(true);
                            } else if si.off == sj.off + 16 {
                                lo_i = Some(false);
                            } else {
                                continue;
                            }
                        }
                        (Kind::Load(bi_) | Kind::Store(bi_), Kind::Load(bj_) | Kind::Store(bj_)) => {
                            let (ri, oi) = cx.addr(f, bi_, si.off as i64);
                            let (rj, oj) = cx.addr(f, bj_, sj.off as i64);
                            if ri != rj {
                                continue;
                            }
                            if oj == oi + 16 {
                                lo_i = Some(true);
                            } else if oi == oj + 16 {
                                lo_i = Some(false);
                            } else {
                                continue;
                            }
                        }
                        _ => {}
                    }
                    for p in 0..si.n {
                        let (a, b) = (si.ops[p], sj.ops[p]);
                        let vote = if vp.get(&a.0) == Some(&(a, b)) {
                            Some(true)
                        } else if vp.get(&b.0) == Some(&(b, a)) {
                            Some(false)
                        } else {
                            None
                        };
                        match (vote, lo_i) {
                            (Some(x), Some(y)) if x != y => ok = false,
                            (Some(x), _) => lo_i = Some(x),
                            // a half of another pair in the wrong place: not this partner
                            (None, _) if a != b && (vp.contains_key(&a.0) || vp.contains_key(&b.0)) => ok = false,
                            (None, _) => {}
                        }
                    }
                    let Some(lo_i) = lo_i else { continue };
                    if !ok {
                        continue;
                    }
                    let (l, h) = if lo_i { (si, sj) } else { (sj, si) };
                    let mut fresh = false;
                    if let (Some(x), Some(y)) = (l.dest, h.dest) {
                        if x == y {
                            continue;
                        }
                        match (vp.get(&x.0), vp.get(&y.0)) {
                            (None, None) => fresh = true,
                            (Some(e), Some(g)) if *e == (x, y) && *g == (x, y) => {}
                            _ => continue,
                        }
                    }
                    let Some(needs) = Cx::movable(f, bi as u32, i as u32, j as u32) else { continue };
                    let mut needs = needs;
                    needs.dedup();
                    if needs.len() > 6 || !needs.iter().all(|k| pair_at.get(&(bi as u32, *k)).is_some_and(|&x| pairs[x].lo.min(pairs[x].hi) < i as u32 && pairs[x].lo.max(pairs[x].hi) == *k)) {
                        continue;
                    }
                    let mut nd = [0u32; 6];
                    nd[..needs.len()].copy_from_slice(&needs);
                    let (lo, hi) = if lo_i { (i, j) } else { (j, i) };
                    pair_at.insert((bi as u32, i as u32), pairs.len());
                    pair_at.insert((bi as u32, j as u32), pairs.len());
                    pairs.push(IPair { b: bi as u32, lo: lo as u32, hi: hi as u32, on: true, needs: nd, nneeds: needs.len() as u8 });
                    if fresh {
                        let (x, y) = (l.dest.unwrap(), h.dest.unwrap());
                        vp.insert(x.0, (x, y));
                        vp.insert(y.0, (x, y));
                    }
                    paired.insert((bi as u32, i as u32));
                    paired.insert((bi as u32, j as u32));
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            break;
        }
    }
    if pairs.is_empty() {
        return false;
    }
    let mut von: HashMap<(u32, u32), bool> = HashMap::new();
    for p in &pairs {
        if let Some(x) = cx.sh[p.b as usize][p.lo as usize].unwrap().dest {
            let (l, h) = vp[&x.0];
            von.insert((l.0, h.0), true);
        }
    }

    // ---- greatest fixpoint: drop pairs whose conditions fail until none does
    let sh = |p: &IPair, lo: bool| cx.sh[p.b as usize][if lo { p.lo } else { p.hi } as usize].unwrap();
    let fused = |von: &HashMap<(u32, u32), bool>, v: VReg| -> Option<(VReg, VReg)> {
        let e = *vp.get(&v.0)?;
        von.get(&(e.0 .0, e.1 .0)).copied().unwrap_or(false).then_some(e)
    };
    loop {
        let mut changed = false;
        // definitions and covered uses of the on pairs
        let mut def_by: HashMap<(u32, u32), (VReg, VReg)> = HashMap::new();
        let mut covered: HashSet<(u32, u32, u32)> = HashSet::new();
        for p in pairs.iter().filter(|p| p.on) {
            let (l, h) = (sh(p, true), sh(p, false));
            if let (Some(x), Some(y)) = (l.dest, h.dest) {
                def_by.insert((p.b, p.lo), (x, y));
                def_by.insert((p.b, p.hi), (x, y));
            }
            for k in 0..l.n {
                if fused(&von, l.ops[k]) == Some((l.ops[k], h.ops[k])) {
                    covered.insert((p.b, p.lo, l.ops[k].0));
                    covered.insert((p.b, p.hi, h.ops[k].0));
                }
            }
        }
        // the fused definition positions of an on vreg pair in a block: (lo pos, hi pos)
        let mut dp: HashMap<(u32, u32), Vec<(u32, u32)>> = HashMap::new();
        for p in pairs.iter().filter(|p| p.on) {
            if let Some(x) = sh(p, true).dest {
                dp.entry((p.b, x.0)).or_default().push((p.lo, p.hi));
            }
        }
        let def_pairs = |b: u32, x: VReg| -> Vec<(u32, u32)> { dp.get(&(b, x.0)).cloned().unwrap_or_default() };
        let keys: Vec<(u32, u32)> = von.iter().filter(|e| *e.1).map(|e| *e.0).collect();
        for (x, y) in keys {
            let (xv, yv) = (VReg(x), VReg(y));
            let mut ok = cx.defs[x as usize].len() == cx.defs[y as usize].len()
                && cx.defs[x as usize].iter().all(|s| def_by.get(s) == Some(&(xv, yv)))
                && cx.defs[y as usize].iter().all(|s| def_by.get(s) == Some(&(xv, yv)));
            // an unfused read of a half must not fall between the halves' definitions
            if ok {
                'u: for &v in &[x, y] {
                    for &(b, k) in &cx.uses[v as usize] {
                        if covered.contains(&(b, k, v)) {
                            continue;
                        }
                        for (dl, dh) in def_pairs(b, xv) {
                            if dl.min(dh) < k && k <= dl.max(dh) {
                                ok = false;
                                break 'u;
                            }
                        }
                    }
                }
            }
            if !ok {
                von.insert((x, y), false);
                changed = true;
            }
        }
        for pi in 0..pairs.len() {
            let p = pairs[pi];
            if !p.on {
                continue;
            }
            let (l, h) = (sh(&p, true), sh(&p, false));
            let mut ok = match (l.dest, h.dest) {
                (Some(x), Some(_)) => fused(&von, x).is_some(),
                _ => true,
            };
            let (lo_pos, hi_pos) = (p.lo, p.hi);
            let (first, q) = (lo_pos.min(hi_pos), lo_pos.max(hi_pos));
            // the fused instruction stands at `first`
            for &k in &p.needs[..p.nneeds as usize] {
                ok &= pair_at.get(&(p.b, k)).is_some_and(|&x| pairs[x].on && pairs[x].lo.min(pairs[x].hi) < first && pairs[x].lo.max(pairs[x].hi) == k);
            }
            for k in 0..l.n {
                if !ok {
                    break;
                }
                let (a, b) = (l.ops[k], h.ops[k]);
                if fused(&von, a) == Some((a, b)) {
                    // the fused read at q sees what the two reads saw
                    for (dl, dh) in def_pairs(p.b, a) {
                        let dd = dl.min(dh);
                        if (dl < lo_pos) != (dd < first) || (dh < hi_pos) != (dd < first) {
                            ok = false;
                        }
                    }
                    continue;
                }
                if fused(&von, a).is_some() || fused(&von, b).is_some() {
                    ok = false;
                    continue;
                }
                let changes = |v: VReg| cx.defs[v.0 as usize].iter().any(|&(db, dk)| db == p.b && dk >= first && dk < q);
                match cx.odd(a, b) {
                    Odd::Same(v) => ok &= !changes(v),
                    Odd::Splat(_) | Odd::Const(_) => {}
                    Odd::Join(..) => ok &= !changes(if lo_pos < hi_pos { b } else { a }),
                }
            }
            if !ok {
                pairs[pi].on = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let on: Vec<IPair> = pairs.iter().copied().filter(|p| p.on).collect();
    let trace = std::env::var_os("MAKEPAD_RUN_PAIR256_TRACE").is_some();
    if trace {
        eprintln!("pair256 {}: {} candidate pairs, {} kept", f.name, pairs.len(), on.len());
        for p in pairs.iter().filter(|p| !p.on) {
            eprintln!("  dropped b{} {} / {}: {:?} / {:?}", p.b, p.lo, p.hi, f.blocks[p.b as usize].insts[p.lo as usize], f.blocks[p.b as usize].insts[p.hi as usize]);
        }
    }
    if on.is_empty() {
        return false;
    }

    // ---- cost: one instruction saved per pair, one spent per half / join / broadcast
    let mut covered: HashSet<(u32, u32, u32)> = HashSet::new();
    let mut extra = 0usize;
    let mut same: HashSet<u32> = HashSet::new();
    for p in &on {
        let (l, h) = (sh(p, true), sh(p, false));
        for k in 0..l.n {
            let (a, b) = (l.ops[k], h.ops[k]);
            if fused(&von, a) == Some((a, b)) {
                covered.insert((p.b, p.lo, a.0));
                covered.insert((p.b, p.hi, b.0));
            } else {
                match cx.odd(a, b) {
                    Odd::Same(v) => {
                        same.insert(v.0);
                    }
                    _ => extra += 1,
                }
            }
        }
    }
    for v in &same {
        extra += cx.defs[*v as usize].len().max(1);
    }
    let mut halves: Vec<(u32, u32, u32)> = Vec::new();
    for v in 0..nv {
        if fused(&von, VReg(v as u32)).is_none() {
            continue;
        }
        for &(b, k) in &cx.uses[v] {
            if !covered.contains(&(b, k, v as u32)) {
                halves.push((b, k, v as u32));
            }
        }
    }
    halves.sort_unstable();
    halves.dedup();
    extra += halves.len();
    if trace {
        eprintln!("pair256 {}: {} fused, {} extra (halves {})", f.name, on.len(), extra, halves.len());
    }
    if on.len() <= extra {
        return false;
    }

    // ---- rewrite
    let mut big: HashMap<u32, VReg> = HashMap::new();
    let mut same_big: HashMap<u32, VReg> = HashMap::new();
    let pair_of: Vec<(u32, u32)> = von.iter().filter(|e| *e.1).map(|e| *e.0).collect();
    for (x, y) in pair_of {
        let w = f.vreg(Cls::V256);
        big.insert(x, w);
        big.insert(y, w);
    }
    for v in &same {
        let w = f.vreg(Cls::V256);
        same_big.insert(*v, w);
    }
    let mut at_q: HashMap<(u32, u32), IPair> = HashMap::new();
    let mut gone: HashSet<(u32, u32)> = HashSet::new();
    for p in &on {
        at_q.insert((p.b, p.lo.min(p.hi)), *p);
        gone.insert((p.b, p.lo.max(p.hi)));
    }
    let halves: HashSet<(u32, u32, u32)> = halves.into_iter().collect();
    let lo_of = |v: u32| vp.get(&v).map(|e| e.0 .0 == v).unwrap_or(true);
    let entry_same: Vec<u32> = same.iter().copied().filter(|v| cx.defs[*v as usize].is_empty()).collect();
    for bi in 0..f.blocks.len() {
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let old_pos = std::mem::take(&mut f.blocks[bi].pos);
        let mut out = Vec::with_capacity(old.len());
        let mut pos = Vec::with_capacity(old.len());
        if bi == 0 {
            for v in &entry_same {
                out.push(Inst::VX(VX::Join, same_big[v], VReg(*v), VReg(*v)));
                pos.push(old_pos.first().copied().unwrap_or(0));
            }
        }
        let b = bi as u32;
        for (k, i) in old.iter().enumerate() {
            let k32 = k as u32;
            let ps = old_pos.get(k).copied().unwrap_or(0);
            if gone.contains(&(b, k32)) {
                continue;
            }
            if let Some(p) = at_q.get(&(b, k32)) {
                let li = &old[p.lo as usize];
                let (l, h) = (cx.sh[bi][p.lo as usize].unwrap(), cx.sh[bi][p.hi as usize].unwrap());
                let mut ops = [VReg(0); 3];
                for j in 0..l.n {
                    let (a, bb) = (l.ops[j], h.ops[j]);
                    ops[j] = if let Some(w) = big.get(&a.0).filter(|_| vp.get(&a.0) == Some(&(a, bb))) {
                        *w
                    } else {
                        match cx.odd(a, bb) {
                            Odd::Same(v) => same_big[&v.0],
                            Odd::Splat(s) => {
                                let t = f.vreg(Cls::V256);
                                out.push(Inst::VSplat(t, s));
                                pos.push(ps);
                                t
                            }
                            Odd::Const(c) => {
                                let t = f.vreg(Cls::V256);
                                out.push(Inst::VConst(t, c));
                                pos.push(ps);
                                t
                            }
                            Odd::Join(x, y) => {
                                let t = f.vreg(Cls::V256);
                                out.push(Inst::VX(VX::Join, t, x, y));
                                pos.push(ps);
                                t
                            }
                        }
                    };
                }
                let dest = l.dest.map(|x| big[&x.0]);
                let fusedi = match *li {
                    Inst::VLoad(_, base, off) => Inst::VLoad(dest.unwrap(), base, off),
                    Inst::VStore(base, off, _) => Inst::VStore(base, off, ops[0]),
                    Inst::VLoadX(_, base, x, sh, off) => Inst::VLoadX(dest.unwrap(), base, x, sh, off),
                    Inst::VStoreX(base, x, sh, off, _) => Inst::VStoreX(base, x, sh, off, ops[0]),
                    Inst::VX(op, ..) => Inst::VX(op, dest.unwrap(), ops[0], ops[1]),
                    Inst::VFma(np, na, ..) => Inst::VFma(np, na, dest.unwrap(), ops[0], ops[1], ops[2]),
                    Inst::Mov(..) => Inst::Mov(dest.unwrap(), ops[0]),
                    _ => unreachable!("pair256: unfusable instruction"),
                };
                out.push(fusedi);
                pos.push(ps);
                continue;
            }
            let mut i2 = i.clone();
            let mut ren: Vec<(VReg, VReg)> = Vec::new();
            let (mut uu, mut dd) = (Vec::new(), Vec::new());
            uses_defs(i, &mut uu, &mut dd);
            for x in &uu {
                if halves.contains(&(b, k32, x.0)) && !ren.iter().any(|r| r.0 == *x) {
                    let t = f.vreg(Cls::V128);
                    out.push(Inst::VX(VX::Half(!lo_of(x.0)), t, big[&x.0], big[&x.0]));
                    pos.push(ps);
                    ren.push((*x, t));
                }
            }
            if !ren.is_empty() {
                rename_uses(&mut i2, &ren);
            }
            out.push(i2);
            pos.push(ps);
            for x in &dd {
                if let Some(w) = same_big.get(&x.0) {
                    let m = match *i {
                        Inst::VSplat(_, s) => Inst::VSplat(*w, s),
                        Inst::VConst(_, c) => Inst::VConst(*w, c),
                        _ => Inst::VX(VX::Join, *w, *x, *x),
                    };
                    out.push(m);
                    pos.push(ps);
                }
            }
        }
        // the terminator's reads of fused halves
        let mut tu = Vec::new();
        term_uses(&f.blocks[bi].term, &mut tu);
        let n = old.len() as u32;
        let mut ren: Vec<(VReg, VReg)> = Vec::new();
        for x in &tu {
            if halves.contains(&(b, n, x.0)) && !ren.iter().any(|r| r.0 == *x) {
                let t = f.vreg(Cls::V128);
                out.push(Inst::VX(VX::Half(!lo_of(x.0)), t, big[&x.0], big[&x.0]));
                pos.push(f.blocks[bi].term_pos);
                ren.push((*x, t));
            }
        }
        if !ren.is_empty() {
            if let Term::Ret(vs) = &mut f.blocks[bi].term {
                for v in vs.iter_mut() {
                    if let Some(r) = ren.iter().find(|r| r.0 == *v) {
                        *v = r.1;
                    }
                }
            }
        }
        f.blocks[bi].insts = out;
        f.blocks[bi].pos = pos;
    }
    true
}

/// Renames operands of `i` (none of them is a result: fused vregs have only fused definitions).
fn rename_uses(i: &mut Inst, ren: &[(VReg, VReg)]) {
    i.vregs_mut(|r| {
        if let Some(e) = ren.iter().find(|e| e.0 == *r) {
            *r = e.1;
        }
    });
}
