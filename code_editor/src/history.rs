use crate::{
    selection::SelectionSet,
    state::SessionId,
    text::{Edit, Text},
};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct History {
    text: Text,
    current_desc: Option<GroupDesc>,
    undo_stack: Stack,
    redo_stack: Stack,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn as_text(&self) -> &Text {
        &self.text
    }

    pub fn force_new_group(&mut self) {
        self.current_desc = None;
    }

    pub fn push_or_extend_group(
        &mut self,
        session_id: SessionId,
        edit_kind: EditKind,
        selections: &SelectionSet,
    ) {
        let desc = GroupDesc {
            session_id,
            edit_kind,
        };
        if !self
            .current_desc
            .map_or(false, |current_desc| current_desc.can_merge_with(desc))
        {
            self.undo_stack.push_group(selections.clone());
            self.current_desc = Some(desc);
        }
    }

    pub fn apply_edit(&mut self, edit: Edit) {
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
        if let Some(new_selections) = self.undo_stack.pop_group(edits) {
            self.redo_stack.push_group(selections.clone());
            for edit in edits {
                let inverted_edit = edit.clone().invert(&self.text);
                self.text.apply_change(edit.change.clone());
                self.redo_stack.push_edit(inverted_edit);
            }
            self.current_desc = None;
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
        if let Some(new_selections) = self.redo_stack.pop_group(edits) {
            self.undo_stack.push_group(selections.clone());
            for edit in edits {
                let inverted_edit = edit.clone().invert(&self.text);
                self.text.apply_change(edit.change.clone());
                self.undo_stack.push_edit(inverted_edit);
            }
            self.current_desc = None;
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
    InsertSpace,
    Delete,
    Indent,
    Outdent,
    Other,
}

impl EditKind {
    fn can_merge_with(self, other: Self) -> bool {
        if self == Self::Other {
            return false;
        }
        self == other
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct GroupDesc {
    session_id: SessionId,
    edit_kind: EditKind,
}

impl GroupDesc {
    fn can_merge_with(self, other: GroupDesc) -> bool {
        self.session_id == other.session_id && self.edit_kind.can_merge_with(other.edit_kind)
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
struct Stack {
    groups: Vec<Group>,
    edits: Vec<Edit>,
}

impl Stack {
    fn push_group(&mut self, selections: SelectionSet) {
        self.groups.push(Group {
            selections,
            edit_start: self.edits.len(),
        });
    }

    fn push_edit(&mut self, edit: Edit) {
        self.edits.push(edit);
    }

    fn pop_group(&mut self, edits: &mut Vec<Edit>) -> Option<SelectionSet> {
        match self.groups.pop() {
            Some(group) => {
                edits.extend(self.edits.drain(group.edit_start..).rev());
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
struct Group {
    selections: SelectionSet,
    edit_start: usize,
}
