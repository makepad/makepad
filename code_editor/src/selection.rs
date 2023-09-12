use {
    crate::text::{Edit, Length, Position, Range},
    std::{ops, ops::Deref, slice::Iter},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Hash, Eq)]
pub struct Selection {
    pub cursor: Cursor,
    pub anchor: Position,
}

impl Selection {
    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor.position
    }

    pub fn overlaps_with(self, other: Self) -> bool {
        if self.is_empty() || other.is_empty() {
            self.end() >= other.start()
        } else {
            self.end() > other.start()
        }
    }

    pub fn start(self) -> Position {
        self.cursor.position.min(self.anchor)
    }

    pub fn start_affinity(self) -> Affinity {
        if self.anchor < self.cursor.position {
            Affinity::After
        } else {
            self.cursor.affinity
        }
    }

    pub fn end(self) -> Position {
        self.cursor.position.max(self.anchor)
    }

    pub fn end_affinity(self) -> Affinity {
        if self.cursor.position < self.anchor {
            Affinity::Before
        } else {
            self.cursor.affinity
        }
    }

    pub fn length(self) -> Length {
        self.end() - self.start()
    }

    pub fn range(self) -> Range {
        Range::new(self.start(), self.end()).unwrap()
    }

    pub fn line_range(self) -> ops::Range<usize> {
        if self.anchor <= self.cursor.position {
            self.anchor.line_index..self.cursor.position.line_index + 1
        } else {
            self.cursor.position.line_index..if self.anchor.byte_index == 0 {
                self.anchor.line_index
            } else {
                self.anchor.line_index + 1
            }
        }
    }

    pub fn update_cursor(self, f: impl FnOnce(Cursor) -> Cursor) -> Self {
        Self {
            cursor: f(self.cursor),
            ..self
        }
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor.position,
            ..self
        }
    }

    pub fn merge_with(self, other: Self) -> Option<Self> {
        if self.overlaps_with(other) {
            Some(if self.anchor <= self.cursor.position {
                Selection {
                    anchor: self.anchor,
                    cursor: other.cursor,
                }
            } else {
                Selection {
                    anchor: other.anchor,
                    cursor: self.cursor,
                }
            })
        } else {
            None
        }
    }

    pub fn apply_edit(self, edit: &Edit) -> Selection {
        Self {
            anchor: self.anchor.apply_edit(edit),
            cursor: self.cursor.apply_edit(edit),
            ..self
        }
    }
}

impl From<Cursor> for Selection {
    fn from(cursor: Cursor) -> Self {
        Self {
            cursor,
            anchor: cursor.position,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SelectionSet {
    selections: Vec<Selection>,
}

impl SelectionSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update_selection(
        &mut self,
        index: usize,
        f: impl FnOnce(Selection) -> Selection,
    ) -> usize {
        self.selections[index] = f(self.selections[index]);
        let mut index = index;
        while index > 0 {
            let prev_index = index - 1;
            if !self.selections[prev_index].overlaps_with(self.selections[index]) {
                break;
            }
            self.selections.remove(prev_index);
            index -= 1;
        }
        while index + 1 < self.selections.len() {
            let next_index = index + 1;
            if !self.selections[index].overlaps_with(self.selections[next_index]) {
                break;
            }
            self.selections.remove(next_index);
        }
        index
    }

    pub fn update_all_selections(
        &mut self,
        retained_index: Option<usize>,
        mut f: impl FnMut(Selection) -> Selection,
    ) -> Option<usize> {
        for selection in &mut self.selections {
            *selection = f(*selection);
        }
        let mut index = retained_index;
        let mut current_index = 0;
        while current_index + 1 < self.selections.len() {
            let next_index = current_index + 1;
            let current_selection = self.selections[current_index];
            let next_selection = self.selections[next_index];
            assert!(current_selection.start() <= next_selection.start());
            if let Some(merged_selection) = current_selection.merge_with(next_selection) {
                self.selections[current_index] = merged_selection;
                self.selections.remove(next_index);
                if let Some(index) = &mut index {
                    if next_index < *index {
                        *index -= 1;
                    }
                }
            } else {
                current_index += 1;
            }
        }
        index
    }

    pub fn apply_change(&mut self, edit: &Edit) {
        for selection in &mut self.selections {
            *selection = selection.apply_edit(edit);
        }
    }

    pub fn push_selection(&mut self, selection: Selection) -> usize {
        match self
            .selections
            .binary_search_by_key(&selection.start(), |selection| selection.start())
        {
            Ok(index) => {
                self.selections[index] = selection;
                index
            }
            Err(index) => {
                self.selections.insert(index, selection);
                index
            }
        }
    }

    pub fn set_selection(&mut self, selection: Selection) {
        self.selections.clear();
        self.selections.push(selection);
    }
}

impl Default for SelectionSet {
    fn default() -> Self {
        Self {
            selections: vec![Selection::default()],
        }
    }
}

impl Deref for SelectionSet {
    type Target = [Selection];

    fn deref(&self) -> &Self::Target {
        &self.selections
    }
}

impl<'a> IntoIterator for &'a SelectionSet {
    type Item = &'a Selection;
    type IntoIter = Iter<'a, Selection>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Cursor {
    pub position: Position,
    pub affinity: Affinity,
    pub preferred_column_index: Option<usize>,
}

impl Cursor {
    pub fn apply_edit(self, edit: &Edit) -> Self {
        Self {
            position: self.position.apply_edit(edit),
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Affinity {
    Before,
    After,
}

impl Default for Affinity {
    fn default() -> Self {
        Self::Before
    }
}
