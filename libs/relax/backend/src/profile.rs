//! Block counts of the counted tier (PGO): the instrumentation a counted build gets, the
//! frequencies O3 derives from the counts it carries on its blocks (`Block::count`), and the
//! profile-driven block layout.
//!
//! Counted builds (jit.rs `counted_build`) are the slot's O1 code plus one counter per block of
//! its O3 input; their counts go onto the O3 input's blocks 1:1 and the passes carry them (copies
//! keep the count of what they copied). Blocks no count reached (inlined callees, blocks the
//! passes made) get an estimate from the nearest counted predecessor, scaled by the static
//! ratio of the two (loop depth and branch splits), so a function never mixes measured and
//! static weights.

use crate::rir::{Block, Callee, Cls, Cond, Func, IOp, Inst, Mem, Sym, Term, I64};

/// Block `b`'s counter is the u64 at `base + 8 * b`. Each block starts with a plain (racy,
/// non-atomic) load, add and store of its counter: counts are a frequency, never a fact.
///
/// A block of moves only that jumps on or returns gets no counter (its count comes from its
/// predecessors): a call followed by such blocks is in tail position (tail::returns_results), and a
/// counter there would turn a sibling call back into a call (a tail-recursive interpreter then
/// overflows its stack in the counted code).
/// Returns per block whether it got a counter.
pub fn instrument(f: &mut Func, base: u64) -> Vec<bool> {
    let mut counted = vec![false; f.blocks.len()];
    for b in 0..f.blocks.len() {
        let blk = &f.blocks[b];
        if blk.insts.iter().all(|i| matches!(i, Inst::Mov(..))) && matches!(blk.term, Term::Jump(_) | Term::Ret(_)) {
            continue;
        }
        counted[b] = true;
        let (a, v, w) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        let blk: &mut Block = &mut f.blocks[b];
        let at = blk.pos.first().copied().unwrap_or(blk.term_pos);
        let had_pos = blk.pos.len() == blk.insts.len();
        let ins = [
            Inst::Iconst(a, (base + 8 * b as u64) as i64),
            Inst::Load(Mem::Int(8, false), v, a, 0),
            Inst::IBinI(IOp::Add, I64, w, v, 1),
            Inst::Store(Mem::Int(8, false), a, 0, w),
        ];
        blk.insts.splice(0..0, ins);
        if had_pos {
            blk.pos.splice(0..0, [at; 4]);
        }
    }
    counted
}

/// Back-edge counting (tier-up of loops; HotSpot's backedge counters, the CLR's Tier0 patchpoint
/// counters): every block a back edge goes to (`header`: back_edge_targets; every loop iteration
/// passes one) starts with a decrement of the
/// slot's loop counter, the 8-byte word at `counter` (a relocated symbol: the code stays
/// cacheable). When the count drops below zero the block calls `helper(counter address)` (the
/// tier-up decision, which resets the count) and goes on. The block's own code moves to a new
/// block right after it, which the counting block falls into; the calls go after all other blocks.
/// False: no loop, nothing changed.
pub fn loop_counters(f: &mut Func, header: &[bool], counter: Sym, helper: Sym) -> bool {
    let nb = f.blocks.len();
    if header.len() != nb || !header.iter().any(|h| *h) {
        return false;
    }
    // block order: each counting block, then its body; the helper calls last
    let mut order: Vec<u32> = Vec::with_capacity(nb + 2 * header.iter().filter(|h| **h).count());
    let mut colds: Vec<u32> = Vec::new();
    for t in 0..nb {
        order.push(t as u32);
        if !header[t] {
            continue;
        }
        let at = f.blocks[t].pos.first().copied().unwrap_or(f.blocks[t].term_pos);
        let body = f.block();
        let cold = f.block();
        let (a, v, w, c, a2) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        let old = std::mem::replace(&mut f.blocks[t], Block { insts: Vec::new(), pos: Vec::new(), term: Term::Unreachable, term_pos: at, count: -1.0 });
        let count = old.count;
        f.blocks[body as usize] = old;
        let blk = &mut f.blocks[t];
        blk.insts = vec![
            Inst::Addr(a, counter),
            Inst::Load(Mem::Int(8, false), v, a, 0),
            Inst::IBinI(IOp::Sub, I64, w, v, 1),
            Inst::Store(Mem::Int(8, false), a, 0, w),
            Inst::ICmpI(Cond::Lt, true, c, w, 0),
        ];
        blk.pos = vec![at; 5];
        blk.term = Term::Branch(c, cold, body);
        blk.count = count;
        let cb = &mut f.blocks[cold as usize];
        cb.insts = vec![Inst::Addr(a2, counter), Inst::Call(Callee::Host(helper), vec![a2], Vec::new())];
        cb.pos = vec![at; 2];
        cb.term = Term::Jump(body);
        cb.term_pos = at;
        order.push(body);
        colds.push(cold);
    }
    order.extend(colds);
    let mut map = vec![0u32; order.len()];
    for (i, b) in order.iter().enumerate() {
        map[*b as usize] = i as u32;
    }
    let mut old: Vec<Option<Block>> = std::mem::take(&mut f.blocks).into_iter().map(Some).collect();
    for b in &order {
        let mut blk = old[*b as usize].take().unwrap();
        blk.term.map_succs(|t| map[t as usize]);
        f.blocks.push(blk);
    }
    true
}

/// The blocks a back edge of a depth-first walk from the entry goes to (an edge to a block still
/// on the walk's stack): every cycle has one, and code without a cycle has none (a return block
/// placed before its callers is no target, so calls before it stay in tail position).
pub fn back_edge_targets(f: &Func) -> Vec<bool> {
    let nb = f.blocks.len();
    let mut header = vec![false; nb];
    if nb == 0 {
        return header;
    }
    // 0 unvisited, 1 on the stack, 2 done
    let mut state = vec![0u8; nb];
    let mut stack: Vec<(u32, Vec<u32>)> = vec![(0, f.blocks[0].term.succs().collect())];
    state[0] = 1;
    while let Some((b, succs)) = stack.last_mut() {
        match succs.pop() {
            Some(t) => {
                let t = t as usize;
                match state[t] {
                    0 => {
                        state[t] = 1;
                        let next: Vec<u32> = f.blocks[t].term.succs().collect();
                        stack.push((t as u32, next));
                    }
                    1 => header[t] = true,
                    _ => {}
                }
            }
            None => {
                state[*b as usize] = 2;
                stack.pop();
            }
        }
    }
    header
}

/// Some block carries a count.
pub fn has_counts(f: &Func) -> bool {
    f.blocks.iter().any(|b| b.count >= 0.0)
}

/// A static relative estimate per block: path mass over the forward CFG (even splits) times
/// 8^loop depth; panic and unwind paths near zero.
fn structural(f: &Func, reach: &[bool], rpo: &[u32]) -> Vec<f64> {
    let nb = f.blocks.len();
    let depth = crate::regalloc::loop_depths(f, reach);
    let mut rank = vec![usize::MAX; nb];
    for (i, &b) in rpo.iter().enumerate() {
        rank[b as usize] = i;
    }
    let cold = |t: u32| matches!(f.blocks[t as usize].term, Term::Unreachable | Term::Resume);
    let mut mass = vec![0.0f64; nb];
    if nb > 0 {
        mass[0] = 1.0;
    }
    for &b in rpo {
        let b = b as usize;
        if !reach[b] {
            continue;
        }
        let fwd = |t: &u32| rank[*t as usize] > rank[b];
        let n = f.blocks[b].term.succs().filter(fwd).count();
        if n == 0 {
            continue;
        }
        let share = mass[b] / n as f64;
        for t in f.blocks[b].term.succs().filter(fwd) {
            mass[t as usize] += if cold(t) { share * 1e-3 } else { share };
        }
    }
    (0..nb).map(|b| (mass[b] * 8f64.powi(depth[b].min(6) as i32)).max(1e-9)).collect()
}

/// Per block, its count, or for a block without one an estimate: the largest over its forward
/// predecessors p (reverse postorder) of `est(p) * s(b) / s(p)` with the static estimate s.
/// None without counts or when nothing ran.
pub fn estimates(f: &Func) -> Option<Vec<f64>> {
    if !has_counts(f) {
        return None;
    }
    let nb = f.blocks.len();
    let reach = crate::regalloc::reachable(f);
    let rpo = crate::frame::block_rpo(f);
    let mut known: Vec<Option<f64>> = f.blocks.iter().map(|b| (b.count >= 0.0).then_some(b.count as f64)).collect();
    let top = known.iter().flatten().copied().fold(0.0f64, f64::max);
    if top <= 0.0 {
        return None;
    }
    let s = structural(f, &reach, &rpo);
    let smax = s.iter().copied().fold(0.0f64, f64::max).max(1e-9);
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); nb];
    for (b, blk) in f.blocks.iter().enumerate() {
        for t in blk.term.succs() {
            preds[t as usize].push(b as u32);
        }
    }
    let mut rank = vec![usize::MAX; nb];
    for (i, &b) in rpo.iter().enumerate() {
        rank[b as usize] = i;
    }
    for &b in &rpo {
        let b = b as usize;
        if known[b].is_some() {
            continue;
        }
        let mut est: Option<f64> = None;
        for &p in &preds[b] {
            let p = p as usize;
            if rank[p] >= rank[b] {
                continue;
            }
            if let Some(c) = known[p] {
                let e = c * s[b] / s[p];
                est = Some(est.map_or(e, |x: f64| x.max(e)));
            }
        }
        known[b] = Some(est.unwrap_or(top * s[b] / smax));
    }
    Some(known.into_iter().map(|x| x.unwrap_or(0.0)).collect())
}

/// Block frequencies relative to the hottest block (0..=1, floored at 1e-4: an estimate is
/// never proof that a block does not run), for the register allocator's block weights.
pub fn freqs(f: &Func) -> Option<Vec<f32>> {
    let est = estimates(f)?;
    let top = est.iter().copied().fold(0.0f64, f64::max);
    if top <= 0.0 {
        return None;
    }
    Some(est.iter().map(|e| ((e / top) as f32).max(1e-4)).collect())
}

/// The inliner's frequencies of a counted body, per entry of the function: a block that runs
/// on half the calls or more gets the inliner's hot level (codegen opt::HOT_FREQ, 0.05) or above,
/// one that did not run exactly 0.0 (its sites are cold: only free bodies inline there). Relative
/// to the entry, not to the hottest block: exact loop counts (millions per call) would leave every
/// site outside the innermost loop below the hot level.
pub fn inline_freqs(f: &Func) -> Option<Vec<f32>> {
    const HOT_PER_ENTRY: f64 = 0.5;
    const HOT_FREQ: f64 = 0.05;
    let est = estimates(f)?;
    let entry = est.first().copied().unwrap_or(0.0);
    (entry > 0.0).then(|| est.iter().map(|e| (e / entry * HOT_FREQ / HOT_PER_ENTRY) as f32).collect())
}

/// Blocks of a counted body that ran at least `REPEAT` times per entry of the function (a loop
/// the counts saw iterate): the sites worth a bottom-up preparation. None without counts.
pub fn repeated_blocks(f: &Func) -> Option<Vec<bool>> {
    const REPEAT: f64 = 2.0;
    let est = estimates(f)?;
    let entry = est.first().copied().unwrap_or(0.0).max(1.0);
    Some(est.iter().map(|e| *e >= REPEAT * entry).collect())
}

/// Profile-driven block order (Pettis-Hansen chains): edges by estimated count, hottest first,
/// join the chain ending at their source to the chain starting at their target, so the likelier
/// successor falls through; chains then follow the entry's, each next the one most heavily
/// entered from the code placed so far, blocks that never ran last (in their old order).
/// Changes only the order (the entry stays first). False: no counts, or the order is the same.
pub fn layout(f: &mut Func) -> bool {
    let nb = f.blocks.len();
    if nb < 3 {
        return false;
    }
    let Some(est) = estimates(f) else { return false };
    let reach = crate::regalloc::reachable(f);
    let mut pred_sum = vec![0.0f64; nb];
    for (b, blk) in f.blocks.iter().enumerate() {
        if !reach[b] {
            continue;
        }
        for t in blk.term.succs() {
            pred_sum[t as usize] += est[b];
        }
    }
    // edge (from, to, weight): to's count split over its predecessors by theirs
    let mut edges: Vec<(u32, u32, f64)> = Vec::new();
    for (b, blk) in f.blocks.iter().enumerate() {
        if !reach[b] {
            continue;
        }
        let mut seen: [u32; 2] = [u32::MAX; 2];
        for (k, t) in blk.term.succs().enumerate() {
            if k < 2 {
                if seen.contains(&t) {
                    continue;
                }
                seen[k] = t;
            }
            let w = if pred_sum[t as usize] > 0.0 { est[t as usize] * est[b] / pred_sum[t as usize] } else { 0.0 };
            edges.push((b as u32, t, w));
        }
    }
    edges.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
    // chains as linked lists: next/prev block, chain id = its head
    let mut next = vec![u32::MAX; nb];
    let mut prev = vec![u32::MAX; nb];
    let mut head: Vec<u32> = (0..nb as u32).collect();
    let mut tail: Vec<u32> = (0..nb as u32).collect();
    for &(a, b, w) in &edges {
        if w <= 0.0 || b == 0 || a == b {
            continue;
        }
        let (a, b) = (a as usize, b as usize);
        // a ends its chain, b starts its own, different chains
        if next[a] != u32::MAX || prev[b] != u32::MAX || head[a] == head[b] {
            continue;
        }
        let ha = head[a] as usize;
        let tb = tail[head[b] as usize] as usize;
        next[a] = b as u32;
        prev[b] = a as u32;
        let mut x = b;
        loop {
            head[x] = ha as u32;
            if x == tb {
                break;
            }
            x = next[x] as usize;
        }
        tail[ha] = tb as u32;
    }
    let chain_of = |b: usize, head: &[u32]| head[b] as usize;
    let mut placed_chain = vec![false; nb];
    let mut order: Vec<u32> = Vec::with_capacity(nb);
    let mut placed = vec![false; nb];
    let place = |h: usize, order: &mut Vec<u32>, placed: &mut Vec<bool>, next: &[u32]| {
        let mut x = h;
        loop {
            order.push(x as u32);
            placed[x] = true;
            if next[x] == u32::MAX {
                break;
            }
            x = next[x] as usize;
        }
    };
    placed_chain[0] = true;
    place(0, &mut order, &mut placed, &next);
    // heaviest entry into each unplaced chain from placed blocks
    let mut into = vec![0.0f64; nb];
    let mut chain_hot = vec![0.0f64; nb];
    for b in 0..nb {
        if reach[b] {
            let c = chain_of(b, &head);
            chain_hot[c] = chain_hot[c].max(est[b]);
        }
    }
    let add_from = |b: usize, into: &mut Vec<f64>, head: &[u32], placed_chain: &[bool]| {
        for &(a, t, w) in &edges {
            if a as usize == b {
                let c = head[t as usize] as usize;
                if !placed_chain[c] {
                    into[c] += w;
                }
            }
        }
    };
    for k in 0..order.len() {
        add_from(order[k] as usize, &mut into, &head, &placed_chain);
    }
    loop {
        // next chain: most entered from placed code, else the hottest left; never-run chains wait
        let mut best: Option<(f64, f64, usize)> = None;
        for c in 0..nb {
            if head[c] as usize != c || placed_chain[c] || !reach[c] || chain_hot[c] <= 0.0 {
                continue;
            }
            let key = (into[c], chain_hot[c], c);
            let better = match best {
                None => true,
                Some(b) => key.0 > b.0 || (key.0 == b.0 && (key.1 > b.1 || (key.1 == b.1 && key.2 < b.2))),
            };
            if better {
                best = Some(key);
            }
        }
        let Some((_, _, c)) = best else { break };
        placed_chain[c] = true;
        let from = order.len();
        place(c, &mut order, &mut placed, &next);
        for k in from..order.len() {
            add_from(order[k] as usize, &mut into, &head, &placed_chain);
        }
    }
    for b in 0..nb {
        if !placed[b] {
            order.push(b as u32);
        }
    }
    if order.iter().enumerate().all(|(i, b)| i as u32 == *b) {
        return false;
    }
    let mut map = vec![0u32; nb];
    for (i, b) in order.iter().enumerate() {
        map[*b as usize] = i as u32;
    }
    let mut old: Vec<Option<Block>> = std::mem::take(&mut f.blocks).into_iter().map(Some).collect();
    for b in &order {
        let mut blk = old[*b as usize].take().unwrap();
        blk.term.map_succs(|t| map[t as usize]);
        f.blocks.push(blk);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rir::VReg;

    fn blk(term: Term, count: f32) -> Block {
        Block { insts: Vec::new(), pos: Vec::new(), term, term_pos: 0, count }
    }

    fn func(blocks: Vec<Block>) -> Func {
        let mut f = Func::new("t".to_string(), 0);
        let c = f.vreg(Cls::I);
        f.blocks = blocks;
        for b in &mut f.blocks {
            if let Term::Branch(ref mut v, ..) = b.term {
                *v = c;
            }
        }
        f
    }

    #[test]
    fn hot_successor_falls_through_and_cold_goes_last() {
        // 0 -> (1 cold | 2 hot); 1 -> 3; 2 -> 3
        let f0 = func(vec![
            blk(Term::Branch(VReg(0), 1, 2), 100.0),
            blk(Term::Jump(3), 0.0),
            blk(Term::Jump(3), 100.0),
            blk(Term::Ret(Vec::new()), 100.0),
        ]);
        let mut f = f0.clone();
        assert!(layout(&mut f));
        // entry, hot arm, join, cold arm
        let order: Vec<f32> = f.blocks.iter().map(|b| b.count).collect();
        assert_eq!(order, vec![100.0, 100.0, 100.0, 0.0]);
        assert!(matches!(f.blocks[0].term, Term::Branch(_, 3, 1)));
        assert!(matches!(f.blocks[1].term, Term::Jump(2)));
    }

    #[test]
    fn unknown_blocks_scale_from_counted_predecessors() {
        // 0 -> 1 (unknown) -> 2; the unknown block inherits its predecessor's count
        let f = func(vec![
            blk(Term::Jump(1), 50.0),
            blk(Term::Jump(2), -1.0),
            blk(Term::Ret(Vec::new()), 50.0),
        ]);
        let e = estimates(&f).unwrap();
        assert!((e[1] - 50.0).abs() < 1e-6, "{:?}", e);
        let fr = freqs(&f).unwrap();
        assert!(fr.iter().all(|x| (*x - 1.0).abs() < 1e-6));
    }

    #[test]
    fn instrumented_blocks_start_with_their_counter() {
        let mut f = func(vec![blk(Term::Jump(1), -1.0), blk(Term::Unreachable, -1.0)]);
        assert_eq!(instrument(&mut f, 0x1000), vec![false, true]);
        assert!(matches!(f.blocks[1].insts[0], Inst::Iconst(_, 0x1008)));
        assert!(matches!(f.blocks[1].insts[3], Inst::Store(..)));
        assert!(f.verify().is_ok());
    }

    #[test]
    fn loop_headers_count_and_keep_their_body_next() {
        // 0 -> 1; 1 -> (1 | 2): block 1 loops on itself
        let mut f = func(vec![
            blk(Term::Jump(1), -1.0),
            blk(Term::Branch(VReg(0), 1, 2), -1.0),
            blk(Term::Ret(Vec::new()), -1.0),
        ]);
        let h = back_edge_targets(&f);
        assert_eq!(h, vec![false, true, false]);
        assert!(loop_counters(&mut f, &h, Sym::new(7), Sym::new(8)));
        // entry, counting block, the loop body, exit, the helper call
        assert_eq!(f.blocks.len(), 5);
        assert!(matches!(f.blocks[1].insts[0], Inst::Addr(_, Sym { id: 7, off: 0 })));
        assert!(matches!(f.blocks[1].term, Term::Branch(_, 4, 2)));
        // the body loops back to the counting block
        assert!(matches!(f.blocks[2].term, Term::Branch(_, 1, 3)));
        assert!(matches!(f.blocks[4].insts[1], Inst::Call(Callee::Host(Sym { id: 8, .. }), _, _)));
        assert!(matches!(f.blocks[4].term, Term::Jump(2)));
        assert!(f.verify().is_ok());
        // a return block placed before its caller is no loop header
        let g = func(vec![blk(Term::Jump(2), -1.0), blk(Term::Ret(Vec::new()), -1.0), blk(Term::Jump(1), -1.0)]);
        assert!(back_edge_targets(&g).iter().all(|h| !*h));
    }
}
