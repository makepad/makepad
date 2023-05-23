use {
    crate::{
        text::{Len, Pos, Range},
        Diff,
    },
    std::{iter::Peekable, slice},
};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct SelSet {
    latest: Sel,
    earlier: Vec<Sel>,
}

impl SelSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.earlier.len() + 1
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            latest: Some(&self.latest),
            earlier: self.earlier.iter().peekable(),
        }
    }

    pub fn update_latest(&mut self, mut f: impl FnMut(Sel) -> Sel) {
        self.latest = f(self.latest);
        self.normalize_latest();
    }

    pub fn update_all(&mut self, mut f: impl FnMut(Sel) -> Sel) {
        for sel in &mut self.earlier {
            *sel = f(*sel);
        }
        self.normalize_earlier();
        self.update_latest(f);
    }

    pub fn push(&mut self, sel: Sel) {
        self.earlier.push(self.latest);
        self.latest = sel;
        self.normalize_latest();
    }

    pub fn apply_diff(&mut self, diff: &Diff, local: bool) {
        for sel in &mut self.earlier {
            *sel = sel.apply_diff(diff, local);
        }
        self.latest = self.latest.apply_diff(diff, local);
    }

    fn normalize_latest(&mut self) {
        let mut index = match self
            .earlier
            .binary_search_by_key(&self.latest.start(), |cursor| cursor.start())
        {
            Ok(index) => index,
            Err(index) => index,
        };
        while index > 0 {
            let prev_index = index - 1;
            if let Some(merged) = self.latest.try_merge(self.earlier[prev_index]) {
                self.latest = merged;
                self.earlier.remove(prev_index);
                index = prev_index;
            } else {
                break;
            }
        }
        while index < self.earlier.len() {
            if let Some(merged) = self.latest.try_merge(self.earlier[index]) {
                self.latest = merged;
                self.earlier.remove(index);
            } else {
                break;
            }
        }
    }

    fn normalize_earlier(&mut self) {
        if self.earlier.is_empty() {
            return;
        }
        self.earlier.sort_by_key(|cursor| cursor.start());
        let mut index = 0;
        while index + 1 < self.earlier.len() {
            if let Some(cursor) = self.earlier[index].try_merge(self.earlier[index + 1]) {
                self.earlier[index] = cursor;
                self.earlier.remove(index + 1);
            } else {
                index += 1;
            }
        }
    }
}

impl<'a> IntoIterator for &'a SelSet {
    type Item = Sel;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a> {
    latest: Option<&'a Sel>,
    earlier: Peekable<slice::Iter<'a, Sel>>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = Sel;

    fn next(&mut self) -> Option<Self::Item> {
        match (self.latest, self.earlier.next()) {
            (Some(cursor_0), Some(cursor_1)) => {
                if cursor_0.start() <= cursor_1.start() {
                    self.latest.take()
                } else {
                    self.earlier.next()
                }
            }
            (Some(_), _) => self.latest.take(),
            (_, Some(_)) => self.earlier.next(),
            _ => None,
        }
        .copied()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Sel {
    pub cursor: Pos,
    pub anchor: Pos,
    pub column: Option<usize>,
}

impl Sel {
    pub fn is_empty(self) -> bool {
        self.cursor == self.anchor
    }

    pub fn len(self) -> Len {
        self.end() - self.start()
    }

    pub fn start(self) -> Pos {
        self.cursor.min(self.anchor)
    }

    pub fn end(self) -> Pos {
        self.cursor.max(self.anchor)
    }

    pub fn range(self) -> Range {
        Range {
            start: self.start(),
            end: self.end(),
        }
    }

    pub fn update_cursor(self, f: impl FnOnce(Pos, Option<usize>) -> (Pos, Option<usize>)) -> Self {
        let (cursor, column) = f(self.cursor, self.column);
        Self {
            cursor,
            column,
            ..self
        }
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor,
            ..self
        }
    }

    pub fn try_merge(self, other: Self) -> Option<Self> {
        use std::{cmp::Ordering, mem};

        let mut first = self;
        let mut second = other;
        if first.start() > second.start() {
            mem::swap(&mut first, &mut second);
        }
        match (first.is_empty(), second.is_empty()) {
            (true, true) if first.cursor == second.cursor => Some(self),
            (false, true) if second.cursor <= first.end() => Some(Self {
                cursor: first.cursor,
                anchor: first.anchor,
                ..self
            }),
            (true, false) if first.cursor == second.start() => Some(Self {
                cursor: second.cursor,
                anchor: second.anchor,
                ..self
            }),
            (false, false) if first.end() > second.start() => {
                Some(match self.cursor.cmp(&self.anchor) {
                    Ordering::Less => Self {
                        cursor: self.cursor.min(other.cursor),
                        anchor: self.anchor.max(other.anchor),
                        ..self
                    },
                    Ordering::Greater => Self {
                        cursor: self.cursor.max(other.cursor),
                        anchor: self.anchor.min(other.anchor),
                        ..self
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
                cursor: self.cursor.apply_diff(diff, true),
                ..self
            }
            .reset_anchor()
        } else {
            match self.cursor.cmp(&self.anchor) {
                Ordering::Less => Self {
                    cursor: self.cursor.apply_diff(diff, false),
                    anchor: self.anchor.apply_diff(diff, true),
                    ..self
                },
                Ordering::Equal => Self {
                    cursor: self.cursor.apply_diff(diff, true),
                    ..self
                }
                .reset_anchor(),
                Ordering::Greater => Self {
                    cursor: self.cursor.apply_diff(diff, true),
                    anchor: self.anchor.apply_diff(diff, false),
                    ..self
                },
            }
        }
    }
}
