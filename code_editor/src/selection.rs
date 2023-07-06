use {
    crate::{Length, Position, Range},
    std::slice,
};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Selection {
    latest: Region,
    earlier: Vec<Region>,
}

impl Selection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.earlier.len() + 1
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            latest: Some(&self.latest),
            earlier: self.earlier.iter(),
        }
    }

    pub fn modify_latest(&mut self, f: impl FnOnce(Region) -> Region) {
        self.latest = f(self.latest);
        let mut index = match self
            .earlier
            .binary_search_by_key(&self.latest.start(), |region| region.start())
        {
            Ok(index) => index,
            Err(index) => index,
        };
        while index > 0 {
            let prev_index = index - 1;
            if self
                .latest
                .try_merge_with(self.earlier[prev_index])
                .is_some()
            {
                self.earlier.remove(prev_index);
                index = prev_index;
            } else {
                break;
            }
        }
        while index < self.earlier.len() {
            if self.latest.try_merge_with(self.earlier[index]).is_some() {
                self.earlier.remove(index);
            } else {
                break;
            }
        }
    }

    pub fn modify_all(&mut self, mut f: impl FnMut(Region) -> Region) {
        self.latest = f(self.latest);
        let mut index = match self
            .earlier
            .binary_search_by_key(&self.latest.start(), |region| region.start())
        {
            Ok(index) => index,
            Err(index) => index,
        };
        while index > 0 {
            let prev_index = index - 1;
            if let Some(merged) = self.latest.try_merge_with(self.earlier[prev_index]) {
                self.latest = merged;
                self.earlier.remove(prev_index);
                index = prev_index;
            } else {
                break;
            }
        }
        while index < self.earlier.len() {
            if let Some(merged) = self.latest.try_merge_with(self.earlier[index]) {
                self.latest = merged;
                self.earlier.remove(index);
            } else {
                break;
            }
        }

        for earlier in &mut self.earlier {
            *earlier = f(*earlier);
        }
        self.earlier.sort_by_key(|earlier| earlier.start());
        let mut index = 0;
        while index + 1 < self.earlier.len() {
            if let Some(merged) = self.earlier[index].try_merge_with(self.earlier[index + 1]) {
                self.earlier[index] = merged;
                self.earlier.remove(index + 1);
            } else {
                index += 1;
            }
        }
    }

    pub fn set(&mut self, region: Region) {
        self.earlier.clear();
        self.latest = region;
    }

    pub fn push(&mut self, region: Region) {
        self.earlier.push(self.latest);
        self.latest = region;
        self.earlier.sort_by_key(|earlier| earlier.start());
    }
}

impl<'a> IntoIterator for &'a Selection {
    type Item = Region;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a> {
    latest: Option<&'a Region>,
    earlier: slice::Iter<'a, Region>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = Region;

    fn next(&mut self) -> Option<Self::Item> {
        match (self.latest, self.earlier.as_slice().first()) {
            (Some(latest), Some(earlier)) => {
                if latest.start() <= earlier.start() {
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
pub struct Region {
    pub anchor: Position,
    pub cursor: Position,
}

impl Region {
    pub fn new(cursor: Position) -> Self {
        Self {
            anchor: cursor,
            cursor,
        }
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn length(self) -> Length {
        self.end() - self.start()
    }

    pub fn start(self) -> Position {
        self.anchor.min(self.cursor)
    }

    pub fn end(self) -> Position {
        self.anchor.max(self.cursor)
    }

    pub fn range(self) -> Range {
        Range::new(self.start(), self.end())
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor,
            ..self
        }
    }

    pub fn update_cursor(self, f: impl FnOnce(Position) -> Position) -> Self {
        Self {
            cursor: f(self.cursor),
            ..self
        }
    }

    pub fn try_merge_with(self, other: Self) -> Option<Self> {
        use std::{cmp, cmp::Ordering, mem};

        let mut first = self;
        let mut second = other;
        if first.start() > second.start() {
            mem::swap(&mut first, &mut second);
        }
        match (first.anchor == first.cursor, second.anchor == second.cursor) {
            (true, true) if first.cursor == second.cursor => Some(self),
            (false, true) if first.end() >= second.cursor => Some(first),
            (true, false) if first.cursor == second.start() => Some(second),
            (false, false) if first.end() > second.start() => {
                Some(match self.anchor.cmp(&self.cursor) {
                    Ordering::Less => Self {
                        anchor: self.anchor.min(other.anchor),
                        cursor: cmp::max_by_key(self.cursor, other.cursor, |cursor| *cursor),
                    },
                    Ordering::Greater => Self {
                        anchor: self.anchor.max(other.anchor),
                        cursor: cmp::min_by_key(self.cursor, other.cursor, |cursor| *cursor),
                    },
                    Ordering::Equal => unreachable!(),
                })
            }
            _ => None,
        }
    }
}
