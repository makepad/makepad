use crate::{CursorSet, Diff, Text};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hist {
    revs: Vec<Rev>,
    rev_id: usize,
}

impl Hist {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn undo(&mut self, text: &mut Text) -> Option<(Diff, CursorSet)> {
        if self.rev_id == 0 {
            return None;
        }
        let diff = self.revs[self.rev_id].diff.clone().invert(&text);
        self.rev_id -= 1;
        let cursors = self.revs[self.rev_id].cursors.clone();
        text.apply_diff(diff.clone());
        Some((diff, cursors))
    }

    pub fn redo(&mut self, text: &mut Text) -> Option<(Diff, CursorSet)> {
        if self.rev_id == self.revs.len() - 1 {
            return None;
        }
        self.rev_id += 1;
        let diff = self.revs[self.rev_id].diff.clone();
        let cursors = self.revs[self.rev_id].cursors.clone();
        text.apply_diff(diff.clone());
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
