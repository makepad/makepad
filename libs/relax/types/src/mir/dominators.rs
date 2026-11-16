//! Shared Cooper-Harvey-Kennedy dominators for borrow checking and CTFE shaping.
use crate::mir::dataflow::BitSet;
use crate::mir::transform::simplify;
use crate::mir::*;

/// Rebase Relax nll.rs::compute_dominators onto MIR; reuse its RPO/intersect
/// algorithm, moved from borrowck/check.rs:1039-1092. Edges come from MIR's
/// shared successors helper, including imaginary and unwind edges.
pub(crate) struct Dominators {
    pub(crate) predecessors: Vec<Vec<BasicBlock>>,
    idom: Vec<usize>,
    pub(crate) reverse_postorder: Vec<usize>,
}
impl Dominators {
    pub(crate) fn new(body: &Body) -> Self {
        let n = body.basic_blocks.len();
        let mut successors = vec![vec![]; n];
        let mut predecessors = vec![vec![]; n];
        for (b, block) in body.basic_blocks.iter().enumerate() {
            let edges = simplify::successors(&block.terminator.as_ref().expect("missing terminator").kind);
            for &s in &edges { predecessors[s.0 as usize].push(BasicBlock(b as u32)); }
            successors[b] = edges;
        }
        if n == 0 { return Self { predecessors, idom: vec![], reverse_postorder: vec![] }; }
        let mut seen = BitSet::new_empty(n);
        let mut stack = vec![(0, false)];
        let mut order = vec![];
        while let Some((b, finish)) = stack.pop() {
            if finish { order.push(b); continue; }
            if !seen.insert(b) { continue; }
            stack.push((b, true));
            // nightly Postorder consumes successors with next_back().
            for s in &successors[b] { stack.push((s.0 as usize, false)); }
        }
        order.reverse();
        let mut rank = vec![usize::MAX; n];
        for (i, &b) in order.iter().enumerate() { rank[b] = i; }
        let mut idom = vec![usize::MAX; n]; idom[0] = 0;
        loop {
            let mut changed = false;
            for &b in order.iter().skip(1) {
                let mut new = usize::MAX;
                for p in predecessors[b].iter().map(|p| p.0 as usize).filter(|&p| idom[p] != usize::MAX) {
                    if new == usize::MAX { new = p; continue; }
                    let mut p = p;
                    while p != new { if rank[p] > rank[new] { p = idom[p]; } else { new = idom[new]; } }
                }
                if new != usize::MAX && new != idom[b] { idom[b] = new; changed = true; }
            }
            if !changed { break; }
        }
        Self { predecessors, idom, reverse_postorder: order }
    }
    pub(crate) fn dominates(&self, a: BasicBlock, b: BasicBlock) -> bool {
        let mut b = b.0 as usize;
        loop {
            if b == a.0 as usize { return true; }
            let parent = self.idom[b];
            if parent == usize::MAX || parent == b { return false; } b = parent;
        }
    }
    pub(crate) fn is_reachable(&self, block: BasicBlock) -> bool {
        self.idom[block.0 as usize] != usize::MAX
    }
}
