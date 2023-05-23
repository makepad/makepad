use crate::{Diff, Hist, SelSet, Text};

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

    pub fn flush(&mut self) {
        if self.edit_group.is_some() {
            self.end_edit_group();
        }
    }

    pub fn edit(&mut self, kind: EditKind, sels_before: &SelSet, diff: Diff) {
        use std::mem;

        self.text.apply_diff(diff.clone());
        if self
            .edit_group
            .as_ref()
            .map_or(false, |edit_group| edit_group.kind != kind)
        {
            self.end_edit_group();
        }
        if let Some(edit_group) = &mut self.edit_group {
            edit_group.diff = mem::take(&mut edit_group.diff).compose(diff);
        } else {
            self.begin_edit_group(kind, sels_before.clone(), diff);
        }
    }

    pub fn undo(&mut self) -> Option<(SelSet, Diff)> {
        self.flush();
        if let Some((sels, diff)) = self.hist.undo() {
            self.text.apply_diff(diff.clone());
            Some((sels, diff))
        } else {
            None
        }
    }

    pub fn redo(&mut self) -> Option<(SelSet, Diff)> {
        if self.edit_group.is_some() {
            return None;
        }
        if let Some((sels, diff)) = self.hist.redo() {
            self.text.apply_diff(diff.clone());
            Some((sels, diff))
        } else {
            None
        }
    }

    fn begin_edit_group(&mut self, kind: EditKind, sels_before: SelSet, diff: Diff) {
        assert!(self.edit_group.is_none());
        self.edit_group = Some(EditGroup {
            kind,
            sels_before: sels_before.clone(),
            diff,
        });
    }

    fn end_edit_group(&mut self) {
        let edit_group = self.edit_group.take().unwrap();
        self.hist.commit(edit_group.sels_before, edit_group.diff);
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
    sels_before: SelSet,
    diff: Diff,
}
