//! Dominators and block layout order on RIR, shared by the emitters and the optimiser.

use crate::regalloc::reachable;
use crate::rir::*;

/// `relayout`, returning the new order: per new block, the block it was.
pub fn relayout_order(f: &mut Func) -> Vec<u32> {
    let n = f.blocks.len();
    if n <= 1 {
        return (0..n as u32).collect();
    }
    let reach = reachable(f);
    // blocks only reachable through unwind edges (cleanup pads) are cold
    let mut normal = vec![false; n];
    let mut st = vec![0u32];
    while let Some(b) = st.pop() {
        if normal[b as usize] {
            continue;
        }
        normal[b as usize] = true;
        match f.blocks[b as usize].term {
            Term::Invoke(t, _) => st.push(t),
            ref t => st.extend(t.succs()),
        }
    }
    // loop blocks: an equality of two values decides a search there (`find`, `position`, a key
    // compare per element): the hit is the rare way out of the scan
    // (MAKEPAD_RUN_OPT=-searchlayout: off, A/B)
    let eq_test = |b: usize| match f.blocks[b].term {
        Term::Branch(c, ..) => f.blocks[b].insts.iter().rev().find(|i| {
            let mut d = false;
            i.defs(|v| d |= v == c);
            d
        }).is_some_and(|i| matches!(i, Inst::ICmp(Cond::Eq, _, _, x, y) if x != y)),
        _ => false,
    };
    let depth = (!crate::stats::opt_env_off("-searchlayout") && (0..n).any(|b| reach[b] && eq_test(b))).then(|| crate::regalloc::loop_depths(f, &reach));
    let search_hit = |b: usize| depth.as_ref().is_some_and(|d| d[b] > 0) && eq_test(b);
    let mut placed = vec![false; n];
    let mut order: Vec<u32> = Vec::new();
    let mut cold: Vec<u32> = Vec::new();
    let mut next = 0u32;
    let mut unplaced = 0;
    loop {
        let mut b = next;
        while !placed[b as usize] && reach[b as usize] {
            placed[b as usize] = true;
            if (matches!(f.blocks[b as usize].term, Term::Unreachable | Term::Resume) || !normal[b as usize]) && b != 0 {
                cold.push(b);
            } else {
                order.push(b);
            }
            b = match f.blocks[b as usize].term {
                Term::Jump(t) | Term::Invoke(t, _) => t,
                Term::Branch(_, t, e) => {
                    // fall through to the true successor (then-block, loop body) unless it is
                    // placed already or a panic path, or the branch is a search's hit test in a
                    // loop (then the scan goes on next: the miss is what repeats)
                    let t_cold = matches!(f.blocks[t as usize].term, Term::Unreachable);
                    let e_first = !placed[e as usize] && !matches!(f.blocks[e as usize].term, Term::Unreachable) && search_hit(b as usize);
                    if !placed[t as usize] && !t_cold && !e_first {
                        t
                    } else {
                        e
                    }
                }
                _ => break,
            };
        }
        // Placement is monotone: earlier blocks never need to be searched again.
        while unplaced < n && (placed[unplaced] || !reach[unplaced]) {
            unplaced += 1;
        }
        if unplaced == n {
            break;
        }
        next = unplaced as u32;
    }
    order.extend(cold);
    let mut map = vec![u32::MAX; n];
    for (i, b) in order.iter().enumerate() {
        map[*b as usize] = i as u32;
    }
    let old = std::mem::take(&mut f.blocks);
    let mut slots: Vec<Option<Block>> = Vec::with_capacity(n);
    for b in old {
        slots.push(Some(b));
    }
    for b in &order {
        let mut blk = slots[*b as usize].take().unwrap();
        blk.term.map_succs(|t| map[t as usize]);
        f.blocks.push(blk);
    }
    order
}

/// Immediate dominators (Cooper/Harvey/Kennedy) over reachable blocks; entry's idom is itself.
pub fn idoms(f: &Func) -> Vec<u32> {
    let succs_of = |b: u32| f.blocks[b as usize].term.succs();
    let n = f.blocks.len();
    if n == 1 {
        return vec![0];
    }
    // reverse postorder
    let mut rpo: Vec<u32> = Vec::with_capacity(n);
    let mut seen = vec![false; n];
    let mut stack: Vec<(u32, u8)> = Vec::with_capacity(2 * n);
    stack.push((0, 0));
    while let Some((b, st)) = stack.pop() {
        if st == 0 {
            if seen[b as usize] {
                continue;
            }
            seen[b as usize] = true;
            stack.push((b, 1));
            // pushed in reverse: the first successor is visited first
            let succ: Vec<u32> = succs_of(b).collect();
            for t in succ.iter().rev() {
                stack.push((*t, 0));
            }
        } else {
            rpo.push(b);
        }
    }
    rpo.reverse();
    let mut order = vec![u32::MAX; n];
    for (i, b) in rpo.iter().enumerate() {
        order[*b as usize] = i as u32;
    }
    // predecessors in reverse postorder, flat (pred_off[b]..pred_off[b + 1])
    let mut pred_off = vec![0u32; n + 1];
    for &b in &rpo {
        for t in succs_of(b) {
            pred_off[t as usize + 1] += 1;
        }
    }
    for i in 0..n {
        pred_off[i + 1] += pred_off[i];
    }
    let mut fill = pred_off.clone();
    let mut preds = vec![0u32; pred_off[n] as usize];
    for &b in &rpo {
        for t in succs_of(b) {
            preds[fill[t as usize] as usize] = b;
            fill[t as usize] += 1;
        }
    }
    let mut idom = vec![u32::MAX; n];
    idom[0] = 0;
    let mut changed = true;
    while changed {
        changed = false;
        for &b in rpo.iter().skip(1) {
            let mut new = u32::MAX;
            for &p in &preds[pred_off[b as usize] as usize..pred_off[b as usize + 1] as usize] {
                if idom[p as usize] == u32::MAX {
                    continue;
                }
                if new == u32::MAX {
                    new = p;
                } else {
                    let (mut x, mut y) = (p, new);
                    while x != y {
                        while order[x as usize] > order[y as usize] {
                            x = idom[x as usize];
                        }
                        while order[y as usize] > order[x as usize] {
                            y = idom[y as usize];
                        }
                    }
                    new = x;
                }
            }
            if new != u32::MAX && idom[b as usize] != new {
                idom[b as usize] = new;
                changed = true;
            }
        }
    }
    idom
}

/// Dominator-tree pre/post numbers: `a` dominates `b` iff pre[a] <= pre[b] && post[b] <= post[a].
/// Dominator-tree children, flat: block b's children (ascending) are
/// `child[off[b]..off[b + 1]]`.
pub struct DomKids {
    off: Vec<u32>,
    child: Vec<u32>,
}

impl std::ops::Index<usize> for DomKids {
    type Output = [u32];
    #[inline]
    fn index(&self, b: usize) -> &[u32] {
        &self.child[self.off[b] as usize..self.off[b + 1] as usize]
    }
}

pub fn dom_numbers(idom: &[u32]) -> (Vec<u32>, Vec<u32>, DomKids) {
    let n = idom.len();
    let mut off = vec![0u32; n + 1];
    for b in 1..n {
        if idom[b] != u32::MAX && idom[b] as usize != b {
            off[idom[b] as usize + 1] += 1;
        }
    }
    for i in 0..n {
        off[i + 1] += off[i];
    }
    let mut fill = off.clone();
    let mut child = vec![0u32; off[n] as usize];
    for b in 1..n {
        if idom[b] != u32::MAX && idom[b] as usize != b {
            let p = idom[b] as usize;
            child[fill[p] as usize] = b as u32;
            fill[p] += 1;
        }
    }
    let children = DomKids { off, child };
    let mut pre = vec![u32::MAX; n];
    let mut post = vec![0u32; n];
    let mut clock = 0u32;
    let mut stack: Vec<(u32, bool)> = Vec::with_capacity(2 * n);
    stack.push((0, false));
    while let Some((b, done)) = stack.pop() {
        if done {
            post[b as usize] = clock;
            clock += 1;
            continue;
        }
        pre[b as usize] = clock;
        clock += 1;
        stack.push((b, true));
        for &c in children[b as usize].iter().rev() {
            stack.push((c, false));
        }
    }
    (pre, post, children)
}
