use crate::{CursorSet, Diff, Hist, Text};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Buf {
    text: Text,
    hist: Hist,
    edit_group: Option<EditGroup>,
}

impl Buf {
    pub fn new(text: Text) -> Self {
        Self {
            text,
            hist: Hist::new(),
            edit_group: None,
        }
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    pub fn end_edit_group(&mut self) {
        if let Some(edit_group) = self.edit_group.take() {
            self.hist.commit(edit_group.cursors_before, edit_group.diff);
        }
    }

    pub fn edit(&mut self, kind: EditKind, cursors_before: &CursorSet, diff: Diff) {
        use std::mem;

        self.text.apply_diff(diff.clone());
        if self
            .edit_group
            .as_ref()
            .map_or(false, |edit_group| edit_group.kind != kind)
        {
            self.end_edit_group();
        }
        let edit_group = self.edit_group.get_or_insert(EditGroup {
            kind,
            cursors_before: cursors_before.clone(),
            diff: Diff::new(),
        });
        edit_group.diff = mem::take(&mut edit_group.diff).compose(diff);
    }

    pub fn undo(&mut self) -> Option<(CursorSet, Diff)> {
        if self.edit_group.is_some() {
            self.end_edit_group();
        }
        if let Some((cursors, diff)) = self.hist.undo() {
            self.text.apply_diff(diff.clone());
            Some((cursors, diff))
        } else {
            None
        }
    }

    pub fn redo(&mut self) -> Option<(CursorSet, Diff)> {
        if self.edit_group.is_some() {
            return None;
        }
        if let Some((cursors, diff)) = self.hist.redo() {
            self.text.apply_diff(diff.clone());
            Some((cursors, diff))
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditKind {
    Insert,
    Delete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EditGroup {
    kind: EditKind,
    cursors_before: CursorSet,
    diff: Diff,
}
