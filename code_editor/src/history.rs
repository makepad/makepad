use crate::{
    selection::SelectionSet,
    state::SessionId,
    text::{Change, Drift, Text},
};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct History {
    text: Text,
    current_edit_group: Option<EditGroupDescriptor>,
    edits: Vec<Edit>,
    undo_stack: EditStack,
    redo_stack: EditStack,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn as_text(&self) -> &Text {
        &self.text
    }

    pub fn force_new_edit_group(&mut self) {
        self.current_edit_group = None;
    }

    pub fn push_or_extend_edit_group(
        &mut self,
        session_id: SessionId,
        kind: EditKind,
        selections: &SelectionSet,
    ) {
        let edit_group = EditGroupDescriptor { session_id, kind };
        if !self.current_edit_group.map_or(false, |current_edit_group| {
            current_edit_group.can_group_with(edit_group)
        }) {
            self.undo_stack.push_edit_group(selections.clone());
            self.current_edit_group = Some(edit_group);
        }
    }

    pub fn edit(&mut self, edit: Edit) {
        let inverted_edit = edit.clone().invert(&self.text);
        self.text.apply_change(edit.change);
        self.undo_stack.push_edit(inverted_edit);
        self.redo_stack.clear();
    }

    pub fn undo(
        &mut self,
        selections: &SelectionSet,
        edits: &mut Vec<Edit>,
    ) -> Option<SelectionSet> {
        if let Some(new_selections) = self.undo_stack.pop_edit_group(edits) {
            self.redo_stack.push_edit_group(selections.clone());
            for edit in edits {
                let inverted_edit = edit.clone().invert(&self.text);
                self.text.apply_change(edit.change.clone());
                self.redo_stack.push_edit(inverted_edit);
            }
            self.current_edit_group = None;
            Some(new_selections)
        } else {
            None
        }
    }

    pub fn redo(
        &mut self,
        selections: &SelectionSet,
        edits: &mut Vec<Edit>,
    ) -> Option<SelectionSet> {
        if let Some(new_selections) = self.redo_stack.pop_edit_group(edits) {
            self.undo_stack.push_edit_group(selections.clone());
            for edit in edits {
                let inverted_edit = edit.clone().invert(&self.text);
                self.text.apply_change(edit.change.clone());
                self.undo_stack.push_edit(inverted_edit);
            }
            self.current_edit_group = None;
            Some(new_selections)
        } else {
            None
        }
    }

    pub fn into_text(self) -> Text {
        self.text
    }
}

impl From<Text> for History {
    fn from(text: Text) -> Self {
        Self {
            text,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditKind {
    Insert,
    Delete,
    Indent,
    Outdent,
    Space,
    Other,
}

impl EditKind {
    fn can_group_with(self, other: Self) -> bool {
        if self == Self::Other {
            return false;
        }
        self == other
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Edit {
    pub change: Change,
    pub drift: Drift,
}

impl Edit {
    fn invert(self, text: &Text) -> Self {
        Self {
            change: self.change.invert(text),
            drift: self.drift,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct EditGroupDescriptor {
    session_id: SessionId,
    kind: EditKind,
}

impl EditGroupDescriptor {
    fn can_group_with(self, other: EditGroupDescriptor) -> bool {
        self.session_id == other.session_id && self.kind.can_group_with(other.kind)
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
struct EditStack {
    groups: Vec<EditGroup>,
    edits: Vec<Edit>,
}

impl EditStack {
    fn push_edit_group(&mut self, selections: SelectionSet) {
        self.groups.push(EditGroup {
            selections,
            start_index: self.edits.len(),
        });
    }

    fn push_edit(&mut self, edit: Edit) {
        self.edits.push(edit);
    }

    fn pop_edit_group(&mut self, edits: &mut Vec<Edit>) -> Option<SelectionSet> {
        match self.groups.pop() {
            Some(group) => {
                edits.extend(self.edits.drain(group.start_index..).rev());
                Some(group.selections)
            }
            None => None,
        }
    }

    fn clear(&mut self) {
        self.groups.clear();
        self.edits.clear();
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct EditGroup {
    selections: SelectionSet,
    start_index: usize,
}
