//! Merging of functions that differ only in constants (size-first module export).
//!
//! Monomorphic copies of one generic body (`Vec<T>::try_reserve` for every `T`, a hash table's
//! rehash per entry type, a derive per field type) are usually the same RIR except for a few
//! constants: element sizes, alignments, the addresses of the data they name (vtables, type
//! info), the functions whose addresses they take. `merge_similar` groups such functions by their
//! shape (every instruction, with those constants masked; call targets exact), and replaces each
//! group by ONE body that takes the differing constants as extra trailing parameters. Direct
//! callers pass the constants (`Iconst`/`Fconst`/`Addr`/`FnAddr` right before the call), so the
//! callers of different members become alike in turn and merge in the next round (a fixpoint, as
//! identical-code folding). Members whose own signature must stay (address taken, exported)
//! become a thunk that forwards to the merged body.
//!
//! Sound for any RIR: a parameter carries exactly the value the constant had; immediate
//! operands (`IBinI`, `ICmpI`) become their register forms. Self-recursive calls of a member
//! call the merged body with its own extra parameters.

use crate::rir::{Block, Callee, Cls, Func, Inst, Sym, Term, VReg};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};

/// A constant operand that differs between the members of a group.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Op {
    I(i64),
    F(u64, bool),
    Addr(Sym),
    Fn(u32),
}

#[derive(Default, Debug, Clone)]
pub struct Stats {
    pub rounds: usize,
    pub groups: usize,
    /// functions replaced by a merged body (callers redirected) or a thunk
    pub merged: usize,
    pub thunks: usize,
    /// extra parameters over all merged bodies
    pub params: usize,
}

/// Shape key: the function with its maskable constants zeroed, its self calls marked. Two
/// 64-bit hashes (collisions are checked again structurally before merging).
fn shape(id: u32, f: &Func) -> (u64, u64) {
    let mut h1 = std::collections::hash_map::DefaultHasher::new();
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    0x5eedu32.hash(&mut h2);
    let mut w = |s: &str| {
        s.hash(&mut h1);
        s.hash(&mut h2);
    };
    w(&format!("{:?}|{:?}|{:?}|{:?}", f.vregs, f.params, f.rets, f.sret));
    for s in &f.slots {
        w(&format!("s{},{},{},{},{}", s.size, s.align, s.arg, s.born, s.ret));
    }
    for b in &f.blocks {
        for i in &b.insts {
            w(&format!("{:?}", masked(id, i)));
        }
        w(&format!("{:?}", b.term));
    }
    (h1.finish(), h2.finish())
}

/// The instruction with its constant operand masked (and a self call's callee marked).
fn masked(id: u32, i: &Inst) -> Inst {
    match i {
        Inst::Iconst(d, _) => Inst::Iconst(*d, 0),
        Inst::Fconst(d, _, w) => Inst::Fconst(*d, 0, *w),
        Inst::IBinI(o, t, d, a, _) => Inst::IBinI(*o, *t, *d, *a, 0),
        Inst::ICmpI(c, s, d, a, _) => Inst::ICmpI(*c, *s, *d, *a, 0),
        Inst::Addr(d, _) => Inst::Addr(*d, Sym { id: 0, off: 0 }),
        Inst::FnAddr(d, _) => Inst::FnAddr(*d, 0),
        Inst::Call(Callee::Fn(t), a, r) if *t == id => Inst::Call(Callee::Fn(u32::MAX), a.clone(), r.clone()),
        i => i.clone(),
    }
}

/// The constant operand of a maskable instruction.
fn operand(i: &Inst) -> Option<Op> {
    Some(match i {
        Inst::Iconst(_, v) | Inst::IBinI(_, _, _, _, v) | Inst::ICmpI(_, _, _, _, v) => Op::I(*v),
        Inst::Fconst(_, v, w) => Op::F(*v, *w),
        Inst::Addr(_, s) => Op::Addr(*s),
        Inst::FnAddr(_, t) => Op::Fn(*t),
        _ => return None,
    })
}

/// Instructions that materialise `op` into a new vreg of `f` (the caller side of a parameter).
fn materialise(f: &mut Func, op: Op) -> (VReg, Inst) {
    match op {
        Op::I(v) => {
            let r = f.vreg(Cls::I);
            (r, Inst::Iconst(r, v))
        }
        Op::F(v, w) => {
            let r = f.vreg(if w { Cls::F64 } else { Cls::F32 });
            (r, Inst::Fconst(r, v, w))
        }
        Op::Addr(s) => {
            let r = f.vreg(Cls::I);
            (r, Inst::Addr(r, s))
        }
        Op::Fn(t) => {
            let r = f.vreg(Cls::I);
            (r, Inst::FnAddr(r, t))
        }
    }
}

fn mergeable(f: &Func) -> bool {
    f.cabi.is_none() && !f.blocks.is_empty()
}

/// Rough wasm bytes of a body (the profit test only).
fn est_bytes(f: &Func) -> usize {
    f.blocks.iter().map(|b| b.insts.len() * 3 + 2).sum::<usize>() + 4
}

/// Merges the functions of `funcs` that differ only in constants. `keep`: functions whose id and
/// signature must stay (their address is taken, they are exported): they become thunks.
/// `next_id`: the first free function id (merged bodies get new ids).
pub fn merge_similar(funcs: &mut BTreeMap<u32, Func>, keep: &HashSet<u32>, next_id: &mut u32, max_params: usize) -> Stats {
    let mut st = Stats::default();
    // thunks forward to a merged body: never members again (identical thunks of kept functions
    // would otherwise merge into new thunks forever)
    let mut thunks: HashSet<u32> = HashSet::new();
    // thunks and merged bodies are not merged again as members of a new group in the round that
    // made them; they take part from the next round on
    loop {
        st.rounds += 1;
        // direct call sites per callee (the cost of extra arguments)
        let mut sites: HashMap<u32, usize> = HashMap::new();
        for f in funcs.values() {
            for b in &f.blocks {
                for i in &b.insts {
                    if let Inst::Call(Callee::Fn(t), _, _) = i {
                        *sites.entry(*t).or_insert(0) += 1;
                    }
                }
            }
        }
        let mut groups: HashMap<(u64, u64), Vec<u32>> = HashMap::new();
        for (id, f) in funcs.iter() {
            if mergeable(f) && !thunks.contains(id) {
                groups.entry(shape(*id, f)).or_default().push(*id);
            }
        }
        let mut groups: Vec<Vec<u32>> = groups.into_values().filter(|g| g.len() >= 2).collect();
        groups.sort();
        // redirect: member -> (merged id, the member's constants)
        let mut redirect: HashMap<u32, (u32, Vec<Op>)> = HashMap::new();
        let mut made: Vec<(u32, Func)> = Vec::new();
        for g in groups {
            let rep = &funcs[&g[0]];
            // the differing constant positions (block, inst), checked structurally
            let mut pos: Vec<(usize, usize)> = Vec::new();
            let mut same = true;
            'shape: for (bi, b) in rep.blocks.iter().enumerate() {
                for (ii, i) in b.insts.iter().enumerate() {
                    let mi = masked(g[0], i);
                    let op0 = operand(i);
                    let mut differs = false;
                    for m in &g[1..] {
                        let f = &funcs[m];
                        let Some(j) = f.blocks.get(bi).and_then(|b| b.insts.get(ii)) else {
                            same = false;
                            break 'shape;
                        };
                        if masked(*m, j) != mi {
                            same = false;
                            break 'shape;
                        }
                        if operand(j) != op0 {
                            differs = true;
                        }
                    }
                    if differs {
                        pos.push((bi, ii));
                    }
                }
            }
            if !same || pos.len() > max_params {
                continue;
            }
            let n_sites: usize = g.iter().map(|m| sites.get(m).copied().unwrap_or(0)).sum();
            let n_thunks = g.iter().filter(|m| keep.contains(m)).count();
            let saved = est_bytes(rep) * (g.len() - 1);
            let cost = n_sites * pos.len() * 3 + n_thunks * (rep.params.len() * 2 + pos.len() * 3 + 6) + if pos.is_empty() { 0 } else { est_bytes(rep) / 8 };
            if saved <= cost {
                continue;
            }
            // the merged body: the representative with each differing constant read from a new
            // trailing parameter; self calls pass the parameters on
            let mid = *next_id;
            *next_id += 1;
            let mut mf = rep.clone();
            mf.name = format!("{} [merged {}]", rep.name, g.len());
            let extra: Vec<VReg> = pos
                .iter()
                .map(|&(bi, ii)| {
                    let c = match &rep.blocks[bi].insts[ii] {
                        Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::Addr(d, _) | Inst::FnAddr(d, _) => rep.vregs[d.0 as usize],
                        _ => Cls::I,
                    };
                    mf.vreg(c)
                })
                .collect();
            for (k, &(bi, ii)) in pos.iter().enumerate() {
                let p = extra[k];
                let i = &mut mf.blocks[bi].insts[ii];
                *i = match i.clone() {
                    Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::Addr(d, _) | Inst::FnAddr(d, _) => Inst::Mov(d, p),
                    Inst::IBinI(o, t, d, a, _) => Inst::IBin(o, t, d, a, p),
                    Inst::ICmpI(c, s, d, a, _) => Inst::ICmp(c, s, d, a, p),
                    _ => unreachable!(),
                };
            }
            for b in mf.blocks.iter_mut() {
                for i in b.insts.iter_mut() {
                    if let Inst::Call(Callee::Fn(t), a, _) = i {
                        if *t == g[0] {
                            *t = mid;
                            a.extend(extra.iter().copied());
                        }
                    }
                }
            }
            mf.params.extend(extra.iter().copied());
            // per-parameter metadata of the representative stays valid: the old params keep their
            // vregs; the new ones carry no facts
            st.groups += 1;
            st.params += pos.len();
            for m in &g {
                let f = &funcs[m];
                let ops: Vec<Op> = pos.iter().map(|&(bi, ii)| operand(&f.blocks[bi].insts[ii]).unwrap()).collect();
                redirect.insert(*m, (mid, ops));
            }
            made.push((mid, mf));
        }
        if redirect.is_empty() {
            break;
        }
        // callers pass the constants; members that must keep their id become thunks
        for (id, f) in funcs.iter_mut() {
            if redirect.contains_key(id) && !keep.contains(id) {
                continue;
            }
            rewrite_calls(f, &redirect);
        }
        for (_, f) in made.iter_mut() {
            rewrite_calls(f, &redirect);
        }
        let ids: Vec<u32> = redirect.keys().copied().collect();
        for id in ids {
            st.merged += 1;
            if keep.contains(&id) {
                let (mid, ops) = redirect[&id].clone();
                let f = funcs.get_mut(&id).unwrap();
                *f = thunk(f, mid, &ops);
                thunks.insert(id);
                st.thunks += 1;
            } else {
                funcs.remove(&id);
            }
        }
        for (mid, mf) in made {
            funcs.insert(mid, mf);
        }
    }
    st
}

/// Direct calls of redirected functions call the merged body with the constants appended.
fn rewrite_calls(f: &mut Func, redirect: &HashMap<u32, (u32, Vec<Op>)>) {
    for bi in 0..f.blocks.len() {
        let mut ii = 0;
        while ii < f.blocks[bi].insts.len() {
            let t = match &f.blocks[bi].insts[ii] {
                Inst::Call(Callee::Fn(t), _, _) => *t,
                _ => {
                    ii += 1;
                    continue;
                }
            };
            let Some((mid, ops)) = redirect.get(&t) else {
                ii += 1;
                continue;
            };
            let mut new_args = Vec::with_capacity(ops.len());
            let mut pre = Vec::with_capacity(ops.len());
            for op in ops {
                let (r, i) = materialise(f, *op);
                new_args.push(r);
                pre.push(i);
            }
            let pos = f.blocks[bi].pos.get(ii).copied().unwrap_or(0);
            let b: &mut Block = &mut f.blocks[bi];
            if let Inst::Call(Callee::Fn(t), a, _) = &mut b.insts[ii] {
                *t = *mid;
                a.extend(new_args);
            }
            let n = pre.len();
            b.insts.splice(ii..ii, pre);
            if b.pos.len() + n == b.insts.len() {
                b.pos.splice(ii..ii, std::iter::repeat(pos).take(n));
            }
            ii += n + 1;
        }
    }
}

/// A body with `f`'s signature that calls the merged body `mid` with `ops` appended.
fn thunk(f: &Func, mid: u32, ops: &[Op]) -> Func {
    let mut t = Func::new(f.name.clone(), f.file);
    t.vregs = f.params.iter().map(|p| f.vregs[p.0 as usize]).collect();
    t.params = (0..f.params.len() as u32).map(VReg).collect();
    t.rets = f.rets.clone();
    t.sret = f.sret.and_then(|s| f.params.iter().position(|p| *p == s)).map(|k| VReg(k as u32));
    let b = t.block();
    let mut args = t.params.clone();
    let mut insts = Vec::new();
    for op in ops {
        let (r, i) = materialise(&mut t, *op);
        args.push(r);
        insts.push(i);
    }
    let rets: Vec<VReg> = f.rets.clone().into_iter().map(|c| t.vreg(c)).collect();
    insts.push(Inst::Call(Callee::Fn(mid), args, rets.clone()));
    let blk = &mut t.blocks[b as usize];
    blk.pos = vec![0; insts.len()];
    blk.insts = insts;
    blk.term = Term::Ret(rets);
    t
}
