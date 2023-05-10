use crate::{Diff, CursorSet, Text};

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
        let sel = self.revs[self.rev_id].sel.clone();
        text.apply_diff(diff.clone());
        Some((diff, sel))
    }

    pub fn redo(&mut self, text: &mut Text) -> Option<(Diff, CursorSet)> {
        if self.rev_id == self.revs.len() {
            return None;
        }
        self.rev_id += 1;
        let diff = self.revs[self.rev_id].diff.clone();
        let sel = self.revs[self.rev_id].sel.clone();
        text.apply_diff(diff.clone());
        Some((diff, sel))
    }

    pub fn commit(&mut self, diff: Diff, sel: CursorSet) {
        self.rev_id += 1;
        self.revs.truncate(self.rev_id);
        self.revs.push(Rev { diff, sel });
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
    sel: CursorSet,
}
