use {
    crate::text::{Edit, Length, Position},
    std::{ops::Deref, slice::Iter},
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Decoration {
    pub id: usize,
    start: Position,
    end: Position,
}

impl Decoration {
    pub fn new(id: usize, start: Position, end: Position) -> Self {
        assert!(start <= end);
        Self {
            id,
            start,
            end,
        }
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
    decorations: Vec<Decoration>
}

impl DecorationSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn as_decorations(&self) -> &[Decoration] {
        &self.decorations
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

impl Default for DecorationSet {
    fn default() -> Self {
        Self {
            decorations: vec![Decoration::new(
                0,
                Position {
                    line_index: 0,
                    byte_index: 4,
                },
                Position {
                    line_index: 3,
                    byte_index: 8,
                },
            )]
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