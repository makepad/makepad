//! rustc_mir_dataflow/src/framework/cursor.rs:113,159,167,175.
use super::*;

pub struct ResultsCursor<'r, 'a> {
    results: &'r Results<'a>,
    state: BitSet,
    block: Option<BasicBlock>,
    next_step: usize,
}

impl<'r, 'a> ResultsCursor<'r, 'a> {
    pub(super) fn new(results: &'r Results<'a>) -> Self {
        Self { results, state: results.analysis.bottom_value(results.body), block: None, next_step: 0 }
    }
    pub fn body(&self) -> &'a Body { self.results.body }
    pub fn get(&self) -> &BitSet {
        assert!(self.block.is_some(), "cursor must be positioned before inspection");
        &self.state
    }
    fn reset(&mut self, block: BasicBlock) {
        self.state.clone_from(self.results.entry_state(block));
        self.block = Some(block);
        self.next_step = 0;
    }
    pub fn seek_to_block_start(&mut self, block: BasicBlock) {
        match self.results.analysis.direction() {
            Direction::Forward => self.reset(block),
            Direction::Backward => self.seek_after_primary_effect(Location { block, statement_index: 0 }),
        }
    }
    pub fn seek_to_block_end(&mut self, block: BasicBlock) {
        match self.results.analysis.direction() {
            Direction::Backward => self.reset(block),
            Direction::Forward => {
                let statement_index = self.results.body.basic_blocks[block.0 as usize].statements.len();
                self.seek_after_primary_effect(Location { block, statement_index });
            }
        }
    }
    /// Before Primary includes Early, even for backward analyses.
    pub fn seek_before_primary_effect(&mut self, target: Location) { self.seek(target, false); }
    pub fn seek_after_primary_effect(&mut self, target: Location) { self.seek(target, true); }
    fn seek(&mut self, target: Location, primary: bool) {
        let end = self.results.body.basic_blocks[target.block.0 as usize].statements.len();
        assert!(target.statement_index <= end);
        let index = match self.results.analysis.direction() {
            Direction::Forward => target.statement_index,
            Direction::Backward => end - target.statement_index,
        };
        let next = index * 2 + if primary { 2 } else { 1 };
        if self.block != Some(target.block) || self.next_step > next { self.reset(target.block); }
        while self.next_step < next {
            apply_effect(self.results.body, self.results.analysis, &mut self.state, target.block, self.next_step);
            self.next_step += 1;
        }
    }
}
