use crate::{Diff, CursorSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hist {
    revs: Vec<Rev>,
    rev_id: usize,
}

impl Hist {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn undo(&mut self) -> Option<(CursorSet, Diff)> {
        if self.rev_id == 0 {
            return None;
        }
        let rev = self.revs[self.rev_id].clone();
        self.rev_id -= 1;
        Some((rev.cursors_before, rev.diff.revert()))
    }

    pub fn redo(&mut self) -> Option<(CursorSet, Diff)> {
        if self.rev_id == self.revs.len() - 1 {
            return None;
        }
        self.rev_id += 1;
        let rev = self.revs[self.rev_id].clone();
        let mut cursors_after = rev.cursors_before;
        cursors_after.apply_diff(&rev.diff, true);
        Some((cursors_after, rev.diff))
    }

    pub fn commit(&mut self, cursors_before: CursorSet, diff: Diff) {
        self.rev_id += 1;
        self.revs.truncate(self.rev_id);
        self.revs.push(Rev {
            cursors_before,
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
    pub cursors_before: CursorSet,
    pub diff: Diff,
}
