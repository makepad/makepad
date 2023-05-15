use crate::{CursorSet, Diff};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hist {
    revs: Vec<Rev>,
    rev_id: usize,
}

impl Hist {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn undo(&mut self) -> Option<(Diff, CursorSet)> {
        if self.rev_id == 0 {
            return None;
        }
        let diff = self.revs[self.rev_id].diff.clone().invert();
        self.rev_id -= 1;
        let cursors = self.revs[self.rev_id].cursors.clone();
        Some((diff, cursors))
    }

    pub fn redo(&mut self) -> Option<(Diff, CursorSet)> {
        if self.rev_id == self.revs.len() - 1 {
            return None;
        }
        self.rev_id += 1;
        let diff = self.revs[self.rev_id].diff.clone();
        let cursors = self.revs[self.rev_id].cursors.clone();
        Some((diff, cursors))
    }

    pub fn commit(&mut self, diff: Diff, cursors: CursorSet) {
        self.rev_id += 1;
        self.revs.truncate(self.rev_id);
        self.revs.push(Rev { diff, cursors });
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
struct Rev {
    diff: Diff,
    cursors: CursorSet,
}
