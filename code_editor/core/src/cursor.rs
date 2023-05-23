use crate::{Diff, text::{Len, Pos, Range}};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Cursor {
    pub caret: Pos,
    pub anchor: Pos,
    pub column: Option<usize>,
}

impl Cursor {
    pub fn is_empty(self) -> bool {
        self.caret == self.anchor
    }

    pub fn len(self) -> Len {
        self.end() - self.start()
    }

    pub fn start(self) -> Pos {
        self.caret.min(self.anchor)
    }

    pub fn end(self) -> Pos {
        self.caret.max(self.anchor)
    }

    pub fn range(self) -> Range {
        Range {
            start: self.start(),
            end: self.end(),
        }
    }

    pub fn do_move(
        self,
        select: bool,
        f: impl FnOnce(Pos, Option<usize>) -> (Pos, Option<usize>),
    ) -> Self {
        let (caret, column) = f(self.caret, self.column);
        let mut cursor = Self {
            caret,
            column,
            ..self
        };
        if !select {
            cursor = cursor.reset_anchor();
        }
        cursor
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.caret,
            ..self
        }
    }

    pub fn merge(self, other: Self) -> Option<Self> {
        use std::{cmp::Ordering, mem};

        let mut first = self;
        let mut second = other;
        if first.start() > second.start() {
            mem::swap(&mut first, &mut second);
        }
        match (first.is_empty(), second.is_empty()) {
            (true, true) if first.caret == second.caret => Some(self),
            (false, true) if second.caret <= first.end() => Some(self),
            (true, false) if first.caret == second.start() => Some(other),
            (false, false) if first.end() > second.start() => {
                Some(match self.caret.cmp(&self.anchor) {
                    Ordering::Less => Self {
                        caret: self.caret.min(other.caret),
                        anchor: self.anchor.max(other.anchor),
                        column: self.column.min(other.column),
                    },
                    Ordering::Greater => Self {
                        caret: self.caret.max(other.caret),
                        anchor: self.anchor.min(other.anchor),
                        column: self.column.max(other.column),
                    },
                    Ordering::Equal => unreachable!(),
                })
            }
            _ => None,
        }
    }

    pub fn apply_diff(self, diff: &Diff, local: bool) -> Self {
        use std::cmp::Ordering;

        if local {
            Self {
                caret: self.caret.apply_diff(diff, true),
                ..self
            }
            .reset_anchor()
        } else {
            match self.caret.cmp(&self.anchor) {
                Ordering::Less => Self {
                    caret: self.caret.apply_diff(diff, false),
                    anchor: self.anchor.apply_diff(diff, true),
                    ..self
                },
                Ordering::Equal => Self {
                    caret: self.caret.apply_diff(diff, true),
                    ..self
                }
                .reset_anchor(),
                Ordering::Greater => Self {
                    caret: self.caret.apply_diff(diff, true),
                    anchor: self.anchor.apply_diff(diff, false),
                    ..self
                },
            }
        }
    }
}
