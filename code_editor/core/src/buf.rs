use crate::{CursorSet, Diff, Hist, Text};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Buf {
    text: Text,
    diff: Diff,
    hist: Hist,
}

impl Buf {
    pub fn new(text: Text) -> Self {
        Self {
            text,
            diff: Diff::new(),
            hist: Hist::new(),
        }
    }

    pub fn needs_commit(&self) -> bool {
        !self.diff.is_empty()
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    pub fn apply_diff(&mut self, diff: Diff) {
        use std::mem;

        self.text.apply_diff(diff.clone());
        self.diff = mem::take(&mut self.diff).compose(diff);
    }

    pub fn undo(&mut self) -> Option<(Diff, CursorSet)> {
        assert!(!self.needs_commit());
        if let Some((diff, cursors)) = self.hist.undo() {
            self.text.apply_diff(diff.clone());
            Some((diff, cursors))
        } else {
            None
        }
    }

    pub fn redo(&mut self) -> Option<(Diff, CursorSet)> {
        assert!(!self.needs_commit());
        if let Some((diff, cursors)) = self.hist.redo() {
            self.text.apply_diff(diff.clone());
            Some((diff, cursors))
        } else {
            None
        }
    }

    pub fn commit(&mut self, cursors: CursorSet) {
        use std::mem;

        let diff = mem::take(&mut self.diff);
        self.hist.commit(diff, cursors);
    }
}
