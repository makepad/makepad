use crate::{CursorSet, Diff, Hist, Text};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Buf {
    text: Text,
    hist: Hist,
    cursors_before: CursorSet,
    diff: Diff,
}

impl Buf {
    pub fn new(text: Text) -> Self {
        Self {
            text,
            hist: Hist::new(),
            cursors_before: CursorSet::new(),
            diff: Diff::new(),
        }
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    pub fn begin_commit(&mut self, cursors_before: CursorSet) {
        self.cursors_before = cursors_before;
    }

    pub fn end_commit(&mut self) {
        use std::mem;

        self.hist.commit(
            mem::take(&mut self.cursors_before),
            mem::take(&mut self.diff),
        );
    }

    pub fn apply_diff(&mut self, diff: Diff) {
        use std::mem;

        self.text.apply_diff(diff.clone());
        self.diff = mem::take(&mut self.diff).compose(diff);
    }

    pub fn undo(&mut self) -> Option<(CursorSet, Diff)> {
        if let Some((cursors_before, diff)) = self.hist.undo() {
            self.text.apply_diff(diff.clone());
            Some((cursors_before, diff))
        } else {
            None
        }
    }

    pub fn redo(&mut self) -> Option<(Diff, CursorSet)> {
        if let Some((diff, cursors_after)) = self.hist.redo() {
            self.text.apply_diff(diff.clone());
            Some((diff, cursors_after))
        } else {
            None
        }
    }
}
