//! Frame layout: stack slot offsets, with slots whose lifetimes cannot overlap sharing storage.

use crate::regalloc::{for_defs, for_uses, term_uses};
use crate::rir::*;

pub fn slot_colors_used(f: &Func, min: u32, addressed: &[bool]) -> Option<Vec<u32>> {
    let ns = f.slots.len();
    let nb = f.blocks.len();
    if ns < 2 || nb == 0 {
        return None;
    }
    // a slot nothing addresses any more (forwarded into another: forward_slot_dests_x) has no
    // lifetime, so it "fits" next to everything -- and a colour takes its members' largest size:
    // Score's SamplerBackend::new, built in place into its result buffer, kept a 108 KB frame
    // for the 768-byte temporaries that shared a colour with the dead 108 KB slot
    // nothing much to share: not worth the analysis
    let (n, bytes) = (0..ns).filter(|&s| addressed[s] && f.slots[s].size >= min).fold((0, 0u64), |(n, b), s| (n + 1, b + f.slots[s].size as u64));
    if n < 2 || bytes < SLOT_SHARE_MIN_BYTES {
        return None;
    }
    let (der, esc, arg_ok) = slot_sets(f)?;
    // candidate slots: big, not escaping; the largest COLOR_MAX of them (colouring is
    // candidates x colours x block words: a big inlined function has thousands of slots)
    const COLOR_MAX: usize = 1024;
    let mut cand: Vec<usize> = (0..ns).filter(|&s| f.slots[s].size >= min && !esc[s] && addressed[s]).collect();
    if cand.len() > COLOR_MAX {
        cand.sort_by(|&x, &y| f.slots[y].size.cmp(&f.slots[x].size).then(x.cmp(&y)));
        cand.truncate(COLOR_MAX);
        cand.sort_unstable();
    }
    if cand.len() < 2 {
        return None;
    }
    let mut ci = vec![usize::MAX; ns];
    for (k, &s) in cand.iter().enumerate() {
        ci[s] = k;
    }
    let nc = cand.len();
    // Positions: instruction i reads at 2i and writes at 2i + 1 (the terminator is instruction
    // insts.len()). An access takes both, except a whole-slot copy between two slots' own
    // addresses: its source is read at 2i, its destination written at 2i + 1, so a slot copied
    // into another and dead after can share its storage (the copy then copies a slot onto
    // itself, which changes nothing) -- the chains of temporaries a value is moved through.
    let exact = slot_addr_vregs(f);
    // which slot an exact address is the address of
    let mut exact_slot = vec![u32::MAX; f.vregs.len()];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::SlotAddr(d, sl) = i {
                if exact[d.0 as usize] {
                    exact_slot[d.0 as usize] = *sl;
                }
            }
        }
    }
    // A result buffer (`Slot::ret`) passed as one to one call only, by its own address: that
    // is the call lowering made it for, which writes the whole value (its type is the slot's),
    // so the slot's earlier contents are dead before it
    let mut sret_uses = vec![0u32; ns];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Call(Callee::Fn(id), args, _) = i {
                if f.sret_callees.contains(id) {
                    if let Some(a) = args.first() {
                        for &sl in &der[a.0 as usize] {
                            sret_uses[sl as usize] += 1;
                        }
                    }
                }
            }
        }
    }
    // accesses per candidate: (block, first, last position) per block with any, in block order,
    // and per such block the access kinds that decide liveness at its ends: the first read or
    // whole write (`ent`: Some(true) read, Some(false) whole write, None neither) and whether the
    // last access is the consuming call of an argument copy (`ext`)
    const READ: u8 = 0;
    const WRITE: u8 = 1;
    const KILL: u8 = 2;
    const CONSUME: u8 = 3;
    let mut acc: Vec<Vec<(u32, u32, u32)>> = vec![Vec::new(); nc];
    let mut ent: Vec<Vec<Option<bool>>> = vec![Vec::new(); nc];
    let mut ext: Vec<Vec<bool>> = vec![Vec::new(); nc];
    let mut tu = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let note = |k: usize, lo: u32, hi: u32, kind: u8, acc: &mut Vec<Vec<(u32, u32, u32)>>, ent: &mut Vec<Vec<Option<bool>>>, ext: &mut Vec<Vec<bool>>| {
            let a = &mut acc[k];
            match a.last_mut() {
                Some(x) if x.0 == bi as u32 => x.2 = x.2.max(hi),
                _ => {
                    a.push((bi as u32, lo, hi));
                    ent[k].push(None);
                    ext[k].push(false);
                }
            }
            let e = ent[k].last_mut().unwrap();
            if e.is_none() {
                *e = match kind {
                    READ | CONSUME => Some(true),
                    KILL => Some(false),
                    _ => None,
                };
            }
            *ext[k].last_mut().unwrap() = kind == CONSUME;
        };
        let each = |v: VReg, g: &mut dyn FnMut(usize)| {
            for &d in &der[v.0 as usize] {
                let k = ci[d as usize];
                if k != usize::MAX {
                    g(k);
                }
            }
        };
        for (ii, i) in b.insts.iter().enumerate() {
            let p = 2 * ii as u32;
            // address arithmetic is no access to the slot's memory (slot addresses may sit at the
            // function entry, computed once): only what reads or writes through the address, or
            // hands it on (calls, compares, terminators), keeps the slot live
            match i {
                Inst::SlotAddr(..) | Inst::Mov(..) | Inst::IBin(IOp::Add, ..) | Inst::IBinI(IOp::Add, ..) | Inst::IBinI(IOp::Sub, ..) => {}
                Inst::Copy(d, sv, n) => {
                    let split = exact[d.0 as usize] && exact[sv.0 as usize];
                    each(*sv, &mut |k| note(k, p, if split { p } else { p + 1 }, READ, &mut acc, &mut ent, &mut ext));
                    let whole = |k: usize| exact[d.0 as usize] && exact_slot[d.0 as usize] as usize == cand[k] && *n >= f.slots[cand[k]].size;
                    each(*d, &mut |k| {
                        let kind = if whole(k) { KILL } else { WRITE };
                        note(k, if split { p + 1 } else { p }, p + 1, kind, &mut acc, &mut ent, &mut ext)
                    });
                }
                Inst::Store(_, a, _, v) | Inst::StoreX(_, a, _, _, _, v) | Inst::VStore(a, _, v) | Inst::VStoreX(a, _, _, _, v) | Inst::AtomicStore(_, _, a, v) => {
                    each(*a, &mut |k| note(k, p, p + 1, WRITE, &mut acc, &mut ent, &mut ext));
                    each(*v, &mut |k| note(k, p, p + 1, READ, &mut acc, &mut ent, &mut ext));
                    if let Inst::StoreX(_, _, x, ..) | Inst::VStoreX(_, x, ..) = i {
                        each(*x, &mut |k| note(k, p, p + 1, READ, &mut acc, &mut ent, &mut ext));
                    }
                }
                Inst::Call(c, args, _) => {
                    let sret = matches!(c, Callee::Fn(id) if f.sret_callees.contains(id));
                    if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                        each(*v, &mut |k| note(k, p, p + 1, READ, &mut acc, &mut ent, &mut ext));
                    }
                    for (j, a) in args.iter().enumerate() {
                        each(*a, &mut |k| {
                            let s = cand[k];
                            let only = der[a.0 as usize].len() == 1;
                            let kind = if sret && j == 0 && only && f.slots[s].ret && sret_uses[s] == 1 && exact_slot[a.0 as usize] as usize == s && !args[1..].iter().any(|x| der[x.0 as usize].contains(&(s as u32))) {
                                KILL
                            } else if arg_ok[s] && only {
                                CONSUME
                            } else {
                                READ
                            };
                            note(k, p, p + 1, kind, &mut acc, &mut ent, &mut ext)
                        });
                    }
                }
                _ => for_uses(i, |v| each(v, &mut |k| note(k, p, p + 1, READ, &mut acc, &mut ent, &mut ext))),
            }
        }
        term_uses(&b.term, &mut tu);
        let p = 2 * b.insts.len() as u32;
        for v in &tu {
            each(*v, &mut |k| note(k, p, p + 1, READ, &mut acc, &mut ent, &mut ext));
        }
    }
    let live = if let Some(positions) = f.baseline.then(|| slot_linear_positions(f)).flatten() {
        // No back edges or joins: the access pass above already gives each slot's
        // lifetime. Combine the blocks' intervals in execution order, without CFG
        // fixed points or a second vreg-liveness analysis for born slots.
        acc.iter().map(|accesses| {
            let mut lo = u32::MAX;
            let mut hi = 0;
            for &(b, a, z) in accesses {
                lo = lo.min(positions[b as usize] + a);
                hi = hi.max(positions[b as usize] + z);
            }
            if lo == u32::MAX { Vec::new() } else { vec![(0, lo, hi)] }
        }).collect()
    } else {
        slot_live_intervals(f, &cand, &der, &acc, &ent, &ext)
    };
    // MAKEPAD_RUN_COLOR_TRACE=<name substring>: each slot of the matching fns, escaping or not
    static TRACE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    if let Some(w) = TRACE.get_or_init(|| std::env::var("MAKEPAD_RUN_COLOR_TRACE").ok()) {
        if f.name.contains(w.as_str()) {
            for s in 0..ns {
                if f.slots[s].size >= min && addressed[s] {
                    let k = ci[s];
                    let iv: &[(u32, u32, u32)] = if k != usize::MAX { &live[k] } else { &[] };
                    eprintln!("color {}: slot {} size {} escapes {} candidate {} live {:?}", f.name, s, f.slots[s].size, esc[s], k != usize::MAX, iv);
                }
            }
        }
    }
    // greedy colouring, biggest first; a colour's storage is its first (largest) member's. A
    // candidate takes the first colour none of whose members' intervals meets one of its own:
    // per block, the intervals placed there with their colours (merging each colour's interval
    // list per placement and walking it per test was quadratic in a big function)
    let mut order: Vec<usize> = (0..nc).collect();
    order.sort_by(|&x, &y| f.slots[cand[y]].size.cmp(&f.slots[cand[x]].size).then(x.cmp(&y)));
    let mut occ: Vec<Vec<(u32, u32, u32)>> = vec![Vec::new(); nb];
    let mut reps: Vec<usize> = Vec::new();
    let mut hit: Vec<u32> = Vec::new();
    let mut to: Vec<u32> = (0..ns as u32).collect();
    let mut shared = false;
    for (n, k) in order.into_iter().enumerate() {
        let s = cand[k];
        let stamp = n as u32 + 1;
        for &(b, lo, hi) in &live[k] {
            for &(c, x, y) in &occ[b as usize] {
                if !(y < lo || hi < x) {
                    hit[c as usize] = stamp;
                }
            }
        }
        let c = match (0..reps.len()).find(|&c| hit[c] != stamp) {
            Some(c) => {
                to[s] = reps[c] as u32;
                shared = true;
                c
            }
            None => {
                reps.push(s);
                hit.push(0);
                reps.len() - 1
            }
        };
        for &(b, lo, hi) in &live[k] {
            occ[b as usize].push((c as u32, lo, hi));
        }
    }
    if shared { Some(to) } else { None }
}

/// Positions for a straight-line body, including chains of unconditional jumps.
/// Require every block in the chain; all other CFGs keep the full lifetime analysis.
pub fn slot_linear_positions(f: &Func) -> Option<Vec<u32>> {
    if f.blocks.is_empty() || f.blocks.iter().any(|b| matches!(b.term, Term::Branch(..) | Term::Invoke(..))) {
        return None;
    }
    let mut positions = vec![u32::MAX; f.blocks.len()];
    let (mut b, mut pos, mut seen) = (0usize, 0u32, 0usize);
    loop {
        if positions[b] != u32::MAX {
            return None;
        }
        positions[b] = pos;
        seen += 1;
        pos += 2 * f.blocks[b].insts.len() as u32 + 2;
        match f.blocks[b].term {
            Term::Jump(next) => b = next as usize,
            Term::Ret(_) | Term::Unreachable | Term::Resume => return (seen == f.blocks.len()).then_some(positions),
            Term::Branch(..) | Term::Switch(..) | Term::Invoke(..) => return None,
        }
    }
}

pub fn slot_live_intervals(f: &Func, cand: &[usize], der: &[Vec<u32>], acc: &[Vec<(u32, u32, u32)>], ent: &[Vec<Option<bool>>], ext: &[Vec<bool>]) -> Vec<Vec<(u32, u32, u32)>> {
    let nb = f.blocks.len();
    let nc = cand.len();
    // predecessors
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); nb];
    for (bi, b) in f.blocks.iter().enumerate() {
        for s in b.term.succs() {
            preds[s as usize].push(bi as u32);
        }
    }
    // Live intervals per candidate, (block, first, last position) in block order: where the
    // slot may hold a value (W: forward from its accesses, ended by an argument copy's
    // consuming call -- the callee owns the copy, the caller never reads it) that may still be
    // read (R: backward from its reads, ended by a whole write -- a copy over all of it, or the
    // call it is the result buffer of). Within a block from its entry when live there, else
    // from its first access, to its end when live there, else to its last access.
    // All candidates at once, one bit each (per-candidate walks over the CFG were most of the
    // analysis' cost at cold start: a slot live across a big function walked all of it).
    let w = (nc + 63) / 64;
    // per block: candidates with an access (acc), whose last access is no consuming call (W
    // gen), whose first is a read (R gen), whose accesses neither read first nor kill (R passes)
    let mut has = vec![0u64; nb * w];
    let mut w_gen = vec![0u64; nb * w];
    let mut r_gen = vec![0u64; nb * w];
    let mut r_pass = vec![0u64; nb * w];
    for k in 0..nc {
        let bit = 1u64 << (k % 64);
        for (j, &(b, _, _)) in acc[k].iter().enumerate() {
            let o = b as usize * w + k / 64;
            has[o] |= bit;
            if !ext[k][j] {
                w_gen[o] |= bit;
            }
            match ent[k][j] {
                Some(true) => r_gen[o] |= bit,
                Some(false) => {}
                None => r_pass[o] |= bit,
            }
        }
    }
    // W (forward): w_out = gen | (w_in & !has), w_in = union of the predecessors' w_out
    // R (backward): r_in = gen | (r_out & (!has | pass)), r_out = union of the successors' r_in
    let mut w_in = vec![0u64; nb * w];
    let mut w_out = w_gen.clone();
    let mut r_in = r_gen.clone();
    let mut r_out = vec![0u64; nb * w];
    let order = block_rpo(f);
    let mut changed = true;
    while changed {
        changed = false;
        for &b in &order {
            let b = b as usize;
            for j in 0..w {
                let mut x = 0u64;
                for &pb in &preds[b] {
                    x |= w_out[pb as usize * w + j];
                }
                let o = b * w + j;
                let y = w_gen[o] | (x & !has[o]);
                if x != w_in[o] || y != w_out[o] {
                    w_in[o] = x;
                    w_out[o] = y;
                    changed = true;
                }
            }
        }
    }
    changed = true;
    while changed {
        changed = false;
        for &b in order.iter().rev() {
            let b = b as usize;
            for j in 0..w {
                let mut x = 0u64;
                for s in f.blocks[b].term.succs() {
                    x |= r_in[s as usize * w + j];
                }
                let o = b * w + j;
                let y = r_gen[o] | (x & (!has[o] | r_pass[o]));
                if x != r_out[o] || y != r_in[o] {
                    r_out[o] = x;
                    r_in[o] = y;
                    changed = true;
                }
            }
        }
    }
    // intervals, in block order: within a block from its entry when live there, else from the
    // first access, to its end when live there, else to the last access
    let mut live: Vec<Vec<(u32, u32, u32)>> = vec![Vec::new(); nc];
    let mut ai = vec![0usize; nc];
    for b in 0..nb {
        let end = 2 * f.blocks[b].insts.len() as u32 + 1;
        for j in 0..w {
            let o = b * w + j;
            let lin = w_in[o] & r_in[o];
            let lout = w_out[o] & r_out[o];
            let mut m = lin | lout | has[o];
            while m != 0 {
                let t = m.trailing_zeros();
                m &= m - 1;
                let k = j * 64 + t as usize;
                let (li, lo) = (lin >> t & 1 != 0, lout >> t & 1 != 0);
                if has[o] >> t & 1 != 0 {
                    let (_, x, y) = acc[k][ai[k]];
                    ai[k] += 1;
                    live[k].push((b as u32, if li { 0 } else { x }, if lo { end } else { y }));
                } else {
                    live[k].push((b as u32, 0, end));
                }
            }
        }
    }
    // Lowered code (a baseline body: no pass has moved anything): lowering emits one SlotAddr
    // per slot, where the slot's value begins (the temporary, the `let`, the call's result
    // buffer or argument copy), so a slot's earlier contents are dead whenever its SlotAddr runs
    // -- a temporary inside a loop body is a new one per iteration (rustc: StorageLive). And
    // memory a slot that does not escape can be reached only through values derived from its
    // address. So such a slot is live only where one of those values is: in a loop that frees
    // what the reachability above cannot (it makes every slot a loop body touches live around
    // the whole loop). Passes move, sink and duplicate SlotAddrs (and inlined bodies come
    // optimised), so this holds only for `Slot::born` slots of a baseline body.
    if f.baseline && cand.iter().any(|&s| f.slots[s].born) {
        born_liveness(f, cand, der, &mut live);
    }
    live
}

/// Per vreg: defined once, by a SlotAddr (the slot's own address, offset 0).
pub fn slot_addr_vregs(f: &Func) -> Vec<bool> {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut sa = vec![false; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            if let Inst::SlotAddr(d, _) = i {
                sa[d.0 as usize] = true;
            }
        }
    }
    (0..nv).map(|v| sa[v] && ndef[v] == 1).collect()
}

/// Per value: its frame position (`at[slot] + offset`, slot_layout's numbering) when its one
/// definition is a slot address, or a single-definition Mov / constant Add of one; i64::MIN
/// otherwise. Slot colouring gives a slot copied whole into another, and dead after, the
/// destination's storage (slot_colors_used): that Copy has the same position on both sides
/// and moves nothing, so the backends emit no code for it (its loads would also wait on the
/// narrow stores that just built the value: a failed store forward per copy).
pub fn slot_addr_positions(f: &Func, at: &[i64]) -> Vec<i64> {
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
    let mut pos = vec![i64::MIN; nv];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::SlotAddr(d, s) = i {
                if ndef[d.0 as usize] == 1 {
                    if let Some(&a) = at.get(*s as usize) {
                        pos[d.0 as usize] = a;
                    }
                }
            }
        }
    }
    let mut grew = true;
    while grew {
        grew = false;
        for b in &f.blocks {
            for i in &b.insts {
                let (d, s, k) = match i {
                    Inst::Mov(d, s) => (*d, *s, 0),
                    Inst::IBinI(IOp::Add, IntTy { bits: 64, .. }, d, s, k) => (*d, *s, *k),
                    _ => continue,
                };
                // a frame position grows downwards: slot bytes at `at - offset`
                if ndef[d.0 as usize] == 1 && pos[d.0 as usize] == i64::MIN && pos[s.0 as usize] != i64::MIN {
                    pos[d.0 as usize] = pos[s.0 as usize] - k;
                    grew = true;
                }
            }
        }
    }
    pos
}

/// Every block once: reverse postorder of those reachable from the entry, then the others in
/// index order (dataflow sweeps in this order settle in about loop depth + 2 passes; lowering's
/// block numbers are not in flow order -- a join is made before the blocks that reach it).
pub fn block_rpo(f: &Func) -> Vec<u32> {
    let nb = f.blocks.len();
    let mut seen = vec![false; nb];
    let mut post: Vec<u32> = Vec::with_capacity(nb);
    let mut stack: Vec<(u32, crate::rir::Succs)> = Vec::new();
    if nb > 0 {
        seen[0] = true;
        stack.push((0, f.blocks[0].term.succs()));
    }
    while let Some((b, it)) = stack.last_mut() {
        match it.next() {
            Some(s) if !seen[s as usize] => {
                seen[s as usize] = true;
                stack.push((s, f.blocks[s as usize].term.succs()));
            }
            Some(_) => {}
            None => {
                post.push(*b);
                stack.pop();
            }
        }
    }
    post.reverse();
    post.extend((0..nb as u32).filter(|&b| !seen[b as usize]));
    post
}

/// `slot_colors` for the `Slot::born` candidates of a baseline body: their intervals narrowed
/// to where a value derived from their address is live (backward liveness of those values;
/// within a block from the entry when one is live in, else from its first reference, to the
/// end when one is live out, else to its last reference).
pub fn born_liveness(f: &Func, cand: &[usize], der: &[Vec<u32>], live: &mut [Vec<(u32, u32, u32)>]) {
    let nb = f.blocks.len();
    let ns = f.slots.len();
    let nc = cand.len();
    let mut ci = vec![usize::MAX; ns];
    for (k, &s) in cand.iter().enumerate() {
        if f.slots[s].born {
            ci[s] = k;
        }
    }
    // the values that may point into a born candidate, numbered
    let mut idx = vec![u32::MAX; der.len()];
    let mut vals: Vec<u32> = Vec::new();
    for (v, d) in der.iter().enumerate() {
        if d.iter().any(|&s| ci[s as usize] != usize::MAX) {
            idx[v] = vals.len() as u32;
            vals.push(v as u32);
        }
    }
    if vals.is_empty() {
        return;
    }
    let nx = vals.len();
    let w = (nx + 63) / 64;
    let has = |set: &[u64], x: usize| set[x / 64] & (1 << (x % 64)) != 0;
    // per block: values used before any def (gen), defined (kill)
    let mut gen = vec![0u64; nb * w];
    let mut kill = vec![0u64; nb * w];
    let mut tu = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let (g, k) = (&mut gen[bi * w..(bi + 1) * w], &mut kill[bi * w..(bi + 1) * w]);
        let use_v = |x: usize, g: &mut [u64], k: &[u64]| {
            if k[x / 64] & (1 << (x % 64)) == 0 {
                g[x / 64] |= 1 << (x % 64);
            }
        };
        for i in &b.insts {
            for_uses(i, |v| {
                let x = idx[v.0 as usize];
                if x != u32::MAX {
                    use_v(x as usize, g, k);
                }
            });
            for_defs(i, |v| {
                let x = idx[v.0 as usize];
                if x != u32::MAX {
                    k[x as usize / 64] |= 1 << (x % 64);
                }
            });
        }
        term_uses(&b.term, &mut tu);
        for v in &tu {
            let x = idx[v.0 as usize];
            if x != u32::MAX {
                use_v(x as usize, g, k);
            }
        }
    }
    // live_in = gen | (live_out & !kill), live_out = union of the successors' live_in
    let mut lin = vec![0u64; nb * w];
    let mut lout = vec![0u64; nb * w];
    let order = block_rpo(f);
    let mut changed = true;
    while changed {
        changed = false;
        for &bi in order.iter().rev() {
            let bi = bi as usize;
            for j in 0..w {
                let mut o = 0u64;
                for s in f.blocks[bi].term.succs() {
                    o |= lin[s as usize * w + j];
                }
                let i = gen[bi * w + j] | (o & !kill[bi * w + j]);
                if o != lout[bi * w + j] || i != lin[bi * w + j] {
                    lout[bi * w + j] = o;
                    lin[bi * w + j] = i;
                    changed = true;
                }
            }
        }
    }
    // per block: each present value's interval, then each born candidate's (their hull)
    let mut born: Vec<Vec<(u32, u32, u32)>> = vec![Vec::new(); nc];
    let mut first = vec![u32::MAX; nx];
    let mut last = vec![0u32; nx];
    let mut touched: Vec<usize> = Vec::new();
    let mut present: Vec<usize> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        touched.clear();
        let mut note = |v: VReg, lo: u32, hi: u32, touched: &mut Vec<usize>| {
            let x = idx[v.0 as usize];
            if x != u32::MAX {
                let x = x as usize;
                if first[x] == u32::MAX {
                    first[x] = lo;
                    touched.push(x);
                }
                last[x] = hi;
            }
        };
        for (ii, i) in b.insts.iter().enumerate() {
            // positions as slot_colors' (a reference takes both of its instruction's)
            let p = 2 * ii as u32;
            for_uses(i, |v| note(v, p, p + 1, &mut touched));
            for_defs(i, |v| note(v, p, p + 1, &mut touched));
        }
        let end = 2 * b.insts.len() as u32 + 1;
        term_uses(&b.term, &mut tu);
        for v in &tu {
            note(*v, end - 1, end, &mut touched);
        }
        let (li, lo) = (&lin[bi * w..(bi + 1) * w], &lout[bi * w..(bi + 1) * w]);
        present.clear();
        present.extend_from_slice(&touched);
        for j in 0..w {
            let mut m = li[j] | lo[j];
            while m != 0 {
                let x = j * 64 + m.trailing_zeros() as usize;
                m &= m - 1;
                if first[x] == u32::MAX {
                    present.push(x);
                }
            }
        }
        for &x in &present {
            let a = if has(li, x) || first[x] == u32::MAX { 0 } else { first[x] };
            let z = if has(lo, x) || first[x] == u32::MAX { end } else { last[x] };
            for &s in &der[vals[x] as usize] {
                let k = ci[s as usize];
                if k == usize::MAX {
                    continue;
                }
                match born[k].last_mut() {
                    Some(e) if e.0 == bi as u32 => {
                        e.1 = e.1.min(a);
                        e.2 = e.2.max(z);
                    }
                    _ => born[k].push((bi as u32, a, z)),
                }
            }
        }
        for &x in &touched {
            first[x] = u32::MAX;
        }
    }
    // narrowed: the intersection of both (each covers every position the slot is live at)
    for (k, &s) in cand.iter().enumerate() {
        if !f.slots[s].born {
            continue;
        }
        let (a, b) = (&live[k], &born[k]);
        let mut out = Vec::with_capacity(a.len());
        let mut j = 0;
        for &(blk, x, y) in a.iter() {
            while j < b.len() && b[j].0 < blk {
                j += 1;
            }
            if j < b.len() && b[j].0 == blk {
                let (lo, hi) = (x.max(b[j].1), y.min(b[j].2));
                if lo <= hi {
                    out.push((blk, lo, hi));
                }
            }
        }
        live[k] = out;
    }
}

/// The frame offsets of a function's stack slots: `off` is where the slot area starts (bytes
/// below the frame's base); each slot something addresses gets the end of its storage
/// (offset of its lowest byte below the base, its alignment respected), slots sharing storage
/// (`slot_colors`) the same; unaddressed slots take no space. Returns the offsets and the new
/// end of the area. `share` false (MAKEPAD_RUN_OPT=-slotcolor): every slot keeps its own storage.
pub fn slot_layout(f: &Func, off: i64, share: bool) -> (Vec<i64>, i64) {
    slot_layout_aligned(f, off, share, |_| 1)
}

/// `slot_layout` with each storage aligned to at least `min_align(size)` (arm64: 8-byte and
/// 16-byte aggregates on 8 / 16, so their 8-byte fields and q pairs take scaled immediate
/// offsets instead of an offset register)
pub fn slot_layout_aligned(f: &Func, mut off: i64, share: bool, min_align: fn(u32) -> u32) -> (Vec<i64>, i64) {
    let ns = f.slots.len();
    if ns == 0 {
        return (Vec::new(), off);
    }
    let used = crate::rir::used_slots(f);
    // Small baseline frames do not repay the address/lifetime analysis. Keep
    // colouring sizeable frames: recursive parsers rely on their stack savings.
    let share = share && (!f.baseline || ns >= 8 && f.slots.iter().map(|s| s.size as u64).sum::<u64>() >= SLOT_SHARE_MIN_BYTES);
    let to = if share { slot_colors_used(f, SLOT_SHARE_MIN, &used) } else { None };
    let mut at = vec![off; ns];
    let Some(to) = to else {
        for (s, slot) in f.slots.iter().enumerate() {
            if used[s] {
                let al = slot.align.max(min_align(slot.size)).max(1) as i64;
                off += slot.size as i64;
                off = (off + al - 1) / al * al;
                at[s] = off;
            }
        }
        return (at, off);
    };
    // a shared storage's size and alignment: its members' largest
    let mut size: Vec<u32> = f.slots.iter().map(|s| s.size).collect();
    let mut align: Vec<u32> = f.slots.iter().map(|s| s.align.max(1)).collect();
    for s in 0..ns {
        let r = to[s] as usize;
        if r != s && used[s] {
            size[r] = size[r].max(f.slots[s].size);
            align[r] = align[r].max(f.slots[s].align.max(1));
        }
    }
    for s in 0..ns {
        if !used[s] || to[s] as usize != s {
            continue;
        }
        let al = align[s].max(min_align(size[s])) as i64;
        off += size[s] as i64;
        off = (off + al - 1) / al * al;
        at[s] = off;
    }
    for s in 0..ns {
        if to[s] as usize != s {
            at[s] = at[to[s] as usize];
        }
    }
    (at, off)
}

/// slots from this size up share storage when their lifetimes cannot overlap (slot_layout)
pub const SLOT_SHARE_MIN: u32 = 8;

/// functions whose slots of that size take less than this keep them apart (slot_colors)
pub const SLOT_SHARE_MIN_BYTES: u64 = 256;

/// color_slots' address analysis: the SET of slots (up to SLOT_SET_MAX) each value may point
/// into -- through moves, address arithmetic, selects and calls (a call's results may point
/// into any slot its arguments point into: `slice::get(&a, &b) -> &T`); a value that may point
/// into several slots keeps all of them live where it is used, instead of making them escape.
/// esc[s]: an address into s is stored to memory, returned, a parameter, or in a set too big.
pub fn slot_sets(f: &Func) -> Option<(Vec<Vec<u32>>, Vec<bool>, Vec<bool>)> {
    const SLOT_SET_MAX: usize = 4;
    let ns = f.slots.len();
    let nv = f.vregs.len();
    let mut der: Vec<Vec<u32>> = vec![Vec::new(); nv];
    let mut esc = vec![false; ns];
    let mut changed = true;
    let mut rounds = 0;
    let mut src: Vec<u32> = Vec::new();
    while changed && rounds < 16 {
        changed = false;
        rounds += 1;
        for b in &f.blocks {
            for i in &b.insts {
                src.clear();
                match i {
                    Inst::SlotAddr(_, s) => src.push(*s),
                    Inst::Load(..) | Inst::LoadX(..) | Inst::VLoad(..) | Inst::VLoadX(..) | Inst::AtomicLoad(..) | Inst::Iconst(..) | Inst::Fconst(..) | Inst::Addr(..) | Inst::FnAddr(..) | Inst::TlsAddr(..) | Inst::TlsStatic(..) | Inst::VConst(..) => continue,
                    // a call's results do not point into its own memory (a result buffer, an
                    // argument copy: see below; a copy that turns out not to be one escapes)
                    Inst::Call(c, args, rets) => {
                        if rets.is_empty() {
                            continue;
                        }
                        if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                            src.extend(der[v.0 as usize].iter().copied());
                        }
                        let sret = matches!(c, Callee::Fn(id) if f.sret_callees.contains(id));
                        for (k, a) in args.iter().enumerate() {
                            let d = &der[a.0 as usize];
                            if (sret && k == 0) || matches!(d.as_slice(), [x] if f.slots[*x as usize].arg) {
                                continue;
                            }
                            for &x in d {
                                if !src.contains(&x) {
                                    src.push(x);
                                }
                            }
                        }
                    }
                    _ => {
                        let mut has_def = false;
                        for_defs(i, |_| has_def = true);
                        if !has_def {
                            continue;
                        }
                        for_uses(i, |v| {
                            for &d in &der[v.0 as usize] {
                                if !src.contains(&d) {
                                    src.push(d);
                                }
                            }
                        });
                    }
                }
                if src.is_empty() {
                    continue;
                }
                for_defs(i, |d| {
                    let cur = &mut der[d.0 as usize];
                    for &s in &src {
                        if !cur.contains(&s) {
                            cur.push(s);
                            changed = true;
                        }
                    }
                });
            }
        }
        // sets too big: their slots escape, the sets stop growing
        for v in der.iter_mut() {
            if v.len() > SLOT_SET_MAX {
                for &s in v.iter() {
                    esc[s as usize] = true;
                }
                v.truncate(SLOT_SET_MAX);
            }
        }
    }
    if changed {
        return None;
    }
    let mark = |v: VReg, esc: &mut Vec<bool>| {
        for &s in &der[v.0 as usize] {
            esc[s as usize] = true;
        }
    };
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::Store(_, _, _, v) | Inst::StoreX(_, _, _, _, _, v) | Inst::AtomicStore(_, _, _, v) | Inst::AtomicRmw(_, _, _, _, _, v) => mark(*v, &mut esc),
                Inst::AtomicCas(_, _, _, _, _, e, n) => {
                    mark(*e, &mut esc);
                    mark(*n, &mut esc);
                }
                _ => {}
            }
        }
        if let Term::Ret(vs) = &b.term {
            for v in vs {
                mark(*v, &mut esc);
            }
        }
    }
    // a slot address handed to a call together with another argument that may point to memory:
    // the callee may store the address there (an iterator adapter built into its result buffer
    // keeps `&hist`: `hist.iter().enumerate().take(n)` not inlined), and later reads of that
    // memory are loads, invisible to `der` -- the slot escapes. Only constants are known not to
    // point anywhere. (deflate's radix_sort_symbols: `offsets` was coloured onto `hist`, dead by
    // `der` after the adapter call while the loop still read it through the adapter.)
    let mut konst = vec![false; nv];
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            if let Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::FnAddr(d, _) = i {
                konst[d.0 as usize] = true;
            }
        }
    }
    // The call's own memory does not escape through it: a result buffer (the first argument of
    // a call returning in memory: `sret_callees`) and a by-value argument copy (`Slot::arg`) --
    // the callee writes the result / owns the copy and keeps no pointer to either past the call
    // (rustc: indirect arguments and the indirect return place are `noalias captures(address)`:
    // no access through a kept pointer after the return). A result buffer is still memory the
    // callee may store the other arguments' addresses into (the result can hold them); a copy
    // is not, when nothing reads it (the callee owns it). A copy counts as one only while it is
    // what lowering made: handed to one call, as one argument, never read, nor mixed with other
    // slots' addresses (passes such as destination forwarding may give it other uses).
    let mut arg_ok: Vec<bool> = f.slots.iter().map(|s| s.arg).collect();
    if arg_ok.iter().any(|x| *x) {
        let mut calls = vec![0u32; ns];
        let one = |v: VReg| -> Option<usize> {
            match der[v.0 as usize].as_slice() {
                [s] => Some(*s as usize),
                _ => None,
            }
        };
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Call(c, args, _) => {
                        if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                            for &d in &der[v.0 as usize] {
                                arg_ok[d as usize] = false;
                            }
                        }
                        for a in args {
                            match one(*a) {
                                Some(d) => calls[d] += 1,
                                None => {
                                    for &d in &der[a.0 as usize] {
                                        arg_ok[d as usize] = false;
                                    }
                                }
                            }
                        }
                    }
                    Inst::Load(_, _, a, _) | Inst::LoadX(_, _, a, ..) | Inst::VLoad(_, a, _) | Inst::VLoadX(_, a, ..) | Inst::VGather(_, a, _) | Inst::VTbl(_, a, _, _) | Inst::AtomicLoad(_, _, _, a) | Inst::AtomicRmw(_, _, _, _, a, _) | Inst::AtomicCas(_, _, _, _, a, _, _) | Inst::Copy(_, a, _) => {
                        for &d in &der[a.0 as usize] {
                            arg_ok[d as usize] = false;
                        }
                    }
                    _ => {}
                }
            }
        }
        for d in 0..ns {
            if calls[d] != 1 {
                arg_ok[d] = false;
            }
            // the derivation above left its call results out
            if f.slots[d].arg && !arg_ok[d] {
                esc[d] = true;
            }
        }
    }
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Call(c, args, _) = i {
                if args.iter().any(|a| !der[a.0 as usize].is_empty()) {
                    let sret = matches!(c, Callee::Fn(id) if f.sret_callees.contains(id));
                    let copy = |a: VReg| matches!(der[a.0 as usize].as_slice(), [d] if arg_ok[*d as usize]);
                    // arguments that may point to memory the callee can store an address into
                    let pointerish = |a: VReg| !(konst[a.0 as usize] && ndef[a.0 as usize] == 1) && !copy(a);
                    let n = args.iter().filter(|a| pointerish(**a)).count();
                    for (k, a) in args.iter().enumerate() {
                        if (sret && k == 0) || copy(*a) {
                            continue;
                        }
                        if n > pointerish(*a) as usize {
                            mark(*a, &mut esc);
                        }
                    }
                }
            }
        }
    }
    Some((der, esc, arg_ok))
}
