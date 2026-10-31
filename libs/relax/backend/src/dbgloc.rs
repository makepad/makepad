//! Variable locations at call sites (makepad inspect `locals`): where each named local of the
//! source (`Func::dbg`) is while a call of the function runs -- a callee-saved register, a
//! frame slot, a constant -- or that it is held nowhere there ("optimised out"). Side metadata of
//! the finished allocation only: nothing here changes an instruction, a register or a slot.
//!
//! Every frame of a parked panic, a breakpoint or a paused thread above the innermost one is
//! stopped at a call (its return address), so call sites are the safepoints: what is live
//! across a call stays where the allocator keeps it until the call returns -- a callee-saved
//! register (the callee saves it where the frame walk finds it), a save slot (a caller-saved
//! register's value, `Alloc::saves`), a spill slot, or it is recomputed (a constant). A value
//! that is not live across the call may be overwritten by then: it is not reported.
//!
//! The table travels with the code in its pc map (`pc_entries`: entries at code offset
//! `PC_VARS`, four bytes of the table each), so every install path, the disk cache and the
//! startup image carry it unchanged.

use crate::regalloc::{for_defs, for_uses, term_uses, Alloc, Loc, Remat};
use crate::rir::*;

/// Code offset of the pc-map entries carrying the variable table (after every instruction's
/// entry, before the inline stack's at u32::MAX: a lookup never reaches them).
pub const PC_VARS: u32 = u32::MAX - 1;

/// Where a value is while a call runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Home {
    /// a callee-saved integer register (the target's number)
    Reg(u8),
    /// a callee-saved float register (its low 64 bits)
    Flt(u8),
    /// memory at the frame pointer + this displacement
    Fp(i32),
    /// recomputed where it is used: this constant (integers; float bits)
    Const(i64),
    /// a caller-saved integer register as the call began (an argument of it, or a value the
    /// call sequence leaves alone): known only at a stop's innermost call, whose trampoline
    /// stores the caller-saved registers
    CallReg(u8),
}

/// How a variable is stored, for the whole function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VarKind {
    /// one value per layout leaf; each is located per call site
    Regs(u32),
    /// the bytes at the frame pointer + this displacement (its own stack slot)
    Frame(i32),
    /// its slot's storage is shared with other slots (lifetimes that do not overlap): its bytes
    /// are not known to be its own at a given call
    Shared,
    /// the bytes at the address the leaf-0 value holds, plus this offset
    Mem(i32),
}

/// A variable of the table: (inline context, local index in its fn's body, storage).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Var {
    pub ctx: u16,
    pub local: u32,
    pub kind: VarKind,
    /// (leaf, value) of `Regs` leaves that are constant everywhere (folded by the optimiser)
    pub consts: Vec<(u32, i64)>,
}

/// The decoded table: variables, and per call site (return-address code offset) the located
/// leaves (variable index, leaf index, home).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub vars: Vec<Var>,
    pub sites: Vec<(u32, Vec<(u32, u32, Home)>)>,
}

impl Table {
    /// The located leaves at the call whose return address is code offset `ret`.
    pub fn site(&self, ret: u32) -> Option<&[(u32, u32, Home)]> {
        self.sites.iter().find(|s| s.0 == ret).map(|s| s.1.as_slice())
    }
}

/// MAKEPAD_RUN_VARS_STATS: [bodies with a table, bodies compiled, table bytes, pc-map bytes of
/// the table's entries (varint form as PcMap stores it), pc-map bytes of the code's own entries].
pub static STATS: [std::sync::atomic::AtomicU64; 5] = [const { std::sync::atomic::AtomicU64::new(0) }; 5];

pub fn stats_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_VARS_STATS").is_some())
}

/// Bytes of `entries` in PcMap's varint delta form, continuing from entry `prev`.
pub fn varint_bytes(prev: (u32, u64), entries: &[(u32, u64)]) -> u64 {
    let len = |x: u64| -> u64 { (64 - x.max(1).leading_zeros() as u64 + 6) / 7 };
    let (mut po, mut pp) = prev;
    let mut n = 0;
    for &(o, p) in entries {
        // positions as the cache stores them: a small file index
        let p = p & 0xffff_ffff;
        n += len(o.wrapping_sub(po) as u64);
        let d = p.wrapping_sub(pp) as i64;
        n += len(((d << 1) ^ (d >> 63)) as u64);
        po = o;
        pp = p;
    }
    n
}

/// Counts a body's pc map (its code entries, then the table's) for MAKEPAD_RUN_VARS_STATS.
pub fn note_stats(code: &[(u32, u64)], vars: &[(u32, u64)], table_bytes: usize) {
    use std::sync::atomic::Ordering::Relaxed;
    if !stats_on() {
        return;
    }
    let n = STATS[1].fetch_add(1, Relaxed) + 1;
    STATS[4].fetch_add(varint_bytes((0, 0), code), Relaxed);
    if n % 2000 == 0 {
        let g = |k: usize| STATS[k].load(Relaxed);
        eprintln!("makepad-run vars: {} of {} bodies with a table, table bytes {}, its pc-map bytes {}, code pc-map bytes {}", g(0), g(1), g(2), g(3), g(4));
    }
    if !vars.is_empty() {
        STATS[0].fetch_add(1, Relaxed);
        STATS[2].fetch_add(table_bytes as u64, Relaxed);
        // (PcMap stores the length entry, then the table's bytes as they are)
        STATS[3].fetch_add(varint_bytes(code.last().map_or((0, 0), |e| (e.0, e.1)), &vars[..1]) + table_bytes as u64, Relaxed);
    }
}

/// Whether the table is built (MAKEPAD_RUN_NO_VARS=1: not; the code is the same either way).
pub fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_NO_VARS").is_none())
}

/// The target's facts the table needs: its callee-saved registers, a spill-area offset as a
/// frame-pointer displacement, a stack slot's displacement.
pub struct Frame<'a> {
    pub callee_int: &'a [u8],
    pub callee_flt: &'a [u8],
    /// the integer argument registers in order (the Relax ABI's)
    pub arg_regs: &'a [u8],
    /// the caller-saved registers a stop's trampoline stores as the call began
    pub captured: &'a [u8],
    /// registers a call sequence may write besides its argument registers (scratch, target)
    pub call_writes: &'a [u8],
    pub spill_fp: &'a dyn Fn(i32) -> i32,
    pub slot_fp: &'a dyn Fn(u32) -> i32,
}

/// Builds the table of `f` (after allocation `al`) for the calls the emitter placed: `calls`
/// = (the call's instruction position `Alloc::inst_pos`, its return address's code offset).
/// Empty when `f` keeps no variables.
pub fn build(f: &Func, al: &Alloc, fr: &Frame, calls: &[(u32, u32)]) -> Vec<u8> {
    if f.dbg.is_empty() || !on() {
        return Vec::new();
    }
    let nv = f.vregs.len();
    // variables by (context, local); every debug entry is one of them (live-range pieces of a
    // leaf are twins of an entry)
    let mut vars: Vec<Var> = Vec::new();
    let mut var_of: std::collections::HashMap<(u16, u32), usize> = std::collections::HashMap::new();
    let mut of_entry: Vec<u32> = Vec::with_capacity(f.dbg.len());
    let used = used_slots(f);
    for d in &f.dbg {
        let kind = match &d.loc {
            DbgLoc::Regs(r) => VarKind::Regs(r.len() as u32),
            DbgLoc::Slot(s, off) => {
                let s = *s as usize;
                if s >= f.slots.len() || !used[s] {
                    // the slot went (promoted to values, forwarded): not held as such
                    of_entry.push(u32::MAX);
                    continue;
                }
                let at = (fr.slot_fp)(s as u32);
                let size = f.slots[s].size as i64;
                let shared = (0..f.slots.len()).any(|t| {
                    t != s && used[t] && {
                        let a = (fr.slot_fp)(t as u32) as i64;
                        a < at as i64 + size && (at as i64) < a + f.slots[t].size as i64
                    }
                });
                if shared {
                    VarKind::Shared
                } else {
                    VarKind::Frame(at + off)
                }
            }
            DbgLoc::Mem(_, off) => VarKind::Mem(*off),
        };
        // a twin (a live-range piece) is the same variable
        let k = *var_of.entry((d.ctx, d.local)).or_insert_with(|| {
            vars.push(Var { ctx: d.ctx, local: d.local, kind, consts: Vec::new() });
            vars.len() - 1
        });
        for &c in &d.k {
            if !vars[k].consts.iter().any(|x| x.0 == c.0) {
                vars[k].consts.push(c);
            }
        }
        of_entry.push(k as u32);
    }
    // copies between single-definition values hold the same value everywhere (O1 code is full
    // of them: a local read through a copy into a temporary): a leaf is held by any value of its
    // class
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] = ndef[d.0 as usize].saturating_add(1));
        }
    }
    let mut rep: Vec<u32> = (0..nv as u32).collect();
    fn find(rep: &mut [u32], mut v: u32) -> u32 {
        while rep[v as usize] != v {
            rep[v as usize] = rep[rep[v as usize] as usize];
            v = rep[v as usize];
        }
        v
    }
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Mov(d, s) = i {
                if d != s && ndef[d.0 as usize] == 1 && ndef[s.0 as usize] == 1 && f.vregs[d.0 as usize] == f.vregs[s.0 as usize] {
                    let (a, c) = (find(&mut rep, d.0), find(&mut rep, s.0));
                    if a != c {
                        rep[a as usize] = c;
                    }
                }
            }
        }
    }
    let mut class: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    let mut named_reps: Vec<u32> = Vec::new();
    for d in &f.dbg {
        if let DbgLoc::Regs(r) = &d.loc {
            for v in r {
                if *v != DBG_NONE && (v.0 as usize) < nv {
                    named_reps.push(find(&mut rep, v.0));
                }
            }
        }
    }
    named_reps.sort_unstable();
    named_reps.dedup();
    for v in 0..nv as u32 {
        let r = find(&mut rep, v);
        if named_reps.binary_search(&r).is_ok() {
            class.entry(r).or_default().push(v);
        }
    }
    // the tracked values (Regs leaves and Mem addresses): dense numbers for the liveness sets,
    // and per number the (variable, leaf) pairs it holds
    const NONE: u32 = u32::MAX;
    let mut idx: Vec<u32> = vec![NONE; nv];
    let mut regs_of: Vec<u32> = Vec::new();
    let mut holds: Vec<Vec<(u32, u32)>> = Vec::new();
    for (e, d) in f.dbg.iter().enumerate() {
        let var = of_entry[e];
        if var == u32::MAX {
            continue;
        }
        let leaves: &[VReg] = match &d.loc {
            DbgLoc::Regs(r) => r,
            DbgLoc::Mem(v, _) => std::slice::from_ref(v),
            DbgLoc::Slot(..) => &[],
        };
        for (k, v) in leaves.iter().enumerate() {
            if *v == DBG_NONE || v.0 as usize >= nv {
                continue;
            }
            let members: Vec<u32> = match &d.loc {
                DbgLoc::Regs(_) => class.get(&find(&mut rep, v.0)).cloned().unwrap_or_else(|| vec![v.0]),
                _ => vec![v.0],
            };
            for m in members {
                if idx[m as usize] == NONE {
                    idx[m as usize] = regs_of.len() as u32;
                    regs_of.push(m);
                    holds.push(Vec::new());
                }
                let h = &mut holds[idx[m as usize] as usize];
                if !h.contains(&(var, k as u32)) {
                    h.push((var, k as u32));
                }
            }
        }
    }
    let mut table = Table { vars, sites: Vec::new() };
    if !regs_of.is_empty() && !calls.is_empty() {
        let at: std::collections::HashMap<u32, u32> = calls.iter().copied().collect();
        let skipped: std::collections::HashSet<u32> = al.store_skip.iter().map(|x| x.1).collect();
        let words = (regs_of.len() + 63) / 64;
        let nb = f.blocks.len();
        // per block: upward-exposed uses and definitions of the tracked values
        let mut gen = vec![0u64; nb * words];
        let mut kill = vec![0u64; nb * words];
        let mut uses = Vec::new();
        let set = |s: &mut [u64], i: u32| s[(i / 64) as usize] |= 1 << (i % 64);
        let clr = |s: &mut [u64], i: u32| s[(i / 64) as usize] &= !(1 << (i % 64));
        for (b, blk) in f.blocks.iter().enumerate() {
            let (g, k) = (&mut gen[b * words..(b + 1) * words], &mut kill[b * words..(b + 1) * words]);
            term_uses(&blk.term, &mut uses);
            for u in &uses {
                if let Some(&i) = idx.get(u.0 as usize).filter(|i| **i != NONE) {
                    set(g, i);
                }
            }
            for inst in blk.insts.iter().rev() {
                for_defs(inst, |d| {
                    if let Some(&i) = idx.get(d.0 as usize).filter(|i| **i != NONE) {
                        set(k, i);
                        clr(g, i);
                    }
                });
                for_uses(inst, |u| {
                    if let Some(&i) = idx.get(u.0 as usize).filter(|i| **i != NONE) {
                        set(g, i);
                    }
                });
            }
        }
        let mut live_in = gen.clone();
        let mut live_out = vec![0u64; nb * words];
        let mut changed = true;
        let mut tmp = vec![0u64; words];
        while changed {
            changed = false;
            for b in (0..nb).rev() {
                tmp.iter_mut().for_each(|w| *w = 0);
                for s in f.blocks[b].term.succs() {
                    let s = s as usize;
                    for w in 0..words {
                        tmp[w] |= live_in[s * words + w];
                    }
                }
                live_out[b * words..(b + 1) * words].copy_from_slice(&tmp);
                for w in 0..words {
                    let n = gen[b * words + w] | (tmp[w] & !kill[b * words + w]);
                    if n != live_in[b * words + w] {
                        live_in[b * words + w] = n;
                        changed = true;
                    }
                }
            }
        }
        // predecessors (the values every predecessor leaves live: held on entry to a block)
        let mut preds: Vec<Vec<u32>> = vec![Vec::new(); nb];
        for (bi, blk) in f.blocks.iter().enumerate() {
            for t in blk.term.succs() {
                if (t as usize) < nb && !preds[t as usize].contains(&(bi as u32)) {
                    preds[t as usize].push(bi as u32);
                }
            }
        }
        let has = |s: &[u64], n: usize| s[n / 64] & (1 << (n % 64)) != 0;
        let mut live = vec![0u64; words];
        for (b, blk) in f.blocks.iter().enumerate() {
            if !blk.insts.iter().any(|i| matches!(i, Inst::Call(..))) {
                continue;
            }
            // backward: the tracked values live after each call of the block
            let mut after: Vec<(usize, Vec<u64>)> = Vec::new();
            live.copy_from_slice(&live_out[b * words..(b + 1) * words]);
            term_uses(&blk.term, &mut uses);
            for u in &uses {
                if let Some(&i) = idx.get(u.0 as usize).filter(|i| **i != NONE) {
                    set(&mut live, i);
                }
            }
            for (ii, inst) in blk.insts.iter().enumerate().rev() {
                if matches!(inst, Inst::Call(..)) {
                    after.push((ii, live.clone()));
                }
                for_defs(inst, |d| {
                    if let Some(&i) = idx.get(d.0 as usize).filter(|i| **i != NONE) {
                        clr(&mut live, i);
                    }
                });
                for_uses(inst, |u| {
                    if let Some(&i) = idx.get(u.0 as usize).filter(|i| **i != NONE) {
                        set(&mut live, i);
                    }
                });
            }
            after.reverse();
            // held on entry: live at the end of every predecessor (dead here, but nothing has
            // overwritten its home yet -- checked below)
            let mut held = vec![0u64; words];
            if b != 0 && !preds[b].is_empty() {
                held.iter_mut().for_each(|w| *w = !0);
                for &pb in &preds[b] {
                    let mut lo: Vec<u64> = live_out[pb as usize * words..(pb as usize + 1) * words].to_vec();
                    // reached by unwinding out of an invoked call: its results were never written
                    let pblk = &f.blocks[pb as usize];
                    if matches!(pblk.term, Term::Invoke(_, u) if u as usize == b) {
                        if let Some(last) = pblk.insts.last() {
                            for_defs(last, |d| {
                                if let Some(&i) = idx.get(d.0 as usize).filter(|i| **i != NONE) {
                                    clr(&mut lo, i);
                                }
                            });
                        }
                    }
                    for w in 0..words {
                        held[w] &= lo[w];
                    }
                }
            }
            // forward: per call, the located leaves; what the block's code has overwritten so far
            let mut hit_regs: Vec<u8> = Vec::new();
            let mut hit_flt: Vec<u8> = Vec::new();
            let mut hit_mem: Vec<(i32, i32)> = Vec::new();
            let mut k_after = 0;
            let start_pos = al.inst_pos(b, 0);
            for (ii, inst) in blk.insts.iter().enumerate() {
                let pos = al.inst_pos(b, ii);
                // region windows load spilled values into free registers before their uses
                for &(_, a, _, r) in al.windows.iter().chain(al.chained.iter()) {
                    if a >= start_pos && a <= pos && !hit_regs.contains(&r) {
                        hit_regs.push(r);
                    }
                }
                if let Inst::Call(callee, args, _) = inst {
                    let live_after = match after.get(k_after) {
                        Some((i, l)) if *i == ii => {
                            k_after += 1;
                            l.as_slice()
                        }
                        _ => &[],
                    };
                    let saves = al.saves.get(&pos);
                    // a call's saves are stored before it runs
                    for &(_, off) in saves.map(|x| x.as_slice()).unwrap_or(&[]) {
                        hit_mem.push(((fr.spill_fp)(off), 32));
                    }
                    if let Some(&ret) = at.get(&pos) {
                        let mut defs_here = Vec::new();
                        for_defs(inst, |d| defs_here.push(d.0));
                        // integer argument k of the call (the Relax ABI's register order), and
                        // the registers the call sequence writes before the call begins
                        let relax_call = matches!(callee, Callee::Fn(_) | Callee::Host(_));
                        let int_args: Vec<u32> = args.iter().filter(|a| f.vregs[a.0 as usize] == Cls::I).map(|a| a.0).collect();
                        let written = |r: u8| fr.call_writes.contains(&r) || !relax_call || fr.arg_regs.iter().take(int_args.len()).any(|x| *x == r);
                        // the stop's innermost call only: a caller-saved register as the call began
                        let call_reg = |v: u32, dead_ok: bool| -> Option<Home> {
                            if relax_call {
                                if let Some(k) = int_args.iter().position(|a| *a == v) {
                                    return fr.arg_regs.get(k).map(|r| Home::CallReg(*r));
                                }
                            }
                            match al.loc[v as usize] {
                                Loc::Reg(r) if dead_ok && fr.captured.contains(&r) && !written(r) => Some(Home::CallReg(r)),
                                _ => None,
                            }
                        };
                        let mut row: Vec<(u32, u32, Home)> = Vec::new();
                        // the candidates: live after the call, held on entry, or its operands
                        let mut cand: Vec<usize> = Vec::new();
                        for w in 0..words {
                            let mut m = held[w] | live_after.get(w).copied().unwrap_or(0);
                            while m != 0 {
                                cand.push(w * 64 + m.trailing_zeros() as usize);
                                m &= m - 1;
                            }
                        }
                        for a in args.iter().chain(match callee { Callee::Indirect(x) => std::slice::from_ref(x), _ => &[] }) {
                            if let Some(&i) = idx.get(a.0 as usize).filter(|i| **i != NONE) {
                                cand.push(i as usize);
                            }
                        }
                        cand.sort_unstable();
                        cand.dedup();
                        for n in cand {
                            let v = regs_of[n];
                            let across = !live_after.is_empty() && has(live_after, n) && !defs_here.contains(&v);
                            let used = args.contains(&VReg(v)) || matches!(callee, Callee::Indirect(x) if x.0 == v);
                            let held_here = !across && !used && has(&held, n);
                            let h = if across || used {
                                home(f, al, fr, saves, &skipped, v, across)
                            } else if held_here {
                                // dead here but live into the block: its home still holds it
                                // unless the block's code before this call wrote there
                                held_home(f, al, fr, &skipped, v).filter(|h| match *h {
                                    Home::Reg(r) => !hit_regs.contains(&r),
                                    Home::Flt(x) => !hit_flt.contains(&x),
                                    Home::Fp(d) => !hit_mem.iter().any(|&(m, n)| m < d + 32 && d < m + n),
                                    _ => true,
                                })
                            } else {
                                None
                            };
                            let h = h.or_else(|| {
                                let still = across || used || held_here && !matches!(al.loc[v as usize], Loc::Reg(r) if hit_regs.contains(&r));
                                if across || used || held_here { call_reg(v, still) } else { None }
                            });
                            let Some(h) = h else { continue };
                            for &(var, leaf) in &holds[n] {
                                row.push((var, leaf, h));
                            }
                        }
                        // one home per leaf (values of one copy class hold the same value)
                        row.sort_by_key(|r| (r.0, r.1));
                        row.dedup_by_key(|r| (r.0, r.1));
                        table.sites.push((ret, row));
                    }
                    // a call clobbers the caller-saved registers
                    for &r in fr.captured.iter().chain(fr.call_writes) {
                        hit_regs.push(r);
                    }
                }
                // what this instruction writes: its results' homes; a value defined is no longer
                // the one held on entry
                for_defs(inst, |d| {
                    if let Some(&i) = idx.get(d.0 as usize).filter(|i| **i != NONE) {
                        clr(&mut held, i);
                    }
                    match al.loc.get(d.0 as usize) {
                        Some(Loc::Reg(r)) => hit_regs.push(*r),
                        Some(Loc::Flt(x)) => hit_flt.push(*x),
                        Some(Loc::Stack(o)) => hit_mem.push(((fr.spill_fp)(*o), 32)),
                        _ => {}
                    }
                });
            }
        }
        table.sites.sort_unstable_by_key(|s| s.0);
    }
    // only what a stop can show: variables located somewhere (a site, a constant, a frame
    // slot) and the sites that locate something; a variable left out reads as optimised out
    let mut used = vec![false; table.vars.len()];
    for (_, row) in &table.sites {
        for r in row {
            used[r.0 as usize] = true;
        }
    }
    for (k, v) in table.vars.iter().enumerate() {
        used[k] |= !v.consts.is_empty() || matches!(v.kind, VarKind::Frame(_) | VarKind::Shared);
    }
    let mut renum = vec![u32::MAX; table.vars.len()];
    let mut kept = Vec::new();
    for (k, v) in table.vars.drain(..).enumerate() {
        if used[k] {
            renum[k] = kept.len() as u32;
            kept.push(v);
        }
    }
    table.vars = kept;
    table.sites.retain(|s| !s.1.is_empty());
    for (_, row) in table.sites.iter_mut() {
        for r in row.iter_mut() {
            r.0 = renum[r.0 as usize];
        }
    }
    if table.vars.is_empty() {
        return Vec::new();
    }
    if let Ok(w) = std::env::var("MAKEPAD_RUN_VARS_TRACE") {
        if f.name.contains(w.as_str()) {
            eprintln!("=== vars {}\ndbg {:?}\nloc {:?}\ntable {:?}\n{}", f.name, f.dbg, f.dbg.iter().flat_map(|d| match &d.loc { DbgLoc::Regs(r) => r.clone(), _ => Vec::new() }).filter(|v| *v != DBG_NONE).map(|v| (v.0, al.loc[v.0 as usize], al.remat[v.0 as usize])).collect::<Vec<_>>(), table, f.dump());
        }
    }
    encode(&table)
}

/// Where tracked value `v` is while a call (whose saves are `saves`) runs: `across` it lives
/// on after the call (its save slot, a callee-saved register, its spill slot); else the call
/// is its last use and only a callee-saved register still holds it for sure (a spill slot's
/// storage may go to another value once it is dead).
#[allow(clippy::too_many_arguments)]
fn home(f: &Func, al: &Alloc, fr: &Frame, saves: Option<&Vec<(VReg, i32)>>, skipped: &std::collections::HashSet<u32>, v: u32, across: bool) -> Option<Home> {
    if across {
        if let Some(&(_, off)) = saves.and_then(|s| s.iter().find(|x| x.0 .0 == v)) {
            return Some(Home::Fp((fr.spill_fp)(off)));
        }
    }
    let vec = f.vregs[v as usize].is_vec();
    match al.loc[v as usize] {
        Loc::Reg(r) if fr.callee_int.contains(&r) => return Some(Home::Reg(r)),
        Loc::Flt(x) if !vec && fr.callee_flt.contains(&x) => return Some(Home::Flt(x)),
        Loc::Stack(o) if across && !skipped.contains(&v) => return Some(Home::Fp((fr.spill_fp)(o))),
        _ => {}
    }
    match al.remat.get(v as usize) {
        Some(Remat::Const(k)) => Some(Home::Const(*k)),
        Some(Remat::FConst(bits, _)) => Some(Home::Const(*bits as i64)),
        _ => None,
    }
}

/// The home of a value held on entry to a block (live at the end of every predecessor), when
/// it is one a stop can read: a callee-saved register, its spill slot, a recomputed constant.
fn held_home(f: &Func, al: &Alloc, fr: &Frame, skipped: &std::collections::HashSet<u32>, v: u32) -> Option<Home> {
    let vec = f.vregs[v as usize].is_vec();
    match al.loc[v as usize] {
        Loc::Reg(r) if fr.callee_int.contains(&r) => Some(Home::Reg(r)),
        Loc::Flt(x) if !vec && fr.callee_flt.contains(&x) => Some(Home::Flt(x)),
        Loc::Stack(o) if !skipped.contains(&v) => Some(Home::Fp((fr.spill_fp)(o))),
        _ => match al.remat.get(v as usize) {
            Some(Remat::Const(k)) => Some(Home::Const(*k)),
            Some(Remat::FConst(bits, _)) => Some(Home::Const(*bits as i64)),
            _ => None,
        },
    }
}

// ------------------------------------------------------------------ encoding

fn put(b: &mut Vec<u8>, mut x: u64) {
    loop {
        if x < 0x80 {
            b.push(x as u8);
            return;
        }
        b.push((x as u8 & 0x7f) | 0x80);
        x >>= 7;
    }
}

fn put_s(b: &mut Vec<u8>, x: i64) {
    put(b, ((x << 1) ^ (x >> 63)) as u64);
}

// A home is one byte, tag in the top three bits: Reg / Flt / CallReg with the register number
// (< 32), Fp and Const followed by the zigzag value.
const H_REG: u8 = 0x00;
const H_FLT: u8 = 0x20;
const H_CALL: u8 = 0x40;
const H_FP: u8 = 0x60;
const H_CONST: u8 = 0x80;

fn put_home(b: &mut Vec<u8>, h: Home) {
    match h {
        Home::Reg(r) => b.push(H_REG | (r & 0x1f)),
        Home::Flt(r) => b.push(H_FLT | (r & 0x1f)),
        Home::CallReg(r) => b.push(H_CALL | (r & 0x1f)),
        Home::Fp(d) => {
            b.push(H_FP);
            put_s(b, d as i64);
        }
        Home::Const(k) => {
            b.push(H_CONST);
            put_s(b, k);
        }
    }
}

/// The table's bytes: the variables (context, local, then one byte: kind in bits 0..1, a Regs
/// variable's leaf count - 1 in bits 3.., bit 2 = constant leaves follow), then per call site
/// that locates something its return offset (delta) and its located leaves (0: the same as the
/// previous site's), each (variable << 2 | leaf, leaf 3 = the leaf follows) and its home.
pub fn encode(t: &Table) -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, t.vars.len() as u64);
    for v in &t.vars {
        put(&mut b, v.ctx as u64);
        put(&mut b, v.local as u64);
        match v.kind {
            VarKind::Regs(n) => {
                put(&mut b, (n.saturating_sub(1) as u64) << 3 | (!v.consts.is_empty() as u64) << 2);
                if !v.consts.is_empty() {
                    put(&mut b, v.consts.len() as u64);
                    for &(leaf, k) in &v.consts {
                        put(&mut b, leaf as u64);
                        put_s(&mut b, k);
                    }
                }
            }
            VarKind::Frame(d) => {
                put(&mut b, 1);
                put_s(&mut b, d as i64);
            }
            VarKind::Shared => put(&mut b, 2),
            VarKind::Mem(o) => {
                put(&mut b, 3);
                put_s(&mut b, o as i64);
            }
        }
    }
    put(&mut b, t.sites.len() as u64);
    let mut prev_ret = 0u32;
    let mut prev: Option<&Vec<(u32, u32, Home)>> = None;
    for (ret, row) in &t.sites {
        put(&mut b, (*ret - prev_ret) as u64);
        prev_ret = *ret;
        if prev == Some(row) {
            put(&mut b, 0);
            continue;
        }
        put(&mut b, row.len() as u64 + 1);
        for &(var, leaf, h) in row {
            put(&mut b, (var as u64) << 2 | leaf.min(3) as u64);
            if leaf >= 3 {
                put(&mut b, leaf as u64);
            }
            put_home(&mut b, h);
        }
        prev = Some(row);
    }
    b
}

struct Rd<'a> {
    b: &'a [u8],
    i: usize,
}

impl Rd<'_> {
    fn u(&mut self) -> Option<u64> {
        let mut x = 0u64;
        let mut sh = 0;
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            if sh < 64 {
                x |= ((c & 0x7f) as u64) << sh;
            }
            if c < 0x80 {
                return Some(x);
            }
            sh += 7;
        }
    }
    fn s(&mut self) -> Option<i64> {
        let z = self.u()?;
        Some(((z >> 1) as i64) ^ -((z & 1) as i64))
    }
}

/// The table encoded in `b` (`encode`'s); None when it does not parse.
pub fn decode(b: &[u8]) -> Option<Table> {
    let mut r = Rd { b, i: 0 };
    let nv = r.u()? as usize;
    let mut vars = Vec::with_capacity(nv.min(4096));
    for _ in 0..nv {
        let ctx = r.u()? as u16;
        let local = r.u()? as u32;
        let mut consts = Vec::new();
        let k = r.u()?;
        let kind = match k & 3 {
            0 => {
                if k & 4 != 0 {
                    for _ in 0..r.u()?.min(4096) {
                        consts.push((r.u()? as u32, r.s()?));
                    }
                }
                VarKind::Regs((k >> 3) as u32 + 1)
            }
            1 => VarKind::Frame(r.s()? as i32),
            2 => VarKind::Shared,
            _ => VarKind::Mem(r.s()? as i32),
        };
        vars.push(Var { ctx, local, kind, consts });
    }
    let ns = r.u()? as usize;
    let mut sites: Vec<(u32, Vec<(u32, u32, Home)>)> = Vec::with_capacity(ns.min(65536));
    let mut ret = 0u32;
    for _ in 0..ns {
        ret = ret.checked_add(r.u()? as u32)?;
        let n = r.u()? as usize;
        let row = if n == 0 {
            sites.last().map(|s| s.1.clone()).unwrap_or_default()
        } else {
            let mut row = Vec::with_capacity((n - 1).min(4096));
            for _ in 0..n - 1 {
                let vl = r.u()?;
                let var = (vl >> 2) as u32;
                let leaf = if vl & 3 == 3 { r.u()? as u32 } else { (vl & 3) as u32 };
                let hb = *r.b.get(r.i)?;
                r.i += 1;
                let h = match hb & 0xe0 {
                    H_REG => Home::Reg(hb & 0x1f),
                    H_FLT => Home::Flt(hb & 0x1f),
                    H_CALL => Home::CallReg(hb & 0x1f),
                    H_FP => Home::Fp(r.s()? as i32),
                    H_CONST => Home::Const(r.s()?),
                    _ => return None,
                };
                row.push((var, leaf, h));
            }
            row
        };
        sites.push((ret, row));
    }
    Some(Table { vars, sites })
}

/// The pc-map entries carrying table bytes `b` (at `PC_VARS`, four bytes per entry after one
/// holding the length; positions in file `file`, so a cache translates them as any other and
/// the bytes, their low 32 bits, stay as they are). The compiler's PcMap stores the chunks as
/// the bytes themselves after the length entry.
pub fn pc_entries(b: &[u8], file: u32) -> Vec<(u32, u64)> {
    if b.is_empty() {
        return Vec::new();
    }
    let hi = (file as u64 & 0xffff) << 32;
    let mut v = Vec::with_capacity(1 + (b.len() + 3) / 4);
    v.push((PC_VARS, hi | b.len() as u64));
    for c in b.chunks(4) {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        v.push((PC_VARS, hi | u32::from_le_bytes(w) as u64));
    }
    v
}

/// The table bytes in a pc map's entries (`pc_entries`), in map order; empty when it has none.
pub fn from_pc_entries(entries: impl Iterator<Item = (u32, u64)>) -> Vec<u8> {
    let mut len = None;
    let mut b = Vec::new();
    for (o, p) in entries {
        if o != PC_VARS {
            continue;
        }
        match len {
            None => len = Some(p as u32 as usize),
            Some(_) => b.extend_from_slice(&(p as u32).to_le_bytes()),
        }
    }
    b.truncate(len.unwrap_or(0));
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_round_trips_through_pc_entries() {
        let t = Table {
            vars: vec![
                Var { ctx: 0, local: 3, kind: VarKind::Regs(2), consts: vec![(1, -7)] },
                Var { ctx: 1, local: 0, kind: VarKind::Frame(-48), consts: Vec::new() },
                Var { ctx: 2, local: 7, kind: VarKind::Shared, consts: Vec::new() },
                Var { ctx: 0, local: 1, kind: VarKind::Mem(8), consts: Vec::new() },
            ],
            sites: vec![
                (12, vec![(0, 0, Home::Reg(19)), (0, 1, Home::Fp(-24)), (3, 0, Home::Const(-5))]),
                (40, vec![(0, 0, Home::Reg(19)), (0, 1, Home::Fp(-24)), (3, 0, Home::Const(-5))]),
                (64, vec![(0, 1, Home::Flt(8)), (3, 0, Home::CallReg(2))]),
                (90, vec![]),
            ],
        };
        let b = encode(&t);
        let e = pc_entries(&b, 7);
        assert!(e.iter().all(|x| x.0 == PC_VARS && (x.1 >> 32) == 7));
        // a lookup map: real entries first, the inline stack's after
        let mut all = vec![(0u32, 1u64), (8, 2)];
        all.extend(e);
        all.push((u32::MAX, 5));
        let back = from_pc_entries(all.into_iter());
        assert_eq!(back, b);
        assert_eq!(decode(&back), Some(t));
    }
}
