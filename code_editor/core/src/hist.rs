use crate::{Diff, SelSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hist {
    revs: Vec<Rev>,
    rev_id: usize,
}

impl Hist {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn undo(&mut self) -> Option<(SelSet, Diff)> {
        if self.rev_id == 0 {
            return None;
        }
        let rev = self.revs[self.rev_id].clone();
        self.rev_id -= 1;
        Some((rev.sels_before, rev.diff.revert()))
    }

    pub fn redo(&mut self) -> Option<(SelSet, Diff)> {
        if self.rev_id == self.revs.len() - 1 {
            return None;
        }
        self.rev_id += 1;
        let rev = self.revs[self.rev_id].clone();
        let mut sels_after = rev.sels_before;
        sels_after.apply_diff(&rev.diff, true);
        Some((sels_after, rev.diff))
    }

    pub fn commit(&mut self, sels_before: SelSet, diff: Diff) {
        self.rev_id += 1;
        self.revs.truncate(self.rev_id);
        self.revs.push(Rev {
            sels_before,
            diff,
        });
    }
}

impl Default for Hist {
    fn default() -> Self {
        Self {
            revs: vec![Rev::default()],
            rev_id: 0,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Rev {
    pub sels_before: SelSet,
    pub diff: Diff,
}
