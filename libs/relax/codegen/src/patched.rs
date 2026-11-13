//! A live session's split root build (`split_patched_path`). Its first inlining round guarded each
//! copy of a patchable body (`opt::inline_site`): a compare of the build's copy version, a block
//! that reads the copy's flags, a slow call of its slot that rejoins the copy's continuation. Those
//! slow calls cost the optimiser more than the reads do: the values they take stay in memory and
//! their results merge back into the fast path. So the body is split before it is optimised: the
//! fast half runs every copy unguarded, and an exit in each stretch that holds one (after a call
//! or a loop's back edge, where a frame may have waited for a patch), before its first copy where
//! the patched path needs the least from it, reads the copy version once. While it holds the value the build read, no flag of a copy the code holds was
//! set; once it moved, the exit leaves for the patched path, a copy of the rest of the fn whose
//! copies keep their guards, and never comes back. The fast half then holds no guard and no slow
//! call, and the optimiser sees the code a run without patches builds.
//!
//! A patch reaches running frames as the guards' did: new calls run new code (reload rebuilds the
//! fns that inlined the body), a frame that returned from a call or crossed a back edge leaves at
//! its next exit, a stretch with no wait point may finish its old copies.

use relax_backend::passes::map_inst;
use relax_backend::regalloc::{for_defs, for_uses, term_uses, term_vregs_mut};
use relax_backend::rir::*;

/// What `split_patched_path` made (the inline trace).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SplitStats {
    /// exits the first copy site of a stretch needs (each where it carries the least)
    pub site_exits: u32,
    /// exits a call the second inlining round may take in needs (provisional: deleted when the
    /// round does not take it in, `prune_exits`)
    pub call_exits: u32,
    /// copy sites an earlier exit of their stretch covers: their guards are gone
    pub covered: u32,
    /// the patched path: blocks and instructions
    pub blocks: u32,
    pub insts: u32,
    /// slots the patched path holds a copy of (filled at each exit), and slots both halves share
    pub cloned: u32,
    pub shared: u32,
    /// the exits' fills (an exit and a slot it fills) and the bytes they fill
    pub filled: u32,
    pub fill_bytes: u32,
    /// registers the exits move into the patched path
    pub moved: u32,
}

/// A body whose copy a live patch has to reach (by an exit or a guard): a patchable slot's
/// (`Func::patch_flag`), or one that holds copies of such (`unguarded_flags`, `guard_calls`).
pub fn patch_reaches(b: &Func) -> bool {
    b.patch_flag.is_some() || !b.unguarded_flags.is_empty() || !b.guard_calls.is_empty()
}

/// Per block, the guard test it ends in, as (fast target, slow side): a `Branch` one of whose
/// targets is a block holding a slow call (`Func::guard_calls`), or a guard test with the same
/// fast target (a copy version's compare leads to its flags' test). And per block, whether it
/// holds a slow call.
fn guard_tests(f: &Func) -> (Vec<Option<(u32, u32)>>, Vec<bool>) {
    let nb = f.blocks.len();
    let slow: Vec<bool> = (0..nb).map(|b| (0..f.blocks[b].insts.len()).any(|ii| f.guard_call(b, ii))).collect();
    let mut test: Vec<Option<(u32, u32)>> = vec![None; nb];
    if !slow.contains(&true) {
        return (test, slow);
    }
    loop {
        let mut changed = false;
        for b in 0..nb {
            let Term::Branch(_, x, y) = f.blocks[b].term else { continue };
            if test[b].is_some() || x == y {
                continue;
            }
            let leads = |side: u32, fast: u32| slow[side as usize] || test[side as usize].is_some_and(|(t, _)| t == fast);
            let found = if leads(y, x) {
                Some((x, y))
            } else if leads(x, y) {
                Some((y, x))
            } else {
                None
            };
            if found.is_some() {
                test[b] = found;
                changed = true;
            }
        }
        if !changed {
            return (test, slow);
        }
    }
}

/// The blocks `succ` reaches from `entries` in reverse postorder, and each block's rank in it
/// (usize::MAX: not reached). An edge to a block not after its source closes a cycle (a loop's back
/// edge, also in irreducible flow).
fn rpo(succ: &[Vec<u32>], entries: &[u32]) -> (Vec<usize>, Vec<usize>) {
    let nb = succ.len();
    let mut seen = vec![false; nb];
    let mut post = Vec::with_capacity(nb);
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for &e in entries {
        if seen[e as usize] {
            continue;
        }
        seen[e as usize] = true;
        stack.push((e as usize, 0));
        while let Some(top) = stack.last_mut() {
            let b = top.0;
            match succ[b].get(top.1) {
                Some(&s) => {
                    top.1 += 1;
                    if !seen[s as usize] {
                        seen[s as usize] = true;
                        stack.push((s as usize, 0));
                    }
                }
                None => {
                    post.push(b);
                    stack.pop();
                }
            }
        }
    }
    post.reverse();
    let mut rank = vec![usize::MAX; nb];
    for (k, &b) in post.iter().enumerate() {
        rank[b] = k;
    }
    (post, rank)
}

fn preds_in(succ: &[Vec<u32>], order: &[usize]) -> Vec<Vec<u32>> {
    let mut preds = vec![Vec::new(); succ.len()];
    for &b in order {
        for &s in &succ[b] {
            preds[s as usize].push(b as u32);
        }
    }
    preds
}

/// Gen and kill sets of the blocks `blocks` lists over the registers `idx` numbers (u32::MAX: not
/// counted), `words` per block in list order.
fn gen_kill(f: &Func, blocks: &[usize], idx: &dyn Fn(VReg) -> u32, words: usize) -> (Vec<u64>, Vec<u64>) {
    let mut gen = vec![0u64; blocks.len() * words];
    let mut kill = vec![0u64; blocks.len() * words];
    let mut tu = Vec::new();
    for (k, &b) in blocks.iter().enumerate() {
        let w0 = k * words;
        let used = |v: VReg, gen: &mut [u64], kill: &[u64]| {
            let x = idx(v);
            if x != u32::MAX && kill[w0 + x as usize / 64] >> (x % 64) & 1 == 0 {
                gen[w0 + x as usize / 64] |= 1 << (x % 64);
            }
        };
        for i in &f.blocks[b].insts {
            for_uses(i, |v| used(v, &mut gen, &kill));
            for_defs(i, |v| {
                let x = idx(v);
                if x != u32::MAX {
                    kill[w0 + x as usize / 64] |= 1 << (x % 64);
                }
            });
        }
        term_uses(&f.blocks[b].term, &mut tu);
        for &v in &tu {
            used(v, &mut gen, &kill);
        }
    }
    (gen, kill)
}

/// The live-in sets of the blocks `blocks` lists (`at`: a block's place in the list, u32::MAX for
/// one outside it; edges out of the list are not followed), from their `gen_kill` sets: a backward
/// worklist to the fixpoint. (regalloc::solve_liveness walks from the entry block, which a part of
/// the fn after it does not hold.)
fn liveness_in(f: &Func, blocks: &[usize], at: &[u32], words: usize, gen: &[u64], kill: &[u64]) -> Vec<u64> {
    let n = blocks.len();
    let mut live = vec![0u64; n * words];
    if words == 0 {
        return live;
    }
    let mut preds = vec![Vec::new(); n];
    for (k, &b) in blocks.iter().enumerate() {
        for s in f.blocks[b].term.succs() {
            if at[s as usize] != u32::MAX {
                preds[at[s as usize] as usize].push(k as u32);
            }
        }
    }
    let mut queued = vec![true; n];
    let mut work: Vec<u32> = (0..n as u32).collect();
    let mut out = vec![0u64; words];
    while let Some(k) = work.pop() {
        let k = k as usize;
        queued[k] = false;
        out.fill(0);
        for s in f.blocks[blocks[k]].term.succs() {
            let j = at[s as usize];
            if j != u32::MAX {
                for (o, l) in out.iter_mut().zip(&live[j as usize * words..(j as usize + 1) * words]) {
                    *o |= *l;
                }
            }
        }
        let mut grew = false;
        for w in 0..words {
            let v = gen[k * words + w] | (out[w] & !kill[k * words + w]);
            if v != live[k * words + w] {
                live[k * words + w] = v;
                grew = true;
            }
        }
        if grew {
            for &p in &preds[k] {
                if !queued[p as usize] {
                    queued[p as usize] = true;
                    work.push(p);
                }
            }
        }
    }
    live
}

/// How an instruction touches the bytes of the slots a `ByteMap` numbers: it reads them, writes
/// them whole (through an exact address: they are dead before it) or may write them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Acc {
    Read,
    Kill,
    Write,
}

/// The bytes of some slots, numbered: a slot's index by its slot (`cof`, u32::MAX: not one of
/// them), each one's first bit (`bit0`, one past the last at the end), and which slot each
/// register addresses (`tp`), in the fn `f`.
struct ByteMap<'a> {
    f: &'a Func,
    tp: &'a Taint,
    cof: &'a [u32],
    bit0: &'a [usize],
}

impl ByteMap<'_> {
    /// The bytes address `a` names, to `at` as `k`: every numbered slot (an address of several),
    /// one whole (an unknown offset, or no length), or a range of one.
    fn put(&self, at: &mut dyn FnMut(Acc, usize, usize), k: Acc, a: VReg, off: i64, len: i64) {
        let s = self.tp.slot[a.0 as usize];
        if s == NONE {
            return;
        }
        let bit0 = self.bit0;
        if s == MIXED {
            for c in 0..bit0.len() - 1 {
                at(k, bit0[c], bit0[c + 1]);
            }
            return;
        }
        let c = self.cof.get(s as usize).copied().unwrap_or(u32::MAX);
        if c == u32::MAX {
            return;
        }
        let c = c as usize;
        let size = (bit0[c + 1] - bit0[c]) as i64;
        match self.tp.off[a.0 as usize] {
            Some((o, _)) if len > 0 => {
                let (lo, hi) = ((o + off).clamp(0, size), (o + off + len).clamp(0, size));
                at(k, bit0[c] + lo as usize, bit0[c] + hi as usize);
            }
            _ => at(k, bit0[c], bit0[c + 1]),
        }
    }

    /// The byte accesses of one instruction: a typed access at a known offset reads or writes its
    /// bytes, a copy reads its source's and writes its destination's, a call writes the whole of a
    /// result buffer it gets when only the call can reach it (`result_buffer`, and no other
    /// escape here: `Taint::kept`), any other use of an address (a call, an indexed or atomic
    /// access, a store of it) reads and may write the slot whole. A buffer whose address escapes
    /// elsewhere is read by the call as before: the cost is fill bytes, not correctness.
    fn access(&self, i: &Inst, at: &mut dyn FnMut(Acc, usize, usize)) {
        let size = |m: Mem| relax_backend::passes::mem_size(m) as i64;
        if let Some((r, rest)) = result_buffer(self.f, self.tp, i).filter(|(r, _)| !self.tp.kept[self.tp.slot[r.0 as usize] as usize]) {
            for &u in rest {
                self.put(at, Acc::Read, u, 0, 0);
                self.put(at, Acc::Write, u, 0, 0);
            }
            let s = self.tp.slot[r.0 as usize];
            return self.put(at, Acc::Kill, r, 0, self.f.slots[s as usize].size as i64);
        }
        let (a, off, len, v) = match *i {
            Inst::Load(m, _, a, off) => return self.put(at, Acc::Read, a, off as i64, size(m)),
            Inst::VLoad(_, a, off) => return self.put(at, Acc::Read, a, off as i64, 16),
            Inst::Store(m, a, off, v) => (a, off as i64, size(m), Some(v)),
            Inst::VStore(a, off, v) => (a, off as i64, 16, Some(v)),
            Inst::Copy(d, src, n) => {
                self.put(at, Acc::Read, src, 0, n as i64);
                (d, 0, n as i64, None)
            }
            Inst::SlotAddr(..) | Inst::Mov(..) | Inst::IBinI(IOp::Add | IOp::Sub, ..) | Inst::IBin(IOp::Add, ..) => return,
            _ => {
                for_uses(i, |u| {
                    self.put(at, Acc::Read, u, 0, 0);
                    self.put(at, Acc::Write, u, 0, 0);
                });
                return;
            }
        };
        if let Some(v) = v {
            self.put(at, Acc::Read, v, 0, 0);
            self.put(at, Acc::Write, v, 0, 0);
        }
        let exact = self.tp.slot[a.0 as usize] != MIXED && self.tp.off[a.0 as usize].is_some();
        self.put(at, if exact { Acc::Kill } else { Acc::Write }, a, off, len);
    }
}

fn bits_set(w: &mut [u64], lo: usize, hi: usize) {
    for x in lo..hi {
        w[x / 64] |= 1 << (x % 64);
    }
}

fn bits_clear(w: &mut [u64], lo: usize, hi: usize) {
    for x in lo..hi {
        w[x / 64] &= !(1 << (x % 64));
    }
}

/// Gen and kill sets over the slot bytes `m` numbers, of the blocks `blocks` lists (in list order,
/// `words` each): the bytes a block reads before it writes them whole, and those it writes whole.
fn byte_gen_kill(f: &Func, blocks: &[usize], m: &ByteMap, words: usize) -> (Vec<u64>, Vec<u64>) {
    let n = blocks.len();
    let mut gen = vec![0u64; n * words];
    let mut kill = vec![0u64; n * words];
    let mut tu = Vec::new();
    for (k, &b) in blocks.iter().enumerate() {
        let (g, kl) = (&mut gen[k * words..(k + 1) * words], &mut kill[k * words..(k + 1) * words]);
        let mut at = |a: Acc, lo: usize, hi: usize| match a {
            Acc::Read => {
                for x in lo..hi {
                    if kl[x / 64] >> (x % 64) & 1 == 0 {
                        g[x / 64] |= 1 << (x % 64);
                    }
                }
            }
            Acc::Kill => bits_set(kl, lo, hi),
            Acc::Write => {}
        };
        let blk = &f.blocks[b];
        for i in &blk.insts {
            m.access(i, &mut at);
        }
        term_uses(&blk.term, &mut tu);
        for &u in &tu {
            m.put(&mut at, Acc::Read, u, 0, 0);
        }
    }
    (gen, kill)
}

/// The slot bytes `m` numbers that may have been written on a way from the entry to the end of
/// each block of `order` (`preds`: that flow's), `words` per block of the fn: a forward union to
/// the fixpoint.
fn may_written(f: &Func, order: &[usize], preds: &[Vec<u32>], m: &ByteMap, words: usize) -> Vec<u64> {
    let mut out = vec![0u64; f.blocks.len() * words];
    let mut tu = Vec::new();
    for &b in order {
        let g = &mut out[b * words..(b + 1) * words];
        let mut at = |a: Acc, lo: usize, hi: usize| {
            if a != Acc::Read {
                bits_set(g, lo, hi);
            }
        };
        let blk = &f.blocks[b];
        for i in &blk.insts {
            m.access(i, &mut at);
        }
        term_uses(&blk.term, &mut tu);
        for &u in &tu {
            m.put(&mut at, Acc::Write, u, 0, 0);
        }
    }
    loop {
        let mut changed = false;
        for &b in order {
            for &p in &preds[b] {
                for w in 0..words {
                    let more = out[p as usize * words + w] & !out[b * words + w];
                    if more != 0 {
                        out[b * words + w] |= more;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            return out;
        }
    }
}

/// Per block, the union of its successors' sets (`live`: `words` per block).
fn outs(f: &Func, live: &[u64], words: usize) -> Vec<u64> {
    let mut out = vec![0u64; f.blocks.len() * words];
    for (b, blk) in f.blocks.iter().enumerate() {
        for s in blk.term.succs() {
            for w in 0..words {
                out[b * words + w] |= live[s as usize * words + w];
            }
        }
    }
    out
}

/// Position `p` in the patched path (none stays none).
fn patched_pos(p: u64) -> u64 {
    if p == 0 { 0 } else { p | POS_PATCHED }
}

/// Whether an exit ran at the start of block `b` on every path since a frame last may have waited:
/// from the entry (a frame that has not waited may finish old copies, as `opt::unguard_prefix`
/// lets it) or from an exit, through no call and no back edge.
fn ran_at(b: usize, preds: &[Vec<u32>], rank: &[usize], out: &[bool]) -> bool {
    preds[b].iter().all(|&p| rank[p as usize] < rank[b] && out[p as usize])
}

/// Per block of a split body: its exit, as (hit, miss, the index of its word read): the block
/// reads the build's copy version (`Func::copy_ver`) and goes on to `hit` while it holds the
/// value the build read, else to `miss`, the exit's block into the patched path (a compare for
/// inequality branching the other way, as a pass may turn it, is one too).
pub fn exits_of(f: &Func) -> Vec<Option<(u32, u32, usize)>> {
    let Some((word, ver)) = f.copy_ver.filter(|_| f.patched_path) else { return vec![None; f.blocks.len()] };
    let mut addr = vec![false; f.vregs.len()];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Addr(a, s) = i {
                if *s == word {
                    addr[a.0 as usize] = true;
                }
            }
        }
    }
    f.blocks
        .iter()
        .map(|b| {
            let Term::Branch(h, x, y) = b.term else { return None };
            let k = b.insts.iter().rposition(|i| matches!(i, Inst::ICmpI(Cond::Eq | Cond::Ne, false, d, _, k) if *d == h && *k == ver as i64))?;
            let Inst::ICmpI(cond, _, _, w, _) = b.insts[k] else { return None };
            let j = b.insts[..k].iter().rposition(|i| matches!(i, Inst::AtomicLoad(Mem::Int(8, false), AtomOrd::Relaxed, d, a) if *d == w && addr[a.0 as usize]))?;
            Some(if cond == Cond::Eq { (x, y, j) } else { (y, x, j) })
        })
        .collect()
}

/// A split body's fast flow: an exit goes on to its hit only.
fn fast_succs(f: &Func, ex: &[Option<(u32, u32, usize)>]) -> Vec<Vec<u32>> {
    (0..f.blocks.len()).map(|b| match ex[b] {
        Some((hit, _, _)) => vec![hit],
        None => f.blocks[b].term.succs().collect(),
    }).collect()
}

fn is_wait(f: &Func, b: usize, ii: usize) -> bool {
    matches!(f.blocks[b].insts[ii], Inst::Call(..)) && !f.guard_call(b, ii)
}

/// The fast half's flow from the entry, where a frame may have waited: per block, whether an exit
/// ran at its start (`ran_at`), and whether it is in the patched path (an exit's block or after).
/// The second inlining round takes a copy a patch must reach into the fast half only where one
/// ran (`opt::inline_calls_ordered`).
pub fn exit_cover(f: &Func) -> (Vec<bool>, Vec<bool>) {
    let nb = f.blocks.len();
    let ex = exits_of(f);
    let succ = fast_succs(f, &ex);
    let (order, rank) = rpo(&succ, &[0]);
    let preds = preds_in(&succ, &order);
    let mut out = vec![false; nb];
    let mut ran_in = vec![false; nb];
    for &b in &order {
        let mut ran = ran_at(b, &preds, &rank, &out);
        ran_in[b] = ran;
        for ii in 0..f.blocks[b].insts.len() {
            if ex[b].is_some_and(|e| e.2 == ii) {
                ran = true;
            }
            if is_wait(f, b, ii) {
                ran = false;
            }
        }
        out[b] = ran;
    }
    let misses: Vec<u32> = ex.iter().flatten().map(|e| e.1).collect();
    let all: Vec<Vec<u32>> = f.blocks.iter().map(|b| b.term.succs().collect()).collect();
    let (pat, _) = rpo(&all, &misses);
    let mut patched = vec![false; nb];
    for b in pat {
        patched[b] = true;
    }
    (patched, ran_in)
}

/// Unused reads of a guard's words (an 8-byte relaxed load of a symbol's address) and of what
/// they feed (addresses, compares, ors): a test that became a jump leaves its compare. A relaxed
/// load whose value is unused has no effect, so any such load goes.
fn drop_dead_reads(f: &mut Func) {
    let mut tu = Vec::new();
    loop {
        let nv = f.vregs.len();
        let mut uses = vec![0u32; nv];
        let mut addr = vec![false; nv];
        for b in &f.blocks {
            for i in &b.insts {
                for_uses(i, |u| uses[u.0 as usize] += 1);
                if let Inst::Addr(a, _) = i {
                    addr[a.0 as usize] = true;
                }
            }
            term_uses(&b.term, &mut tu);
            for u in &tu {
                uses[u.0 as usize] += 1;
            }
        }
        let dead = |i: &Inst| -> bool {
            let d = match *i {
                Inst::AtomicLoad(Mem::Int(8, false), AtomOrd::Relaxed, d, a) if addr[a.0 as usize] => d,
                Inst::Addr(d, _) | Inst::ICmpI(_, _, d, _, _) | Inst::IBin(IOp::Or, _, d, _, _) => d,
                _ => return false,
            };
            uses[d.0 as usize] == 0
        };
        let mut any = false;
        for b in &mut f.blocks {
            if !b.insts.iter().any(|i| dead(i)) {
                continue;
            }
            let keep: Vec<bool> = b.insts.iter().map(|i| !dead(i)).collect();
            let mut it = keep.iter();
            b.insts.retain(|_| *it.next().unwrap_or(&true));
            if b.pos.len() == keep.len() {
                let mut it = keep.iter();
                b.pos.retain(|_| *it.next().unwrap_or(&true));
            }
            any = true;
        }
        if !any {
            return;
        }
    }
}

/// Which slot each register of the fast half addresses (`blocks`): `slot[v]` (NONE, MIXED for
/// more than one), its offset in it when every definition agrees (`off`, with the integer type
/// of the add that made it), whether some definition is not such an address (`other`), the
/// slots whose address escapes there (anything but a memory access's base or a copy's operand),
/// and of those the ones something may keep a pointer to (`kept`: any escape but the result
/// buffer of a call that only writes it, `result_buffer`).
struct Taint {
    slot: Vec<u32>,
    off: Vec<Option<(i64, IntTy)>>,
    other: Vec<bool>,
    escaped: Vec<bool>,
    kept: Vec<bool>,
}

const NONE: u32 = u32::MAX;
const MIXED: u32 = u32::MAX - 1;

/// A call that gets a result buffer (a result returned in memory) it writes whole and only
/// writes: the call is to a callee in `Func::sret_callees`, its first argument is a lowering's
/// `Slot::ret` at offset 0 (`t`), and no other argument names that slot. The buffer and the
/// other arguments. Whether anything else may keep a pointer to the slot is `Taint::kept`.
fn result_buffer<'a>(f: &Func, t: &Taint, i: &'a Inst) -> Option<(VReg, &'a [VReg])> {
    let Inst::Call(Callee::Fn(id) | Callee::CFn(id, _), args, _) = i else { return None };
    let (&r, rest) = args.split_first().filter(|_| f.sret_callees.contains(id))?;
    let s = t.slot[r.0 as usize];
    (s != NONE && s != MIXED && f.slots[s as usize].ret && matches!(t.off[r.0 as usize], Some((0, _))) && rest.iter().all(|u| !matches!(t.slot[u.0 as usize], x if x == s || x == MIXED))).then_some((r, rest))
}

fn slot_taint(f: &Func, blocks: &[bool]) -> Taint {
    let nv = f.vregs.len();
    let mut t = Taint { slot: vec![NONE; nv], off: vec![None; nv], other: vec![false; nv], escaped: vec![false; f.slots.len()], kept: vec![false; f.slots.len()] };
    // an address definition: (dst, src slot or vreg, offset added)
    fn join(t: &mut Taint, d: VReg, s: u32, off: Option<(i64, IntTy)>) -> bool {
        let d = d.0 as usize;
        let cur = t.slot[d];
        if cur == NONE {
            t.slot[d] = s;
            t.off[d] = off;
            return true;
        }
        if cur == MIXED {
            return false;
        }
        if s == MIXED || s != cur {
            t.escaped[cur as usize] = true;
            if s != MIXED {
                t.escaped[s as usize] = true;
            }
            t.slot[d] = MIXED;
            t.off[d] = None;
            return true;
        }
        if t.off[d].is_some() && t.off[d].map(|x| x.0) != off.map(|x| x.0) {
            t.off[d] = None;
            return true;
        }
        false
    }
    let from = |t: &Taint, a: VReg, k: Option<(i64, IntTy)>| -> Option<(u32, Option<(i64, IntTy)>)> {
        let s = t.slot[a.0 as usize];
        (s != NONE).then(|| (s, match (t.off[a.0 as usize], k) {
            (Some((o, ty0)), Some((k, ty))) => Some((o.wrapping_add(k), if k == 0 { ty0 } else { ty })),
            _ => None,
        }))
    };
    loop {
        let mut changed = false;
        for (bi, b) in f.blocks.iter().enumerate() {
            if !blocks[bi] {
                continue;
            }
            for i in &b.insts {
                let (d, src) = match *i {
                    Inst::SlotAddr(d, s) => (d, Some((s, Some((0, U64))))),
                    Inst::Mov(d, a) => (d, from(&t, a, Some((0, U64)))),
                    Inst::IBinI(IOp::Add, ty, d, a, k) => (d, from(&t, a, Some((k, ty)))),
                    Inst::IBinI(IOp::Sub, ty, d, a, k) => (d, from(&t, a, Some((k.wrapping_neg(), ty)))),
                    Inst::IBin(IOp::Add, _, d, a, x) => {
                        if let Some((s, _)) = from(&t, x, None) {
                            changed |= join(&mut t, d, s, None);
                        }
                        (d, from(&t, a, None).map(|(s, _)| (s, None)))
                    }
                    _ => continue,
                };
                if let Some((s, off)) = src {
                    changed |= join(&mut t, d, s, off);
                }
            }
        }
        if !changed {
            break;
        }
    }
    // definitions that are not a slot's address at a known offset (an indexed one's offset is
    // unknown anyway)
    for &p in &f.params {
        t.other[p.0 as usize] = true;
    }
    loop {
        let mut changed = false;
        for (bi, b) in f.blocks.iter().enumerate() {
            if !blocks[bi] {
                continue;
            }
            for i in &b.insts {
                let bad = match *i {
                    Inst::SlotAddr(..) => continue,
                    Inst::Mov(d, a) | Inst::IBinI(IOp::Add | IOp::Sub, _, d, a, _) => (t.slot[a.0 as usize] == NONE || t.other[a.0 as usize]).then_some(d),
                    _ => {
                        for_defs(i, |d| {
                            if !t.other[d.0 as usize] {
                                t.other[d.0 as usize] = true;
                                changed = true;
                            }
                        });
                        continue;
                    }
                };
                if let Some(d) = bad.filter(|d| !t.other[d.0 as usize]) {
                    t.other[d.0 as usize] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    // escapes
    fn esc(t: &mut Taint, v: VReg) {
        let s = t.slot[v.0 as usize];
        if s != NONE && s != MIXED {
            t.escaped[s as usize] = true;
            t.kept[s as usize] = true;
        }
    }
    let mut tu = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        if !blocks[bi] {
            continue;
        }
        for i in &b.insts {
            if let Some((r, rest)) = result_buffer(f, &t, i) {
                // the callee only writes its result buffer and keeps no pointer to it
                t.escaped[t.slot[r.0 as usize] as usize] = true;
                for &u in rest {
                    esc(&mut t, u);
                }
                continue;
            }
            match *i {
                Inst::Load(..) | Inst::VLoad(..) | Inst::AtomicLoad(..) | Inst::Copy(..) | Inst::SlotAddr(..) | Inst::Mov(..) | Inst::IBinI(IOp::Add | IOp::Sub, ..) | Inst::IBin(IOp::Add, ..) => {}
                Inst::Store(_, _, _, s) | Inst::VStore(_, _, s) | Inst::AtomicStore(_, _, _, s) | Inst::AtomicRmw(_, _, _, _, _, s) => esc(&mut t, s),
                Inst::LoadX(_, _, _, x, _, _) | Inst::VLoadX(_, _, x, _, _) => esc(&mut t, x),
                Inst::StoreX(_, _, x, _, _, s) | Inst::VStoreX(_, x, _, _, s) => {
                    esc(&mut t, x);
                    esc(&mut t, s);
                }
                Inst::AtomicCas(_, _, _, _, _, e, n) => {
                    esc(&mut t, e);
                    esc(&mut t, n);
                }
                _ => for_uses(i, |u| esc(&mut t, u)),
            }
        }
        term_uses(&b.term, &mut tu);
        for &u in &tu {
            esc(&mut t, u);
        }
    }
    t
}

/// How the patched path makes a value it uses where it uses it, so that no exit carries it: a
/// slot's address at a known offset (every definition agrees), or a constant defined once.
#[derive(Clone)]
enum Remake {
    Slot(u32, i64, IntTy),
    Const(Inst),
}

/// Per register of the body (`tp`: its addresses, over all its blocks), how the patched path
/// remakes it, if it can.
fn remakes(f: &Func, tp: &Taint) -> Vec<Option<Remake>> {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut konst: Vec<Option<Inst>> = vec![None; nv];
    for &p in &f.params {
        ndef[p.0 as usize] += 2;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            if let Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::VConst(d, _) | Inst::Addr(d, _) = i {
                konst[d.0 as usize] = Some(i.clone());
            }
        }
    }
    (0..nv)
        .map(|v| {
            let s = tp.slot[v];
            if s != NONE && s != MIXED && !tp.other[v] {
                if let Some((o, ty)) = tp.off[v] {
                    return Some(Remake::Slot(s, o, ty));
                }
            }
            if ndef[v] == 1 {
                return konst[v].clone().map(Remake::Const);
            }
            None
        })
        .collect()
}

/// Appends the instructions that remake a value (`Remake`) of class `cls` at position `p`; its
/// new register.
fn remake_into(f: &mut Func, r: &Remake, cls: Cls, insts: &mut Vec<Inst>, pos: &mut Vec<u64>, p: u64) -> VReg {
    match *r {
        Remake::Slot(s, off, ty) => {
            let a = f.vreg(Cls::I);
            insts.push(Inst::SlotAddr(a, s));
            pos.push(p);
            if off == 0 {
                return a;
            }
            let d = f.vreg(Cls::I);
            insts.push(Inst::IBinI(IOp::Add, ty, d, a, off));
            pos.push(p);
            d
        }
        Remake::Const(ref i) => {
            let d = f.vreg(cls);
            insts.push(map_inst(i, &|_| d, 0));
            pos.push(p);
            d
        }
    }
}

/// What an exit placed at a point of the body before the split would carry into the patched path,
/// which copies the code after that point (both sides of each test): the registers live there (but
/// what it remakes, `Remake`), and the bytes of the slots the fast half keeps to itself that are
/// live there and may have been written on a way to it (`order` / `preds`: the fast half's flow).
struct Estimate {
    ridx: Vec<u32>,
    rwords: usize,
    rout: Vec<u64>,
    tp: Taint,
    cof: Vec<u32>,
    bit0: Vec<usize>,
    bwords: usize,
    bout: Vec<u64>,
    wout: Vec<u64>,
}

impl Estimate {
    fn new(f: &Func, order: &[usize], preds: &[Vec<u32>], remake: &[Option<Remake>], tp: Taint) -> Estimate {
        let nb = f.blocks.len();
        let all: Vec<usize> = (0..nb).collect();
        let at: Vec<u32> = (0..nb as u32).collect();
        let ridx: Vec<u32> = remake.iter().enumerate().map(|(v, r)| if r.is_some() { u32::MAX } else { v as u32 }).collect();
        let rwords = ridx.len().div_ceil(64);
        let (gen, kill) = gen_kill(f, &all, &|v| ridx.get(v.0 as usize).copied().unwrap_or(u32::MAX), rwords);
        let rout = outs(f, &liveness_in(f, &all, &at, rwords, &gen, &kill), rwords);
        let mut in_fast = vec![false; nb];
        let mut addressed = vec![false; f.slots.len()];
        for &b in order {
            in_fast[b] = true;
            for i in &f.blocks[b].insts {
                if let Inst::SlotAddr(_, s) = i {
                    addressed[*s as usize] = true;
                }
            }
        }
        let tf = slot_taint(f, &in_fast);
        let mut cof = vec![u32::MAX; f.slots.len()];
        let mut bit0 = vec![0usize];
        for s in 0..f.slots.len() {
            if addressed[s] && !tf.escaped[s] {
                cof[s] = (bit0.len() - 1) as u32;
                bit0.push(bit0[bit0.len() - 1] + f.slots[s].size as usize);
            }
        }
        let bwords = bit0[bit0.len() - 1].div_ceil(64);
        let m = ByteMap { f, tp: &tp, cof: &cof, bit0: &bit0 };
        let (gen, kill) = byte_gen_kill(f, &all, &m, bwords);
        let bout = outs(f, &liveness_in(f, &all, &at, bwords, &gen, &kill), bwords);
        let wout = may_written(f, order, preds, &m, bwords);
        Estimate { ridx, rwords, rout, tp, cof, bit0, bwords, bout, wout }
    }

    /// The estimate at each point of block `b`: before each of its instructions, then before its
    /// term.
    fn costs(&self, f: &Func, b: usize, preds: &[Vec<u32>]) -> Vec<u32> {
        let blk = &f.blocks[b];
        let n = blk.insts.len();
        let (rw, bw) = (self.rwords, self.bwords);
        let m = ByteMap { f, tp: &self.tp, cof: &self.cof, bit0: &self.bit0 };
        // what may have been written before each point
        let mut w = vec![0u64; bw];
        for &p in &preds[b] {
            for k in 0..bw {
                w[k] |= self.wout[p as usize * bw + k];
            }
        }
        let mut wat = vec![0u64; (n + 1) * bw];
        for (ii, i) in blk.insts.iter().enumerate() {
            wat[ii * bw..(ii + 1) * bw].copy_from_slice(&w);
            m.access(i, &mut |a, lo, hi| {
                if a != Acc::Read {
                    bits_set(&mut w, lo, hi);
                }
            });
        }
        wat[n * bw..].copy_from_slice(&w);
        // what is live at each point, from the end back
        let mut r = self.rout[b * rw..(b + 1) * rw].to_vec();
        let mut l = self.bout[b * bw..(b + 1) * bw].to_vec();
        let ri = |v: VReg| self.ridx.get(v.0 as usize).copied().unwrap_or(u32::MAX);
        let mut tu = Vec::new();
        term_uses(&blk.term, &mut tu);
        for &u in &tu {
            let x = ri(u);
            if x != u32::MAX {
                bits_set(&mut r, x as usize, x as usize + 1);
            }
            m.put(&mut |_, lo, hi| bits_set(&mut l, lo, hi), Acc::Read, u, 0, 0);
        }
        let count = |r: &[u64], l: &[u64], w: &[u64]| r.iter().map(|x| x.count_ones()).sum::<u32>() + l.iter().zip(w).map(|(a, b)| (a & b).count_ones()).sum::<u32>();
        let mut cost = vec![0u32; n + 1];
        cost[n] = count(&r[..], &l[..], &wat[n * bw..]);
        for ii in (0..n).rev() {
            let i = &blk.insts[ii];
            for_defs(i, |d| {
                let x = ri(d);
                if x != u32::MAX {
                    bits_clear(&mut r, x as usize, x as usize + 1);
                }
            });
            for_uses(i, |u| {
                let x = ri(u);
                if x != u32::MAX {
                    bits_set(&mut r, x as usize, x as usize + 1);
                }
            });
            m.access(i, &mut |a, lo, hi| {
                if a == Acc::Kill {
                    bits_clear(&mut l, lo, hi);
                }
            });
            m.access(i, &mut |a, lo, hi| {
                if a == Acc::Read {
                    bits_set(&mut l, lo, hi);
                }
            });
            cost[ii] = count(&r[..], &l[..], &wat[ii * bw..(ii + 1) * bw]);
        }
        cost
    }
}

/// Immediate dominators in a flow (`order`: its blocks in reverse postorder from the entry,
/// `rank`: their places, `preds`): the entry's is itself, a block not reached has none
/// (usize::MAX). Cooper, Harvey and Kennedy's iteration.
fn idoms_in(order: &[usize], rank: &[usize], preds: &[Vec<u32>]) -> Vec<usize> {
    let mut idom = vec![usize::MAX; rank.len()];
    let Some(&entry) = order.first() else { return idom };
    idom[entry] = entry;
    loop {
        let mut changed = false;
        for &b in &order[1..] {
            let mut new = usize::MAX;
            for &p in &preds[b] {
                let p = p as usize;
                if idom[p] == usize::MAX {
                    continue;
                }
                if new == usize::MAX {
                    new = p;
                    continue;
                }
                let (mut x, mut y) = (p, new);
                while x != y {
                    while rank[x] > rank[y] {
                        x = idom[x];
                    }
                    while rank[y] > rank[x] {
                        y = idom[y];
                    }
                }
                new = x;
            }
            if new != idom[b] {
                idom[b] = new;
                changed = true;
            }
        }
        if !changed {
            return idom;
        }
    }
}

/// The fast half's flow before the split.
struct Flow<'a> {
    order: &'a [usize],
    rank: &'a [usize],
    preds: &'a [Vec<u32>],
    test: &'a [Option<(u32, u32)>],
}

/// The first point, in the flow's order, that needs an exit none covers, with exits at the points
/// `pts` (per block, the instruction an exit goes before; the block's length: before its term) and
/// at `extra`: a copy site's test (an exit must have run since the stretch began), or a call the
/// second inlining round may take in (`takes`). As (block, index, whether a copy site).
fn first_need(f: &Func, fl: &Flow, takes: &dyn Fn(&Callee) -> bool, pts: &[Vec<usize>], extra: Option<(usize, usize)>, out: &mut [bool]) -> Option<(usize, usize, bool)> {
    out.fill(false);
    for &b in fl.order {
        let mut ran = ran_at(b, fl.preds, fl.rank, out);
        let blk = &f.blocks[b];
        for ii in 0..=blk.insts.len() {
            if pts[b].contains(&ii) || extra == Some((b, ii)) {
                ran = true;
            }
            if ii == blk.insts.len() {
                break;
            }
            if let Inst::Call(c, _, _) = &blk.insts[ii] {
                if !ran && takes(c) {
                    return Some((b, ii, false));
                }
                ran = false;
            }
        }
        if fl.test[b].is_some() && !ran {
            return Some((b, blk.insts.len(), true));
        }
        out[b] = ran;
    }
    None
}

/// Splits a live session's root build after its first inlining round (see the module doc): the
/// first copy site of each stretch needs an exit, the later ones (an exit ran since the stretch
/// began) jump to their copies; a call the second round may take in (a direct call of a body a
/// patch must reach, `patch_reaches`, or an indirect call) in a stretch with no exit needs one too.
/// Each exit sits at the point of its stretch, from where a frame may last have waited to the
/// point that needs it, where it carries the least into the patched path (`Estimate`): a
/// temporary the stretch builds and uses is not live there yet. The patched path is a copy of the
/// code after the exits, with fresh registers; its copy version compares go straight to their
/// flags, and it makes the slot addresses and constants it uses where it uses them (`Remake`).
/// Each exit's block is a parallel copy: a move per other register live into its target, and the
/// cells of the slots the patched path holds a copy of (a slot whose address does not escape in
/// the fast half) that the target reads before it writes them and the fast half may have written
/// on a way to the exit, a load and a store each, so the fast half's slot stays a promotable
/// temporary and only those cells stay live at the exit. None: nothing to split (no guard, no
/// call to cover), or a guard whose shape the tests do not know.
pub fn split_patched_path(f: &mut Func, get: &dyn Fn(u32) -> Option<(*const Func, u8)>) -> Option<SplitStats> {
    let (word, ver) = f.copy_ver?;
    if f.patched_path || f.blocks.is_empty() {
        return None;
    }
    let (mut test, slow) = guard_tests(f);
    let nb = f.blocks.len();
    let takes = |c: &Callee| match c {
        Callee::Fn(id) => get(*id).is_some_and(|(p, _)| patch_reaches(unsafe { &*p })),
        Callee::Indirect(_) => true,
        _ => false,
    };
    // the fast half's flow: a guard test goes on to its copy
    let succ: Vec<Vec<u32>> = (0..nb).map(|b| match test[b] {
        Some((t, _)) => vec![t],
        None => f.blocks[b].term.succs().collect(),
    }).collect();
    let (order, rank) = rpo(&succ, &[0]);
    if order.iter().any(|&b| slow[b]) {
        return None;
    }
    let preds = preds_in(&succ, &order);
    let mut st = SplitStats::default();
    let fl = Flow { order: &order, rank: &rank, preds: &preds, test: &test };
    let tp_all = slot_taint(f, &vec![true; nb]);
    let remake = remakes(f, &tp_all);
    // each exit at the point of its stretch where it carries the least (`Estimate`), from where a
    // frame may last have waited to the point that needs it: a point that dominates it with no wait
    // point between, so its loop's; ties go to the nearest. The first point that needs an exit,
    // again, until none does
    let mut pts: Vec<Vec<usize>> = vec![Vec::new(); nb];
    let mut out = vec![false; nb];
    first_need(f, &fl, &takes, &pts, None, &mut out)?;
    let est = Estimate::new(f, &order, &preds, &remake, tp_all);
    let idom = idoms_in(&order, &rank, &preds);
    let mut costs: Vec<Option<Vec<u32>>> = vec![None; nb];
    let mut sites = 0u32;
    while let Some(need) = first_need(f, &fl, &takes, &pts, None, &mut out) {
        let (nbk, ni, site) = need;
        // (cost, distance, block, index)
        let mut best: Option<(u32, u32, usize, usize)> = None;
        let (mut d, mut hi, mut dist) = (nbk, ni, 0u32);
        loop {
            if d != nbk && first_need(f, &fl, &takes, &pts, Some((d, f.blocks[d].insts.len())), &mut out) == Some(need) {
                break;
            }
            let lo = f.blocks[d].insts[..hi].iter().rposition(|i| matches!(i, Inst::Call(..))).map_or(0, |k| k + 1);
            let c = costs[d].get_or_insert_with(|| est.costs(f, d, &preds));
            for ii in (lo..=hi).rev() {
                if best.is_none_or(|x| (c[ii], dist) < (x.0, x.1)) {
                    best = Some((c[ii], dist, d, ii));
                }
                dist += 1;
            }
            if lo > 0 || idom[d] == d || idom[d] == usize::MAX {
                break;
            }
            d = idom[d];
            hi = f.blocks[d].insts.len();
        }
        let (_, _, b, ii) = best.expect("the point that needs the exit");
        pts[b].push(ii);
        if site {
            st.site_exits += 1;
            sites += 1;
        } else {
            st.call_exits += 1;
        }
    }
    drop((est, costs, out));
    st.covered = order.iter().filter(|&&b| test[b].is_some()).count() as u32 - sites;
    // each exit's block splits before its point (from the last point of a block back: the tail
    // takes the block's term); its hit and its way into the patched path are the tail:
    // (exit's block, its hit, the block whose copy it enters)
    let mut points: Vec<(usize, usize)> = (0..nb).flat_map(|b| pts[b].iter().map(move |&ii| (b, ii))).collect();
    points.sort_unstable();
    points.dedup();
    let mut exits: Vec<(usize, u32, u32)> = Vec::new();
    let mut term_at: Vec<u32> = (0..nb as u32).collect();
    let mut k = 0;
    while k < points.len() {
        let b = points[k].0;
        let j = k + points[k..].iter().take_while(|x| x.0 == b).count();
        let mut prev: Option<usize> = None;
        for &(_, ii) in points[k..j].iter().rev() {
            let t = f.blocks.len() as u32;
            let blk = &mut f.blocks[b];
            let insts = blk.insts.split_off(ii);
            let pos = blk.pos.split_off(ii.min(blk.pos.len()));
            let term = std::mem::replace(&mut blk.term, Term::Jump(t));
            let tail = Block { insts, pos, term, term_pos: blk.term_pos, count: blk.count };
            f.blocks.push(tail);
            if term_at[b] == b as u32 {
                term_at[b] = t;
            }
            if let Some(p) = prev {
                exits[p].0 = t as usize;
            }
            exits.push((b, t, t));
            prev = Some(exits.len() - 1);
        }
        k = j;
    }
    test.resize(f.blocks.len(), None);
    for b in 0..nb {
        let t = term_at[b] as usize;
        if t != b {
            test[t] = test[b].take();
        }
    }
    // the patched path: what the exits' targets reach, a copy version's compare going straight
    // to its flags' test
    let nb2 = f.blocks.len();
    let version = |test: &[Option<(u32, u32)>], b: usize| test[b].is_some_and(|(_, e)| test[e as usize].is_some());
    let mut newidx = vec![u32::MAX; nb2];
    let mut region: Vec<usize> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for &(_, _, e) in &exits {
        if newidx[e as usize] == u32::MAX {
            newidx[e as usize] = 0;
            stack.push(e as usize);
        }
        while let Some(b) = stack.pop() {
            region.push(b);
            let next: Vec<u32> = if version(&test, b) { vec![test[b].unwrap().1] } else { f.blocks[b].term.succs().collect() };
            for s in next {
                if newidx[s as usize] == u32::MAX {
                    newidx[s as usize] = 0;
                    stack.push(s as usize);
                }
            }
        }
    }
    region.sort_unstable();
    let base = nb2;
    for (k, &b) in region.iter().enumerate() {
        newidx[b] = (base + k) as u32;
    }
    // fresh registers for every one the patched path names
    let nv0 = f.vregs.len();
    let mut need = vec![false; nv0];
    let mut tu = Vec::new();
    for &b in &region {
        for i in &f.blocks[b].insts {
            for_defs(i, |v| need[v.0 as usize] = true);
            for_uses(i, |v| need[v.0 as usize] = true);
        }
        if !version(&test, b) {
            term_uses(&f.blocks[b].term, &mut tu);
            for v in &tu {
                need[v.0 as usize] = true;
            }
        }
    }
    let mut vmap = vec![u32::MAX; nv0];
    let mut inv: Vec<u32> = Vec::new();
    for v in 0..nv0 {
        if need[v] {
            let cls = f.vregs[v];
            vmap[v] = f.vreg(cls).0;
            inv.push(v as u32);
        }
    }
    let cold = |c: f32| if c >= 0.0 { 0.0 } else { -1.0 };
    {
        let m = |v: VReg| VReg(vmap[v.0 as usize]);
        let mut copied = Vec::with_capacity(region.len());
        for &b in &region {
            let src = &f.blocks[b];
            let insts: Vec<Inst> = src.insts.iter().map(|i| map_inst(i, &m, 0)).collect();
            let term = if version(&test, b) {
                Term::Jump(newidx[test[b].unwrap().1 as usize])
            } else {
                let mut t = src.term.clone();
                term_vregs_mut(&mut t, |v| *v = m(*v));
                t.map_succs(|s| newidx[s as usize]);
                t
            };
            st.insts += insts.len() as u32;
            // (its positions carry POS_PATCHED: the pc map tells a frame here from one in the fast half)
            let pos = src.pos.iter().map(|&p| patched_pos(p)).collect();
            copied.push(Block { insts, pos, term, term_pos: patched_pos(src.term_pos), count: cold(src.count) });
        }
        f.blocks.extend(copied);
        for k in 0..f.nonnull.len() {
            let v = f.nonnull[k];
            if vmap[v.0 as usize] != u32::MAX {
                f.nonnull.push(m(v));
            }
        }
        for k in 0..f.ranged.len() {
            let (v, n, lo, hi) = f.ranged[k];
            if vmap[v.0 as usize] != u32::MAX {
                f.ranged.push((m(v), n, lo, hi));
            }
        }
        for k in 0..f.halves.len() {
            let (a, b) = f.halves[k];
            if vmap[a.0 as usize] != u32::MAX && vmap[b.0 as usize] != u32::MAX {
                f.halves.push((m(a), m(b)));
            }
        }
    }
    st.blocks = region.len() as u32;
    // the patched path makes the slot addresses and constants it uses where it uses them
    // (`Remake`): no exit carries one. An instruction that defines one (a copy of an address)
    // is left in the patched path, dead: every use of what it defines is remade too, its own
    // operand included, so no exit block moves that address (`verify_patched_path` checks the
    // exit blocks' moves) and the pipeline's dce drops it
    {
        let rk = |v: VReg| -> Option<&Remake> {
            let x = (v.0 as usize).checked_sub(nv0)?;
            remake.get(*inv.get(x)? as usize)?.as_ref()
        };
        for b in base..base + region.len() {
            let insts = std::mem::take(&mut f.blocks[b].insts);
            let pos = std::mem::take(&mut f.blocks[b].pos);
            let tpos = f.blocks[b].term_pos;
            let mut ni: Vec<Inst> = Vec::with_capacity(insts.len());
            let mut npos: Vec<u64> = Vec::with_capacity(insts.len());
            let mut map: Vec<(VReg, VReg)> = Vec::new();
            for (k, i) in insts.into_iter().enumerate() {
                let p = pos.get(k).copied().unwrap_or(tpos);
                map.clear();
                for_uses(&i, |u| {
                    if rk(u).is_some() && !map.iter().any(|m| m.0 == u) {
                        map.push((u, u));
                    }
                });
                for m in map.iter_mut() {
                    let cls = f.vregs[m.0 .0 as usize];
                    m.1 = remake_into(f, rk(m.0).expect("a remade value"), cls, &mut ni, &mut npos, p);
                }
                ni.push(if map.is_empty() { i } else { map_inst(&i, &|v| map.iter().find(|m| m.0 == v).map_or(v, |m| m.1), 0) });
                npos.push(p);
            }
            let mut term = std::mem::replace(&mut f.blocks[b].term, Term::Unreachable);
            map.clear();
            term_vregs_mut(&mut term, |v| {
                if rk(*v).is_some() && !map.iter().any(|m| m.0 == *v) {
                    map.push((*v, *v));
                }
            });
            for m in map.iter_mut() {
                let cls = f.vregs[m.0 .0 as usize];
                m.1 = remake_into(f, rk(m.0).expect("a remade value"), cls, &mut ni, &mut npos, tpos);
            }
            term_vregs_mut(&mut term, |v| {
                if let Some(m) = map.iter().find(|m| m.0 == *v) {
                    *v = m.1;
                }
            });
            let blk = &mut f.blocks[b];
            blk.term = term;
            blk.insts = ni;
            blk.pos = npos;
        }
    }
    // what is live into each exit's target, in the patched path's own registers
    let np = region.len();
    let pv = inv.len();
    let words = pv.div_ceil(64);
    let pblocks: Vec<usize> = (base..base + np).collect();
    let mut at = vec![u32::MAX; f.blocks.len()];
    for (k, &b) in pblocks.iter().enumerate() {
        at[b] = k as u32;
    }
    let (gen, kill) = gen_kill(f, &pblocks, &|v| (v.0 as usize).checked_sub(nv0).filter(|&x| x < pv).map_or(u32::MAX, |x| x as u32), words);
    let live_in = liveness_in(f, &pblocks, &at, words, &gen, &kill);
    drop((gen, kill, at));
    // each exit: its block of moves into the target, and the word's read at the end of its block
    let x0 = f.blocks.len();
    let mut is_head = vec![false; x0];
    // (each exit's block in turn: its exit's block)
    let mut xhead: Vec<usize> = Vec::with_capacity(exits.len());
    for &(head, hit, entry) in &exits {
        let e2 = newidx[entry as usize] as usize;
        let ek = e2 - base;
        // (an exit before a call: at the call)
        let p = if hit == entry { f.blocks[hit as usize].pos.first().copied().unwrap_or(f.blocks[head].term_pos) } else { f.blocks[head].term_pos };
        let mut insts = Vec::new();
        for x in 0..pv {
            if live_in[ek * words + x / 64] >> (x % 64) & 1 != 0 {
                insts.push(Inst::Mov(VReg((nv0 + x) as u32), VReg(inv[x])));
            }
        }
        let n = insts.len();
        let xb = f.blocks.len() as u32;
        let count = cold(f.blocks[head].count);
        f.blocks.push(Block { insts, pos: vec![patched_pos(p); n], term: Term::Jump(e2 as u32), term_pos: patched_pos(p), count });
        xhead.push(head);
        st.moved += n as u32;
        let (a, w, h) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        let blk = &mut f.blocks[head];
        blk.insts.extend([Inst::Addr(a, word), Inst::AtomicLoad(Mem::Int(8, false), AtomOrd::Relaxed, w, a), Inst::ICmpI(Cond::Eq, false, h, w, ver as i64)]);
        blk.pos.extend([p; 3]);
        blk.term = Term::Branch(h, hit, xb);
        is_head[head] = true;
    }
    drop(live_in);
    // the fast half's other tests go on to their copies
    for b in 0..base {
        if let Some((t, _)) = test[b] {
            if !is_head[b] {
                f.blocks[b].term = Term::Jump(t);
            }
        }
    }
    crate::loops::clear_unreachable(f);
    // the slots: a copy in the patched path for one both halves use (by its address there, or one
    // carried across) whose address the fast half keeps to itself, its cells the target reads
    // filled at each exit
    let fast: Vec<bool> = (0..f.blocks.len()).map(|b| b < base).collect();
    let t = slot_taint(f, &fast);
    let ns = f.slots.len();
    let (mut in_fast, mut in_pat_slot) = (vec![false; ns], vec![false; ns]);
    for (bi, b) in f.blocks.iter().enumerate().take(base + np) {
        for i in &b.insts {
            if let Inst::SlotAddr(_, s) = i {
                if bi < base { in_fast[*s as usize] = true } else { in_pat_slot[*s as usize] = true }
            }
        }
    }
    let mut escaped = t.escaped.clone();
    for b in &f.blocks[x0..] {
        for i in &b.insts {
            if let Inst::Mov(_, v) = *i {
                let s = t.slot[v.0 as usize];
                if s != NONE && s != MIXED {
                    in_pat_slot[s as usize] = true;
                    // (remade from the copy only at a known offset)
                    escaped[s as usize] |= t.off[v.0 as usize].is_none() || t.other[v.0 as usize];
                }
            }
        }
    }
    let mut smap = vec![u32::MAX; ns];
    let mut cloned: Vec<u32> = Vec::new();
    for s in 0..ns {
        if !(in_fast[s] && in_pat_slot[s]) {
            continue;
        }
        if escaped[s] {
            st.shared += 1;
            continue;
        }
        smap[s] = f.slots.len() as u32;
        let c = Slot { born: false, ..f.slots[s].clone() };
        f.slots.push(c);
        cloned.push(s as u32);
    }
    st.cloned = cloned.len() as u32;
    if !cloned.is_empty() {
        for b in &mut f.blocks[base..base + np] {
            for i in &mut b.insts {
                if let Inst::SlotAddr(_, s) = i {
                    if smap[*s as usize] != u32::MAX {
                        *s = smap[*s as usize];
                    }
                }
            }
        }
        let nc = cloned.len();
        // a copy's index among them, by the copy's slot
        let mut cof = vec![u32::MAX; f.slots.len()];
        for (k, &s) in cloned.iter().enumerate() {
            cof[smap[s as usize] as usize] = k as u32;
        }
        // the exits' blocks: an address of a copied slot carried across is the copy's, at the
        // same offset (remade there)
        for xb in x0..f.blocks.len() {
            if !matches!(f.blocks[xb].term, Term::Jump(_)) {
                continue;
            }
            let mut moves = Vec::new();
            for i in std::mem::take(&mut f.blocks[xb].insts) {
                let Inst::Mov(d, v) = i else { unreachable!("an exit block holds moves") };
                let s = t.slot[v.0 as usize];
                if s == NONE || s == MIXED || smap[s as usize] == u32::MAX {
                    moves.push(i);
                    continue;
                }
                let (off, ty) = t.off[v.0 as usize].expect("a copied slot's address carried across has a known offset");
                if off == 0 {
                    moves.push(Inst::SlotAddr(d, smap[s as usize]));
                } else {
                    let a = f.vreg(Cls::I);
                    moves.extend([Inst::SlotAddr(a, smap[s as usize]), Inst::IBinI(IOp::Add, ty, d, a, off)]);
                }
            }
            let p = f.blocks[xb].term_pos;
            f.blocks[xb].pos = vec![p; moves.len()];
            f.blocks[xb].insts = moves;
        }
        // which bytes of the copies the patched path reads before it writes them, live into each
        // of its blocks: an exit fills only those (a byte it does not read keeps no fast cell live)
        let mut bit0 = vec![0usize; nc + 1];
        for (k, &s) in cloned.iter().enumerate() {
            bit0[k + 1] = bit0[k] + f.slots[s as usize].size as usize;
        }
        let words = bit0[nc].div_ceil(64);
        let pmask: Vec<bool> = (0..f.blocks.len()).map(|b| b >= base && b < base + np || b >= x0).collect();
        let tp = slot_taint(f, &pmask);
        let pbl: Vec<usize> = (base..base + np).collect();
        let mut at = vec![u32::MAX; f.blocks.len()];
        for (k, &b) in pbl.iter().enumerate() {
            at[b] = k as u32;
        }
        let (gen, kill) = byte_gen_kill(f, &pbl, &ByteMap { f, tp: &tp, cof: &cof, bit0: &bit0 }, words);
        let live = liveness_in(f, &pbl, &at, words, &gen, &kill);
        drop((gen, kill, at, pbl));
        // and the bytes the fast half may have written on a way to each exit: the others hold no
        // value yet (padding, a buffer not filled), so no exit fills them
        let mut fof = vec![u32::MAX; f.slots.len()];
        for (k, &s) in cloned.iter().enumerate() {
            fof[s as usize] = k as u32;
        }
        let fsucc: Vec<Vec<u32>> = (0..f.blocks.len()).map(|b| if b < base { f.blocks[b].term.succs().filter(|&x| (x as usize) < base).collect() } else { Vec::new() }).collect();
        let (forder, _) = rpo(&fsucc, &[0]);
        let fpreds = preds_in(&fsucc, &forder);
        let written = may_written(f, &forder, &fpreds, &ByteMap { f, tp: &t, cof: &fof, bit0: &bit0 }, words);
        drop((fsucc, forder, fpreds, fof));
        // the fast half's cells of each copied slot (its typed accesses at known offsets), whether
        // it has others (an indexed or atomic access: it stays in memory, copied whole) and
        // whether a copy fills it
        let mut cells: Vec<Vec<(i64, Option<Mem>)>> = vec![Vec::new(); nc];
        let mut untyped = vec![false; nc];
        let mut copied_in = vec![false; nc];
        {
            let ci = |v: VReg| -> Option<usize> {
                let s = t.slot[v.0 as usize];
                (s != NONE && s != MIXED && smap[s as usize] != u32::MAX).then(|| cof[smap[s as usize] as usize] as usize)
            };
            for b in &f.blocks[..base] {
                for i in &b.insts {
                    let (a, off, m) = match *i {
                        Inst::Load(m, _, a, off) | Inst::Store(m, a, off, _) => (a, off, Some(m)),
                        Inst::VLoad(_, a, off) | Inst::VStore(a, off, _) => (a, off, None),
                        Inst::Copy(d, _, _) => {
                            if let Some(c) = ci(d) {
                                copied_in[c] = true;
                            }
                            continue;
                        }
                        Inst::LoadX(_, _, a, ..) | Inst::StoreX(_, a, ..) | Inst::VLoadX(_, a, ..) | Inst::VStoreX(a, ..) | Inst::AtomicLoad(_, _, _, a) | Inst::AtomicStore(_, _, a, _) | Inst::AtomicRmw(_, _, _, _, a, _) | Inst::AtomicCas(_, _, _, _, a, _, _) => {
                            if let Some(c) = ci(a) {
                                untyped[c] = true;
                            }
                            continue;
                        }
                        _ => continue,
                    };
                    let Some(c) = ci(a) else { continue };
                    match t.off[a.0 as usize] {
                        Some((o, _)) => cells[c].push((o + off as i64, m)),
                        None => untyped[c] = true,
                    }
                }
            }
        }
        for c in cells.iter_mut() {
            c.sort_unstable_by_key(|&(o, m)| (o, m.map_or(0, |m| relax_backend::passes::mem_size(m))));
            c.dedup();
        }
        let has = |w: &[u64], k: usize| w[k / 64] >> (k % 64) & 1 != 0;
        for xb in x0..f.blocks.len() {
            let Term::Jump(e2) = f.blocks[xb].term else { continue };
            let p = f.blocks[xb].term_pos;
            let ek = e2 as usize - base;
            let h = xhead[xb - x0];
            let lw: Vec<u64> = live[ek * words..(ek + 1) * words].iter().zip(&written[h * words..(h + 1) * words]).map(|(l, w)| l & w).collect();
            let mut fill = Vec::new();
            for (c, &s) in cloned.iter().enumerate() {
                let size = f.slots[s as usize].size as usize;
                let lb: Vec<bool> = (0..size).map(|k| has(&lw, bit0[c] + k)).collect();
                let Some(first) = lb.iter().position(|&x| x) else { continue };
                st.filled += 1;
                st.fill_bytes += lb.iter().filter(|&&x| x).count() as u32;
                let last = lb.iter().rposition(|&x| x).unwrap_or(first);
                let (ad, asrc) = (f.vreg(Cls::I), f.vreg(Cls::I));
                fill.extend([Inst::SlotAddr(ad, smap[s as usize]), Inst::SlotAddr(asrc, s)]);
                let mut covered = vec![false; size];
                if !untyped[c] {
                    // a load and a store per cell the patched path reads: the fast slot's cells
                    // stay its registers (a whole copy keeps it in memory: passes::sroa's copy
                    // limits) and only the cells read stay live at the exit
                    for &(o, m) in &cells[c] {
                        let len = m.map_or(16, |m| relax_backend::passes::mem_size(m)) as usize;
                        let (lo, hi) = (o as usize, o as usize + len);
                        if !lb[lo..hi.min(size)].iter().any(|&x| x) {
                            continue;
                        }
                        match m {
                            Some(m) => {
                                let v = f.vreg(match m { Mem::F32 => Cls::F32, Mem::F64 => Cls::F64, Mem::Int(..) => Cls::I });
                                fill.extend([Inst::Load(m, v, asrc, o as i32), Inst::Store(m, ad, o as i32, v)]);
                            }
                            None => {
                                let v = f.vreg(Cls::V128);
                                fill.extend([Inst::VLoad(v, asrc, o as i32), Inst::VStore(ad, o as i32, v)]);
                            }
                        }
                        covered[lo..hi.min(size)].iter_mut().for_each(|x| *x = true);
                    }
                }
                // bytes no cell names: copied as they are when something may have filled them
                // (an untyped access or a copy; the others the fast half never wrote)
                let mut k = first;
                while k <= last {
                    if !lb[k] || covered[k] || !(untyped[c] || copied_in[c]) {
                        k += 1;
                        continue;
                    }
                    let lo = k;
                    while k <= last && lb[k] && !covered[k] {
                        k += 1;
                    }
                    if lo == 0 {
                        fill.push(Inst::Copy(ad, asrc, (k - lo) as u32));
                    } else {
                        let (d2, s2) = (f.vreg(Cls::I), f.vreg(Cls::I));
                        fill.extend([Inst::IBinI(IOp::Add, U64, d2, ad, lo as i64), Inst::IBinI(IOp::Add, U64, s2, asrc, lo as i64), Inst::Copy(d2, s2, (k - lo) as u32)]);
                    }
                }
            }
            fill.extend(std::mem::take(&mut f.blocks[xb].insts));
            let n = fill.len();
            let blk = &mut f.blocks[xb];
            blk.insts = fill;
            blk.pos = vec![p; n];
        }
    }
    drop_dead_reads(f);
    f.patched_path = true;
    if relax_backend::rir::verify_passes_on() {
        if let Err(e) = verify_patched_path(f) {
            panic!("relax: patched path broken in {}: {}\n{}", f.name, e, f.dump());
        }
    }
    Some(st)
}

/// The exits of a split body (`split_patched_path`) and what they lead to: per block, whether it is
/// in the fast half, an exit's block, or in the patched path.
fn halves(f: &Func) -> (Vec<Option<(u32, u32, usize)>>, Vec<bool>, Vec<bool>, Vec<bool>) {
    let nb = f.blocks.len();
    let ex = exits_of(f);
    let succ = fast_succs(f, &ex);
    let (order, _) = rpo(&succ, &[0]);
    let mut fast = vec![false; nb];
    for b in order {
        fast[b] = true;
    }
    let mut xb = vec![false; nb];
    let mut entries = Vec::new();
    for b in 0..nb {
        if let (true, Some((_, miss, _))) = (fast[b], ex[b]) {
            xb[miss as usize] = true;
            entries.extend(f.blocks[miss as usize].term.succs());
        }
    }
    let all: Vec<Vec<u32>> = f.blocks.iter().map(|b| b.term.succs().collect()).collect();
    let (pat_order, _) = rpo(&all, &entries);
    let mut pat = vec![false; nb];
    for b in pat_order {
        pat[b] = true;
    }
    (ex, fast, xb, pat)
}

/// The split's invariants, after it (and in its tests): the halves are apart (the only edges into
/// the patched path leave from exits' blocks, which only exits enter); the fast half holds no
/// guard's slow call; no register is named in both; an exit's block moves (or remakes) every
/// register live into its target and fills the slots the patched path holds a copy of, whose
/// address never crosses.
pub fn verify_patched_path(f: &Func) -> Result<(), String> {
    let (ex, fast, xb, pat) = halves(f);
    let nb = f.blocks.len();
    for b in 0..nb {
        if fast[b] && (xb[b] || pat[b]) || xb[b] && pat[b] {
            return Err(format!("block {} is in two halves (fast {} exit {} patched {})", b, fast[b], xb[b], pat[b]));
        }
        if fast[b] && (0..f.blocks[b].insts.len()).any(|ii| f.guard_call(b, ii)) {
            return Err(format!("the fast half's block {} holds a guard's slow call", b));
        }
        if (fast[b] || xb[b] || pat[b]) && f.blocks[b].pos.iter().chain([&f.blocks[b].term_pos]).any(|&p| p != 0 && pos_patched(p) != !fast[b]) {
            return Err(format!("block {} has a position {} the patched path", b, if fast[b] { "marked in" } else { "not marked in" }));
        }
        if xb[b] {
            let Term::Jump(t) = f.blocks[b].term else { return Err(format!("exit block {} does not jump", b)) };
            if !pat[t as usize] {
                return Err(format!("exit block {} jumps to {}, not into the patched path", b, t));
            }
            if f.blocks[b].insts.iter().any(|i| !matches!(i, Inst::Mov(..) | Inst::SlotAddr(..) | Inst::IBinI(IOp::Add, ..) | Inst::Copy(..) | Inst::Load(..) | Inst::Store(..) | Inst::VLoad(..) | Inst::VStore(..))) {
                return Err(format!("exit block {} holds more than moves and slot copies", b));
            }
        }
    }
    if !ex.iter().enumerate().any(|(b, e)| fast[b] && e.is_some()) {
        return Ok(());
    }
    let nv = f.vregs.len();
    let mut seen = vec![0u8; nv];
    let mut tu = Vec::new();
    for b in 0..nb {
        let side = if fast[b] { 1 } else if pat[b] { 2 } else { continue };
        let blk = &f.blocks[b];
        let mut bad = None;
        let mut note = |v: VReg| {
            seen[v.0 as usize] |= side;
            if seen[v.0 as usize] == 3 {
                bad = Some(v);
            }
        };
        for i in &blk.insts {
            for_defs(i, &mut note);
            for_uses(i, &mut note);
        }
        term_uses(&blk.term, &mut tu);
        for &v in &tu {
            note(v);
        }
        if let Some(v) = bad {
            return Err(format!("register {:?} is named in both halves (block {})", v, b));
        }
    }
    // slots: a copied one is the fast half's only, its copy the patched path's only
    let ns = f.slots.len();
    let mut addressed = vec![0u8; ns];
    for b in 0..nb {
        let side = if fast[b] { 1 } else if pat[b] { 2 } else { continue };
        for i in &f.blocks[b].insts {
            if let Inst::SlotAddr(_, s) = i {
                addressed[*s as usize] |= side;
            }
        }
    }
    let t = slot_taint(f, &fast);
    let mut copied = vec![false; ns];
    for b in (0..nb).filter(|&b| xb[b]) {
        let blk = &f.blocks[b];
        // (an address: its slot and offset; a loaded value: the slot and offset it came from)
        let mut slot_of = std::collections::HashMap::new();
        let mut loaded = std::collections::HashMap::new();
        let pair = |ss: u32, sd: u32| addressed[ss as usize] & 2 == 0 && addressed[sd as usize] & 1 == 0 && f.slots[ss as usize].size == f.slots[sd as usize].size;
        for i in &blk.insts {
            match *i {
                Inst::SlotAddr(a, s) => {
                    slot_of.insert(a, (s, 0i64));
                }
                Inst::IBinI(IOp::Add, _, d, a, k) => {
                    if let Some(&(s, o)) = slot_of.get(&a) {
                        slot_of.insert(d, (s, o + k));
                    }
                }
                Inst::Copy(d, s, n) => {
                    let (Some(&(sd, od)), Some(&(ss, os))) = (slot_of.get(&d), slot_of.get(&s)) else { return Err(format!("exit block {}: a copy not between slots", b)) };
                    if !pair(ss, sd) || od != os || os < 0 || os + n as i64 > f.slots[ss as usize].size as i64 {
                        return Err(format!("exit block {}: slot {} copied to {} is not a fast slot and its patched copy", b, ss, sd));
                    }
                    copied[ss as usize] = true;
                }
                Inst::Load(_, v, a, off) | Inst::VLoad(v, a, off) => {
                    let Some(&(ss, os)) = slot_of.get(&a) else { return Err(format!("exit block {}: a load not of a slot", b)) };
                    loaded.insert(v, (ss, os + off as i64));
                }
                Inst::Store(_, a, off, v) | Inst::VStore(a, off, v) => {
                    let (Some(&(sd, od)), Some(&(ss, os))) = (slot_of.get(&a), loaded.get(&v)) else { return Err(format!("exit block {}: a store not of a slot's loaded cell", b)) };
                    if !pair(ss, sd) || od + off as i64 != os {
                        return Err(format!("exit block {}: a cell of slot {} stored to {} is not a fast slot's cell into its patched copy", b, ss, sd));
                    }
                    copied[ss as usize] = true;
                }
                _ => {}
            }
        }
    }
    for b in (0..nb).filter(|&b| xb[b]) {
        for i in &f.blocks[b].insts {
            if let Inst::Mov(_, v) = *i {
                let s = t.slot[v.0 as usize];
                if s != NONE && s != MIXED && copied[s as usize] {
                    return Err(format!("exit block {}: the address of copied slot {} crosses ({:?})", b, s, v));
                }
            }
        }
    }
    // registers live into an exit's target: each defined in the exit's block
    let mut idx = vec![u32::MAX; nv];
    let mut n = 0usize;
    for v in 0..nv {
        if seen[v] == 2 {
            idx[v] = n as u32;
            n += 1;
        }
    }
    let words = n.div_ceil(64);
    let pblocks: Vec<usize> = (0..nb).filter(|&b| pat[b]).collect();
    let mut at = vec![u32::MAX; nb];
    for (k, &b) in pblocks.iter().enumerate() {
        at[b] = k as u32;
    }
    let (gen, kill) = gen_kill(f, &pblocks, &|v| idx[v.0 as usize], words);
    let live_in = liveness_in(f, &pblocks, &at, words, &gen, &kill);
    for b in (0..nb).filter(|&b| xb[b]) {
        let Term::Jump(t) = f.blocks[b].term else { continue };
        let mut defd = vec![false; n];
        for i in &f.blocks[b].insts {
            for_defs(i, |v| {
                if idx[v.0 as usize] != u32::MAX {
                    defd[idx[v.0 as usize] as usize] = true;
                }
            });
        }
        for x in 0..n {
            if live_in[at[t as usize] as usize * words + x / 64] >> (x % 64) & 1 != 0 && !defd[x] {
                let v = (0..nv).find(|&v| idx[v] == x as u32).unwrap_or(0);
                return Err(format!("exit block {} leaves v{} live into block {} undefined", b, v, t));
            }
        }
    }
    Ok(())
}

/// A split body's exits that an earlier one covers (it ran on every path since a frame last may
/// have waited) become jumps to their hits: the share pass of a split body (`opt::share_copy_ver_reads`),
/// after the second inlining round (a copy it took in left no call between two exits) and after
/// full unrolling (a constant loop's back edge is gone). Their blocks into the patched path, and
/// what only they reach of it, go. The number deleted.
pub fn share_exits(f: &mut Func) -> u32 {
    let ex = exits_of(f);
    if ex.iter().all(|e| e.is_none()) {
        return 0;
    }
    let nb = f.blocks.len();
    let succ = fast_succs(f, &ex);
    let (order, rank) = rpo(&succ, &[0]);
    let preds = preds_in(&succ, &order);
    let mut out = vec![false; nb];
    let mut gone: Vec<usize> = Vec::new();
    for &b in &order {
        let mut ran = ran_at(b, &preds, &rank, &out);
        for ii in 0..f.blocks[b].insts.len() {
            if ex[b].is_some_and(|e| e.2 == ii) {
                if ran {
                    gone.push(b);
                }
                ran = true;
            }
            if is_wait(f, b, ii) {
                ran = false;
            }
        }
        out[b] = ran;
    }
    for &b in &gone {
        let (hit, _, _) = ex[b].expect("an exit");
        f.blocks[b].term = Term::Jump(hit);
    }
    if !gone.is_empty() {
        crate::loops::clear_unreachable(f);
        drop_dead_reads(f);
    }
    gone.len() as u32
}

/// After the second inlining round: the exits whose stretch (from the exit on, to the calls and
/// back edges where a frame may wait) holds no instruction of a copy a patch must reach (an inline
/// context of a slot `patchable` names, or inside one) become jumps. An exit before a call the
/// round did not take is one. The number deleted.
pub fn prune_exits(f: &mut Func, patchable: &dyn Fn(u32) -> bool) -> u32 {
    let ex = exits_of(f);
    if ex.iter().all(|e| e.is_none()) {
        return 0;
    }
    let nb = f.blocks.len();
    let nc = f.inl.len();
    let mut pc = vec![false; nc + 1];
    for c in 1..=nc {
        let parent = pos_ctx(f.inl[c - 1].0) as usize;
        pc[c] = f.inl_slots.get(c - 1).is_none_or(|&s| patchable(s)) || parent >= c || pc[parent];
    }
    let copy = |p: u64| {
        let c = pos_ctx(p) as usize;
        c != 0 && (c > nc || pc[c])
    };
    let succ = fast_succs(f, &ex);
    let (_, rank) = rpo(&succ, &[0]);
    let mut stamp = vec![0u32; nb];
    let mut gone: Vec<usize> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for b in 0..nb {
        let Some((hit, _, _)) = ex[b] else { continue };
        if rank[b] == usize::MAX {
            continue;
        }
        let mark = b as u32 + 1;
        let mut holds = false;
        stack.clear();
        stack.push(hit as usize);
        stamp[hit as usize] = mark;
        'walk: while let Some(x) = stack.pop() {
            let blk = &f.blocks[x];
            for ii in 0..blk.insts.len() {
                if copy(blk.pos.get(ii).copied().unwrap_or(0)) {
                    holds = true;
                    break 'walk;
                }
                if is_wait(f, x, ii) {
                    continue 'walk;
                }
            }
            if copy(blk.term_pos) {
                holds = true;
                break;
            }
            for &s in &succ[x] {
                if rank[s as usize] > rank[x] && stamp[s as usize] != mark {
                    stamp[s as usize] = mark;
                    stack.push(s as usize);
                }
            }
        }
        if !holds {
            gone.push(b);
        }
    }
    for &b in &gone {
        let (hit, _, _) = ex[b].expect("an exit");
        f.blocks[b].term = Term::Jump(hit);
    }
    if !gone.is_empty() {
        crate::loops::clear_unreachable(f);
        drop_dead_reads(f);
    }
    gone.len() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opt::{inline_calls, inline_calls_ordered, InlineLimits, InlineStats, ATTR_ALWAYS, WHY_NO_EXIT, WHY_PATCHED_PATH};

    const FLAG: Sym = Sym { id: 9, off: 16 };
    const SIX_FLAG: Sym = Sym { id: 10, off: 16 };
    const WORD: Sym = Sym { id: 11, off: 24 };
    const M8: Mem = Mem::Int(8, false);

    /// `fn(x) -> x + 1 + 3` (positions in its own body), patchable by `flag`
    fn leaf(name: &str, flag: Option<Sym>) -> Func {
        let mut f = Func::new(name.into(), 0);
        let (x, y) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.push(x);
        f.rets.push(Cls::I);
        f.block();
        f.blocks[0].insts = vec![Inst::IBinI(IOp::Add, I64, y, x, 1), Inst::IBinI(IOp::Add, I64, y, y, 3)];
        f.blocks[0].pos = vec![1, 1];
        f.blocks[0].term = Term::Ret(vec![y]);
        f.blocks[0].term_pos = 1;
        f.patch_flag = flag;
        f
    }

    /// `fn(x) -> leaf(x)`: not patchable, but its body calls a patchable one
    fn mid() -> Func {
        let mut f = Func::new("mid".into(), 0);
        let x = f.vreg(Cls::I);
        f.params.push(x);
        f.rets.push(Cls::I);
        f.block();
        f.blocks[0].insts = vec![Inst::Call(Callee::Fn(1), vec![x], vec![x])];
        f.blocks[0].pos = vec![2];
        f.blocks[0].term = Term::Ret(vec![x]);
        f.blocks[0].term_pos = 2;
        f
    }

    /// A live session's root build: `v = 0; loop { v = calls(v)...; if !c { break } }; v`, with
    /// `before` at the end of the entry block and `each` (and the calls) in the loop
    fn root(before: Vec<Inst>, calls: &[u32], each: Vec<Inst>, v: VReg, f: &mut Func) {
        root_head(before, Vec::new(), calls, each, v, f);
    }

    /// `root`, with `head` in the loop before the calls
    /// A patchable leaf that returns its result in memory: it writes both words of the buffer its
    /// first argument names.
    fn ret_leaf() -> Func {
        let mut f = Func::new("ret_leaf".into(), 0);
        let (r, x) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.extend([r, x]);
        f.block();
        f.blocks[0].insts = vec![Inst::Store(M8, r, 0, x), Inst::Store(M8, r, 8, x)];
        f.blocks[0].pos = vec![1, 1];
        f.blocks[0].term = Term::Ret(Vec::new());
        f.blocks[0].term_pos = 1;
        f.sret = Some(r);
        f.patch_flag = Some(FLAG);
        f
    }
    fn root_head(before: Vec<Inst>, head: Vec<Inst>, calls: &[u32], each: Vec<Inst>, v: VReg, f: &mut Func) {
        for _ in 0..3 {
            f.block();
        }
        let c = f.params[0];
        f.blocks[0].insts = [Inst::Iconst(v, 0)].into_iter().chain(before).collect();
        f.blocks[0].pos = vec![9; f.blocks[0].insts.len()];
        f.blocks[0].term = Term::Jump(1);
        let mut insts = head;
        insts.extend(calls.iter().map(|&id| Inst::Call(Callee::Fn(id), vec![v], vec![v])));
        insts.extend(each);
        f.blocks[1].pos = (0..insts.len() as u64).map(|k| 10 + k).collect();
        f.blocks[1].insts = insts;
        f.blocks[1].term = Term::Branch(c, 1, 2);
        f.blocks[2].term = Term::Ret(vec![v]);
        f.copy_ver = Some((WORD, 3));
    }

    fn new_root() -> (Func, VReg) {
        let mut f = Func::new("root".into(), 0);
        let (c, v) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.push(c);
        f.rets.push(Cls::I);
        (f, v)
    }

    fn count(f: &Func, blocks: &[bool], p: impl Fn(&Inst) -> bool) -> usize {
        f.blocks.iter().enumerate().filter(|(b, _)| blocks[*b]).flat_map(|(_, b)| &b.insts).filter(|i| p(i)).count()
    }

    fn guard_calls_in(f: &Func, blocks: &[bool]) -> usize {
        (0..f.blocks.len()).filter(|&b| blocks[b]).map(|b| (0..f.blocks[b].insts.len()).filter(|&ii| f.guard_call(b, ii)).count()).sum()
    }

    #[test]
    fn a_split_body_exits_once_per_stretch_into_its_patched_path() {
        // three guarded copies in a loop, a call between the second and the third: the first
        // copy of each stretch (the loop's head, after the call) becomes an exit, the second one
        // jumps to its copy; the patched path holds all three guards (each reads its flag, then
        // calls the slot) and no exit; the fast half holds no guard, no slow call, no flag read
        let options = &relax_session::Options::default();
        let lf = leaf("leaf", Some(FLAG));
        let get = |id: u32| (id == 1).then_some((&lf as *const Func, ATTR_ALWAYS));
        let (mut f, v) = new_root();
        root(Vec::new(), &[1, 1, 5, 1], Vec::new(), v, &mut f);
        assert_eq!(inline_calls(options, &mut f, 0, &get, &|_| false, &InlineLimits::tier1()).len(), 3);
        assert_eq!(f.guard_calls.len(), 3, "{}", f.dump());
        let st = split_patched_path(&mut f, &get).expect("a split");
        assert_eq!((st.site_exits, st.covered, st.call_exits), (2, 1, 0), "{:?} {}", st, f.dump());
        f.verify().unwrap();
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
        let (ex, fast, xb, pat) = halves(&f);
        assert_eq!((0..f.blocks.len()).filter(|&b| fast[b] && ex[b].is_some()).count(), 2, "{}", f.dump());
        assert_eq!(guard_calls_in(&f, &fast), 0, "{}", f.dump());
        assert_eq!(guard_calls_in(&f, &pat), 3, "{}", f.dump());
        let addr = |s: Sym| move |i: &Inst| matches!(i, Inst::Addr(_, x) if *x == s);
        assert_eq!((count(&f, &fast, addr(FLAG)), count(&f, &pat, addr(FLAG))), (0, 3), "{}", f.dump());
        assert_eq!((count(&f, &fast, addr(WORD)), count(&f, &pat, addr(WORD))), (2, 0), "{}", f.dump());
        // the exit's block moves the loop's value into the patched path: without it the verifier
        // finds it live into the target undefined
        let mut broken = f.clone();
        let x = (0..broken.blocks.len()).find(|&b| xb[b] && !broken.blocks[b].insts.is_empty()).expect("an exit block with moves");
        broken.blocks[x].insts.clear();
        broken.blocks[x].pos.clear();
        assert!(verify_patched_path(&broken).is_err_and(|e| e.contains("undefined")));
        // a stretch with no wait point between two exits keeps the first: the call between them
        // gone (as full unrolling removes a copy's constant loop), the share pass deletes the second
        for b in 0..f.blocks.len() {
            if fast[b] {
                if let Some(k) = f.blocks[b].insts.iter().position(|i| matches!(i, Inst::Call(Callee::Fn(5), ..))) {
                    f.blocks[b].insts.remove(k);
                    f.blocks[b].pos.remove(k);
                }
            }
        }
        assert_eq!(share_exits(&mut f), 1, "{}", f.dump());
        f.verify().unwrap();
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
        let (ex, fast, _, pat) = halves(&f);
        assert_eq!((0..f.blocks.len()).filter(|&b| fast[b] && ex[b].is_some()).count(), 1, "{}", f.dump());
        assert_eq!(count(&f, &fast, addr(WORD)), 1, "{}", f.dump());
        assert_eq!(guard_calls_in(&f, &pat), 3, "{}", f.dump());
    }

    #[test]
    fn calls_lowered_at_one_place_are_sites_of_their_own() {
        // The front lowers a match guard of an or-pattern once per alternative: calls of one
        // callee at one source place, here `v = leaf(v)` in a loop and again behind a bounds
        // check, `if v < 8 { v = leaf(v) } else { panic }`. Keyed by place and callee alone, the
        // first copy's slow call took the second call for one of its own: the inliner kept it a
        // call, and the split read the bounds check before it as a guard test whose fast side is
        // the panic, so the fast half panicked on every iteration.
        let options = &relax_session::Options::default();
        let lf = leaf("leaf", Some(FLAG));
        let get = |id: u32| (id == 1).then_some((&lf as *const Func, ATTR_ALWAYS));
        let build = |second: bool| {
            let (mut f, v) = new_root();
            root(Vec::new(), &[1], Vec::new(), v, &mut f);
            let (c, k) = (f.params[0], f.vreg(Cls::I));
            let (ok, panic) = (f.block(), f.block());
            f.blocks[1].insts.push(Inst::ICmpI(Cond::Lt, false, k, v, 8));
            f.blocks[1].pos.push(11);
            f.blocks[1].term = Term::Branch(k, ok, panic);
            if second {
                f.blocks[ok as usize].insts = vec![Inst::Call(Callee::Fn(1), vec![v], vec![v])];
                f.blocks[ok as usize].pos = vec![10];
            }
            f.blocks[ok as usize].term = Term::Branch(c, 1, 2);
            f.blocks[panic as usize].insts = vec![Inst::Call(Callee::Fn(7), vec![], vec![])];
            f.blocks[panic as usize].pos = vec![30];
            f.blocks[panic as usize].term = Term::Unreachable;
            (f, v, k, ok as usize)
        };
        let keeps_check = |f: &Func, k: VReg| {
            let (_, fast, _, _) = halves(f);
            (0..f.blocks.len()).any(|b| fast[b] && matches!(f.blocks[b].term, Term::Branch(x, t, e) if x == k && t != e))
        };
        // both calls inlined, each copy with its own guard; the split keeps the bounds check
        let (mut f, _, k, _) = build(true);
        assert_eq!(inline_calls(options, &mut f, 0, &get, &|_| false, &InlineLimits::tier1()).len(), 2, "each call is a site: {}", f.dump());
        assert_eq!(f.guard_calls.len(), 2, "{}", f.dump());
        split_patched_path(&mut f, &get).expect("a split");
        f.verify().unwrap();
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
        assert!(keeps_check(&f, k), "the fast half keeps the bounds check: {}", f.dump());
        // the second call left a call (as an inliner that priced it out leaves it): a plain call,
        // not the copy's slow call, and the bounds check before it stays a branch
        let (mut f, v, k, ok) = build(false);
        assert_eq!(inline_calls(options, &mut f, 0, &get, &|_| false, &InlineLimits::tier1()).len(), 1);
        f.blocks[ok].insts.insert(0, Inst::Call(Callee::Fn(1), vec![v], vec![v]));
        f.blocks[ok].pos.insert(0, 10);
        assert_eq!(f.guard_calls.len(), 1, "{}", f.dump());
        assert!(!f.guard_call(ok, 0), "a plain call at a guarded copy's place is no slow call: {}", f.dump());
        split_patched_path(&mut f, &get).expect("a split");
        f.verify().unwrap();
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
        assert!(keeps_check(&f, k), "the fast half keeps the bounds check before a plain call: {}", f.dump());
    }

    #[test]
    fn exits_fill_the_slots_the_patched_path_copies() {
        // a slot both halves address whose address stays in the fast half is the patched path's
        // own copy: an exit fills the cells the patched path reads before it writes them, a load
        // and a store each (a whole copy out of a slot copied in from memory kept the fast slot in
        // memory: passes::sroa), so the fast slot's cells become registers; the patched path makes
        // the slot addresses it uses (a copy's, or a shared slot's: one whose address a call takes)
        // and the exits carry none; a slot the patched path writes before it reads gets no fill,
        // nor does a result buffer its call writes (`Slot::ret`, `Func::sret_callees`); and the
        // exit sits before the loop builds a temporary, so it fills none of it
        let options = &relax_session::Options::default();
        let lf = leaf("leaf", Some(FLAG));
        let rl = ret_leaf();
        let get = |id: u32| match id {
            1 => Some((&lf as *const Func, ATTR_ALWAYS)),
            2 => Some((&rl as *const Func, ATTR_ALWAYS)),
            _ => None,
        };
        let (mut f, v) = new_root();
        let src = f.vreg(Cls::I);
        f.params.push(src);
        let (s, t, u, x, r) = (f.slot(72, 8), f.slot(8, 8), f.slot(8, 8), f.slot(16, 8), f.slot(16, 8));
        f.slots[r as usize].ret = true;
        f.sret_callees.push(2);
        let (a, b, au, wu, xa, wx) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        let (ra, wr) = (f.vreg(Cls::I), f.vreg(Cls::I));
        let w: Vec<VReg> = (0..9).map(|_| f.vreg(Cls::I)).collect();
        let before = vec![Inst::SlotAddr(a, s), Inst::Copy(a, src, 72), Inst::SlotAddr(b, t), Inst::SlotAddr(au, u), Inst::SlotAddr(xa, x), Inst::SlotAddr(ra, r)];
        let head = vec![Inst::Store(M8, xa, 0, v), Inst::Store(M8, xa, 8, v)];
        let mut each: Vec<Inst> = vec![Inst::Call(Callee::Fn(2), vec![ra, v], vec![]), Inst::Load(M8, wr, ra, 8), Inst::IBin(IOp::Add, I64, v, v, wr)];
        for (k, &y) in w.iter().enumerate() {
            each.extend([Inst::Load(M8, y, a, 8 * k as i32), Inst::IBin(IOp::Add, I64, v, v, y)]);
        }
        each.extend([Inst::Call(Callee::Fn(5), vec![b], vec![]), Inst::Store(M8, au, 0, v), Inst::Load(M8, wu, au, 0), Inst::IBin(IOp::Add, I64, v, v, wu)]);
        each.extend([Inst::Load(M8, wx, xa, 8), Inst::IBin(IOp::Add, I64, v, v, wx)]);
        each.extend((0..9).map(|k| Inst::Store(M8, a, 8 * k, v)));
        root_head(before, head, &[1], each, v, &mut f);
        inline_calls(options, &mut f, 0, &get, &|_| false, &InlineLimits::tier1());
        let st = split_patched_path(&mut f, &get).expect("a split");
        assert_eq!((st.cloned, st.shared), (4, 1), "{:?} {}", st, f.dump());
        assert_eq!((st.filled, st.fill_bytes, st.moved), (1, 72, 2), "{:?} {}", st, f.dump());
        f.verify().unwrap();
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
        let (_, fast, xb, pat) = halves(&f);
        let n = f.slots.len() as u32;
        let (s2, u2, x2, r2) = (n - 4, n - 3, n - 2, n - 1);
        assert_eq!((f.slots[s2 as usize].size, f.slots[x2 as usize].size, f.slots[r2 as usize].ret), (72, 16, true));
        let exit_insts: Vec<&Inst> = (0..f.blocks.len()).filter(|&x| xb[x]).flat_map(|x| &f.blocks[x].insts).collect();
        assert!(!exit_insts.iter().any(|i| matches!(i, Inst::Copy(..))), "cells, not a copy: {}", f.dump());
        assert_eq!((exit_insts.iter().filter(|i| matches!(i, Inst::Load(M8, ..))).count(), exit_insts.iter().filter(|i| matches!(i, Inst::Store(M8, ..))).count()), (9, 9), "a load and a store per cell read, none of the temporary: {}", f.dump());
        let addr_of = |blocks: &[bool], sl: u32| count(&f, blocks, |i| matches!(i, Inst::SlotAddr(_, y) if *y == sl));
        assert_eq!((addr_of(&xb, s2), addr_of(&xb, u2), addr_of(&xb, x2), addr_of(&xb, t), addr_of(&xb, r2)), (1, 0, 0, 0, 0), "the fill's address only: {}", f.dump());
        assert!(addr_of(&pat, u2) > 0 && addr_of(&pat, x2) > 0 && addr_of(&pat, t) > 0 && addr_of(&pat, r2) > 0, "the patched path makes its addresses: {}", f.dump());
        assert!(!exit_insts.iter().any(|i| matches!(i, Inst::Mov(_, y) if [a, b, au, xa].contains(y))), "no address crosses: {}", f.dump());
        // the fast half's slot is promoted: its cells are registers there
        assert!(relax_backend::passes::sroa(&mut f), "{}", f.dump());
        relax_backend::passes::dce(&mut f);
        assert_eq!(count(&f, &fast, |i| matches!(i, Inst::SlotAddr(_, y) if *y == s)), 0, "{}", f.dump());

        // a result buffer whose address the patched path also hands to another call (the leaf,
        // taken in in the fast half) may be kept there: its call reads it as before and the exit
        // fills both of its cells
        let (mut f, v) = new_root();
        let r = f.slot(16, 8);
        f.slots[r as usize].ret = true;
        f.sret_callees.push(2);
        let (ra, wr, wl) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        let each = vec![Inst::Call(Callee::Fn(2), vec![ra, v], vec![]), Inst::Load(M8, wr, ra, 8), Inst::IBin(IOp::Add, I64, v, v, wr), Inst::Call(Callee::Fn(1), vec![ra], vec![wl])];
        root_head(vec![Inst::SlotAddr(ra, r)], Vec::new(), &[1], each, v, &mut f);
        inline_calls(options, &mut f, 0, &get, &|_| false, &InlineLimits::tier1());
        let st = split_patched_path(&mut f, &get).expect("a split");
        assert_eq!((st.cloned, st.filled, st.fill_bytes), (1, 1, 16), "{:?} {}", st, f.dump());
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
    }

    #[test]
    fn the_second_round_takes_copies_in_behind_exits_only() {
        // no copy yet: an exit before each call the second round may take in (a patchable body's)
        // where none ran since the loop's head; the round takes the patchable leaf in unguarded
        // behind its exit, keeps a forbidden one a call (its exit then covers no copy and goes),
        // takes a body that is not patchable in anywhere, and keeps the patchable call that body
        // exposes (no exit covers it); the patched path keeps its calls
        let lf = leaf("leaf", Some(FLAG));
        let six = leaf("six", Some(SIX_FLAG));
        let md = mid();
        let get = |id: u32| match id {
            1 => Some((&lf as *const Func, ATTR_ALWAYS)),
            6 => Some((&six as *const Func, ATTR_ALWAYS)),
            7 => Some((&md as *const Func, ATTR_ALWAYS)),
            _ => None,
        };
        let (mut f, v) = new_root();
        root(Vec::new(), &[6, 1, 7], Vec::new(), v, &mut f);
        let st = split_patched_path(&mut f, &get).expect("a split");
        assert_eq!((st.site_exits, st.call_exits), (0, 2), "{:?} {}", st, f.dump());
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
        let options = &relax_session::Options { opt: vec!["-inl@6".into()], ..Default::default() };
        let mut stats = InlineStats::remarks();
        let took = inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| false, &|_| false, &InlineLimits::tier1(), None, Some(&mut stats));
        let mut took: Vec<u32> = took.iter().map(|x| x.0).collect();
        took.sort_unstable();
        assert_eq!(took, vec![1, 7], "{}", f.dump());
        assert!(f.guard_calls.is_empty() && f.unguarded_flags == vec![FLAG], "{:?} {}", f.unguarded_flags, f.dump());
        let why = |w: u8| stats.remarks.iter().filter(|r| r.why == w).count();
        assert_eq!(why(WHY_NO_EXIT), 1, "{}", f.dump());
        assert!(why(WHY_PATCHED_PATH) >= 3, "{}", f.dump());
        assert_eq!(prune_exits(&mut f, &|s| s == 1 || s == 6), 1, "{}", f.dump());
        f.verify().unwrap();
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
        let (ex, fast, _, pat) = halves(&f);
        assert_eq!((0..f.blocks.len()).filter(|&b| fast[b] && ex[b].is_some()).count(), 1, "{}", f.dump());
        let calls = |blocks: &[bool], id: u32| count(&f, blocks, |i| matches!(i, Inst::Call(Callee::Fn(x), ..) if *x == id));
        assert_eq!((calls(&fast, 6), calls(&fast, 1), calls(&fast, 7)), (1, 1, 0), "{}", f.dump());
        assert_eq!((calls(&pat, 6), calls(&pat, 1), calls(&pat, 7)), (1, 1, 1), "{}", f.dump());
    }
}
