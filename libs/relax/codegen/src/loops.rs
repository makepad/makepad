//! Loop transformations for hot code (run by O3 after the optimiser): unrolling of small
//! single-block counted loops.

use crate::regalloc::{for_defs, for_uses};
use crate::rir::*;

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
        let hb = Block { insts: hi.clone(), pos: vec![pos; hi.len()], term: Term::Branch(g1, h + 1, bi as u32), term_pos: pos };
        // H2: n - i >= K (unsigned; i < n, so no wrap)
        let (rem, g2, lim) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        // lim = n - K: U runs while i <= lim (n - i >= K here, so n - K does not overflow)
        let h2i = vec![Inst::IBin(IOp::Sub, U64_TY, rem, bvreg, iv), Inst::IBinI(IOp::Sub, U64_TY, lim, bvreg, k), Inst::ICmpI(Cond::Ge, false, g2, rem, k)];
        let h2 = Block { insts: h2i.clone(), pos: vec![pos; h2i.len()], term: Term::Branch(g2, h + 2, bi as u32), term_pos: pos };
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
        let ub = Block { insts: ui, pos: up, term: Term::Branch(g4, h + 2, h + 3), term_pos: pos };
        // HL: the rest (< K iterations) in B, or none
        let g3 = f.vreg(Cls::I);
        let hl = vec![Inst::ICmp(if lt { Cond::Lt } else { Cond::Ne }, signed, g3, iv, bvreg)];
        let hlb = Block { insts: hl, pos: vec![pos], term: Term::Branch(g3, bi as u32, exit), term_pos: pos };
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

const U64_TY: IntTy = IntTy { bits: 64, signed: false };

fn vectorize_on(options: &relax_session::Options) -> bool {
    !options.no_vectorize
}

/// Widen straight-line byte maps over pointer/count iterators. The scalar loop remains
/// the tail and fallback, including its exact short-circuit iterator exhaustion behavior.
/// A vector iteration is admitted only when every scalar guard succeeds sixteen times;
/// runtime distance checks exclude dependencies within a vector (equal pointers are OK).
fn vectorize_byte_maps(f: &mut Func) -> bool {
    if !f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::Store(Mem::Int(1, false), ..)))) {
        return false;
    }
    let reach = crate::regalloc::reachable(f);
    let mut preds = vec![Vec::new(); f.blocks.len()];
    for (bi, b) in f.blocks.iter().enumerate() {
        if reach[bi] {
            for s in b.term.succs() {
                preds[s as usize].push(bi as u32);
            }
        }
    }
    let loops = crate::regalloc::natural_loops(f, &reach, &preds, false);
    let mut changed = false;
    let mut transformed = vec![false; f.blocks.len()];
    'candidate: for (head, members) in loops {
        if members.iter().zip(&transformed).any(|(a, b)| *a && *b) { continue; }
        // Only a linear cycle: guards exit the loop, and the sole back edge follows
        // the store. No interior entries or nested control flow can be bypassed.
        let mut chain = Vec::new();
        let mut bi = head;
        loop {
            if chain.contains(&bi) {
                if bi != head { continue 'candidate; }
                break;
            }
            if bi != head && preds[bi as usize].iter().any(|p| !members[*p as usize]) {
                continue 'candidate;
            }
            chain.push(bi);
            let inside: Vec<_> = f.blocks[bi as usize].term.succs().filter(|s| members.get(*s as usize).copied().unwrap_or(false)).collect();
            if inside.len() != 1 || !matches!(f.blocks[bi as usize].term, Term::Jump(_) | Term::Branch(..)) {
                continue 'candidate;
            }
            bi = inside[0];
        }
        if chain.len() != members.iter().filter(|x| **x).count() { continue; }
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
        if definitions.iter().any(|n| *n > 1) { continue; }
        let roots: Vec<_> = (0..nv).filter(|v| inputs[*v]).map(|v| VReg(v as u32)).collect();
        // Symbolically execute pointer arithmetic. Every address must be the current
        // element of a +1 recurrence; every length guard a -1 recurrence.
        let mut addr = vec![None; nv];
        for &v in &roots { addr[v.0 as usize] = Some((v, 0i64)); }
        let mut guard = Vec::new();
        let mut loads = Vec::new();
        let mut ops = Vec::new();
        let mut store = None;
        let mut bytes = vec![false; nv];
        for &b in &chain {
            let block = &f.blocks[b as usize];
            if let Term::Branch(c, ..) = block.term {
                if !matches!(block.insts.last(), Some(Inst::ICmpI(Cond::Eq | Cond::Ne, false, d, _, 0)) if *d == c) {
                    continue 'candidate;
                }
            }
            for i in &block.insts {
                match *i {
                    Inst::Mov(d, s) if addr[s.0 as usize].is_some() => addr[d.0 as usize] = addr[s.0 as usize],
                    Inst::IBinI(op @ (IOp::Add | IOp::Sub), it, d, s, k) if it == U64_TY => {
                        let Some((r, off)) = addr[s.0 as usize] else { continue 'candidate };
                        let Some(off) = (if op == IOp::Add { off.checked_add(k) } else { off.checked_sub(k) }) else { continue 'candidate };
                        addr[d.0 as usize] = Some((r, off));
                    }
                    Inst::ICmpI(c @ (Cond::Eq | Cond::Ne), false, d, s, 0) => {
                        let Term::Branch(cv, t, e) = block.term else { continue 'candidate };
                        if cv != d || (if c == Cond::Ne { !members[t as usize] || members[e as usize] } else { members[t as usize] || !members[e as usize] }) {
                            continue 'candidate;
                        }
                        let Some((r, 0)) = addr[s.0 as usize] else { continue 'candidate };
                        guard.push(r);
                    }
                    Inst::Load(Mem::Int(1, false), d, p, off) if store.is_none() => {
                        let Some((r, o)) = addr[p.0 as usize] else { continue 'candidate };
                        if o.checked_add(off as i64) != Some(0) { continue 'candidate; }
                        loads.push((d, r));
                        bytes[d.0 as usize] = true;
                    }
                    Inst::IBin(op @ (IOp::Add | IOp::Sub | IOp::And | IOp::Or | IOp::Xor), it, d, a, b) if it.bits == 8 && bytes[a.0 as usize] && bytes[b.0 as usize] && store.is_none() => {
                        ops.push((op, d, a, b));
                        bytes[d.0 as usize] = true;
                    }
                    Inst::Store(Mem::Int(1, false), p, off, s) if store.is_none() && bytes[s.0 as usize] => {
                        let Some((r, o)) = addr[p.0 as usize] else { continue 'candidate };
                        if o.checked_add(off as i64) != Some(0) { continue 'candidate; }
                        store = Some((r, s));
                    }
                    _ => continue 'candidate,
                }
            }
            if store.is_some() && !matches!(block.term, Term::Jump(t) if t == head) { continue 'candidate; }
        }
        let Some((dst, value)) = store else { continue };
        if loads.is_empty() { continue; }
        let mut pointers: Vec<_> = loads.iter().map(|x| x.1).collect();
        pointers.push(dst);
        pointers.sort_unstable_by_key(|v| v.0);
        pointers.dedup();
        let mut counts = Vec::new();
        for &v in &roots {
            match addr[v.0 as usize] {
                Some((r, 1)) if r == v && pointers.contains(&v) => {}
                Some((r, -1)) if r == v && guard.contains(&v) && !pointers.contains(&v) => counts.push(v),
                _ => continue 'candidate,
            }
        }
        if counts.is_empty() || guard.iter().any(|v| !roots.contains(v)) { continue; }
        // The scalar temporaries are not produced by the widened body. All observable
        // loop-carried values must be precisely the pointer and length recurrences.
        for (b, block) in f.blocks.iter().enumerate() {
            if b < members.len() && members[b] { continue; }
            let mut bad = false;
            for i in &block.insts {
                for_uses(i, |v| bad |= (v.0 as usize) < nv && defined[v.0 as usize] && !inputs[v.0 as usize]);
            }
            let mut uses = Vec::new();
            crate::regalloc::term_uses(&block.term, &mut uses);
            bad |= uses.iter().any(|v| (v.0 as usize) < nv && defined[v.0 as usize] && !inputs[v.0 as usize]);
            if bad { continue 'candidate; }
        }
        let entry = f.blocks.len() as u32;
        let body = entry + 1;
        let pos = f.blocks[head as usize].term_pos;
        let trip = f.vreg(Cls::I);
        let mut init = vec![Inst::Mov(trip, counts[0])];
        for &c in &counts[1..] {
            let less = f.vreg(Cls::I);
            init.push(Inst::ICmp(Cond::Lt, false, less, c, trip));
            init.push(Inst::Sel(trip, less, c, trip));
        }
        // A pointer guard may stop at zero. Cap the fast run before any pointer
        // wraps, even for RIR not originating from Rust's in-bounds pointer add.
        for &p in &pointers {
            let (until_zero, less) = (f.vreg(Cls::I), f.vreg(Cls::I));
            init.push(Inst::INeg(U64_TY, until_zero, p));
            init.push(Inst::ICmp(Cond::Lt, false, less, until_zero, trip));
            init.push(Inst::Sel(trip, less, until_zero, trip));
        }
        let ok = f.vreg(Cls::I);
        init.push(Inst::ICmpI(Cond::Ge, false, ok, trip, 16));
        for &src in &pointers {
            if src == dst { continue; }
            let (distance, far, same, safe) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
            // Only forward dependencies matter: loads precede the store and a
            // destination behind the source cannot overwrite a later input.
            init.push(Inst::IBin(IOp::Sub, U64_TY, distance, dst, src));
            init.push(Inst::ICmpI(Cond::Ge, false, far, distance, 16));
            init.push(Inst::ICmp(Cond::Eq, false, same, dst, src));
            init.push(Inst::IBin(IOp::Or, U64_TY, safe, far, same));
            init.push(Inst::IBin(IOp::And, U64_TY, ok, ok, safe));
        }
        let mut wide = Vec::new();
        let mut vectors = vec![VReg(u32::MAX); nv];
        for &(d, p) in &loads {
            let v = f.vreg(Cls::V128);
            vectors[d.0 as usize] = v;
            wide.push(Inst::VLoad(v, p, 0));
        }
        for &(op, d, a, b) in &ops {
            let v = f.vreg(Cls::V128);
            let op = match op { IOp::Add => VX::Add(VW::B), IOp::Sub => VX::Sub(VW::B), IOp::And => VX::And, IOp::Or => VX::Or, IOp::Xor => VX::Xor, _ => unreachable!() };
            wide.push(Inst::VX(op, v, vectors[a.0 as usize], vectors[b.0 as usize]));
            vectors[d.0 as usize] = v;
        }
        wide.push(Inst::VStore(dst, 0, vectors[value.0 as usize]));
        for &p in &pointers { wide.push(Inst::IBinI(IOp::Add, U64_TY, p, p, 16)); }
        for &c in &counts { wide.push(Inst::IBinI(IOp::Sub, U64_TY, c, c, 16)); }
        wide.push(Inst::IBinI(IOp::Sub, U64_TY, trip, trip, 16));
        let again = f.vreg(Cls::I);
        wide.push(Inst::ICmpI(Cond::Ge, false, again, trip, 16));
        for (bi, block) in f.blocks.iter_mut().enumerate() {
            if bi >= members.len() || !members[bi] {
                block.term.map_succs(|s| if s == head { entry } else { s });
            }
        }
        f.blocks.push(Block { pos: vec![pos; init.len()], insts: init, term: Term::Branch(ok, body, head), term_pos: pos });
        f.blocks.push(Block { pos: vec![pos; wide.len()], insts: wide, term: Term::Branch(again, body, head), term_pos: pos });
        for (done, member) in transformed.iter_mut().zip(&members) { *done |= *member; }
        changed = true;
    }
    changed
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
    let mut changed = vectorize_byte_maps(f);
    let nb0 = f.blocks.len();
    let mut ndef = vec![0u32; f.vregs.len()];
    let mut nuse = vec![0u32; f.vregs.len()];
    for b in &f.blocks {
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
        let it = match b.insts[idefs[0]] {
            Inst::IBinI(IOp::Add, it, d, a, 1) if d == iv && a == iv => it,
            Inst::Mov(d, t) if d == iv => match b.insts[..idefs[0]].iter().enumerate().rev().find(|(_, i)| {
                let mut d2 = false;
                for_defs(i, |x| d2 |= x == t);
                d2
            }) {
                Some((ti, Inst::IBinI(IOp::Add, it, _, a, 1))) if *a == iv && ndef[t.0 as usize] == 1 && nuse[t.0 as usize] == 1 => {
                    used[ti] = true;
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
        let mut load: Option<(usize, VReg, VReg, u8)> = None; // (index, x, base, size bytes)
        for (k, i) in b.insts.iter().enumerate() {
            match *i {
                Inst::LoadX(Mem::Int(sz @ (4 | 8), sg), x, base, ix, sh, 0) if ix == iv && (1u32 << sh) == sz as u32 && !(sz == 4 && sg) && !defined_in(base) => {
                    if load.is_some() {
                        load = None;
                        break;
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
                    let ok = matches!(b.insts[sk], Inst::IBinI(IOp::Shl, it2, _, a2, k2) if a2 == iv && k2 == sh && it2.bits == 64)
                        || matches!(b.insts[sk], Inst::IBinI(IOp::Mul, it2, _, a2, k2) if a2 == iv && k2 == sz as i64 && it2.bits == 64);
                    if !ok {
                        continue;
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
        let hb = Block { insts: hi.clone(), pos: vec![pos; hi.len()], term: Term::Branch(g1, h + 1, bi as u32), term_pos: pos };
        // H2
        let (rem, g2, va, vb, lim) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::I));
        // lim = n - K: the vector loop runs while i <= lim (n - i >= K here, so no wrap)
        let h2i = vec![Inst::VConst(va, 0), Inst::VConst(vb, 0), Inst::IBin(IOp::Sub, U64_TY, rem, bvreg, iv), Inst::IBinI(IOp::Sub, U64_TY, lim, bvreg, kk), Inst::ICmpI(Cond::Ge, false, g2, rem, kk)];
        let h2 = Block { insts: h2i.clone(), pos: vec![pos; h2i.len()], term: Term::Branch(g2, h + 2, bi as u32), term_pos: pos };
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
        let vb_ = Block { insts: vi.clone(), pos: vec![pos; vi.len()], term: Term::Branch(g3, h + 2, h + 3), term_pos: pos };
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
        let veb = Block { insts: ve.clone(), pos: vec![pos; ve.len()], term: Term::Branch(g4, bi as u32, exit), term_pos: pos };
        for pb in 0..nb0 {
            if pb != bi {
                f.blocks[pb].term.map_succs(|s2| if s2 as usize == bi { h } else { s2 });
            }
        }
        f.blocks.push(hb);
        f.blocks.push(h2);
        f.blocks.push(vb_);
        f.blocks.push(veb);
        changed = true;
    }
    changed
}

fn merge_stores_on(options: &relax_session::Options) -> bool {
    !options.no_store_merge
}

/// Consecutive narrow stores of one value's bytes become one wide store: in a block, byte stores
/// `[p + off + j] = x >> 8j` (j = 0.., the shift optionally under a truncating Conv), with no other
/// memory access between them, become `[p + off] = x` as 8, 4 or 2 bytes (little-endian: byte j
/// of the wide store is bits 8j.. of x). Frontends split `[u8; N]` copies and byte-wise writes
/// this way (inflate's match copy: 8 byte stores of one u64 load, whose byte temporaries also
/// spilled). Shifts and truncations left without uses go. Returns whether anything changed.
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
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        let n = f.blocks[bi].insts.len();
        if n < 2 || !f.blocks[bi].insts.iter().any(|i| matches!(i, Inst::Store(Mem::Int(1, _), ..))) {
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
        for (k, i) in insts.iter().enumerate() {
            let touches = matches!(i, Inst::Load(..) | Inst::LoadX(..) | Inst::VLoad(..) | Inst::VGather(..)) || crate::opt::is_mem_effect_pub(i);
            if let Inst::Store(Mem::Int(1, _), p, off, v) = *i {
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
            } else {
                // the base or x redefined in between: the group ends
                let mut redef = false;
                if let Some((_, bp, _, cx)) = &cur {
                    for_defs(i, |d| redef |= d == *bp || d == *cx);
                }
                if redef {
                    groups.push(cur.take().unwrap());
                }
            }
        }
        if let Some(g) = cur.take() {
            groups.push(g);
        }
        let mut remove: Vec<usize> = Vec::new();
        let mut replace: Vec<(usize, Inst)> = Vec::new();
        for (ks, p, st, x) in groups {
            let w = if ks.len() >= 8 { 8 } else if ks.len() >= 4 { 4 } else if ks.len() >= 2 { 2 } else { continue };
            let last = ks[w - 1];
            replace.push((last, Inst::Store(Mem::Int(w as u8, false), p, st, x)));
            remove.extend_from_slice(&ks[..w - 1]);
        }
        if replace.is_empty() {
            continue;
        }
        if std::env::var("MAKEPAD_RUN_MERGE_TRACE").is_ok() {
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

/// Full unrolling of loops with a small constant trip count (`for _ in 0..3`, deflate's hash-chain
/// probes): an innermost loop with one latch whose counter `c` is set once outside the loop by a
/// constant and stepped once in the latch by `c = c + s` (c read nowhere outside the loop, no
/// calls in the loop) and whose latch decides "again" by `c' cmp k` with constants, runs T <= 4
/// times; it gets T copies in front of it, copy j reading c as the constant c0 + j*s (a fresh
/// vreg each, so constant folding sees single definitions). Copy j's back edge enters copy j+1;
/// the last copy's back edge enters the original loop with c restored (never taken: its compare
/// folds to the exit). Exits from inside the body stay as they are. Run `simplify` after it.
/// MAKEPAD_RUN_NO_FULL_UNROLL=1 off. Returns whether anything changed.
pub fn unroll_const_loops(options: &relax_session::Options, f: &mut Func) -> bool {
    if !full_unroll_on(options) || f.blocks.len() < 2 {
        return false;
    }
    let tr = std::env::var("MAKEPAD_RUN_UNROLL_TRACE").map_or(false, |w| f.name.ends_with(w.as_str()));
    let reach = crate::regalloc::reachable(f);
    let preds = crate::regalloc::preds_of(f, &reach);
    let loops = crate::regalloc::natural_loops(f, &reach, &preds, false);
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
    let mut changed = false;
    let mut budget = 800usize;
    // innermost first (natural_loops sorts by size); loops nested in a changed one are skipped
    let mut touched = vec![false; f.blocks.len()];
    'next: for (h, m) in &loops {
        let h = *h;
        let members: Vec<u32> = (0..m.len() as u32).filter(|b| m[*b as usize]).collect();
        if members.iter().any(|b| touched[*b as usize]) {
            continue;
        }
        // innermost: no other loop's header inside
        if loops.iter().any(|(h2, _)| *h2 != h && m[*h2 as usize]) {
            continue;
        }
        let size: usize = members.iter().map(|b| f.blocks[*b as usize].insts.len() + 1).sum();
        if tr {
            eprintln!("full-unroll {}: loop b{} {} blocks {} insts", f.name, h, members.len(), size);
        }
        // (the function's budget below bounds the total growth)
        if size > 160 {
            continue;
        }
        if members.iter().any(|b| f.blocks[*b as usize].insts.iter().any(|i| matches!(i, Inst::Call(..))) || matches!(f.blocks[*b as usize].term, Term::Invoke(..) | Term::Resume)) {
            continue;
        }
        let latches: Vec<u32> = members.iter().copied().filter(|b| f.blocks[*b as usize].term.succs().any(|s| s == h)).collect();
        // only loops that leave from the header or the latch: a body with early exits (a search
        // loop returning from inside) grows by every copy of its exit paths for little gain
        // (deflate's probe loop measured slower unrolled on x64)
        let early_exit = members.iter().any(|b| *b != h && !latches.contains(b) && f.blocks[*b as usize].term.succs().any(|s| !m.get(s as usize).copied().unwrap_or(false)));
        if early_exit {
            if tr {
                eprintln!("  early exits from the body");
            }
            continue;
        }
        if latches.len() != 1 {
            if tr {
                eprintln!("  {} latches", latches.len());
            }
            continue;
        }
        let l = latches[0];
        let outside: Vec<u32> = preds[h as usize].iter().copied().filter(|p| !m[*p as usize]).collect();
        if outside.is_empty() {
            continue;
        }
        // the counter step c = c + s: in the latch, or (with the test in the header) in a block
        // that dominates the latch -- an iterator's `start += 1` right after its test
        let lb = &f.blocks[l as usize];
        let mut found = None;
        // (block, k, type, counter, step, the step's value made at k is a constant in each copy)
        for &sb in std::iter::once(&l).chain(members.iter().filter(|b| **b != l)) {
            if found.is_some() {
                break;
            }
            for (k, i) in f.blocks[sb as usize].insts.iter().enumerate() {
                if let Inst::IBinI(IOp::Add, it, d, a, s) = *i {
                    if d == a && ndef[d.0 as usize] == 2 {
                        found = Some((sb, k, it, d, s, false));
                    }
                }
                // a narrow counter steps through a normalizing chain: `c = conv(conv(c) + s)`
                // (an i32 `for i in 0..8`), or a copy of `c + s` (a register-promoted field),
                // the last link a move into c
                if let Inst::Mov(d, x) = *i {
                    if ndef[d.0 as usize] == 2 {
                        if let Some((it, s)) = narrow_step(&f.blocks[sb as usize], k, d, x, &ndef) {
                            found = Some((sb, k, it, d, s, true));
                        }
                    }
                }
            }
        }
        let Some((sb, ak, it, c, step, step_const)) = found else {
            if tr {
                eprintln!("  no counter step c = c + s in latch b{}", l);
            }
            continue;
        };
        // a step outside the latch: it must dominate the latch, and blocks it dominates read the
        // stepped counter in the copies
        let idom = if sb != l { crate::opt::idoms(f) } else { Vec::new() };
        let dominated = |b: u32| -> bool {
            if sb == l {
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
        if sb != l && (sb == h || !dominated(l)) {
            if tr {
                eprintln!("  step in b{} does not dominate the latch b{}", sb, l);
            }
            continue;
        }
        if !f.vregs.get(c.0 as usize).map_or(false, |x| *x == Cls::I) {
            continue;
        }
        // the decision: in the latch after the step (`Branch(c' cmp k, h, exit)`: iteration j
        // runs again iff cmp(c0 + (j+1)s)), or in the header before anything redefines c
        // (`Branch(c cmp k, body, exit)`: the header's test of iteration j sees c0 + j*s)
        let find_cmp = |b: &Block, from: usize| -> Option<(Cond, bool, i64, VReg)> {
            let Term::Branch(cv, _, _) = b.term else { return None };
            let mut cmp = None;
            for i in &b.insts[from..] {
                if let Inst::ICmpI(cond, signed, d, x, k) = *i {
                    if d == cv && x == c {
                        cmp = Some((cond, signed, k, cv));
                        continue;
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
        let latch_cmp = if sb == l && matches!(lb.term, Term::Branch(_, a, b) if a == h || b == h) { find_cmp(lb, ak + 1) } else { None };
        let header_cmp = if latch_cmp.is_none() && h != l {
            let hb = &f.blocks[h as usize];
            if hb.insts.iter().any(|i| { let mut r = false; for_defs(i, |d| r |= d == c); r }) { None } else { find_cmp(hb, 0) }
        } else {
            None
        };
        let (cond, signed, k, in_latch) = match (latch_cmp, header_cmp) {
            (Some((a, b, k, _)), _) => (a, b, k, true),
            (None, Some((a, b, k, _))) => (a, b, k, false),
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
        // the one outside definition: a constant
        let mut c0 = None;
        for (bi, b) in f.blocks.iter().enumerate() {
            if m.get(bi).copied().unwrap_or(false) {
                continue;
            }
            for i in &b.insts {
                if let Inst::Iconst(d, v) = *i {
                    if d == c {
                        c0 = Some(v);
                    }
                }
            }
        }
        let Some(c0) = c0 else {
            if tr {
                eprintln!("  counter v{} has no constant outside definition", c.0);
            }
            continue;
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
            crate::regalloc::term_uses(&b.term, &mut tu);
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
        if size * t > budget {
            continue;
        }
        budget -= size * t;
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
        for j in 0..t as u32 {
            let rj = f.vreg(Cls::I);
            let rj2 = f.vreg(Cls::I);
            let mut fresh: std::collections::HashMap<u32, VReg> = std::collections::HashMap::new();
            for &lv in &local {
                let cls = f.vregs[lv as usize];
                fresh.insert(lv, f.vreg(cls));
            }
            for (q, &b) in idx.iter().enumerate() {
                let mut blk = f.blocks[b as usize].clone();
                let latch = b == l;
                // the block holding the step: after it, the copy's next counter; blocks the step
                // dominates: the next counter throughout
                let step_blk = b == sb;
                let past = b != sb && dominated(b);
                for (ii, ins) in blk.insts.iter_mut().enumerate() {
                    let after = past || (step_blk && ii > ak);
                    if step_blk && ii == ak {
                        // the normalizing chain's value is this copy's next counter: a constant
                        *ins = if step_const { Inst::Iconst(rj2, extend_const(v.wrapping_add(step), it)) } else { Inst::IBinI(IOp::Add, it, rj2, rj, step) };
                        continue;
                    }
                    ins.vregs_mut(|r| {
                        if *r == c {
                            *r = if after { rj2 } else { rj };
                        } else if let Some(x) = fresh.get(&r.0) {
                            *r = *x;
                        }
                    });
                }
                crate::regalloc::term_vregs_mut(&mut blk.term, |r| {
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
                    // the original loop continues from the counter this copy reached
                    let p = blk.term_pos;
                    blk.insts.push(Inst::Mov(c, rj2));
                    blk.pos.push(p);
                }
                if b == h {
                    let p = blk.pos.first().copied().unwrap_or(blk.term_pos);
                    blk.insts.insert(0, Inst::Iconst(rj, v));
                    blk.pos.insert(0, p);
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
    let idom = crate::opt::idoms(f);
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
        crate::regalloc::term_uses(&b.term, &mut tu);
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
    let reach = crate::regalloc::reachable(f);
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
