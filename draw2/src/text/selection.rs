#[derive(Clone, Copy, Debug, Default)]
pub struct Selection {
    pub cursor: Cursor,
    pub anchor: Cursor,
}

impl Selection {
    pub fn start(self) -> Cursor {
        self.cursor.min(self.anchor)
    }

    pub fn end(self) -> Cursor {
        self.cursor.max(self.anchor)
    }

    /// Returns `true` if this `Selection` and the `other` Selection
    /// have the same `index`` values for both their `cursor` and `anchor`.
    pub fn index_eq(self, other: Selection) -> bool {
        self.cursor.index == other.cursor.index
            && self.anchor.index == other.anchor.index
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Cursor {
    pub index: usize,
    pub prefer_next_row: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CursorPosition {
    pub row_index: usize,
    pub x_in_lpxs: f32,
}
