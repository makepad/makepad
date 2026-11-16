//! Instrument reachable calls and loop back edges for const evaluation.
use crate::mir::dominators::Dominators;
use crate::mir::*;
use super::simplify;

/// Port of rustc_mir_transform/src/ctfe_limit.rs:17-40,48-58.
/// The requested reachable-only policy also excludes unreachable calls. rustc's
/// Call/TailCall arm itself is unconditional, but its preceding SimplifyCfg has
/// already removed dead blocks from mir_for_ctfe.
pub fn ctfe_limit(body: &mut Body) {
    let doms = Dominators::new(body);
    let indices: Vec<usize> = body.basic_blocks.iter().enumerate().filter_map(|(i, block)| {
        let node = BasicBlock(i as u32);
        if !doms.is_reachable(node) { return None; }
        let term = &block.terminator.as_ref().expect("missing terminator").kind;
        if matches!(term, TerminatorKind::Call { .. } | TerminatorKind::TailCall { .. })
            || simplify::successors(term).into_iter().any(|succ| doms.dominates(succ, node))
        { Some(i) } else { None }
    }).collect();
    for index in indices {
        let block = &mut body.basic_blocks[index];
        let source_info = block.terminator.as_ref().unwrap().source_info;
        block.statements.push(Statement { source_info, kind: StatementKind::ConstEvalCounter });
    }
}
