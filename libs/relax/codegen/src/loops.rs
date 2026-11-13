//! Loop transformations for hot code (run by O3 after the optimiser): unrolling of small
//! single-block counted loops.

use relax_backend::regalloc::{for_defs, for_uses};
use relax_backend::rir::*;

fn unroll_on(options: &relax_session::Options) -> bool {
    !options.no_unroll
}

/// A single-block loop `B: body; i += 1; c = i < n; branch c -> B, exit` (or `!=`), with nothing
/// else defining `i` or `n` in `B` and no calls, gets an unrolled copy in front of it:
///
/// ```text
/// H:  if i < n { H2 } else { B }      (entry: B runs the do-while's first iteration)
/// H2: if n - i >= K { U } else { B }  (unsigned; i < n, so no wrap)
/// U:  t_j = i + j; the body K times reading i as t_0..t_(K-1), no increments, no compares;
///     i += K; if i <= n - K { U } else { HL }
/// HL: if i < n { B } else { exit }
/// B:  the original loop, now running fewer than K iterations
/// ```
///
/// Every iteration U runs is one the original would have run: from H, `i < n` with `n - i >= K`
/// means the K - 1 compares U leaves out would all have continued. K = 4 for bodies of up to 12
/// instructions, 2 up to 32. Returns whether anything changed.
pub fn unroll_loops(options: &relax_session::Options, f: &mut Func) -> bool {
    if !unroll_on(options) {
        return false;
    }
    if let Some(w) = &options.unroll_only {
        if !f.name.contains(w.as_str()) {
            return false;
        }
    }
    let nb0 = f.blocks.len();
    let mut ndef = vec![0u32; f.vregs.len()];
    let mut nuse = vec![0u32; f.vregs.len()];
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            for_uses(i, |u| nuse[u.0 as usize] += 1);
        }
        if let Term::Branch(c, _, _) = &b.term {
            nuse[c.0 as usize] += 1;
        }
        if let Term::Ret(vs) = &b.term {
            for v in vs {
                nuse[v.0 as usize] += 1;
            }
        }
    }
    let mut changed = false;
    for bi in 1..nb0 {
        let (c, cont_on_true, exit) = match f.blocks[bi].term {
            Term::Branch(c, t, e) if t as usize == bi && e as usize != bi => (c, true, e),
            Term::Branch(c, t, e) if e as usize == bi && t as usize != bi => (c, false, t),
            _ => continue,
        };
        // entered through an unwind edge: a landing pad, leave it
        if f.blocks.iter().any(|x| matches!(x.term, Term::Invoke(_, w) if w as usize == bi)) {
            continue;
        }
        let b = &f.blocks[bi];
        let n = b.insts.len();
        if n < 2 || n > 32 || nuse[c.0 as usize] != 1 || b.insts.iter().any(|i| matches!(i, Inst::Call(..))) {
            continue;
        }
        // the compare: the last instruction, on the counter
        let (cond, signed, iv, bound) = match b.insts[n - 1] {
            Inst::ICmpI(cc, s, d, i, k) if d == c => (cc, s, i, Err(k)),
            Inst::ICmp(cc, s, d, i, m) if d == c => (cc, s, i, Ok(m)),
            _ => continue,
        };
        // continue while i < n (true edge) or i != n; `i >= n` / `i == n` on the false edge
        let lt = match (cond, cont_on_true) {
            (Cond::Lt, true) | (Cond::Ge, false) => true,
            (Cond::Ne, true) | (Cond::Eq, false) => false,
            _ => continue,
        };
        // n is not defined in the loop (loop invariant); i has one def here: i = i + 1, or
        // t = i + 1; i = t
        if let Ok(m) = bound {
            if m == iv || b.insts.iter().any(|i| {
                let mut d = false;
                for_defs(i, |x| d |= x == m);
                d
            }) {
                continue;
            }
        }
        let mut idefs = Vec::new();
        for (k, i) in b.insts.iter().enumerate() {
            for_defs(i, |x| {
                if x == iv {
                    idefs.push(k);
                }
            });
        }
        if idefs.len() != 1 {
            continue;
        }
        // the increment's instructions (skipped in the unrolled copies)
        let mut inc: Vec<usize> = vec![idefs[0]];
        let it = match b.insts[idefs[0]] {
            Inst::IBinI(IOp::Add, it, d, a, 1) if d == iv && a == iv => it,
            Inst::Mov(d, t) if d == iv => {
                // t = iv + 1 defined once in this block before the Mov, used only by it
                match b.insts[..idefs[0]].iter().enumerate().rev().find(|(_, i)| {
                    let mut d2 = false;
                    for_defs(i, |x| d2 |= x == t);
                    d2
                }) {
                    Some((ti, Inst::IBinI(IOp::Add, it, _, a, 1))) if *a == iv && ndef[t.0 as usize] == 1 && nuse[t.0 as usize] == 1 => {
                        inc.push(ti);
                        *it
                    }
                    _ => continue,
                }
            }
            _ => continue,
        };
        // != loops of narrower counters may wrap within the type: 64-bit only
        if !lt && it.bits != 64 {
            continue;
        }
        // a single-def value read before its def in the block carries over from the previous
        // iteration: the renamed copies would break that, leave the loop
        {
            let mut defined: Vec<u32> = Vec::new();
            let mut carried = false;
            for i in &b.insts {
                for_uses(i, |u2| {
                    if ndef[u2.0 as usize] == 1 && u2 != iv && !defined.contains(&u2.0) && b.insts.iter().any(|j| {
                        let mut d2 = false;
                        for_defs(j, |x| d2 |= x == u2);
                        d2
                    }) {
                        carried = true;
                    }
                });
                for_defs(i, |d| defined.push(d.0));
            }
            if carried {
                continue;
            }
        }
        let k: i64 = if n <= 12 { 4 } else { 2 };
        let h = f.blocks.len() as u32;
        let pos = f.blocks[bi].term_pos;
        let body: Vec<(Inst, u64)> = f.blocks[bi].insts[..n - 1].iter().cloned().zip(f.blocks[bi].pos[..n - 1].iter().copied()).collect();
        let bvreg = match bound {
            Ok(m) => m,
            Err(_) => f.vreg(Cls::I),
        };
        // H: i < n (or !=): from outside, i >= n still runs the do-while's one iteration (B)
        let g1 = f.vreg(Cls::I);
        let mut hi: Vec<Inst> = Vec::new();
        if let Err(kc) = bound {
            hi.push(Inst::Iconst(bvreg, kc));
        }
        hi.push(Inst::ICmp(if lt { Cond::Lt } else { Cond::Ne }, signed, g1, iv, bvreg));
        let hb = Block { insts: hi.clone(), pos: vec![pos; hi.len()], term: Term::Branch(g1, h + 1, bi as u32), term_pos: pos, count: -1.0 };
        // H2: n - i >= K (unsigned; i < n, so no wrap)
        let (rem, g2, lim) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        // lim = n - K: U runs while i <= lim (n - i >= K here, so n - K does not overflow)
        let h2i = vec![Inst::IBin(IOp::Sub, U64_TY, rem, bvreg, iv), Inst::IBinI(IOp::Sub, U64_TY, lim, bvreg, k), Inst::ICmpI(Cond::Ge, false, g2, rem, k)];
        let h2 = Block { insts: h2i.clone(), pos: vec![pos; h2i.len()], term: Term::Branch(g2, h + 2, bi as u32), term_pos: pos, count: -1.0 };
        // U: t_j = i + j up front; copy c of the body reads i as t_(c + increments so far);
        // the increments go; i += K; loop while n - i >= K
        let tv: Vec<VReg> = (0..=k).map(|j| if j == 0 { iv } else { f.vreg(Cls::I) }).collect();
        let mut ui: Vec<Inst> = Vec::new();
        let mut up: Vec<u64> = Vec::new();
        for j in 1..=k {
            ui.push(Inst::IBinI(IOp::Add, U64_TY, tv[j as usize], iv, j));
            up.push(pos);
        }
        // single-def temporaries get fresh names in every copy, so single-def passes (address
        // modes) still see them as such; ones read after the loop get their original back at the end
        let mut cur: std::collections::HashMap<u32, VReg> = std::collections::HashMap::new();
        for copy in 0..k {
            let mut off = copy;
            let _ = copy;
            for (ii, (i, p)) in body.iter().enumerate() {
                if inc.contains(&ii) {
                    if ii == idefs[0] {
                        off += 1;
                    }
                    continue;
                }
                let mut i2 = i.clone();
                let t = tv[off as usize];
                let mut defs: Vec<VReg> = Vec::new();
                for_defs(&i2, |d| defs.push(d));
                let mut newdef: std::collections::HashMap<u32, VReg> = std::collections::HashMap::new();
                for d in &defs {
                    if ndef[d.0 as usize] == 1 && *d != iv {
                        let nd = f.vreg(f.vregs[d.0 as usize]);
                        newdef.insert(d.0, nd);
                    }
                }
                // a single-def vreg is never read by the instruction defining it: an occurrence of
                // one in newdef is its def; other occurrences read the current names
                i2.vregs_mut(|r| {
                    let v = r.0;
                    if v == iv.0 {
                        *r = t;
                    } else if let Some(nd) = newdef.get(&v) {
                        *r = *nd;
                    } else if let Some(m) = cur.get(&v) {
                        *r = *m;
                    }
                });
                for (v, nd) in newdef {
                    cur.insert(v, nd);
                }
                ui.push(i2);
                up.push(*p);
            }
        }
        // temporaries read after the loop: their originals take the last copy's values
        for (v, nv2) in cur.iter() {
            let used_outside = f.blocks.iter().enumerate().any(|(bj, bl)| {
                bj != bi && (bl.insts.iter().any(|i| {
                    let mut u3 = false;
                    for_uses(i, |x| u3 |= x.0 == *v);
                    u3
                }) || matches!(&bl.term, Term::Branch(cc, _, _) if cc.0 == *v) || matches!(&bl.term, Term::Ret(vs) if vs.iter().any(|x| x.0 == *v)))
            });
            if used_outside {
                ui.push(Inst::Mov(VReg(*v), *nv2));
                up.push(pos);
            }
        }
        // offsets no copy reads (i + K when the last copy reads no i after its increment) go;
        // addr_modes drops the ones it folds into displacements
        {
            let mut used = vec![false; k as usize + 1];
            for i in &ui[k as usize..] {
                for_uses(i, |u| {
                    if let Some(j) = tv.iter().position(|t| *t == u) {
                        used[j] = true;
                    }
                });
            }
            let defs: Vec<(Inst, u64)> = ui.drain(..k as usize).zip(up.drain(..k as usize)).collect();
            for (j, (i, p)) in defs.into_iter().enumerate().rev() {
                if used[j + 1] {
                    ui.insert(0, i);
                    up.insert(0, p);
                }
            }
        }
        let g4 = f.vreg(Cls::I);
        ui.push(Inst::IBinI(IOp::Add, it, iv, iv, k));
        ui.push(Inst::ICmp(Cond::Le, signed, g4, iv, lim));
        up.extend([pos, pos]);
        let ub = Block { insts: ui, pos: up, term: Term::Branch(g4, h + 2, h + 3), term_pos: pos, count: -1.0 };
        // HL: the rest (< K iterations) in B, or none
        let g3 = f.vreg(Cls::I);
        let hl = vec![Inst::ICmp(if lt { Cond::Lt } else { Cond::Ne }, signed, g3, iv, bvreg)];
        let hlb = Block { insts: hl, pos: vec![pos], term: Term::Branch(g3, bi as u32, exit), term_pos: pos, count: -1.0 };
        // outside entries into B now enter H
        for pb in 0..nb0 {
            if pb != bi {
                f.blocks[pb].term.map_succs(|s| if s as usize == bi { h } else { s });
            }
        }
        f.blocks.push(hb);
        f.blocks.push(h2);
        f.blocks.push(ub);
        f.blocks.push(hlb);
        changed = true;
    }
    changed
}

pub(crate) const U64_TY: IntTy = IntTy { bits: 64, signed: false };

fn vectorize_on(options: &relax_session::Options) -> bool {
    !options.no_vectorize
}

/// A loop head's source position (makepad inspect remarks): its first instruction's, else its
/// terminator's.
pub(crate) fn head_pos(f: &Func, head: usize) -> u64 {
    f.blocks.get(head).map_or(0, |b| b.pos.first().copied().unwrap_or(b.term_pos))
}

/// Integer sum reductions over a contiguous array, vectorised: a single-block counted loop
/// `x = load u32/u64 [base + i * size]; acc = acc + x (or + x * k, k invariant); i += 1; i < n`
/// with nothing else in it gets, in front of it (as unroll_loops):
///
/// ```text
/// H:  if i < n { H2 } else { B }
/// H2: v0 = v1 = 0; if n - i >= K { V } else { B }
/// V:  p = base + i * size; v0 += widen-pairs(load p) + widen-pairs(load p + 32);
///     v1 += widen-pairs(load p + 16) + widen-pairs(load p + 48); i += K; if i <= n - K { V } else { VE }
/// VE: acc += sum-across(v0 + v1) (* k); if i < n { B } else { exit }
/// B:  the original loop for the rest
/// ```
///
/// Exact: the sums are wrapping 64-bit adds, which reassociate, and k * sum(x) = sum(k * x) in
/// wrapping arithmetic. K = 16 (u32) or 8 (u64) elements per round. Returns whether anything changed.
pub fn vectorize_reductions(options: &relax_session::Options, f: &mut Func) -> bool {
    if !vectorize_on(options) {
        return false;
    }
    let mut changed = crate::vectorize::vectorize_stream_maps(options, f);
    changed |= vectorize_lane_accumulators(options, f);
    let nb0 = f.blocks.len();
    let mut ndef = vec![0u32; f.vregs.len()];
    let mut nuse = vec![0u32; f.vregs.len()];
    // uses in blocks nothing reaches (a bounds check's panic call once the check folded) count
    // for nothing
    let reach = relax_backend::regalloc::reachable(f);
    for (bk, b) in f.blocks.iter().enumerate() {
        if !reach[bk] {
            continue;
        }
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            for_uses(i, |u| nuse[u.0 as usize] += 1);
        }
        match &b.term {
            Term::Branch(c, _, _) => nuse[c.0 as usize] += 1,
            Term::Ret(vs) => vs.iter().for_each(|v| nuse[v.0 as usize] += 1),
            _ => {}
        }
    }
    for bi in 1..nb0 {
        let (c, cont_on_true, exit) = match f.blocks[bi].term {
            Term::Branch(c, t, e) if t as usize == bi && e as usize != bi => (c, true, e),
            Term::Branch(c, t, e) if e as usize == bi && t as usize != bi => (c, false, t),
            _ => continue,
        };
        relax_backend::remark::loop_try("reduction", head_pos(f, bi));
        if f.blocks.iter().any(|x| matches!(x.term, Term::Invoke(_, w) if w as usize == bi)) {
            continue;
        }
        let b = &f.blocks[bi];
        let n = b.insts.len();
        if n < 4 || n > 16 || nuse[c.0 as usize] != 1 {
            continue;
        }
        let (cond, signed, iv, bound) = match b.insts[n - 1] {
            Inst::ICmpI(cc, s, d, i, k) if d == c => (cc, s, i, Err(k)),
            Inst::ICmp(cc, s, d, i, m) if d == c => (cc, s, i, Ok(m)),
            _ => continue,
        };
        let lt = match (cond, cont_on_true) {
            (Cond::Lt, true) | (Cond::Ge, false) => true,
            (Cond::Ne, true) | (Cond::Eq, false) => false,
            _ => continue,
        };
        relax_backend::remark::loop_stage(1);
        let defined_in = |v: VReg| b.insts.iter().any(|i| {
            let mut d = false;
            for_defs(i, |x| d |= x == v);
            d
        });
        if let Ok(m) = bound {
            if m == iv || defined_in(m) {
                continue;
            }
        }
        // the increment (i = i + 1, or t = i + 1; i = t)
        let idefs: Vec<usize> = (0..n).filter(|k| {
            let mut d = false;
            for_defs(&b.insts[*k], |x| d |= x == iv);
            d
        }).collect();
        if idefs.len() != 1 {
            continue;
        }
        let mut used: Vec<bool> = vec![false; n];
        used[n - 1] = true;
        used[idefs[0]] = true;
        let mut inc_at = idefs[0];
        let it = match b.insts[idefs[0]] {
            Inst::IBinI(IOp::Add, it, d, a, 1) if d == iv && a == iv => it,
            Inst::Mov(d, t) if d == iv => match b.insts[..idefs[0]].iter().enumerate().rev().find(|(_, i)| {
                let mut d2 = false;
                for_defs(i, |x| d2 |= x == t);
                d2
            }) {
                Some((ti, Inst::IBinI(IOp::Add, it, _, a, 1))) if *a == iv && ndef[t.0 as usize] == 1 && nuse[t.0 as usize] == 1 => {
                    used[ti] = true;
                    inc_at = ti;
                    *it
                }
                _ => continue,
            },
            _ => continue,
        };
        if !lt && it.bits != 64 {
            continue;
        }
        // the load: [base + i * size] (Shl/Mul + Add temporaries, or LoadX)
        let def_at = |v: VReg| (0..n).find(|k| {
            let mut d = false;
            for_defs(&b.insts[*k], |x| d |= x == v);
            d
        });
        // the counter's value before the step: the counter itself (`Some(None)`) or a copy of it
        // taken before the step (`j = i; i = i + 1; load [j]`, an iterator's next inlined: the
        // Mov to mark used)
        let counter_copy = |ix: VReg| -> Option<Option<usize>> {
            if ix == iv {
                return Some(None);
            }
            if ndef[ix.0 as usize] != 1 || nuse[ix.0 as usize] != 1 {
                return None;
            }
            match def_at(ix) {
                Some(mk) if mk < inc_at && matches!(b.insts[mk], Inst::Mov(d2, s2) if d2 == ix && s2 == iv) => Some(Some(mk)),
                _ => None,
            }
        };
        let mut load: Option<(usize, VReg, VReg, u8)> = None; // (index, x, base, size bytes)
        for (k, i) in b.insts.iter().enumerate() {
            match *i {
                Inst::LoadX(Mem::Int(sz @ (4 | 8), sg), x, base, ix, sh, 0) if (1u32 << sh) == sz as u32 && !(sz == 4 && sg) && !defined_in(base) => {
                    let Some(copy) = counter_copy(ix) else { continue };
                    if load.is_some() {
                        load = None;
                        break;
                    }
                    if let Some(mk) = copy {
                        used[mk] = true;
                    }
                    load = Some((k, x, base, sz));
                    used[k] = true;
                }
                Inst::Load(Mem::Int(sz @ (4 | 8), sg), x, p, 0) if !(sz == 4 && sg) => {
                    if load.is_some() {
                        load = None;
                        break;
                    }
                    let sh = if sz == 4 { 2 } else { 3 };
                    // p = base + s, s = i << sh (or i * size), both single-def temporaries used once
                    let pk = match def_at(p) {
                        Some(pk) if ndef[p.0 as usize] == 1 && nuse[p.0 as usize] == 1 => pk,
                        _ => continue,
                    };
                    let (base, s) = match b.insts[pk] {
                        Inst::IBin(IOp::Add, it2, _, a2, b2) if it2.bits == 64 => {
                            if !defined_in(a2) {
                                (a2, b2)
                            } else if !defined_in(b2) {
                                (b2, a2)
                            } else {
                                continue;
                            }
                        }
                        _ => continue,
                    };
                    let sk = match def_at(s) {
                        Some(sk) if ndef[s.0 as usize] == 1 && nuse[s.0 as usize] == 1 => sk,
                        _ => continue,
                    };
                    let idx = match b.insts[sk] {
                        Inst::IBinI(IOp::Shl, it2, _, a2, k2) if k2 == sh && it2.bits == 64 => counter_copy(a2),
                        Inst::IBinI(IOp::Mul, it2, _, a2, k2) if k2 == sz as i64 && it2.bits == 64 => counter_copy(a2),
                        _ => None,
                    };
                    let Some(idx) = idx else { continue };
                    if let Some(mk) = idx {
                        used[mk] = true;
                    }
                    load = Some((k, x, base, sz));
                    used[k] = true;
                    used[pk] = true;
                    used[sk] = true;
                }
                _ => {}
            }
        }
        let (lk, x, base, sz) = match load {
            Some(l) => l,
            None => continue,
        };
        if nuse[x.0 as usize] != 1 || ndef[x.0 as usize] != 1 {
            continue;
        }
        // y = x or x * k (k invariant, the product used once)
        let mut y = x;
        let mut kmul: Option<VReg> = None;
        for (k, i) in b.insts.iter().enumerate() {
            if let Inst::IBin(IOp::Mul, it2, d, a2, b2) = *i {
                if it2.bits == 64 && (a2 == x || b2 == x) {
                    let other = if a2 == x { b2 } else { a2 };
                    if other == x || defined_in(other) || nuse[d.0 as usize] != 1 || ndef[d.0 as usize] != 1 {
                        continue;
                    }
                    y = d;
                    kmul = Some(other);
                    used[k] = true;
                }
            }
        }
        // acc = acc + y (the add's result is acc itself, or a temporary copied into it)
        let mut acc: Option<VReg> = None;
        for (k, i) in b.insts.iter().enumerate() {
            if let Inst::IBin(IOp::Add, it2, d, a2, b2) = *i {
                if it2.bits == 64 && (a2 == y || b2 == y) && a2 != b2 {
                    let a = if a2 == y { b2 } else { a2 };
                    if d == a {
                        acc = Some(a);
                        used[k] = true;
                    } else if ndef[d.0 as usize] == 1 && nuse[d.0 as usize] == 1 {
                        // the Mov(a, d) after it
                        if let Some(mk) = (k + 1..n).find(|m| matches!(b.insts[*m], Inst::Mov(dd, ss) if dd == a && ss == d)) {
                            acc = Some(a);
                            used[k] = true;
                            used[mk] = true;
                        }
                    }
                }
            }
        }
        let acc = match acc {
            Some(a) => a,
            None => continue,
        };
        // acc has no other def in the loop; nothing else in the loop
        let acc_defs = b.insts.iter().filter(|i| {
            let mut d = false;
            for_defs(i, |v| d |= v == acc);
            d
        }).count();
        if acc_defs != 1 || acc == iv || used.iter().any(|u| !*u) || f.vregs[acc.0 as usize] != Cls::I {
            continue;
        }
        let _ = lk;
        // ---- build
        let lanes: i64 = if sz == 4 { 4 } else { 2 };
        // four vectors per round (two accumulators)
        let kk = 4 * lanes;
        let sh: i64 = if sz == 4 { 2 } else { 3 };
        let h = f.blocks.len() as u32;
        let pos = f.blocks[bi].term_pos;
        let bvreg = match bound {
            Ok(m) => m,
            Err(_) => f.vreg(Cls::I),
        };
        // H
        let g1 = f.vreg(Cls::I);
        let mut hi: Vec<Inst> = Vec::new();
        if let Err(kc) = bound {
            hi.push(Inst::Iconst(bvreg, kc));
        }
        hi.push(Inst::ICmp(if lt { Cond::Lt } else { Cond::Ne }, signed, g1, iv, bvreg));
        let hb = Block { insts: hi.clone(), pos: vec![pos; hi.len()], term: Term::Branch(g1, h + 1, bi as u32), term_pos: pos, count: -1.0 };
        // H2
        let (rem, g2, va, vb, lim) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::I));
        // lim = n - K: the vector loop runs while i <= lim (n - i >= K here, so no wrap)
        let h2i = vec![Inst::VConst(va, 0), Inst::VConst(vb, 0), Inst::IBin(IOp::Sub, U64_TY, rem, bvreg, iv), Inst::IBinI(IOp::Sub, U64_TY, lim, bvreg, kk), Inst::ICmpI(Cond::Ge, false, g2, rem, kk)];
        let h2 = Block { insts: h2i.clone(), pos: vec![pos; h2i.len()], term: Term::Branch(g2, h + 2, bi as u32), term_pos: pos, count: -1.0 };
        // V
        let (s1, p, g3) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        let mut vi = vec![Inst::IBinI(IOp::Shl, U64_TY, s1, iv, sh), Inst::IBin(IOp::Add, U64_TY, p, base, s1)];
        for j in 0..4 {
            let l = f.vreg(Cls::V128);
            vi.push(Inst::VLoad(l, p, 16 * j));
            let acc = if j % 2 == 0 { va } else { vb };
            if sz == 4 {
                let w = f.vreg(Cls::V128);
                vi.push(Inst::VX(VX::PairAddLongU(VW::S), w, l, l));
                vi.push(Inst::VX(VX::Add(VW::D), acc, acc, w));
            } else {
                vi.push(Inst::VX(VX::Add(VW::D), acc, acc, l));
            }
        }
        vi.push(Inst::IBinI(IOp::Add, it, iv, iv, kk));
        // the loop's own signedness (n - i >= K here: n - K does not overflow either way)
        vi.push(Inst::ICmp(Cond::Le, signed, g3, iv, lim));
        let vb_ = Block { insts: vi.clone(), pos: vec![pos; vi.len()], term: Term::Branch(g3, h + 2, h + 3), term_pos: pos, count: -1.0 };
        // VE
        let (vs, s, g4) = (f.vreg(Cls::V128), f.vreg(Cls::I), f.vreg(Cls::I));
        let mut ve = vec![Inst::VX(VX::Add(VW::D), vs, va, vb), Inst::VX(VX::AddAcross(VW::D), s, vs, vs)];
        let s_fin = if let Some(k) = kmul {
            let m = f.vreg(Cls::I);
            ve.push(Inst::IBin(IOp::Mul, U64_TY, m, s, k));
            m
        } else {
            s
        };
        ve.push(Inst::IBin(IOp::Add, U64_TY, acc, acc, s_fin));
        ve.push(Inst::ICmp(if lt { Cond::Lt } else { Cond::Ne }, signed, g4, iv, bvreg));
        let veb = Block { insts: ve.clone(), pos: vec![pos; ve.len()], term: Term::Branch(g4, bi as u32, exit), term_pos: pos, count: -1.0 };
        for pb in 0..nb0 {
            if pb != bi {
                f.blocks[pb].term.map_succs(|s2| if s2 as usize == bi { h } else { s2 });
            }
        }
        f.blocks.push(hb);
        f.blocks.push(h2);
        f.blocks.push(vb_);
        f.blocks.push(veb);
        relax_backend::remark::loop_done();
        changed = true;
    }
    changed
}

/// Chains of constant steps flattened: in a block, `t1 = x + a; t2 = t1 + b` (single-definition
/// temporaries, 64-bit) becomes `t2 = x + (a + b)` while `x` is not redefined in between, so the
/// steps of an unrolled copy (`s += 8` repeated, each address read from the one before) all hang off
/// the first value and address modes fold their offsets as displacements instead of every copy
/// waiting for the previous one's add. Returns whether anything changed.
pub fn flatten_step_chains(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
    }
    let mut changed = false;
    // (temporary, root, offset from the root) for the block being read
    let mut known: Vec<(VReg, VReg, i64)> = Vec::new();
    for b in f.blocks.iter_mut() {
        known.clear();
        for i in b.insts.iter_mut() {
            if let Inst::IBinI(op @ (IOp::Add | IOp::Sub), it, d, x, k) = *i {
                if it.bits == 64 && ndef[d.0 as usize] == 1 && d != x {
                    let k = if op == IOp::Add { Some(k) } else { k.checked_neg() };
                    if let Some(k) = k {
                        let (root, base) = match known.iter().find(|e| e.0 == x) {
                            Some(e) => (e.1, e.2),
                            None => (x, 0),
                        };
                        if let Some(off) = base.checked_add(k) {
                            if off != k && off.abs() < 1 << 30 && root != d {
                                *i = if off >= 0 { Inst::IBinI(IOp::Add, it, d, root, off) } else { Inst::IBinI(IOp::Sub, it, d, root, -off) };
                                changed = true;
                            }
                            if off.abs() < 1 << 30 && root != d {
                                // d changes now; every entry resting on d is stale
                                known.retain(|e| e.0 != d && e.1 != d);
                                known.push((d, root, off));
                                continue;
                            }
                        }
                    }
                }
            }
            // anything else that defines a vreg ends the entries resting on it
            let mut ds: Vec<VReg> = Vec::new();
            for_defs(i, |d| ds.push(d));
            for d in ds {
                known.retain(|e| e.0 != d && e.1 != d);
            }
        }
    }
    changed
}

/// One lane of a straight-line vector group: the expression an f32 lane computes, with its loads
/// at addresses `root + disp`.
#[derive(Clone, PartialEq, Debug)]
enum LNode {
    Load { root: VReg, disp: i64 },
    Bin(FOp, Box<LNode>, Box<LNode>),
    Un(FUn, Box<LNode>),
    /// a value the same in every lane (read as it is at the group)
    Inv(VReg),
    /// an f32 constant (bits)
    K(u64),
}

struct LaneBlock<'a> {
    b: &'a Block,
    ndef: &'a [u32],
    nuse: &'a [u32],
}

impl LaneBlock<'_> {
    /// the last instruction before `before` that defines `v`
    fn def_before(&self, v: VReg, before: usize) -> Option<usize> {
        (0..before).rev().find(|k| {
            let mut d = false;
            for_defs(&self.b.insts[*k], |x| d |= x == v);
            d
        })
    }
    fn defined_in(&self, v: VReg, from: usize, to: usize) -> bool {
        (from..to.min(self.b.insts.len())).any(|k| {
            let mut d = false;
            for_defs(&self.b.insts[k], |x| d |= x == v);
            d
        })
    }
    /// the address `p + off` read at `at` as (root, offset): moves and constant steps of
    /// single-definition values are followed to the vreg the group's other addresses share
    fn address(&self, mut p: VReg, mut off: i64, at: usize) -> (VReg, i64) {
        let mut at = at;
        for _ in 0..8 {
            let Some(k) = self.def_before(p, at) else { break };
            if self.ndef[p.0 as usize] != 1 {
                break;
            }
            let (x, step) = match self.b.insts[k] {
                Inst::IBinI(IOp::Add, it, _, x, c) if it.bits == 64 => (x, c),
                Inst::IBinI(IOp::Sub, it, _, x, c) if it.bits == 64 => (x, -c),
                Inst::Mov(_, x) => (x, 0),
                _ => break,
            };
            // x must hold the same value at `at` as it did at the step
            if self.defined_in(x, k + 1, at) {
                break;
            }
            let Some(o) = off.checked_add(step) else { break };
            p = x;
            off = o;
            at = k;
        }
        (p, off)
    }
    /// The expression computing `v` as read at position `at`, and the positions of the
    /// instructions only it uses (they go when the group is vectorised).
    fn tree(&self, v: VReg, at: usize, used: &mut Vec<usize>, depth: u32) -> Option<LNode> {
        if depth > 6 {
            return None;
        }
        let leaf = |v: VReg| Some(LNode::Inv(v));
        let Some(k) = self.def_before(v, at) else { return leaf(v) };
        if self.ndef[v.0 as usize] != 1 || self.nuse[v.0 as usize] != 1 {
            return leaf(v);
        }
        let node = match self.b.insts[k] {
            Inst::Load(Mem::F32, _, p, off) => {
                let (root, disp) = self.address(p, off as i64, k);
                LNode::Load { root, disp }
            }
            Inst::Fconst(_, bits, false) => LNode::K(bits),
            Inst::FBin(op, false, _, a, b2) => {
                if !matches!(op, FOp::Add | FOp::Sub | FOp::Mul | FOp::Div | FOp::MinLt | FOp::MaxGt) {
                    return leaf(v);
                }
                let x = self.tree(a, k, used, depth + 1)?;
                let y = self.tree(b2, k, used, depth + 1)?;
                LNode::Bin(op, Box::new(x), Box::new(y))
            }
            Inst::FUnary(op, false, _, a) => {
                if !matches!(op, FUn::Neg | FUn::Abs | FUn::Sqrt) {
                    return leaf(v);
                }
                let x = self.tree(a, k, used, depth + 1)?;
                LNode::Un(op, Box::new(x))
            }
            _ => return leaf(v),
        };
        used.push(k);
        Some(node)
    }
}

/// `b` is lane `k` of the lane-0 expression `a`: the same operations and invariants, loads
/// `4k` bytes further on.
fn lane_of(a: &LNode, b: &LNode, k: i64) -> bool {
    match (a, b) {
        (LNode::Load { root: r0, disp: d0 }, LNode::Load { root: r1, disp: d1 }) => r0 == r1 && *d1 == d0 + 4 * k,
        (LNode::Bin(o0, x0, y0), LNode::Bin(o1, x1, y1)) => o0 == o1 && lane_of(x0, x1, k) && lane_of(y0, y1, k),
        (LNode::Un(o0, x0), LNode::Un(o1, x1)) => o0 == o1 && lane_of(x0, x1, k),
        (LNode::Inv(v0), LNode::Inv(v1)) => v0 == v1,
        (LNode::K(c0), LNode::K(c1)) => c0 == c1,
        _ => false,
    }
}

fn first_disp(n: &LNode) -> Option<(VReg, i64)> {
    match n {
        LNode::Load { root, disp } => Some((*root, *disp)),
        LNode::Bin(_, x, y) => first_disp(x).or_else(|| first_disp(y)),
        LNode::Un(_, x) => first_disp(x),
        _ => None,
    }
}

fn node_vregs(n: &LNode, out: &mut Vec<VReg>) {
    match n {
        LNode::Load { root, .. } => out.push(*root),
        LNode::Inv(v) => out.push(*v),
        LNode::Bin(_, x, y) => {
            node_vregs(x, out);
            node_vregs(y, out);
        }
        LNode::Un(_, x) => node_vregs(x, out),
        LNode::K(_) => {}
    }
}

/// The vector for a lane-0 expression, its instructions pushed on `ins`: loads of 16 bytes at the
/// lane-0 address, invariants and constants splat. Returns the vreg holding the result.
fn emit_lanes(f: &mut Func, n: &LNode, ins: &mut Vec<Inst>) -> VReg {
    let v = f.vreg(Cls::V128);
    match n {
        LNode::Load { root, disp } => ins.push(Inst::VLoad(v, *root, *disp as i32)),
        LNode::Inv(s) => ins.push(Inst::VSplat(v, *s)),
        LNode::K(bits) => ins.push(Inst::VConst(v, *bits as u32)),
        LNode::Bin(op, x, y) => {
            let a = emit_lanes(f, x, ins);
            let b = emit_lanes(f, y, ins);
            let o = match op {
                FOp::Add => VX::FAdd(VW::S, NanBits::Host),
                FOp::Sub => VX::FSub(VW::S, NanBits::Host),
                FOp::Mul => VX::FMul(VW::S, NanBits::Host),
                FOp::Div => VX::FDiv(VW::S, NanBits::Host),
                FOp::MinLt => VX::FMinLt(VW::S),
                _ => VX::FMaxGt(VW::S),
            };
            ins.push(Inst::VX(o, v, a, b));
        }
        LNode::Un(op, x) => {
            let a = emit_lanes(f, x, ins);
            let o = match op {
                FUn::Neg => VX::FNeg(VW::S),
                FUn::Abs => VX::FAbs(VW::S),
                _ => VX::FSqrt(VW::S, NanBits::Host),
            };
            ins.push(Inst::VX(o, v, a, a));
        }
    }
    v
}

/// Lane accumulators: in a loop, four f32 accumulators updated by one lane-wise expression each
/// (`acc[l] += x[l] * y[l]` for l in 0..4: the array, unrolled, lives in registers) become one
/// vector accumulator updated by the vector expression. Each lane keeps its own sequence of
/// operations in its own order, so the result is bit-identical. The accumulators enter the vector
/// where the loop is entered and leave it on every edge that exits the loop.
pub fn vectorize_lane_accumulators(options: &relax_session::Options, f: &mut Func) -> bool {
    if !vectorize_on(options) {
        return false;
    }
    let mut changed = false;
    // a bounded number of groups per function, one rewrite at a time (the CFG changes)
    for _ in 0..8 {
        if !lane_accumulators_once(f) {
            break;
        }
        changed = true;
    }
    changed
}

fn lane_accumulators_once(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let reach = relax_backend::regalloc::reachable(f);
    let mut ndef = vec![0u32; nv];
    let mut nuse = vec![0u32; nv];
    let mut any_f32_add = false;
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            for_uses(i, |u| nuse[u.0 as usize] += 1);
            any_f32_add |= matches!(i, Inst::FBin(FOp::Add | FOp::Sub | FOp::Mul, false, ..));
        }
        let mut tu = Vec::new();
        relax_backend::regalloc::term_uses(&b.term, &mut tu);
        for u in tu {
            nuse[u.0 as usize] += 1;
        }
    }
    if !any_f32_add {
        return false;
    }
    let preds = relax_backend::regalloc::preds_of(f, &reach);
    let loops = relax_backend::regalloc::natural_loops(f, &reach, &preds, false);
    for (head, members) in &loops {
        // straight-line loops: no calls, stores or exceptional edges anywhere in them
        let ok = members.iter().enumerate().all(|(bi, m)| {
            !*m || (!matches!(f.blocks[bi].term, Term::Invoke(..) | Term::Resume)
                && f.blocks[bi].insts.iter().all(|i| !crate::opt::is_mem_effect_pub(i)))
        });
        if !ok {
            continue;
        }
        for (bi, m) in members.iter().enumerate() {
            if !*m {
                continue;
            }
            if let Some(plan) = lane_group(f, bi, &ndef, &nuse, members) {
                apply_lane_group(f, *head, members, &preds, bi, plan);
                return true;
            }
        }
    }
    false
}

struct LaneGroup {
    /// the four accumulators, lane 0 first
    accs: [VReg; 4],
    op: FOp,
    acc_left: bool,
    tree: LNode,
    /// instructions to remove from the block, ascending
    gone: Vec<usize>,
}

fn lane_group(f: &Func, bi: usize, ndef: &[u32], nuse: &[u32], members: &[bool]) -> Option<LaneGroup> {
    let b = &f.blocks[bi];
    let lb = LaneBlock { b, ndef, nuse };
    // candidates: acc = acc op expr (through a temporary and a move, or directly)
    struct Cand {
        acc: VReg,
        op: FOp,
        acc_left: bool,
        tree: LNode,
        gone: Vec<usize>,
    }
    let mut cands: Vec<Cand> = Vec::new();
    for (i, ins) in b.insts.iter().enumerate() {
        let Inst::FBin(op @ (FOp::Add | FOp::Sub | FOp::Mul), false, t, x, y) = *ins else { continue };
        if f.vregs[t.0 as usize] != Cls::F32 {
            continue;
        }
        // the accumulator: an operand defined more than once whose update this is
        for (acc, other, acc_left) in [(x, y, true), (y, x, false)] {
            if ndef[acc.0 as usize] < 2 || acc == other {
                continue;
            }
            let mut gone = vec![i];
            if t == acc {
                // direct
            } else {
                if ndef[t.0 as usize] != 1 || nuse[t.0 as usize] != 1 {
                    continue;
                }
                let Some(mk) = (i + 1..b.insts.len()).find(|k| matches!(b.insts[*k], Inst::Mov(d, s) if d == acc && s == t)) else { continue };
                gone.push(mk);
            }
            let mut used = Vec::new();
            let Some(tree) = lb.tree(other, i, &mut used, 0) else { continue };
            // the expression must read memory (a load); a plain scalar recurrence stays scalar
            if first_disp(&tree).is_none() {
                continue;
            }
            let mut vs = Vec::new();
            node_vregs(&tree, &mut vs);
            if vs.contains(&acc) {
                continue;
            }
            gone.extend(used);
            gone.sort_unstable();
            gone.dedup();
            cands.push(Cand { acc, op, acc_left, tree, gone });
            break;
        }
        if cands.len() > 32 {
            return None;
        }
    }
    // four candidates that are lanes 0..3 of one expression
    'lead: for c0 in &cands {
        let Some((root0, d0)) = first_disp(&c0.tree) else { continue };
        let _ = (root0, d0);
        let mut lanes: Vec<&Cand> = vec![c0];
        for k in 1..4i64 {
            let next = cands.iter().find(|c| !lanes.iter().any(|l| std::ptr::eq(*l, *c)) && c.op == c0.op && c.acc_left == c0.acc_left && lane_of(&c0.tree, &c.tree, k));
            match next {
                Some(c) => lanes.push(c),
                None => continue 'lead,
            }
        }
        // distinct accumulators
        let accs = [lanes[0].acc, lanes[1].acc, lanes[2].acc, lanes[3].acc];
        if (0..4).any(|i| (0..i).any(|j| accs[i] == accs[j])) {
            continue;
        }
        let mut gone: Vec<usize> = lanes.iter().flat_map(|l| l.gone.iter().copied()).collect();
        gone.sort_unstable();
        gone.dedup();
        if gone.len() != lanes.iter().map(|l| l.gone.len()).sum::<usize>() {
            continue;
        }
        let first = gone[0];
        let last = *gone.last().unwrap();
        // the vector goes where the last lane finishes: nothing between may write memory or
        // redefine a value the group reads
        let mut vs = Vec::new();
        node_vregs(&c0.tree, &mut vs);
        if vs.iter().any(|v| lb.defined_in(*v, first, last + 1)) {
            continue;
        }
        // the accumulators are touched in the loop only by their updates
        let mut touched = false;
        for (mi, m) in members.iter().enumerate() {
            if !*m {
                continue;
            }
            for (k, ins) in f.blocks[mi].insts.iter().enumerate() {
                let in_group = mi == bi && gone.binary_search(&k).is_ok();
                let mut u = 0;
                let mut d = 0;
                for_uses(ins, |x| u += accs.contains(&x) as u32);
                for_defs(ins, |x| d += accs.contains(&x) as u32);
                if !in_group && (u > 0 || d > 0) {
                    touched = true;
                }
            }
            let mut tu = Vec::new();
            relax_backend::regalloc::term_uses(&f.blocks[mi].term, &mut tu);
            touched |= tu.iter().any(|x| accs.contains(x));
        }
        if touched || accs.iter().any(|a| f.vregs[a.0 as usize] != Cls::F32) {
            continue;
        }
        return Some(LaneGroup { accs, op: c0.op, acc_left: c0.acc_left, tree: c0.tree.clone(), gone });
    }
    None
}

fn apply_lane_group(f: &mut Func, head: u32, members: &[bool], preds: &[Vec<u32>], bi: usize, g: LaneGroup) {
    let pos = f.blocks[bi].pos[*g.gone.last().unwrap()];
    // the vector accumulator and the vector the group adds
    let va = f.vreg(Cls::V128);
    let mut ins: Vec<Inst> = Vec::new();
    let x = emit_lanes(f, &g.tree, &mut ins);
    let o = match g.op {
        FOp::Add => VX::FAdd(VW::S, NanBits::Host),
        FOp::Sub => VX::FSub(VW::S, NanBits::Host),
        _ => VX::FMul(VW::S, NanBits::Host),
    };
    ins.push(if g.acc_left { Inst::VX(o, va, va, x) } else { Inst::VX(o, va, x, va) });
    // remove the scalar lanes, add the vector at the last one's place
    let at = *g.gone.last().unwrap();
    let b = &mut f.blocks[bi];
    let mut keep_insts = Vec::with_capacity(b.insts.len());
    let mut keep_pos = Vec::with_capacity(b.insts.len());
    for (k, (i, p)) in b.insts.iter().zip(b.pos.iter()).enumerate() {
        if k == at {
            for v in ins.drain(..) {
                keep_insts.push(v);
                keep_pos.push(pos);
            }
        }
        if g.gone.binary_search(&k).is_err() {
            keep_insts.push(i.clone());
            keep_pos.push(*p);
        }
    }
    b.insts = keep_insts;
    b.pos = keep_pos;
    // entering the loop: the accumulators into the vector
    let mut enter = vec![Inst::VSplat(va, g.accs[0])];
    for (l, a) in g.accs.iter().enumerate().skip(1) {
        enter.push(Inst::VInsert(va, va, l as u8, *a));
    }
    for &p in &preds[head as usize] {
        if members[p as usize] {
            continue;
        }
        let nb = f.blocks.len() as u32;
        let tp = f.blocks[head as usize].term_pos;
        f.blocks.push(Block { insts: enter.clone(), pos: vec![tp; enter.len()], term: Term::Jump(head), term_pos: tp, count: -1.0 });
        f.blocks[p as usize].term.map_succs(|s| if s == head { nb } else { s });
    }
    // leaving it: back out of the vector on every exit edge
    let mut leave: Vec<Inst> = Vec::new();
    for (l, a) in g.accs.iter().enumerate() {
        leave.push(Inst::VExtract(*a, va, l as u8));
    }
    let nbl = members.len();
    for u in 0..nbl {
        if !members[u] {
            continue;
        }
        let exits: Vec<u32> = f.blocks[u].term.succs().filter(|s| !members.get(*s as usize).copied().unwrap_or(false)).collect();
        for x in exits {
            let nb = f.blocks.len() as u32;
            let tp = f.blocks[u].term_pos;
            f.blocks.push(Block { insts: leave.clone(), pos: vec![tp; leave.len()], term: Term::Jump(x), term_pos: tp, count: -1.0 });
            f.blocks[u].term.map_succs(|s| if s == x { nb } else { s });
        }
    }
}

/// Four consecutive f32 stores to the lanes of one 16 bytes, `[p + o + 4j] = v_j` (j = 0..3, no
/// other instruction between them), become one vector store of the four values (splat v0, insert
/// v1..v3): core::arch `_mm_setr_ps` and lane setters write a vector a lane at a time, and the
/// 16-byte load that reads it back cannot forward from four 4-byte stores (a store-forwarding
/// stall per vector; LLVM's DAG combiner merges such stores the same way). Returns whether
/// anything changed.
pub fn merge_lane_stores(options: &relax_session::Options, f: &mut Func) -> bool {
    if !merge_stores_on(options) {
        return false;
    }
    let mut changed = false;
    // use / def counts: a lane value loaded right before the stores, used only there, is loaded
    // next to its insert instead (x64 folds the load into vinsertps)
    let nv = f.vregs.len();
    let mut nuse = vec![0u32; nv];
    let mut ndef = vec![0u32; nv];
    for b in &f.blocks {
        for i in &b.insts {
            for_uses(i, |u| nuse[u.0 as usize] += 1);
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
        let mut tu = Vec::new();
        relax_backend::regalloc::term_uses(&b.term, &mut tu);
        tu.iter().for_each(|v| nuse[v.0 as usize] += 1);
    }
    for bi in 0..f.blocks.len() {
        if f.blocks[bi].insts.iter().filter(|i| matches!(i, Inst::Store(Mem::F32, ..))).count() < 4 {
            continue;
        }
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let old_pos = std::mem::take(&mut f.blocks[bi].pos);
        let mut out = Vec::with_capacity(old.len());
        let mut pos = Vec::with_capacity(old.len());
        let mut k = 0;
        while k < old.len() {
            let lanes = (k + 4 <= old.len()).then(|| {
                let Inst::Store(Mem::F32, base, o, _) = old[k] else { return None };
                let mut vs = [VReg(0); 4];
                for j in 0..4 {
                    match old[k + j] {
                        Inst::Store(Mem::F32, b2, o2, v) if b2 == base && o2 as i64 == o as i64 + 4 * j as i64 => vs[j] = v,
                        _ => return None,
                    }
                }
                Some((base, o, vs))
            }).flatten();
            let p = old_pos.get(k).copied().unwrap_or(0);
            // only when it saves instructions (LLVM SLP's build-vector cost): 4 stores (+ the m
            // single-use lane loads right before) against splat + inserts + 1 store, the m loads
            // folded into their inserts and lanes equal to lane 0 needing none. Lanes computed in
            // registers gain nothing and cost twice: the store waits for the whole insert chain,
            // and scalar reloads of its lanes forward from a 128-bit store slowly (sdfer esdt1d
            // pushes a CriticalMinimum whose four f32 the next iteration rereads as scalars:
            // sdf_glyphs 1.27x slower on x64 at equal instruction counts)
            let lanes = lanes.filter(|(base, _, vs)| {
                let mut back = out.len();
                while back > 0 && matches!(out[back - 1], Inst::Load(..)) {
                    back -= 1;
                }
                let folded = (1..4)
                    .filter(|&j| {
                        vs[j] == vs[0]
                            || (vs[1..j].iter().all(|v| *v != vs[j])
                                && out[back..].iter().any(|i| matches!(i, Inst::Load(Mem::F32, d, ..) if *d == vs[j] && nuse[d.0 as usize] == 1 && ndef[d.0 as usize] == 1 && base != d)))
                    })
                    .count();
                folded >= 2
            });
            if let Some((base, o, vs)) = lanes {
                // the loads of v1..v3 among the instructions right before (only loads there):
                // each goes next to its insert
                let mut moved: [Option<(Inst, u64)>; 4] = [None, None, None, None];
                let mut back = out.len();
                while back > 0 && matches!(out[back - 1], Inst::Load(..)) {
                    back -= 1;
                }
                let mut keep = Vec::new();
                for (q, i) in out.drain(back..).enumerate() {
                    let pq = pos[back + q];
                    match i {
                        Inst::Load(Mem::F32, d, ..) if vs[1..].contains(&d) && nuse[d.0 as usize] == 1 && ndef[d.0 as usize] == 1 && base != d => {
                            let j = vs.iter().position(|v| *v == d).unwrap();
                            moved[j] = Some((i, pq));
                        }
                        i => keep.push((i, pq)),
                    }
                }
                pos.truncate(back);
                for (i, pq) in keep {
                    out.push(i);
                    pos.push(pq);
                }
                let mut t = f.vreg(Cls::V128);
                out.push(Inst::VSplat(t, vs[0]));
                pos.push(p);
                for j in 1..4 {
                    if let Some((l, pq)) = moved[j].take() {
                        out.push(l);
                        pos.push(pq);
                    }
                    // the splat already holds lane 0's value in every lane (a zeroed vector)
                    if vs[j] == vs[0] {
                        continue;
                    }
                    let t2 = f.vreg(Cls::V128);
                    out.push(Inst::VInsert(t2, t, j as u8, vs[j]));
                    pos.push(p);
                    t = t2;
                }
                out.push(Inst::VStore(base, o, t));
                pos.push(p);
                changed = true;
                k += 4;
                continue;
            }
            out.push(old[k].clone());
            pos.push(p);
            k += 1;
        }
        f.blocks[bi].insts = out;
        f.blocks[bi].pos = pos;
    }
    changed
}

fn merge_stores_on(options: &relax_session::Options) -> bool {
    !options.no_store_merge
}

fn merge_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RELAX_MERGE_TRACE").is_ok())
}

/// Consecutive narrow stores of one value's bytes become one wide store: in a block, byte stores
/// `[p + off + j] = x >> 8j` (j = 0.., the shift optionally under a truncating Conv), with no other
/// memory access between them, become `[p + off] = x` as 8, 4 or 2 bytes (little-endian: byte j
/// of the wide store is bits 8j.. of x). Frontends split `[u8; N]` copies and byte-wise writes
/// this way (inflate's match copy: 8 byte stores of one u64 load, whose byte temporaries also
/// spilled). Byte stores of constants (an unrolled `[0u8; 8]` fill) become 8-, 4- and 2-byte
/// stores of the combined constant (LLVM: memset formation / MergeConsecutiveStores): a slot
/// filled byte-wise and read as a word is then one cell for sroa. Shifts and truncations left
/// without uses go. Returns whether anything changed.
pub fn merge_stores(options: &relax_session::Options, f: &mut Func) -> bool {
    if !merge_stores_on(options) {
        return false;
    }
    let nv = f.vregs.len();
    let mut nuse = vec![0u32; nv];
    let mut ndef = vec![0u32; nv];
    for b in &f.blocks {
        for i in &b.insts {
            for_uses(i, |u| nuse[u.0 as usize] += 1);
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
        match &b.term {
            Term::Branch(c, _, _) => nuse[c.0 as usize] += 1,
            Term::Ret(vs) => vs.iter().for_each(|v| nuse[v.0 as usize] += 1),
            _ => {}
        }
    }
    // single-definition integer constants (stored bytes of a fill)
    let mut kconst: Vec<Option<i64>> = vec![None; nv];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Iconst(d, k) = *i {
                if ndef[d.0 as usize] == 1 {
                    kconst[d.0 as usize] = Some(k);
                }
            }
        }
    }
    // merged constants: one definition each, at the top of the entry block
    let mut consts: std::collections::HashMap<i64, VReg> = std::collections::HashMap::new();
    let mut new_consts: Vec<(VReg, i64)> = Vec::new();
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        let n = f.blocks[bi].insts.len();
        if n < 2 || !f.blocks[bi].insts.iter().any(|i| matches!(i, Inst::Store(Mem::Int(1, _), ..)) || matches!(*i, Inst::Store(Mem::Int(2 | 4, _), _, _, v) if kconst[v.0 as usize].is_some())) {
            continue;
        }
        // single-def values defined in this block: index of their def
        let mut def_at: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
        for (k, i) in f.blocks[bi].insts.iter().enumerate() {
            for_defs(i, |d| {
                if ndef[d.0 as usize] == 1 {
                    def_at.insert(d.0, k);
                }
            });
        }
        let insts = &f.blocks[bi].insts;
        // (x, shift) a stored byte value comes from
        let source = |v: VReg| -> (VReg, i64) {
            let mut v = v;
            let mut sh = 0i64;
            for _ in 0..2 {
                match def_at.get(&v.0).map(|k| &insts[*k]) {
                    Some(Inst::Conv(Conv::IntToInt(it), _, w)) if it.bits == 8 && !it.signed => v = *w,
                    Some(Inst::IBinI(IOp::Shr, it, _, w, s)) if it.bits == 64 && sh == 0 && (0..64).contains(s) => {
                        v = *w;
                        sh = *s;
                    }
                    _ => break,
                }
            }
            (v, sh)
        };
        // groups: (store indices, base, start offset, x)
        let mut groups: Vec<(Vec<usize>, VReg, i32, VReg)> = Vec::new();
        let mut cur: Option<(Vec<usize>, VReg, i32, VReg)> = None;
        // runs of constant byte stores: (store indices, base, start offset, bytes)
        let mut cgroups: Vec<(Vec<usize>, VReg, i32, Vec<u8>)> = Vec::new();
        let mut ccur: Option<(Vec<usize>, VReg, i32, Vec<u8>)> = None;
        for (k, i) in insts.iter().enumerate() {
            let touches = matches!(i, Inst::Load(..) | Inst::LoadX(..) | Inst::VLoad(..) | Inst::VGather(..) | Inst::VTbl(..)) || crate::opt::is_mem_effect_pub(i);
            // 2- and 4-byte stores of constants join a run of constant stores too (two zeroed
            // u32 fields are one 8-byte store of 0: LLVM MergeConsecutiveStores)
            if let Inst::Store(Mem::Int(w @ (2 | 4), _), p, off, v) = *i {
                if let Some(c) = kconst[v.0 as usize].filter(|_| !relax_backend::stats::opt_off(options, "-cstores")) {
                    if let Some(g) = cur.take() {
                        groups.push(g);
                    }
                    let cont = matches!(&ccur, Some((ks, bp, st, _)) if *bp == p && off == st + ks.len() as i32);
                    if !cont {
                        if let Some(g) = ccur.take() {
                            cgroups.push(g);
                        }
                        ccur = Some((Vec::new(), p, off, Vec::new()));
                    }
                    let g = ccur.as_mut().unwrap();
                    for b in 0..w as usize {
                        g.0.push(k);
                        g.3.push((c as u64 >> (8 * b)) as u8);
                    }
                    continue;
                }
            }
            if let Inst::Store(Mem::Int(1, _), p, off, v) = *i {
                if let Some(c) = kconst[v.0 as usize].filter(|_| !relax_backend::stats::opt_off(options, "-cstores")) {
                    if let Some(g) = cur.take() {
                        groups.push(g);
                    }
                    let cont = matches!(&ccur, Some((ks, bp, st, _)) if *bp == p && off == st + ks.len() as i32);
                    if cont {
                        let g = ccur.as_mut().unwrap();
                        g.0.push(k);
                        g.3.push(c as u8);
                    } else {
                        if let Some(g) = ccur.take() {
                            cgroups.push(g);
                        }
                        ccur = Some((vec![k], p, off, vec![c as u8]));
                    }
                    continue;
                }
                if let Some(g) = ccur.take() {
                    cgroups.push(g);
                }
                let (x, sh) = source(v);
                let cont = match &cur {
                    Some((ks, bp, st, cx)) => *bp == p && *cx == x && off == st + ks.len() as i32 && sh == 8 * ks.len() as i64,
                    None => false,
                };
                if cont {
                    cur.as_mut().unwrap().0.push(k);
                    continue;
                }
                if let Some(g) = cur.take() {
                    groups.push(g);
                }
                // a store that defines its own base / value as something else ends nothing more
                if sh == 0 {
                    cur = Some((vec![k], p, off, x));
                }
                continue;
            }
            if touches {
                if let Some(g) = cur.take() {
                    groups.push(g);
                }
                if let Some(g) = ccur.take() {
                    cgroups.push(g);
                }
            } else {
                // the base or x redefined in between: the group ends
                let mut redef = false;
                if let Some((_, bp, _, cx)) = &cur {
                    for_defs(i, |d| redef |= d == *bp || d == *cx);
                }
                if redef {
                    groups.push(cur.take().unwrap());
                }
                let mut redef = false;
                if let Some((_, bp, _, _)) = &ccur {
                    for_defs(i, |d| redef |= d == *bp);
                }
                if redef {
                    cgroups.push(ccur.take().unwrap());
                }
            }
        }
        if let Some(g) = cur.take() {
            groups.push(g);
        }
        if let Some(g) = ccur.take() {
            cgroups.push(g);
        }
        let mut remove: Vec<usize> = Vec::new();
        let mut replace: Vec<(usize, Inst)> = Vec::new();
        for (ks, p, st, x) in groups {
            let w = if ks.len() >= 8 { 8 } else if ks.len() >= 4 { 4 } else if ks.len() >= 2 { 2 } else { continue };
            let last = ks[w - 1];
            replace.push((last, Inst::Store(Mem::Int(w as u8, false), p, st, x)));
            remove.extend_from_slice(&ks[..w - 1]);
        }
        for (ks, p, st, bytes) in cgroups {
            // ks: per byte, the store that writes it; a wide store covers whole stores only
            let boundary = |q: usize| q == 0 || q == ks.len() || ks[q] != ks[q - 1];
            let mut j = 0;
            while ks.len() - j >= 2 {
                let Some(w) = [8usize, 4, 2].into_iter().find(|&w| ks.len() - j >= w && boundary(j + w) && ks[j] != ks[j + w - 1]) else {
                    // no wider store starts here: step over this store's bytes
                    let mut e = j + 1;
                    while !boundary(e) {
                        e += 1;
                    }
                    j = e;
                    continue;
                };
                let mut val = 0u64;
                for (b, &x) in bytes[j..j + w].iter().enumerate() {
                    val |= (x as u64) << (8 * b);
                }
                // sign-extended to the register: the store writes the low w bytes
                let val = ((val << (64 - 8 * w)) as i64) >> (64 - 8 * w);
                let kv = match consts.get(&val) {
                    Some(v) => *v,
                    None => {
                        let v = f.vreg(Cls::I);
                        nuse.push(0);
                        ndef.push(1);
                        new_consts.push((v, val));
                        consts.insert(val, v);
                        v
                    }
                };
                replace.push((ks[j + w - 1], Inst::Store(Mem::Int(w as u8, false), p, st + j as i32, kv)));
                let mut gone: Vec<usize> = ks[j..j + w - 1].iter().copied().filter(|&x| x != ks[j + w - 1]).collect();
                gone.dedup();
                remove.extend_from_slice(&gone);
                j += w;
            }
        }
        if replace.is_empty() {
            continue;
        }
        if merge_trace() {
            for (_, i) in &replace {
                eprintln!("merge_stores {}: b{} {:?}", f.name, bi, i);
            }
        }
        changed = true;
        let blk = &mut f.blocks[bi];
        for (k, i) in &replace {
            // uses of the old value go away
            for_uses(&blk.insts[*k], |u| nuse[u.0 as usize] -= 1);
            for_uses(i, |u| nuse[u.0 as usize] += 1);
            blk.insts[*k] = i.clone();
        }
        let mut dead = vec![false; blk.insts.len()];
        for k in remove {
            for_uses(&blk.insts[k], |u| nuse[u.0 as usize] -= 1);
            dead[k] = true;
        }
        // shifts / truncations nobody reads any more (back to front: chains)
        for k in (0..blk.insts.len()).rev() {
            if dead[k] {
                continue;
            }
            let d = match blk.insts[k] {
                Inst::IBinI(IOp::Shr, _, d, _, _) | Inst::Conv(Conv::IntToInt(_), d, _) => d,
                _ => continue,
            };
            if nuse[d.0 as usize] == 0 && ndef[d.0 as usize] == 1 {
                for_uses(&blk.insts[k], |u| nuse[u.0 as usize] -= 1);
                dead[k] = true;
            }
        }
        let mut k = 0;
        blk.insts.retain(|_| {
            k += 1;
            !dead[k - 1]
        });
        let mut k2 = 0;
        blk.pos.retain(|_| {
            k2 += 1;
            !dead[k2 - 1]
        });
    }
    if !new_consts.is_empty() {
        let b0 = &mut f.blocks[0];
        let has_pos = b0.pos.len() == b0.insts.len();
        let p0 = b0.pos.first().copied().unwrap_or(b0.term_pos);
        for (k, (v, val)) in new_consts.into_iter().enumerate() {
            b0.insts.insert(k, Inst::Iconst(v, val));
            if has_pos {
                b0.pos.insert(k, p0);
            }
        }
    }
    changed
}

fn full_unroll_on(options: &relax_session::Options) -> bool {
    !options.no_full_unroll
}

/// The loop count `cond(v)` decides for `ICmpI(cond, signed, _, x, k)` at value v of x.
fn cmp_holds(cond: Cond, signed: bool, v: i64, k: i64) -> bool {
    let (a, b) = (v, k);
    if signed {
        match cond {
            Cond::Eq => a == b,
            Cond::Ne => a != b,
            Cond::Lt => a < b,
            Cond::Le => a <= b,
            Cond::Gt => a > b,
            Cond::Ge => a >= b,
        }
    } else {
        let (a, b) = (a as u64, b as u64);
        match cond {
            Cond::Eq => a == b,
            Cond::Ne => a != b,
            Cond::Lt => a < b,
            Cond::Le => a <= b,
            Cond::Gt => a > b,
            Cond::Ge => a >= b,
        }
    }
}

fn retarget(t: &mut Term, mut m: impl FnMut(u32) -> u32) {
    match t {
        Term::Jump(a) => *a = m(*a),
        Term::Branch(_, a, b) => {
            *a = m(*a);
            *b = m(*b);
        }
        Term::Invoke(a, b) => {
            *a = m(*a);
            *b = m(*b);
        }
        _ => {}
    }
}

fn unroll_trace() -> Option<&'static str> {
    static VALUE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    VALUE.get_or_init(|| {
        let _k = relax_session::phase::kept();
        std::env::var("RELAX_UNROLL_TRACE").ok()
    }).as_deref()
}

/// Full unrolling of loops with a small constant trip count (`for _ in 0..3`, deflate's hash-chain
/// probes): an innermost loop with one latch whose counter `c` is set once outside the loop by a
/// constant and stepped once in the latch by `c = c + s` (c read nowhere outside the loop; calls
/// in the body are copied with it) and whose latch decides "again" by `c' cmp k` with constants, runs T <= 4
/// times; it gets T copies in front of it, copy j reading c as the constant c0 + j*s (a fresh
/// vreg each, so constant folding sees single definitions). Copy j's back edge enters copy j+1;
/// the last copy's back edge enters the original loop with c restored (never taken: its compare
/// folds to the exit). Exits from inside the body stay as they are. Run `simplify` after it.
/// RELAX_NO_FULL_UNROLL=1 off. Returns whether anything changed.
pub fn unroll_const_loops(options: &relax_session::Options, f: &mut Func) -> bool {
    if !full_unroll_on(options) || f.blocks.len() < 2 {
        return false;
    }
    let tr = unroll_trace().is_some_and(|w| f.name.ends_with(w));
    let reach = relax_backend::regalloc::reachable(f);
    let preds = relax_backend::regalloc::preds_of(f, &reach);
    let loops = relax_backend::regalloc::natural_loops(f, &reach, &preds, false);
    if loops.is_empty() {
        return false;
    }
    let nv = f.vregs.len();
    // defs and uses per vreg: (count, block of the last def)
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
    }
    // the single definitions (block, instruction): base-relative counters below
    let mut def1: std::collections::HashMap<u32, (u32, Inst)> = std::collections::HashMap::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            for_defs(i, |d| {
                if ndef[d.0 as usize] == 1 {
                    def1.insert(d.0, (bi as u32, i.clone()));
                }
            });
        }
    }
    let mut changed = false;
    let mut budget = 800usize;
    // innermost first (natural_loops sorts by size); loops nested in a changed one are skipped
    let mut touched = vec![false; f.blocks.len()];
    'next: for (h, m0) in &loops {
        let h = *h;
        let core: Vec<u32> = (0..m0.len() as u32).filter(|b| m0[*b as usize]).collect();
        if core.iter().any(|b| touched[*b as usize]) {
            if tr {
                eprintln!("full-unroll {}: loop b{}: overlaps a changed loop", f.name, h);
            }
            continue;
        }
        // innermost: no other loop's header inside
        if loops.iter().any(|(h2, _)| *h2 != h && m0[*h2 as usize]) {
            if tr {
                eprintln!("full-unroll {}: loop b{} ({} blocks): not innermost (headers {:?})", f.name, h, core.len(), loops.iter().filter(|(h2, _)| *h2 != h && m0[*h2 as usize]).map(|x| x.0).collect::<Vec<_>>());
            }
            continue;
        }
        // The loop's panic exits (a bounds check's failing edge: a small block that calls the panic
        // routine and never returns, entered only from the loop) travel with its copies: each copy
        // has its own, reading its own counter, and once the copy's checks fold (constant index
        // against a known length) they are dead. They are not early exits.
        let mut mm = m0.clone();
        let mut tails: Vec<u32> = Vec::new();
        for &b in &core {
            for sc in f.blocks[b as usize].term.succs() {
                let sb = &f.blocks[sc as usize];
                if !mm[sc as usize] && matches!(sb.term, Term::Unreachable) && sb.insts.len() <= 8 && !sb.insts.is_empty()
                    && preds[sc as usize].len() == 1
                    && m0[preds[sc as usize][0] as usize]
                    && !loops.iter().any(|(h2, _)| *h2 == sc)
                {
                    mm[sc as usize] = true;
                    tails.push(sc);
                }
            }
        }
        let m = &mm;
        let mut members = core.clone();
        members.extend(tails.iter().copied());
        members.sort_unstable();
        // The size that runs: a counted build's blocks the counts never saw run (the rare arms of
        // a loop body, inlined) are copied but cost no time; the copies' code is O3 tier growth,
        // capped by `all` (an app's reconcile loop over two borrowed shape fields: 330 insts with
        // its never-taken create/destroy arms, about 100 that run; GAPB)
        let all: usize = members.iter().map(|b| f.blocks[*b as usize].insts.len() + 1).sum();
        let run_only = !relax_backend::stats::opt_off(options, "-unrollrun");
        let size: usize = members.iter().filter(|b| !run_only || f.blocks[**b as usize].count != 0.0).map(|b| f.blocks[*b as usize].insts.len() + 1).sum();
        if tr {
            eprintln!("full-unroll {}: loop b{} {} blocks {} insts ({} run)", f.name, h, members.len(), all, size);
        }
        // (the function's budget below bounds the total growth)
        if size > 160 || all > 640 {
            if tr {
                eprintln!("  too large");
            }
            continue;
        }
        // Calls in the body are copied with it, as LLVM's full unroll does (a call is part of the
        // body's cost, not a bar): reconcile's `for (shape, applied, wanted, solid) in [(&mut
        // entry.a, ..), (..)]` holds the shape calls of the rare changed case, and only unrolled
        // do the array's borrows become constant-offset stores that forward, so `entry` stops
        // escaping and its copies join (GAPB). An invoke's unwind edge leaves for the shared
        // cleanup: each copy keeps it, it is no early exit.
        // RELAX_OPT=-unrollcalls: loops with calls stay rolled
        if relax_backend::stats::opt_off(options, "-unrollcalls") && core.iter().any(|b| f.blocks[*b as usize].insts.iter().any(|i| matches!(i, Inst::Call(..))) || matches!(f.blocks[*b as usize].term, Term::Invoke(..) | Term::Resume)) {
            continue;
        }
        let latches: Vec<u32> = members.iter().copied().filter(|b| f.blocks[*b as usize].term.succs().any(|s| s == h)).collect();
        // only loops that leave from the header or the latch: a body with early exits (a search
        // loop returning from inside) grows by every copy of its exit paths for little gain
        // (deflate's probe loop measured slower unrolled on x64)
        // An edge to the block the header leaves for is no early exit: it is the loop's own end,
        // tested in two steps (a `for` over an array: the header's `start == end`, then the
        // loaded Option's tag in the next block, which only the copies' constant loads fold).
        let h_exit: Vec<u32> = f.blocks[h as usize].term.succs().filter(|s| !m.get(*s as usize).copied().unwrap_or(false)).collect();
        let split_ok = !relax_backend::stats::opt_off(options, "-unrollmulti");
        let early_exit = members.iter().any(|b| *b != h && !latches.contains(b) && {
            let t = &f.blocks[*b as usize].term;
            let unwind = if let Term::Invoke(_, u) = *t { Some(u) } else { None };
            t.succs().any(|s| Some(s) != unwind && !(split_ok && h_exit.contains(&s)) && !m.get(s as usize).copied().unwrap_or(false))
        });
        let split_used = !early_exit && members.iter().any(|b| *b != h && !latches.contains(b) && f.blocks[*b as usize].term.succs().any(|s| h_exit.contains(&s)));
        // Early exits are worth their copies where the counter indexes a local array that the
        // copies turn into plain values: camera_path_limit's `for a in 0..3` over `[o.x, o.y,
        // o.z]`-style arrays with a `continue 'entities` from two tests stayed a loop of indexed
        // stack loads and six stores per entity; unrolled, every access has a constant offset and
        // the slots go to registers (rustc: no arrays left, the axis tests hoisted out of the
        // entity loop). Only short trips (checked with the trip count below).
        let early_ok = early_exit && !relax_backend::stats::opt_off(options, "-unrollearly");
        let early_exit = early_exit && !early_ok;
        if early_exit {
            if tr {
                eprintln!("  early exits from the body (header b{}, latches {:?})", h, latches);
                for &b in &members {
                    eprintln!("    b{}: {:?} -> {:?}", b, f.blocks[b as usize].insts, f.blocks[b as usize].term);
                }
            }
            continue;
        }
        // Several latches (a `continue` in the body) are fine when the header decides and the
        // counter's step dominates every latch: each back edge enters the next copy.
        if latches.is_empty() || (latches.len() > 1 && relax_backend::stats::opt_off(options, "-unrollmulti")) {
            continue;
        }
        let l = latches[0];
        let multi = latches.len() > 1;
        // (the entries of the pass start: a copy of an earlier loop that leaves into this one keeps
        // entering the original. Taking the current entries instead made the race's draw +11..30%
        // instructions: the newly live copies of consecutive loops; GAPB2 racebisect)
        let outside: Vec<u32> = preds[h as usize].iter().copied().filter(|p| !m[*p as usize]).collect();
        if outside.is_empty() {
            continue;
        }
        // the counter step c = c + s: in the latch, or (with the test in the header) in a block
        // that dominates the latch -- an iterator's `start += 1` right after its test
        let lb = &f.blocks[l as usize];
        // (block, k, type, counter, step, the step's value made at k is a constant in each copy)
        let mut cands = Vec::new();
        let mut first = None;
        for &sb in std::iter::once(&l).filter(|_| !multi).chain(members.iter().filter(|b| multi || **b != l)) {
            for (k, i) in f.blocks[sb as usize].insts.iter().enumerate() {
                if let Inst::IBinI(IOp::Add, it, d, a, s) = *i {
                    if d == a && ndef[d.0 as usize] == 2 {
                        cands.push((sb, k, it, d, s, false));
                    }
                }
                // a narrow counter steps through a normalizing chain: `c = conv(conv(c) + s)`
                // (an i32 `for i in 0..8`), or a copy of `c + s` (a register-promoted field),
                // the last link a move into c
                if let Inst::Mov(d, x) = *i {
                    if ndef[d.0 as usize] == 2 {
                        if let Some((it, s)) = narrow_step(&f.blocks[sb as usize], k, d, x, &ndef) {
                            cands.push((sb, k, it, d, s, true));
                        }
                    }
                }
            }
            if first.is_none() && !cands.is_empty() {
                first = cands.last().copied();
            }
        }
        // the counter the header's or the latch's test reads (an enumerated cursor steps two:
        // `enumerate()`'s index and the slice cursor whose end decides), else the last step of
        // the first block with one
        let decides = |d: VReg| -> bool {
            [h, l].iter().any(|&b| {
                let blk = &f.blocks[b as usize];
                let Term::Branch(cv, _, _) = blk.term else { return false };
                blk.insts.iter().any(|i| match *i {
                    Inst::ICmpI(_, _, x, a, _) => x == cv && a == d,
                    Inst::ICmp(_, _, x, a, e) => x == cv && (a == d || e == d),
                    _ => false,
                })
            })
        };
        let found = cands.iter().copied().find(|x| decides(x.3)).or(first);
        let Some((sb, ak, it, c, step, step_const)) = found else {
            if tr {
                eprintln!("  no counter step c = c + s in latch b{}", l);
            }
            continue;
        };
        // a step outside the latch: it must dominate the latch, and blocks it dominates read the
        // stepped counter in the copies. A step in one of several latches must dominate the
        // others (`iter().filter(..).count()`: the kept arm counts and jumps back too)
        let step_in_latches = multi && latches.contains(&sb);
        if step_in_latches && relax_backend::stats::opt_off(options, "-unrollsteplatch") {
            if tr {
                eprintln!("  {} latches and the step in latch b{}", latches.len(), sb);
            }
            continue;
        }
        let doms = sb != l || step_in_latches;
        let idom = if doms { relax_backend::dom::idoms(f) } else { Vec::new() };
        let dominated = |b: u32| -> bool {
            if !doms {
                return false;
            }
            let mut x = b;
            for _ in 0..100_000 {
                if x == sb {
                    return true;
                }
                let p = idom[x as usize];
                if p == u32::MAX || p == x {
                    return false;
                }
                x = p;
            }
            false
        };
        if doms && (sb == h || !latches.iter().all(|&x| dominated(x))) {
            if tr {
                eprintln!("  step in b{} does not dominate the latches {:?}", sb, latches);
            }
            continue;
        }
        if !f.vregs.get(c.0 as usize).map_or(false, |x| *x == Cls::I) {
            continue;
        }
        // where the counter starts: a constant, or (an iterator's cursor) an address plus a
        // constant -- a slot's, a parameter's or one made in front of the loop -- with its end
        // the same address plus a constant (`[T; N].iter()`: the trip count is N)
        let mut start: Option<(Base, i64)> = None;
        let mut starts = 0;
        for (bi, b) in f.blocks.iter().enumerate() {
            if m.get(bi).copied().unwrap_or(false) {
                continue;
            }
            for i in &b.insts {
                let s0 = match *i {
                    Inst::Iconst(d, v) if d == c => Some((Base::Const, v)),
                    Inst::Mov(d, x) if d == c => affine(&def1, &ndef, x),
                    Inst::IBinI(IOp::Add, _, d, x, k) if d == c => affine(&def1, &ndef, x).map(|(b, o)| (b, o.wrapping_add(k))),
                    _ => {
                        let mut redef = false;
                        for_defs(i, |d| redef |= d == c);
                        if redef {
                            starts += 1;
                        }
                        continue;
                    }
                };
                starts += 1;
                start = s0;
            }
        }
        let start = if starts == 1 { start } else { None };
        let start = start.filter(|(b, _)| *b == Base::Const || it.bits == 64);
        // an address base must hold at the header: a slot (made afresh in each copy), a parameter,
        // or a definition in a block that dominates the loop
        let start = start.filter(|(b, _)| match *b {
            Base::Const | Base::Slot(_) => true,
            Base::Reg(x) => f.params.contains(&x) || def1.get(&x.0).is_some_and(|(db, _)| {
                let id = relax_backend::dom::idoms(f);
                let mut y = h;
                for _ in 0..100_000 {
                    if y == *db {
                        return !m.get(*db as usize).copied().unwrap_or(false);
                    }
                    let p = id[y as usize];
                    if p == u32::MAX || p == y {
                        return false;
                    }
                    y = p;
                }
                false
            }),
        });
        // a cursor over a local array: the copies read it at constant offsets (sroa takes it)
        let slot_cursor = matches!(start, Some((Base::Slot(_), _))) && !relax_backend::stats::opt_off(options, "-unrollcursor");
        // The two generalisations (several latches, an exit split over two blocks) pay only where
        // the copies remove an escape: reconcile's `[(&mut entry.a, ..), (..)]` array read by its
        // IntoIter (`data + start * stride`) becomes constant-offset loads that forward the
        // borrows, and `entry` stops escaping (LLVM's full-unroll analysis grants the same bonus
        // for allocas SROA can then split). box3d's scatter_bodies (`for lane in 0..4` with a
        // `continue`, reading `simd_body.v.x.get(lane)` from a slot of floats, inlined twice in
        // solve_contacts_convex) only saves an index add per access: unrolled, it cost +277
        // static insts and +1.8..11% instructions on box3d_physics (GAPB2). The copies of such
        // loops also stay within a tighter budget.
        if early_ok && !counter_indexes_local_values(f, &members, c, tr) {
            if tr {
                eprintln!("  early exits from the body and no local array indexed by the counter (header b{})", h);
            }
            continue;
        }
        let general = multi || split_used;
        // a loop whose latches leave too (`continue 'outer` from the test that ends an iteration):
        // its copies pay where the counter indexes a local array of plain values (camera_path_limit's
        // slab test over `[f32; 3]`s), as for early exits; short trips only (checked below)
        let mut local_gain = early_ok;
        if general {
            let gain = counter_indexes_fixed_memory(f, &members, c);
            local_gain = local_gain || slot_cursor || (!gain && !relax_backend::stats::opt_off(options, "-unrollearly") && counter_indexes_local_values(f, &members, c, tr));
            if tr {
                eprintln!("  several latches {} / split exit {}: fixed-address gain {} local-values gain {}", multi, split_used, gain, local_gain);
            }
            if !gain && !local_gain {
                continue;
            }
        }
        // the decision: in the latch after the step (`Branch(c' cmp k, h, exit)`: iteration j
        // runs again iff cmp(c0 + (j+1)s)), or in the header before anything redefines c
        // (`Branch(c cmp k, body, exit)`: the header's test of iteration j sees c0 + j*s)
        let Some((cbase, c0)) = start else {
            if tr {
                eprintln!("  counter v{} has no constant or address-plus-constant start outside", c.0);
            }
            continue;
        };
        // the end of a base-relative counter: the same base plus a constant, made outside the
        // loop; only (in)equality (exact however the addresses wrap)
        let end_of = |e: VReg| -> Option<i64> {
            let (eb, ek) = affine(&def1, &ndef, e)?;
            let outside_def = f.params.contains(&e) || def1.get(&e.0).is_some_and(|(db, _)| !m.get(*db as usize).copied().unwrap_or(false));
            (eb == cbase && outside_def).then_some(ek)
        };
        let find_cmp = |b: &Block, from: usize| -> Option<(Cond, bool, i64, VReg, usize)> {
            let Term::Branch(cv, _, _) = b.term else { return None };
            let mut cmp = None;
            for (k0, i) in b.insts[from..].iter().enumerate() {
                if let Inst::ICmpI(cond, signed, d, x, k) = *i {
                    if d == cv && x == c && cbase == Base::Const {
                        cmp = Some((cond, signed, k, cv, from + k0));
                        continue;
                    }
                }
                if let Inst::ICmp(cond, signed, d, x, e) = *i {
                    let swapped = match cond {
                        Cond::Lt => Cond::Gt,
                        Cond::Gt => Cond::Lt,
                        Cond::Le => Cond::Ge,
                        Cond::Ge => Cond::Le,
                        q => q,
                    };
                    let hit = if x == c && e != c { end_of(e).map(|k| (cond, k)) } else if e == c && x != c { end_of(x).map(|k| (swapped, k)) } else { None };
                    if let Some((q, k)) = hit.filter(|(q, _)| cbase == Base::Const || matches!(q, Cond::Eq | Cond::Ne)) {
                        if d == cv {
                            cmp = Some((q, signed, k, cv, from + k0));
                            continue;
                        }
                    }
                }
                let mut redef = false;
                for_defs(i, |d| redef |= d == cv || d == c);
                if redef {
                    cmp = None;
                }
            }
            cmp
        };
        let latch_cmp = if !multi && sb == l && matches!(lb.term, Term::Branch(_, a, b) if a == h || b == h) { find_cmp(lb, ak + 1) } else { None };
        let header_cmp = if latch_cmp.is_none() && !latches.contains(&h) {
            let hb = &f.blocks[h as usize];
            if hb.insts.iter().any(|i| { let mut r = false; for_defs(i, |d| r |= d == c); r }) { None } else { find_cmp(hb, 0) }
        } else {
            None
        };
        let (cond, signed, k, in_latch, cmp_at) = match (latch_cmp, header_cmp) {
            (Some((a, b, k, _, at)), _) => (a, b, k, true, at),
            (None, Some((a, b, k, _, at))) => (a, b, k, false, at),
            _ => {
                if tr {
                    eprintln!("  no compare of the counter deciding the loop (latch b{}, header b{})", l, h);
                }
                continue;
            }
        };
        let (bt, be) = match if in_latch { &f.blocks[l as usize].term } else { &f.blocks[h as usize].term } {
            Term::Branch(_, a, b) => (*a, *b),
            _ => continue,
        };
        // c is read nowhere outside the loop
        for (bi, b) in f.blocks.iter().enumerate() {
            if m.get(bi).copied().unwrap_or(false) {
                continue;
            }
            let mut used = false;
            for i in &b.insts {
                for_uses(i, |u| used |= u == c);
            }
            let mut tu = Vec::new();
            relax_backend::regalloc::term_uses(&b.term, &mut tu);
            if used || tu.contains(&c) {
                if tr {
                    eprintln!("  counter v{} read outside the loop (b{})", c.0, bi);
                }
                continue 'next;
            }
        }
        // trip count (copies): a latch test runs iteration j again iff cmp(c0 + (j+1)s) holds the
        // way back to the header; a header test enters the body of iteration j iff cmp(c0 + j*s)
        // holds toward the loop (one more copy: the header that exits)
        let stay_on_true = if in_latch { bt == h } else { m.get(bt as usize).copied().unwrap_or(false) };
        if in_latch && !(bt == h || be == h) {
            continue;
        }
        if !in_latch && m.get(bt as usize).copied().unwrap_or(false) == m.get(be as usize).copied().unwrap_or(false) {
            continue;
        }
        let mut t = 1usize;
        let mut v = if in_latch { extend_const(c0.wrapping_add(step), it) } else { c0 };
        loop {
            if cmp_holds(cond, signed, v, k) != stay_on_true {
                break;
            }
            v = extend_const(v.wrapping_add(step), it);
            t += 1;
            // up to 4 trips; a small body (the copies fold: `i & 1` selects, `for i in 0..8` over
            // a matrix's corners) up to 8
            if t > if size <= 64 { 9 } else { 5 } {
                if tr {
                    eprintln!("  trip count too large for a body of {} insts", size);
                }
                continue 'next;
            }
        }
        let trips = if in_latch { t } else { t - 1 };
        if local_gain && trips > 4 {
            if tr {
                eprintln!("  early exits: {} copies, over 4", t);
            }
            continue;
        }
        if size * t > budget || all * t > 2400 || ((general || early_ok) && size * t > 400) {
            if tr {
                eprintln!("  {} copies of {} insts over the budget {}", t, size, budget);
            }
            continue;
        }
        budget -= size * t;
        if tr {
            eprintln!("  unrolled {}: {} copies", f.name, t);
        }
        // iteration-local values: single-definition vregs defined in the loop whose every use is
        // in the loop, in a block the definition's block dominates (after it, in its own block).
        // Each copy gets its own: as clones of one definition they would be defined T+1 times,
        // and constant folding (the copy's counter is a constant) follows single definitions only
        let local = iteration_locals(f, &members, m, &ndef, c);
        // T copies
        let base = f.blocks.len() as u32;
        let idx: Vec<u32> = members.clone();
        let pos_of = |b: u32| -> usize { idx.iter().position(|x| *x == b).unwrap() };
        let n = idx.len() as u32;
        let mut v = c0;
        // the block and index of the deciding compare: a base-relative counter's copies get its
        // value (the constant folding a constant counter gets from simplify)
        let cmp_blk = if in_latch { l } else { h };
        for j in 0..t as u32 {
            let rj = f.vreg(Cls::I);
            let rj2 = f.vreg(Cls::I);
            // this copy's base: a slot's address afresh (in the header copy), else the register
            let bj = match cbase {
                Base::Slot(_) => f.vreg(Cls::I),
                Base::Reg(x) => x,
                Base::Const => rj,
            };
            let mut fresh: std::collections::HashMap<u32, VReg> = std::collections::HashMap::new();
            for &lv in &local {
                let cls = f.vregs[lv as usize];
                fresh.insert(lv, f.vreg(cls));
            }
            for (q, &b) in idx.iter().enumerate() {
                let mut blk = f.blocks[b as usize].clone();
                let latch = latches.contains(&b);
                // the block holding the step: after it, the copy's next counter; blocks the step
                // dominates: the next counter throughout
                let step_blk = b == sb;
                let past = b != sb && dominated(b);
                for (ii, ins) in blk.insts.iter_mut().enumerate() {
                    let after = past || (step_blk && ii > ak);
                    if step_blk && ii == ak {
                        // the normalizing chain's value is this copy's next counter: a constant
                        *ins = if !step_const {
                            Inst::IBinI(IOp::Add, it, rj2, rj, step)
                        } else if cbase == Base::Const {
                            Inst::Iconst(rj2, extend_const(v.wrapping_add(step), it))
                        } else {
                            Inst::IBinI(IOp::Add, it, rj2, bj, v.wrapping_add(step))
                        };
                        continue;
                    }
                    if cbase != Base::Const && b == cmp_blk && ii == cmp_at {
                        if let Inst::ICmp(_, _, d, ..) = *ins {
                            let cv = fresh.get(&d.0).copied().unwrap_or(d);
                            let at = if in_latch { extend_const(v.wrapping_add(step), it) } else { v };
                            *ins = Inst::Iconst(cv, cmp_holds(cond, signed, at, k) as i64);
                            continue;
                        }
                    }
                    ins.vregs_mut(|r| {
                        if *r == c {
                            *r = if after { rj2 } else { rj };
                        } else if let Some(x) = fresh.get(&r.0) {
                            *r = *x;
                        }
                    });
                }
                relax_backend::regalloc::term_vregs_mut(&mut blk.term, |r| {
                    if *r == c {
                        *r = if step_blk || past { rj2 } else { rj };
                    } else if let Some(x) = fresh.get(&r.0) {
                        *r = *x;
                    }
                });
                let last = j + 1 == t as u32;
                retarget(&mut blk.term, |s| {
                    if latch && s == h {
                        if last { h } else { base + (j + 1) * n + pos_of(h) as u32 }
                    } else if m.get(s as usize).copied().unwrap_or(false) {
                        base + j * n + pos_of(s) as u32
                    } else {
                        s
                    }
                });
                if latch && last {
                    // the original loop continues from the counter this copy reached (in front of
                    // an invoke's call, which ends its block; nothing in the copy reads c)
                    let p = blk.term_pos;
                    let at = blk.insts.len() - usize::from(matches!(blk.term, Term::Invoke(..)) && !blk.insts.is_empty());
                    blk.insts.insert(at, Inst::Mov(c, rj2));
                    if blk.pos.len() >= at { blk.pos.insert(at, p); }
                }
                if b == h {
                    let p = blk.pos.first().copied().unwrap_or(blk.term_pos);
                    match cbase {
                        Base::Const => {
                            blk.insts.insert(0, Inst::Iconst(rj, v));
                            blk.pos.insert(0, p);
                        }
                        Base::Slot(sl) => {
                            blk.insts.insert(0, Inst::IBinI(IOp::Add, it, rj, bj, v));
                            blk.insts.insert(0, Inst::SlotAddr(bj, sl));
                            blk.pos.insert(0, p);
                            blk.pos.insert(0, p);
                        }
                        Base::Reg(_) => {
                            blk.insts.insert(0, Inst::IBinI(IOp::Add, it, rj, bj, v));
                            blk.pos.insert(0, p);
                        }
                    }
                }
                debug_assert_eq!(f.blocks.len() as u32, base + j * n + q as u32);
                f.blocks.push(blk);
            }
            v = extend_const(v.wrapping_add(step), it);
        }
        // the loop's entries enter the first copy
        let first_h = base + pos_of(h) as u32;
        for &p in &outside {
            retarget(&mut f.blocks[p as usize].term, |s| if s == h { first_h } else { s });
        }
        for &b in &idx {
            touched[b as usize] = true;
        }
        changed = true;
    }
    changed
}

/// What a counter's start and end are relative to: nothing (constants), a stack slot's address, or
/// a register (a parameter or a definition in front of the loop).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Base {
    Const,
    Slot(u32),
    Reg(VReg),
}

/// `v` as a base plus a constant, through single definitions (constants, slot addresses, moves,
/// adds and subtracts of constants); any other single definition or parameter is its own base.
fn affine(def1: &std::collections::HashMap<u32, (u32, Inst)>, ndef: &[u32], v: VReg) -> Option<(Base, i64)> {
    let (mut v, mut off) = (v, 0i64);
    for _ in 0..32 {
        if ndef.get(v.0 as usize).copied() != Some(1) {
            return None;
        }
        match def1.get(&v.0).map(|x| &x.1) {
            Some(Inst::Iconst(_, k)) => return Some((Base::Const, off.wrapping_add(*k))),
            Some(Inst::SlotAddr(_, sl)) => return Some((Base::Slot(*sl), off)),
            Some(Inst::Mov(_, a)) => v = *a,
            Some(Inst::IBinI(IOp::Add, it, _, a, k)) if it.bits == 64 => {
                off = off.wrapping_add(*k);
                v = *a;
            }
            Some(Inst::IBinI(IOp::Sub, it, _, a, k)) if it.bits == 64 => {
                off = off.wrapping_sub(*k);
                v = *a;
            }
            _ => return Some((Base::Reg(v), off)),
        }
    }
    None
}

/// Whether the loop indexes, with its counter `c`, a stack slot that every other access reaches at
/// a constant offset: a local array (`let d = [dir.x, dir.y, dir.z]; .. d[a] ..`) whose address
/// goes nowhere but load and store bases (directly, plus constants) and the counter-indexed
/// addresses of the loop, which in turn are only load and store bases. In the copies the index is
/// a constant, every access has a fixed offset, and the slot's values forward to registers.
fn counter_indexes_local_values(f: &Func, members: &[u32], c: VReg, tr: bool) -> bool {
    const NONE: u32 = u32::MAX;
    let nv = f.vregs.len();
    // definitions counted now (the pass's counts predate copies made earlier in this pass)
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
    }
    let inloop = |b: usize| members.contains(&(b as u32));
    // values computed from the counter inside the loop (not through memory)
    let mut ctr = vec![false; nv];
    ctr[c.0 as usize] = true;
    loop {
        let mut grew = false;
        for &b in members {
            for i in &f.blocks[b as usize].insts {
                let d = match *i {
                    Inst::Mov(d, a) | Inst::IBinI(_, _, d, a, _) | Inst::Conv(Conv::IntToInt(_), d, a) if ctr[a.0 as usize] => d,
                    _ => continue,
                };
                if !ctr[d.0 as usize] && d != c {
                    ctr[d.0 as usize] = true;
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    // the slot a single-definition vreg points into at a constant offset (SlotAddr, moves, + k)
    let mut root = vec![NONE; nv];
    loop {
        let mut grew = false;
        for b in &f.blocks {
            for i in &b.insts {
                let (d, r) = match *i {
                    Inst::SlotAddr(d, sl) => (d, sl),
                    Inst::Mov(d, a) | Inst::IBinI(IOp::Add | IOp::Sub, _, d, a, _) if root[a.0 as usize] != NONE => (d, root[a.0 as usize]),
                    _ => continue,
                };
                if ndef[d.0 as usize] == 1 && root[d.0 as usize] == NONE {
                    root[d.0 as usize] = r;
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    // addresses `slot + counter value` made in the loop (single definitions)
    let mut indexed = vec![NONE; nv];
    for &b in members {
        for i in &f.blocks[b as usize].insts {
            if let Inst::IBin(IOp::Add, _, d, a, x) = *i {
                let r = if root[a.0 as usize] != NONE && ctr[x.0 as usize] {
                    root[a.0 as usize]
                } else if root[x.0 as usize] != NONE && ctr[a.0 as usize] {
                    root[x.0 as usize]
                } else {
                    continue;
                };
                if ndef[d.0 as usize] == 1 {
                    indexed[d.0 as usize] = r;
                }
            }
        }
    }
    if indexed.iter().all(|r| *r == NONE) {
        if tr {
            eprintln!("  local-values: counter {:?}: no slot + counter address", c);
        }
        return false;
    }
    // each slot's address may only be a load / store base, the source of a fixed-offset address,
    // or (in the loop) the base of an indexed address that is itself only a load / store base
    let ns = f.slots.len();
    let mut bad = vec![false; ns];
    let mut uses = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            let mut check = |u: VReg, ok: bool| {
                let r = if root[u.0 as usize] != NONE { root[u.0 as usize] } else { indexed[u.0 as usize] };
                if r != NONE && !ok {
                    if tr && !bad[r as usize] {
                        eprintln!("  local-values: slot {} escapes at b{}: {:?}", r, bi, i);
                    }
                    bad[r as usize] = true;
                }
            };
            match *i {
                Inst::Load(_, _, base, _) => check(base, true),
                Inst::Store(_, base, _, src) => {
                    check(base, true);
                    check(src, false);
                }
                Inst::Mov(_, a) | Inst::IBinI(IOp::Add | IOp::Sub, _, _, a, _) => check(a, indexed[a.0 as usize] == NONE),
                Inst::IBin(IOp::Add, _, d, a, x) if indexed[d.0 as usize] != NONE && inloop(bi) => {
                    check(a, true);
                    check(x, true);
                }
                _ => for_uses(i, |u| check(u, false)),
            }
        }
        uses.clear();
        relax_backend::regalloc::term_uses(&b.term, &mut uses);
        for u in &uses {
            let r = if root[u.0 as usize] != NONE { root[u.0 as usize] } else { indexed[u.0 as usize] };
            if r != NONE {
                bad[r as usize] = true;
            }
        }
    }
    indexed.iter().any(|r| *r != NONE && !bad[*r as usize])
}

/// Whether the loop reads, at `slot + (value of the counter c)`, a stack slot that holds addresses of
/// other slots (stored there, or copied in from such a slot): an array of borrows (`[(&mut e.a, ..),
/// (..)]`) walked by its IntoIter. The copies make those reads fixed-offset loads that forward the
/// stored addresses, so the borrowed slots stop escaping and their copies can join (SROA's bonus in
/// LLVM's full-unroll analysis). A counter indexing memory behind a pointer, or a slot of plain
/// values (box3d scatter_bodies' `simd_body.v.x.get(lane)`), gains only an index add per access.
fn counter_indexes_fixed_memory(f: &Func, members: &[u32], c: VReg) -> bool {
    const NONE: u32 = u32::MAX;
    let nv = f.vregs.len();
    // definitions counted now: the pass's own counts predate the copies of loops it already
    // unrolled in this function (their fresh vregs: an out-of-range index that panicked the O3
    // build and dropped it -- piano_render's render_chunk stayed below O3, +7.6%)
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
    }
    // the slot a single-definition vreg points into (its address plus constant offsets)
    let mut root = vec![NONE; nv];
    loop {
        let mut grew = false;
        for b in &f.blocks {
            for i in &b.insts {
                let (d, r) = match *i {
                    Inst::SlotAddr(d, s) => (d, s),
                    Inst::Mov(d, a) | Inst::IBinI(IOp::Add | IOp::Sub, _, d, a, _) if root[a.0 as usize] != NONE => (d, root[a.0 as usize]),
                    _ => continue,
                };
                if ndef[d.0 as usize] == 1 && root[d.0 as usize] == NONE {
                    root[d.0 as usize] = r;
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    // slots holding slot addresses
    let mut holds = vec![false; f.slots.len()];
    loop {
        let mut grew = false;
        for b in &f.blocks {
            for i in &b.insts {
                let (to, yes) = match *i {
                    Inst::Store(_, base, _, src) => (root[base.0 as usize], root[src.0 as usize] != NONE),
                    Inst::Copy(dst, src, _) => (root[dst.0 as usize], root[src.0 as usize] != NONE && holds[root[src.0 as usize] as usize]),
                    _ => continue,
                };
                if yes && to != NONE && !holds[to as usize] {
                    holds[to as usize] = true;
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    // values computed from the counter inside the loop (not through memory)
    let mut ctr = vec![false; nv];
    ctr[c.0 as usize] = true;
    loop {
        let mut grew = false;
        for &b in members {
            for i in &f.blocks[b as usize].insts {
                let d = match *i {
                    Inst::Mov(d, a) | Inst::IBinI(_, _, d, a, _) | Inst::Conv(Conv::IntToInt(_), d, a) | Inst::INeg(_, d, a) if ctr[a.0 as usize] => d,
                    Inst::IBin(_, _, d, a, x) if ctr[a.0 as usize] || ctr[x.0 as usize] => d,
                    _ => continue,
                };
                if !ctr[d.0 as usize] {
                    ctr[d.0 as usize] = true;
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    let held = |v: VReg| root[v.0 as usize] != NONE && holds[root[v.0 as usize] as usize];
    members.iter().any(|&b| {
        f.blocks[b as usize].insts.iter().any(|i| match *i {
            Inst::IBin(IOp::Add, _, _, a, x) => (held(a) && ctr[x.0 as usize]) || (ctr[a.0 as usize] && held(x)),
            _ => false,
        })
    })
}

/// `c = conv(conv(c) + s)` ending in `Mov(c, x)` at `k` of block `b`: the counter step of a
/// narrow integer as lowering writes it (each link single-defined in `b` before `k`). Returns the
/// final conversion's type (the counter's representation) and the step.
fn narrow_step(b: &Block, k: usize, c: VReg, x: VReg, ndef: &[u32]) -> Option<(IntTy, i64)> {
    let def_of = |v: VReg| -> Option<&Inst> {
        if ndef[v.0 as usize] != 1 {
            return None;
        }
        b.insts[..k].iter().rev().find(|i| {
            let mut d = false;
            for_defs(i, |r| d |= r == v);
            d
        })
    };
    // optional final normalisation, then the add
    let (it, y) = match def_of(x) {
        Some(&Inst::Conv(Conv::IntToInt(it), _, y)) => (Some(it), y),
        _ => (None, x),
    };
    let Some(&Inst::IBinI(IOp::Add, add_ty, _, z, s)) = def_of(y) else { return None };
    // the add reads the counter: directly, through a copy, or through a conversion
    let src = if z == c {
        z
    } else {
        match def_of(z) {
            Some(&Inst::Conv(Conv::IntToInt(_), _, w)) | Some(&Inst::Mov(_, w)) if w == c => w,
            _ => return None,
        }
    };
    let it = it.unwrap_or(add_ty);
    (src == c && (it.bits <= 32 || it.bits == 64)).then_some((it, s))
}

/// Vregs defined once (`ndef`), inside the loop (`m`), whose every use is inside the loop and
/// dominated by the definition (a later instruction of its block, or a block it dominates): the
/// value of one iteration, never read by another or after the loop. The counter `c` is excluded.
fn iteration_locals(f: &Func, members: &[u32], m: &[bool], ndef: &[u32], c: VReg) -> Vec<u32> {
    let idom = relax_backend::dom::idoms(f);
    let dominates = |a: u32, b: u32| -> bool {
        let mut x = b;
        for _ in 0..100_000 {
            if x == a {
                return true;
            }
            let p = idom[x as usize];
            if p == u32::MAX || p == x {
                return false;
            }
            x = p;
        }
        false
    };
    let nv = f.vregs.len();
    const NONE: u32 = u32::MAX;
    // definition site in the loop: (block, inst)
    let mut def_at = vec![(NONE, 0u32); nv];
    for &b in members {
        for (k, i) in f.blocks[b as usize].insts.iter().enumerate() {
            for_defs(i, |d| {
                if ndef[d.0 as usize] == 1 && d != c {
                    def_at[d.0 as usize] = (b, k as u32);
                }
            });
        }
    }
    let mut ok: Vec<bool> = def_at.iter().map(|d| d.0 != NONE).collect();
    for (bi, b) in f.blocks.iter().enumerate() {
        let inside = m.get(bi).copied().unwrap_or(false);
        let mut check = |u: VReg, at: u32| {
            let (db, dk) = def_at[u.0 as usize];
            if db == NONE {
                return;
            }
            let fine = inside && if db == bi as u32 { dk < at } else { dominates(db, bi as u32) };
            if !fine {
                ok[u.0 as usize] = false;
            }
        };
        for (k, i) in b.insts.iter().enumerate() {
            for_uses(i, |u| check(u, k as u32));
        }
        let mut tu = Vec::new();
        relax_backend::regalloc::term_uses(&b.term, &mut tu);
        for u in tu {
            check(u, u32::MAX);
        }
    }
    (0..nv as u32).filter(|v| ok[*v as usize]).collect()
}

/// Empties the blocks the entry no longer reaches (a fully unrolled loop's original body, which
/// only its last copy's folded back edge entered): passes that scan every block (slot
/// promotion, SROA) would otherwise still see their accesses. Returns whether any block emptied.
pub fn clear_unreachable(f: &mut Func) -> bool {
    let reach = relax_backend::regalloc::reachable(f);
    let mut any = false;
    for (bi, b) in f.blocks.iter_mut().enumerate() {
        if !reach[bi] && (!b.insts.is_empty() || !matches!(b.term, Term::Unreachable)) {
            b.insts.clear();
            b.pos.clear();
            b.term = Term::Unreachable;
            any = true;
        }
    }
    any
}

static MEMSET: std::sync::OnceLock<Sym> = std::sync::OnceLock::new();

/// The host memset (the runtime registers it once): zero_fill_loops calls it.
pub fn set_memset(memset: Sym) {
    let _ = MEMSET.set(memset);
}

fn zerofill_trace() -> Option<&'static str> {
    static VALUE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    VALUE.get_or_init(|| {
        let _k = relax_session::phase::kept();
        std::env::var("RELAX_ZEROFILL_TRACE").ok()
    }).as_deref()
}

fn zerofill_dump_dir() -> Option<&'static str> {
    static VALUE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    VALUE.get_or_init(|| {
        let _k = relax_session::phase::kept();
        std::env::var("RELAX_DUMP_DIR").ok()
    }).as_deref()
}

/// Counted loops that only store zeros over each element of an array become one memset (LLVM
/// LoopIdiomRecognize): a header `c = i < N; branch c -> B, X` (N a constant) and one body block
/// `B: off = i * S; p = a + off; stores of zero at p + k...; i += 1; jump H` whose stores cover
/// bytes 0..S of every element (a loop-invariant base `a`, `i` set to 0 outside the loop and read
/// nowhere else, nothing else in the body but constants it reads) is `memset(a, 0, N * S)` in the
/// header, which then jumps to X. `[T::default(); 64]` of an all-zero T (box3d's mover plane
/// buffer, 1792 bytes per collide_mover call) ran 64 iterations of 7 instructions. Fills under 128
/// bytes stay loops (unrolling and store merging make them a few wide stores). RELAX_OPT=-zerofill
/// off. Returns whether anything changed; the body is left unreachable (clear_unreachable).
pub fn zero_fill_loops(options: &relax_session::Options, f: &mut Func) -> bool {
    let Some(ms) = MEMSET.get().copied() else { return false };
    if relax_backend::stats::opt_off(options, "-zerofill") || f.blocks.len() < 3 {
        return false;
    }
    // a cheap screen: a zero vector / float / int constant stored in some loop body
    if !f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::Store(..) | Inst::VStore(..))) && matches!(b.term, Term::Jump(_))) {
        return false;
    }
    // RELAX_ZEROFILL_TRACE=<fn name part>: the loops looked at, appended to
    // $RELAX_DUMP_DIR/zerofill.txt (stderr of compile threads is not kept)
    let tr = zerofill_trace().is_some_and(|w| f.name.contains(w));
    let mut log = String::new();
    let reach = relax_backend::regalloc::reachable(f);
    let preds = relax_backend::regalloc::preds_of(f, &reach);
    let loops = relax_backend::regalloc::natural_loops(f, &reach, &preds, false);
    if loops.is_empty() {
        return false;
    }
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut nuse = vec![0u32; nv];
    // the block of each definition (the last one seen)
    let mut def_blk = vec![u32::MAX; nv];
    let mut kconst: Vec<Option<i64>> = vec![None; nv];
    let mut zero = vec![false; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            for_defs(i, |d| {
                ndef[d.0 as usize] += 1;
                def_blk[d.0 as usize] = bi as u32;
            });
            for_uses(i, |u| nuse[u.0 as usize] += 1);
            match *i {
                Inst::Iconst(d, k) => {
                    kconst[d.0 as usize] = Some(k);
                    zero[d.0 as usize] = k == 0;
                }
                Inst::Fconst(d, 0, _) | Inst::VConst(d, 0) => zero[d.0 as usize] = true,
                _ => {}
            }
        }
        match &b.term {
            Term::Branch(c, _, _) | Term::Switch(c, _) => nuse[c.0 as usize] += 1,
            Term::Ret(vs) => vs.iter().for_each(|v| nuse[v.0 as usize] += 1),
            _ => {}
        }
    }
    let single_k = |v: VReg| if ndef[v.0 as usize] == 1 { kconst[v.0 as usize] } else { None };
    let mut changed = false;
    'next: for (h, m) in &loops {
        let h = *h as usize;
        let members: Vec<usize> = (0..m.len()).filter(|b| m[*b]).collect();
        if tr {
            log.push_str(&format!("zero-fill {}: loop b{} blocks {:?}\n", f.name, h, members));
            for &bb in &members {
                log.push_str(&format!("  b{}: {:?} -> {:?}\n", bb, f.blocks[bb].insts, f.blocks[bb].term));
            }
        }
        if members.len() != 2 {
            continue;
        }
        let body = if members[0] == h { members[1] } else { members[0] };
        let Term::Branch(c, t, x) = f.blocks[h].term else { continue };
        if t as usize != body || m[x as usize] || nuse[c.0 as usize] != 1 || !matches!(f.blocks[body].term, Term::Jump(j) if j as usize == h) {
            continue;
        }
        let (iv, n) = match f.blocks[h].insts.as_slice() {
            [Inst::ICmp(Cond::Lt, false, d, i, n)] if *d == c => match single_k(*n) {
                Some(n) => (*i, n),
                None => continue,
            },
            [Inst::ICmpI(Cond::Lt, false, d, i, n)] if *d == c => (*i, *n),
            _ => continue,
        };
        // the counter: 0 outside the loop, + 1 in the body, read only by the loop
        if ndef[iv.0 as usize] != 2 || n <= 0 {
            continue;
        }
        let outside: Vec<u32> = preds[h].iter().copied().filter(|p| !m[*p as usize]).collect();
        let [pre] = outside.as_slice() else { continue };
        if !f.blocks[*pre as usize].insts.iter().any(|i| matches!(*i, Inst::Iconst(d, 0) if d == iv)) {
            continue;
        }
        let mut iv_uses = 0u32;
        let mut step = false;
        let (mut off, mut addr, mut base, mut stride): (Option<VReg>, Option<VReg>, Option<VReg>, Option<i64>) = (None, None, None, None);
        // a slot's address made in the body (Alloc remat leaves it there): (vreg, slot)
        let mut slot_base: Option<(VReg, u32)> = None;
        let mut spans: Vec<(i64, i64)> = Vec::new();
        let in_body = |v: VReg| def_blk[v.0 as usize] == body as u32 && ndef[v.0 as usize] == 1;
        for i in &f.blocks[body].insts {
            match *i {
                Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::VConst(d, _) if ndef[d.0 as usize] == 1 => {}
                Inst::SlotAddr(d, sl) if slot_base.is_none() && in_body(d) && nuse[d.0 as usize] == 1 => slot_base = Some((d, sl)),
                Inst::IBinI(IOp::Add, it, d, a, 1) if d == iv && a == iv && it.bits == 64 && !step => {
                    step = true;
                    iv_uses += 1;
                }
                Inst::IBin(IOp::Add, it, d, a, one) if d == iv && a == iv && it.bits == 64 && single_k(one) == Some(1) && !step => {
                    step = true;
                    iv_uses += 1;
                }
                Inst::IBin(IOp::Mul, it, d, a, s) if a == iv && off.is_none() && it.bits == 64 && in_body(d) => match single_k(s) {
                    Some(s) => {
                        off = Some(d);
                        stride = Some(s);
                        iv_uses += 1;
                    }
                    None => continue 'next,
                },
                Inst::IBinI(IOp::Mul, it, d, a, s) if a == iv && off.is_none() && it.bits == 64 && in_body(d) => {
                    off = Some(d);
                    stride = Some(s);
                    iv_uses += 1;
                }
                Inst::IBin(IOp::Add, it, d, a, b) if addr.is_none() && it.bits == 64 && in_body(d) && off.is_some() && (b == off.unwrap() || a == off.unwrap()) => {
                    let a = if b == off.unwrap() { a } else { b };
                    // the base: defined outside the loop, or the body's slot address
                    if a == iv || (def_blk[a.0 as usize] != u32::MAX && m[def_blk[a.0 as usize] as usize] && slot_base.map(|s| s.0) != Some(a)) {
                        continue 'next;
                    }
                    addr = Some(d);
                    base = Some(a);
                }
                Inst::Store(mm, p, k, z) if Some(p) == addr && zero[z.0 as usize] && ndef[z.0 as usize] == 1 => {
                    let size = match mm {
                        Mem::Int(b, _) => b as i64,
                        Mem::F32 => 4,
                        Mem::F64 => 8,
                    };
                    spans.push((k as i64, k as i64 + size));
                }
                Inst::VStore(p, k, z) if Some(p) == addr && zero[z.0 as usize] && ndef[z.0 as usize] == 1 => spans.push((k as i64, k as i64 + 16)),
                _ => continue 'next,
            }
        }
        let (Some(off), Some(addr), Some(base), Some(stride)) = (off, addr, base, stride) else { continue };
        if !step || stride <= 0 || nuse[iv.0 as usize] != iv_uses + 1 {
            continue;
        }
        // the body's temporaries are read only in the body: off by addr, addr by the stores
        if nuse[off.0 as usize] != 1 || nuse[addr.0 as usize] as usize != spans.len() {
            continue;
        }
        let consts_local = f.blocks[body].insts.iter().all(|i| match *i {
            Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::VConst(d, _) => {
                let mut inner = 0u32;
                for j in &f.blocks[body].insts {
                    for_uses(j, |u| inner += (u == d) as u32);
                }
                inner == nuse[d.0 as usize]
            }
            _ => true,
        });
        if !consts_local {
            continue;
        }
        // the stores cover each element's bytes 0..stride exactly
        spans.sort();
        let mut covered = 0i64;
        for (lo, hi) in &spans {
            if *lo > covered || *hi > stride {
                continue 'next;
            }
            covered = covered.max(*hi);
        }
        if covered != stride {
            continue;
        }
        let Some(len) = n.checked_mul(stride) else { continue };
        if len < 128 {
            continue;
        }
        let tp = f.blocks[h].term_pos;
        let zr = f.vreg(Cls::I);
        let ln = f.vreg(Cls::I);
        let mut ins = Vec::new();
        let base = match slot_base {
            Some((sv, sl)) if sv == base => {
                let p = f.vreg(Cls::I);
                ins.push(Inst::SlotAddr(p, sl));
                p
            }
            _ => base,
        };
        ins.extend([Inst::Iconst(zr, 0), Inst::Iconst(ln, len), Inst::Call(Callee::Host(ms), vec![base, zr, ln], Vec::new())]);
        let hb = &mut f.blocks[h];
        hb.pos = vec![tp; ins.len()];
        hb.insts = ins;
        hb.term = Term::Jump(x);
        changed = true;
        if tr {
            log.push_str(&format!("  -> memset {} bytes\n", len));
        }
    }
    if tr {
        if let Some(d) = zerofill_dump_dir() {
            use std::io::Write;
            if let Ok(mut o) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{}/zerofill.txt", d)) {
                let _ = o.write_all(format!("zero-fill {}: {} loops\n{}", f.name, loops.len(), log).as_bytes());
            }
        }
    }
    changed
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&MEMSET, std::sync::OnceLock::new());
}
