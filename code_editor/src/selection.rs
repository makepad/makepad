use {
    crate::text::{Change, Drift, Length, Position, Range},
    std::{ops, ops::Deref},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Hash, Eq)]
pub struct Selection {
    pub anchor: Position,
    pub cursor: Position,
    pub affinity: Affinity,
    pub preferred_column: Option<usize>,
}

impl Selection {
    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn should_merge(self, other: Self) -> bool {
        if self.is_empty() || other.is_empty() {
            self.end() >= other.start()
        } else {
            self.end() > other.start()
        }
    }

    pub fn start(self) -> Position {
        self.anchor.min(self.cursor)
    }

    pub fn start_affinity(self) -> Affinity {
        if self.anchor < self.cursor {
            Affinity::After
        } else {
            self.affinity
        }
    }

    pub fn end(self) -> Position {
        self.anchor.max(self.cursor)
    }

    pub fn end_affinity(self) -> Affinity {
        if self.cursor < self.anchor {
            Affinity::Before
        } else {
            self.affinity
        }
    }

    pub fn extent(self) -> Length {
        self.end() - self.start()
    }

    pub fn range(self) -> Range {
        Range::new(self.start(), self.end()).unwrap()
    }

    pub fn line_range(self) -> ops::Range<usize> {
        if self.anchor <= self.cursor {
            self.anchor.line_index..self.cursor.line_index + 1
        } else {
            self.cursor.line_index..if self.anchor.byte_index == 0 {
                self.anchor.line_index
            } else {
                self.anchor.line_index + 1
            }
        }
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor,
            ..self
        }
    }

    pub fn update_cursor(
        self,
        f: impl FnOnce(Position, Affinity, Option<usize>) -> (Position, Affinity, Option<usize>),
    ) -> Self {
        let (cursor, affinity, preferred_column) =
            f(self.cursor, self.affinity, self.preferred_column);
        Self {
            cursor,
            affinity,
            preferred_column,
            ..self
        }
    }

    pub fn merge(self, other: Self) -> Option<Self> {
        if self.should_merge(other) {
            Some(if self.anchor <= self.cursor {
                Selection {
                    anchor: self.anchor,
                    cursor: other.cursor,
                    affinity: other.affinity,
                    preferred_column: other.preferred_column,
                }
            } else {
                Selection {
                    anchor: other.anchor,
                    cursor: self.cursor,
                    affinity: self.affinity,
                    preferred_column: self.preferred_column,
                }
            })
        } else {
            None
        }
    }

    pub fn apply_change(self, change: &Change, drift: Drift) -> Selection {
        Self {
            anchor: self.anchor.apply_change(change, drift),
            cursor: self.cursor.apply_change(change, drift),
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Affinity {
    Before,
    After,
}

impl Default for Affinity {
    fn default() -> Self {
        Self::Before
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
            if !self.selections[prev_index].should_merge(self.selections[index]) {
                break;
            }
            self.selections.remove(prev_index);
            index -= 1;
        }
        while index + 1 < self.selections.len() {
            let next_index = index + 1;
            if !self.selections[index].should_merge(self.selections[next_index]) {
                break;
            }
            self.selections.remove(next_index);
        }
        index
    }

    pub fn update_all_selections(
        &mut self,
        index: Option<usize>,
        mut f: impl FnMut(Selection) -> Selection,
    ) -> Option<usize> {
        for selection in &mut self.selections {
            *selection = f(*selection);
        }
        let mut index = index;
        let mut current_index = 0;
        while current_index + 1 < self.selections.len() {
            let next_index = current_index + 1;
            let current_selection = self.selections[current_index];
            let next_selection = self.selections[next_index];
            assert!(current_selection.start() <= next_selection.start());
            if let Some(merged_selection) = current_selection.merge(next_selection) {
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

    pub fn apply_change(&mut self, change: &Change, drift: Drift) {
        for selection in &mut self.selections {
            *selection = selection.apply_change(change, drift);
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