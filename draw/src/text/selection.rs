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
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Cursor {
    pub index: usize,
    pub affinity: Affinity,
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum Affinity {
    #[default]
    Before,
    After,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Position {
    pub row_index: usize,
    pub x_in_lpxs: f32,
}
