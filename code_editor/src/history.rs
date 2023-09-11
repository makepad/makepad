use crate::{
    state::SessionId,
    text::{Change, Drift, Text},
    selection::SelectionSet,
};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct History {
    current_edit: Option<(SessionId, EditKind)>,
    undos: Vec<(SelectionSet, Vec<(Change, Drift)>)>,
    redos: Vec<(SelectionSet, Vec<(Change, Drift)>)>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn force_new_edit_group(&mut self) {
        self.current_edit = None;
    }

    pub fn edit(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &SelectionSet,
        inverted_changes: Vec<(Change, Drift)>,
    ) {
        if self
            .current_edit
            .map_or(false, |(current_origin_id, current_kind)| {
                current_origin_id == origin_id && current_kind.can_merge(kind)
            })
        {
            self.undos.last_mut().unwrap().1.extend(inverted_changes);
        } else {
            self.current_edit = Some((origin_id, kind));
            self.undos.push((selections.clone(), inverted_changes));
        }
        self.redos.clear();
    }

    pub fn undo(&mut self, text: &mut Text) -> Option<(SelectionSet, Vec<(Change, Drift)>)> {
        if let Some((selections, mut inverted_changes)) = self.undos.pop() {
            self.current_edit = None;
            let mut changes = Vec::new();
            inverted_changes.reverse();
            for (inverted_change, drift) in inverted_changes.iter().cloned() {
                let change = inverted_change.clone().invert(&text);
                text.apply_change(inverted_change);
                changes.push((change, drift));
            }
            changes.reverse();
            self.redos.push((selections.clone(), changes.clone()));
            Some((selections, inverted_changes))
        } else {
            None
        }
    }

    pub fn redo(&mut self, text: &mut Text) -> Option<(SelectionSet, Vec<(Change, Drift)>)> {
        if let Some((selections, changes)) = self.redos.pop() {
            self.current_edit = None;
            let mut inverted_changes = Vec::new();
            for (change, drift) in changes.iter().cloned() {
                inverted_changes.push((change.clone().invert(&text), drift));
                text.apply_change(change);
            }
            self.undos.push((selections.clone(), inverted_changes));
            Some((selections, changes))
        } else {
            None
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
    fn can_merge(self, other: Self) -> bool {
        if self == Self::Other {
            return false;
        }
        self == other
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EditGroup {
    pub selections: SelectionSet,
    pub changes: Vec<(Change, Drift)>,
}
