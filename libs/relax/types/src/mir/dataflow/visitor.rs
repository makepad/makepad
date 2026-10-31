//! rustc_mir_dataflow/src/framework/visitor.rs:7,45; visits effects in analysis order.
use super::*;

pub trait ResultsVisitor {
    fn visit_block_start(&mut self, _block: BasicBlock, _state: &BitSet) {}
    fn visit_after_early_statement_effect(&mut self, _analysis: &dyn Analysis, _state: &BitSet, _stmt: &Statement, _at: Location) {}
    fn visit_after_primary_statement_effect(&mut self, _analysis: &dyn Analysis, _state: &BitSet, _stmt: &Statement, _at: Location) {}
    fn visit_after_early_terminator_effect(&mut self, _analysis: &dyn Analysis, _state: &BitSet, _term: &Terminator, _at: Location) {}
    fn visit_after_primary_terminator_effect(&mut self, _analysis: &dyn Analysis, _state: &BitSet, _term: &Terminator, _at: Location) {}
    fn visit_block_end(&mut self, _block: BasicBlock, _state: &BitSet) {}
}

pub fn visit_results(results: &Results<'_>, blocks: &[BasicBlock], visitor: &mut dyn ResultsVisitor) {
    let mut state = results.analysis.bottom_value(results.body);
    for &block in blocks { visit_block(results, block, &mut state, visitor); }
}

pub fn visit_reachable_results(results: &Results<'_>, visitor: &mut dyn ResultsVisitor) {
    let mut state = results.analysis.bottom_value(results.body);
    for b in results.reachable.iter() { visit_block(results, BasicBlock(b as u32), &mut state, visitor); }
}

fn visit_block(results: &Results<'_>, block: BasicBlock, state: &mut BitSet, visitor: &mut dyn ResultsVisitor) {
    state.clone_from(results.entry_state(block));
    let analysis = results.analysis;
    let forward = analysis.direction() == Direction::Forward;
    if forward { visitor.visit_block_start(block, state); }
    else { visitor.visit_block_end(block, state); }
    let data = &results.body.basic_blocks[block.0 as usize];
    for step in 0..(data.statements.len() + 1) * 2 {
        let at = direction::location_for_step(results.body, analysis.direction(), block, step);
        apply_effect(results.body, analysis, state, block, step);
        if at.statement_index == data.statements.len() {
            let term = data.terminator.as_ref().unwrap();
            if step % 2 == 0 { visitor.visit_after_early_terminator_effect(analysis, state, term, at); }
            else { visitor.visit_after_primary_terminator_effect(analysis, state, term, at); }
        } else {
            let stmt = &data.statements[at.statement_index];
            if step % 2 == 0 { visitor.visit_after_early_statement_effect(analysis, state, stmt, at); }
            else { visitor.visit_after_primary_statement_effect(analysis, state, stmt, at); }
        }
    }
    if forward { visitor.visit_block_end(block, state); }
    else { visitor.visit_block_start(block, state); }
}
