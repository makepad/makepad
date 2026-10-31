//! CFG simplification before analysis: rustc_mir_transform/src/simplify.rs (1.97.1), `SimplifyCfg`
//! as run in `mir_built` (Initial) and `mir_promoted` (PromoteConsts).
//!
//! **WARNING** (rustc): this is one of the few optimizations that runs on built and analysis MIR, so
//! its effects may affect the type-checking, borrow-checking, and other analysis of MIR. It must
//! preserve reads, including those whose undefined behaviour borrow checking diagnoses: a `SwitchInt`
//! whose targets are all equal is not turned into a `goto` before runtime MIR.
//!
//! The CFG traversal helpers (`successors`, `preorder`, `reachable`, `reverse_postorder`) live here
//! because every pass of this lane needs them.
use crate::mir::dataflow::BitSet;
use crate::mir::*;

/// Every successor of a terminator, in rustc's order (normal edges, then the unwind edge).
pub fn successors(kind: &TerminatorKind) -> Vec<BasicBlock> {
    let cleanup = |unwind: &UnwindAction| match unwind {
        UnwindAction::Cleanup(target) => Some(*target),
        _ => None,
    };
    match kind {
        TerminatorKind::Goto { target } => vec![*target],
        TerminatorKind::SwitchInt { targets, .. } => targets.targets.iter().copied().chain([targets.otherwise]).collect(),
        TerminatorKind::Drop { target, unwind, .. } | TerminatorKind::Assert { target, unwind, .. } | TerminatorKind::FalseUnwind { real_target: target, unwind } => {
            [Some(*target), cleanup(unwind)].into_iter().flatten().collect()
        }
        TerminatorKind::Call { target, unwind, .. } => [*target, cleanup(unwind)].into_iter().flatten().collect(),
        TerminatorKind::Yield { resume, drop, .. } => [Some(*resume), *drop].into_iter().flatten().collect(),
        TerminatorKind::FalseEdge { real_target, imaginary_target } => vec![*real_target, *imaginary_target],
        TerminatorKind::UnwindResume
        | TerminatorKind::UnwindTerminate(_)
        | TerminatorKind::Return
        | TerminatorKind::Unreachable
        | TerminatorKind::TailCall { .. }
        | TerminatorKind::CoroutineDrop
        | TerminatorKind::InlineAsm(_) => Vec::new(),
    }
}

fn unwind_cleanup(unwind: &mut UnwindAction, f: &mut impl FnMut(&mut BasicBlock)) {
    if let UnwindAction::Cleanup(target) = unwind {
        f(target);
    }
}

/// rustc `Terminator::successors_mut`.
pub fn successors_mut(kind: &mut TerminatorKind, mut f: impl FnMut(&mut BasicBlock)) {
    match kind {
        TerminatorKind::Goto { target } => f(target),
        TerminatorKind::SwitchInt { targets, .. } => {
            for target in targets.targets.iter_mut() {
                f(target);
            }
            f(&mut targets.otherwise);
        }
        TerminatorKind::Drop { target, unwind, .. } | TerminatorKind::Assert { target, unwind, .. } | TerminatorKind::FalseUnwind { real_target: target, unwind } => {
            f(target);
            unwind_cleanup(unwind, &mut f);
        }
        TerminatorKind::Call { target, unwind, .. } => {
            if let Some(target) = target {
                f(target);
            }
            unwind_cleanup(unwind, &mut f);
        }
        TerminatorKind::Yield { resume, drop, .. } => {
            f(resume);
            if let Some(drop) = drop {
                f(drop);
            }
        }
        TerminatorKind::FalseEdge { real_target, imaginary_target } => {
            f(real_target);
            f(imaginary_target);
        }
        TerminatorKind::UnwindResume
        | TerminatorKind::UnwindTerminate(_)
        | TerminatorKind::Return
        | TerminatorKind::Unreachable
        | TerminatorKind::TailCall { .. }
        | TerminatorKind::CoroutineDrop
        | TerminatorKind::InlineAsm(_) => {}
    }
}

/// Reachable blocks in preorder (depth-first, successors in terminator order).
pub fn preorder(body: &Body) -> Vec<BasicBlock> {
    let mut visited = vec![false; body.basic_blocks.len()];
    let mut out = Vec::new();
    if visited.is_empty() {
        return out;
    }
    let mut stack = vec![BasicBlock(0)];
    visited[0] = true;
    while let Some(bb) = stack.pop() {
        out.push(bb);
        if let Some(term) = &body.basic_blocks[bb.0 as usize].terminator {
            // pushed in reverse so the first successor is visited first, like rustc's preorder
            for succ in successors(&term.kind).into_iter().rev() {
                if !visited[succ.0 as usize] {
                    visited[succ.0 as usize] = true;
                    stack.push(succ);
                }
            }
        }
    }
    out
}

pub fn reachable_as_bitset(body: &Body) -> BitSet {
    let mut set = BitSet::new_empty(body.basic_blocks.len());
    for bb in preorder(body) {
        set.insert(bb.0 as usize);
    }
    set
}

/// Reachable blocks in reverse postorder.
pub fn reverse_postorder(body: &Body) -> Vec<BasicBlock> {
    let n = body.basic_blocks.len();
    let mut visited = vec![false; n];
    let mut post = Vec::new();
    if n == 0 {
        return post;
    }
    // iterative DFS with an explicit successor cursor
    let mut stack: Vec<(BasicBlock, Vec<BasicBlock>, usize)> = Vec::new();
    visited[0] = true;
    let first = body.basic_blocks[0].terminator.as_ref().map_or_else(Vec::new, |t| successors(&t.kind));
    stack.push((BasicBlock(0), first, 0));
    while let Some((bb, succs, i)) = stack.last_mut() {
        if *i < succs.len() {
            let succ = succs[*i];
            *i += 1;
            if !visited[succ.0 as usize] {
                visited[succ.0 as usize] = true;
                let next = body.basic_blocks[succ.0 as usize].terminator.as_ref().map_or_else(Vec::new, |t| successors(&t.kind));
                stack.push((succ, next, 0));
            }
        } else {
            post.push(*bb);
            stack.pop();
        }
    }
    post.reverse();
    post
}

/// simplify.rs:`simplify_cfg`. `preserve_ub` is nightly's runtime optimization policy; built and
/// analysis MIR always preserve it. Relax has no CFG cache or statement-attached debug locations to
/// invalidate.
pub fn simplify_cfg(body: &mut Body, preserve_ub: bool) {
    CfgSimplifier::new(body, preserve_ub).simplify();
    remove_dead_blocks(body);
    body.basic_blocks.shrink_to_fit();
}

struct CfgSimplifier<'a> {
    preserve_switch_reads: bool,
    basic_blocks: &'a mut [BasicBlockData],
    pred_count: Vec<u32>,
    // Reuse the stack across successors instead of allocating once per chain.
    goto_chain: Vec<(BasicBlock, Terminator)>,
}

impl<'a> CfgSimplifier<'a> {
    /// simplify.rs:121-144. Count reachable predecessors only; dead blocks must not prevent chain
    /// merging.
    fn new(body: &'a mut Body, preserve_ub: bool) -> Self {
        let mut pred_count = vec![0; body.basic_blocks.len()];
        pred_count[0] = 1;
        for bb in preorder(body) {
            if let Some(term) = &body.basic_blocks[bb.0 as usize].terminator {
                for target in successors(&term.kind) {
                    pred_count[target.0 as usize] += 1;
                }
            }
        }
        // Preserve `SwitchInt` reads on built and analysis MIR.
        let preserve_switch_reads = matches!(body.phase, MirPhase::Built | MirPhase::Analysis(_)) || preserve_ub;
        Self { preserve_switch_reads, basic_blocks: &mut body.basic_blocks, pred_count, goto_chain: Vec::new() }
    }

    /// simplify.rs:149-213. Statement storage moves only once when merging a chain; source_info
    /// stays on statements.
    fn simplify(mut self) {
        self.strip_nops();
        let mut merged_blocks = Vec::new();
        loop {
            let mut changed = false;
            for bb in 0..self.basic_blocks.len() {
                if self.pred_count[bb] == 0 {
                    continue;
                }
                let mut terminator = self.basic_blocks[bb].terminator.take().expect("invalid terminator state");
                successors_mut(&mut terminator.kind, |successor| self.collapse_goto_chain(successor, &mut changed));
                let mut inner_changed = true;
                merged_blocks.clear();
                while inner_changed {
                    inner_changed = self.simplify_branch(&mut terminator);
                    inner_changed |= self.merge_successor(&mut merged_blocks, &mut terminator);
                    changed |= inner_changed;
                }
                let statements_to_merge: usize = merged_blocks.iter().map(|bb| self.basic_blocks[bb.0 as usize].statements.len()).sum();
                if statements_to_merge > 0 {
                    let mut statements = std::mem::take(&mut self.basic_blocks[bb].statements);
                    statements.reserve(statements_to_merge);
                    for from in &merged_blocks {
                        statements.append(&mut self.basic_blocks[from.0 as usize].statements);
                    }
                    self.basic_blocks[bb].statements = statements;
                }
                self.basic_blocks[bb].terminator = Some(terminator);
            }
            if !changed {
                break;
            }
        }
    }

    /// simplify.rs:216-231. Taking terminators breaks cycles without a recursion bound or a
    /// separate visited set.
    fn take_terminator_if_simple_goto(&mut self, bb: BasicBlock) -> Option<Terminator> {
        let data = &mut self.basic_blocks[bb.0 as usize];
        if data.statements.is_empty() && matches!(&data.terminator, Some(Terminator { kind: TerminatorKind::Goto { .. }, .. })) {
            data.terminator.take()
        } else {
            None
        }
    }

    /// simplify.rs:234-278.
    fn collapse_goto_chain(&mut self, start: &mut BasicBlock, changed: &mut bool) {
        assert!(self.goto_chain.is_empty());
        let mut current = *start;
        while let Some(terminator) = self.take_terminator_if_simple_goto(current) {
            let TerminatorKind::Goto { target } = terminator.kind else { unreachable!() };
            self.goto_chain.push((current, terminator));
            current = target;
        }
        let last = current;
        *changed |= *start != last;
        *start = last;
        while let Some((current, mut terminator)) = self.goto_chain.pop() {
            let TerminatorKind::Goto { target } = &mut terminator.kind else { unreachable!() };
            *changed |= *target != last;
            *target = last;
            if self.pred_count[current.0 as usize] == 1 {
                // This is the last reference to current, so the pred-count to to target is moved into
                // the current block.
                self.pred_count[current.0 as usize] = 0;
            } else {
                self.pred_count[last.0 as usize] += 1;
                self.pred_count[current.0 as usize] -= 1;
            }
            self.basic_blocks[current.0 as usize].terminator = Some(terminator);
        }
    }

    /// simplify.rs:280-305: merge a block with 1 `goto` predecessor to its parent.
    fn merge_successor(&mut self, merged_blocks: &mut Vec<BasicBlock>, terminator: &mut Terminator) -> bool {
        let target = match terminator.kind {
            TerminatorKind::Goto { target } if self.pred_count[target.0 as usize] == 1 => target,
            _ => return false,
        };
        *terminator = match self.basic_blocks[target.0 as usize].terminator.take() {
            Some(terminator) => terminator,
            // unreachable loop - this should not be possible, as we don't strand blocks
            None => return false,
        };
        merged_blocks.push(target);
        self.pred_count[target.0 as usize] = 0;
        true
    }

    /// simplify.rs:307-332: turn a branch with all successors identical to a goto.
    fn simplify_branch(&mut self, terminator: &mut Terminator) -> bool {
        // Removing a `SwitchInt` terminator may remove reads that result in UB, so we must not apply
        // this optimization before borrowck.
        if self.preserve_switch_reads {
            return false;
        }
        if !matches!(terminator.kind, TerminatorKind::SwitchInt { .. }) {
            return false;
        }
        let successors = successors(&terminator.kind);
        let Some(&first) = successors.first() else { return false };
        if !successors.iter().all(|&target| target == first) {
            return false;
        }
        self.pred_count[first.0 as usize] -= (successors.len() - 1) as u32;
        terminator.kind = TerminatorKind::Goto { target: first };
        true
    }

    /// simplify.rs:334-338.
    fn strip_nops(&mut self) {
        for block in self.basic_blocks.iter_mut() {
            block.statements.retain(|statement| !matches!(statement.kind, StatementKind::Nop));
        }
    }
}

/// simplify.rs:341-353. Unlike replacing the switch by a goto, removing redundant cases retains the
/// discriminant read.
pub fn simplify_duplicate_switch_targets(terminator: &mut Terminator) {
    if let TerminatorKind::SwitchInt { targets, .. } = &mut terminator.kind {
        assert_eq!(targets.values.len(), targets.targets.len());
        let otherwise = targets.otherwise;
        let mut index = 0;
        targets.values.retain(|_| {
            let keep = targets.targets[index] != otherwise;
            index += 1;
            keep
        });
        targets.targets.retain(|target| *target != otherwise);
    }
}

/// simplify.rs:355-425. Dead code can contain ill-typed assignments produced after diverging
/// expressions. Coalesce empty unreachable blocks, keeping cleanup and ordinary blocks distinct.
pub fn remove_dead_blocks(body: &mut Body) {
    let should_deduplicate = |block: &BasicBlockData| {
        // CfgSimplifier::simplify leaves behind some unreachable basic blocks without a terminator;
        // those blocks are deleted below, but we run just before then so handle missing terminators.
        block.statements.is_empty() && !block.is_cleanup && matches!(&block.terminator, Some(Terminator { kind: TerminatorKind::Unreachable, .. }))
    };
    let reachable = reachable_as_bitset(body);
    let empty_unreachable = body.basic_blocks.iter().enumerate().filter(|(bb, block)| reachable.contains(*bb) && should_deduplicate(block)).count();
    let count = body.basic_blocks.len();
    if count == reachable.count() && empty_unreachable <= 1 {
        return;
    }
    let mut replacements: Vec<_> = (0..count).map(|bb| BasicBlock(bb as u32)).collect();
    let mut original = 0;
    let mut used = 0;
    let mut kept_unreachable = None;
    let mut deduplicated = false;
    body.basic_blocks.retain(|block| {
        let index = original;
        original += 1;
        if !reachable.contains(index) {
            return false;
        }
        let kept = BasicBlock(used);
        if should_deduplicate(block) {
            let first = *kept_unreachable.get_or_insert(kept);
            if first != kept {
                replacements[index] = first;
                deduplicated = true;
                return false;
            }
        }
        replacements[index] = kept;
        used += 1;
        true
    });
    // If we deduplicated unreachable blocks we erase their source_info as we can no longer attribute
    // their code to a particular location in the source.
    if deduplicated {
        body.basic_blocks[kept_unreachable.unwrap().0 as usize].terminator.as_mut().unwrap().source_info = SourceInfo::default();
    }
    for block in &mut body.basic_blocks {
        successors_mut(&mut block.terminator.as_mut().expect("reachable block has no terminator").kind, |target| *target = replacements[target.0 as usize]);
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::*;
    use super::*;
    use crate::mir::BodyKind as K;

    fn unit_body(fx: &Fx, blocks: Vec<BasicBlockData>) -> Body {
        body(fx.def("nf"), K::Fn, fx.u8(), &[], &[], blocks)
    }
    fn goto(target: u32) -> TerminatorKind {
        TerminatorKind::Goto { target: BasicBlock(target) }
    }
    fn assigns(n: usize, fx: &Fx) -> Vec<Statement> {
        (0..n).map(|i| assign(local(0), Rvalue::Use(int(fx.u8(), i as u128)))).collect()
    }

    #[test]
    fn goto_chains_collapse_and_single_predecessor_blocks_merge() {
        let fx = Fx::new("");
        let mut b = unit_body(&fx, vec![
            block(assigns(1, &fx), goto(1)),
            block(vec![], goto(2)),
            block(assigns(2, &fx), goto(3)),
            block(vec![], TerminatorKind::Return),
        ]);
        simplify_cfg(&mut b, false);
        assert_eq!(b.basic_blocks.len(), 1);
        assert_eq!(b.basic_blocks[0].statements.len(), 3);
        assert!(matches!(b.basic_blocks[0].terminator.as_ref().unwrap().kind, TerminatorKind::Return));
    }

    #[test]
    fn unreachable_blocks_are_removed_and_targets_renumbered() {
        let fx = Fx::new("");
        let mut b = unit_body(&fx, vec![
            block(assigns(1, &fx), TerminatorKind::Return),
            block(assigns(1, &fx), goto(3)),
            block(vec![], TerminatorKind::Unreachable),
            block(assigns(2, &fx), TerminatorKind::Return),
        ]);
        simplify_cfg(&mut b, false);
        assert_eq!(b.basic_blocks.len(), 1);
    }

    #[test]
    fn identical_switch_targets_keep_the_read_before_runtime() {
        let fx = Fx::new("");
        let make = |phase| {
            let mut b = unit_body(&fx, vec![
                block(vec![], TerminatorKind::SwitchInt { discr: copy(0), targets: SwitchTargets { values: vec![0, 1], targets: vec![BasicBlock(1), BasicBlock(1)], otherwise: BasicBlock(1) } }),
                block(vec![], TerminatorKind::Return),
            ]);
            b.phase = phase;
            b
        };
        // built and analysis MIR: the switch (a read of its discriminant) stays
        for phase in [MirPhase::Built, MirPhase::Analysis(AnalysisPhase::Initial)] {
            let mut b = make(phase);
            simplify_cfg(&mut b, false);
            assert!(matches!(b.basic_blocks[0].terminator.as_ref().unwrap().kind, TerminatorKind::SwitchInt { .. }), "{phase:?}");
        }
        // runtime MIR: it becomes a goto and merges
        let mut b = make(MirPhase::Runtime(RuntimePhase::Initial));
        simplify_cfg(&mut b, false);
        assert_eq!(b.basic_blocks.len(), 1);
    }

    #[test]
    fn empty_unreachable_blocks_are_shared_but_cleanup_ones_are_not() {
        let fx = Fx::new("");
        let mut b = unit_body(&fx, vec![
            block(vec![], TerminatorKind::SwitchInt { discr: copy(0), targets: SwitchTargets { values: vec![0], targets: vec![BasicBlock(1)], otherwise: BasicBlock(2) } }),
            block(vec![], TerminatorKind::Unreachable),
            block(vec![], TerminatorKind::Unreachable),
        ]);
        simplify_cfg(&mut b, false);
        let TerminatorKind::SwitchInt { targets, .. } = &b.basic_blocks[0].terminator.as_ref().unwrap().kind else { panic!() };
        assert_eq!(b.basic_blocks.len(), 2);
        assert_eq!(targets.targets[0], targets.otherwise);
    }

    #[test]
    fn nops_are_stripped_and_loops_survive() {
        let fx = Fx::new("");
        let mut b = unit_body(&fx, vec![
            block(vec![stmt(StatementKind::Nop), stmt(StatementKind::Nop)], goto(1)),
            block(vec![], goto(1)),
        ]);
        simplify_cfg(&mut b, false);
        assert!(b.basic_blocks[0].statements.is_empty());
        assert!(matches!(b.basic_blocks[0].terminator.as_ref().unwrap().kind, TerminatorKind::Goto { .. }));
    }

    #[test]
    fn reverse_postorder_starts_at_the_entry_and_visits_each_block_once() {
        let fx = Fx::new("");
        let b = unit_body(&fx, vec![
            block(vec![], TerminatorKind::SwitchInt { discr: copy(0), targets: SwitchTargets { values: vec![0], targets: vec![BasicBlock(1)], otherwise: BasicBlock(2) } }),
            block(vec![], goto(3)),
            block(vec![], goto(3)),
            block(vec![], TerminatorKind::Return),
            block(vec![], TerminatorKind::Return),
        ]);
        let rpo = reverse_postorder(&b);
        assert_eq!(rpo.first(), Some(&BasicBlock(0)));
        assert_eq!(rpo.last(), Some(&BasicBlock(3)));
        assert_eq!(rpo.len(), 4, "block 4 is unreachable");
    }
}
