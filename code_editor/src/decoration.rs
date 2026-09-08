use {
    crate::text::{Edit, Length, Position},
    std::{ops::Deref, slice::Iter},
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DecorationType {
    Error,
    Warning,
    /// Full display rows, with a half-open range ending at column zero.
    DiffAdded,
    DiffRemoved,
    /// Gutter-only change mark; never an underline or a row tint.
    DiffChangedGutter,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Decoration {
    pub id: usize,
    pub ty: DecorationType,
    start: Position,
    end: Position,
}

impl Decoration {
    pub fn new(id: usize, start: Position, end: Position, ty: DecorationType) -> Self {
        if start > end {
            return Self {
                ty,
                id,
                start: end,
                end: start,
            };
        }
        Self { ty, id, start, end }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn overlaps_with(self, other: Self) -> bool {
        self.end() > other.start()
    }

    pub fn length(self) -> Length {
        self.end - self.start
    }

    pub fn start(self) -> Position {
        self.start
    }

    pub fn end(self) -> Position {
        self.end
    }

    pub fn apply_edit(self, edit: &Edit) -> Self {
        Self {
            start: self.start.apply_edit(edit),
            end: self.end.apply_edit(edit),
            ..self
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DecorationSet {
    decorations: Vec<Decoration>,
}

impl DecorationSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn as_decorations(&self) -> &[Decoration] {
        &self.decorations
    }

    /// Replace the whole set with sorted, nonoverlapping worker-prepared runs.
    /// Validation is linear and belongs on the worker, before document attachment.
    /// Invalid input leaves the existing set intact. Include any diagnostics that
    /// should survive in the replacement; `add_decoration` keeps its overlap policy.
    pub fn replace_prepared(
        &mut self,
        decorations: Vec<Decoration>,
    ) -> Result<(), PreparedDecorationsError> {
        if decorations.windows(2).any(|pair| {
            pair[0].start() > pair[1].start() || pair[0].overlaps_with(pair[1])
        }) {
            return Err(PreparedDecorationsError::OverlappingOrUnsorted);
        }
        self.decorations = decorations;
        Ok(())
    }

    pub fn add_decoration(&mut self, decoration: Decoration) {
        let index = match self
            .decorations
            .binary_search_by_key(&decoration.start(), |decoration| decoration.start())
        {
            Ok(index) => {
                self.decorations[index] = decoration;
                index
            }
            Err(index) => {
                self.decorations.insert(index, decoration);
                index
            }
        };
        self.remove_overlapping_decorations(index);
    }

    pub fn clear(&mut self) {
        self.decorations.clear();
    }

    pub fn apply_edit(&mut self, edit: &Edit) {
        for decoration in &mut self.decorations {
            *decoration = decoration.apply_edit(edit);
        }
    }

    fn remove_overlapping_decorations(&mut self, index: usize) {
        let mut index = index;
        while index > 0 {
            let prev_index = index - 1;
            if !self.decorations[prev_index].overlaps_with(self.decorations[index]) {
                break;
            }
            self.decorations.remove(prev_index);
            index -= 1;
        }
        while index + 1 < self.decorations.len() {
            let next_index = index + 1;
            if !self.decorations[index].overlaps_with(self.decorations[next_index]) {
                break;
            }
            self.decorations.remove(next_index);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparedDecorationsError {
    OverlappingOrUnsorted,
}

impl Default for DecorationSet {
    fn default() -> Self {
        Self {
            decorations: vec![],
        }
    }
}

impl Deref for DecorationSet {
    type Target = [Decoration];

    fn deref(&self) -> &Self::Target {
        self.decorations.as_slice()
    }
}

impl<'a> IntoIterator for &'a DecorationSet {
    type Item = &'a Decoration;
    type IntoIter = Iter<'a, Decoration>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
