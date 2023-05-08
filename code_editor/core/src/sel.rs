use {
    crate::{Diff, Len, Pos},
    std::{iter::Peekable, slice},
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Sel {
    latest: Region,
    earlier: Vec<Region>,
}

impl Sel {
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

    pub fn spans(&self) -> Spans<'_> {
        Spans {
            pos: Pos::default(),
            iter: self.iter().peekable(),
        }
    }

    pub fn update_latest(&mut self, mut f: impl FnMut(&mut Region)) {
        f(&mut self.latest);
        self.normalize_latest();
    }

    pub fn update_all(&mut self, mut f: impl FnMut(&mut Region)) {
        for region in &mut self.earlier {
            f(region);
        }
        self.normalize_earlier();
        self.update_latest(f);
    }

    pub fn push(&mut self, region: Region) {
        self.earlier.push(self.latest);
        self.latest = region;
        self.normalize_latest();
    }

    pub fn apply_diff(&mut self, diff: &Diff, local: bool) {
        for region in &mut self.earlier {
            region.apply_diff(diff, local);
        }
        self.latest.apply_diff(diff, local);
    }

    fn normalize_latest(&mut self) {
        let mut index = match self
            .earlier
            .binary_search_by_key(&self.latest.start(), |region| region.start())
        {
            Ok(index) => index,
            Err(index) => index,
        };
        while index > 0 {
            let prev_index = index - 1;
            if self.latest.merge(self.earlier[prev_index]) {
                self.earlier.remove(prev_index);
                index = prev_index;
            } else {
                break;
            }
        }
        while index < self.earlier.len() {
            if self.latest.merge(self.earlier[index]) {
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
        self.earlier.sort_by_key(|region| region.start());
        let mut index = 1;
        while index < self.earlier.len() {
            let region = self.earlier[index];
            if self.earlier[index - 1].merge(region) {
                self.earlier.remove(index + 1);
            } else {
                index += 1;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a> {
    latest: Option<&'a Region>,
    earlier: Peekable<slice::Iter<'a, Region>>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = Region;

    fn next(&mut self) -> Option<Self::Item> {
        match (self.latest, self.earlier.next()) {
            (Some(region_0), Some(region_1)) => {
                if region_0.start() <= region_1.start() {
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Region {
    pub active: Pos,
    pub column: Option<usize>,
    pub inactive: Pos,
}

impl Region {
    pub fn is_empty(self) -> bool {
        self.active == self.inactive
    }

    pub fn len(self) -> Len {
        self.end() - self.start()
    }

    pub fn start(self) -> Pos {
        self.active.min(self.inactive)
    }

    pub fn end(self) -> Pos {
        self.active.max(self.inactive)
    }

    pub fn update(&mut self, f: impl FnOnce(Pos, Option<usize>) -> (Pos, Option<usize>)) {
        let (active, column) = f(self.active, self.column);
        self.active = active;
        self.column = column;
    }

    pub fn clear(&mut self) {
        self.inactive = self.active;
    }

    pub fn merge(&mut self, other: Region) -> bool {
        use std::{cmp::Ordering, mem};

        let mut first = *self;
        let mut second = other;
        if first.start() > second.start() {
            mem::swap(&mut first, &mut second);
        }
        match (first.is_empty(), second.is_empty()) {
            (true, true) if first.active == second.active => true,
            (false, true) if second.active <= first.end() => true,
            (true, false) if first.active == second.start() => {
                *self = other;
                true
            }
            (false, false) if first.end() > second.start() => {
                match self.active.cmp(&self.inactive) {
                    Ordering::Less => {
                        self.active = self.active.min(other.active);
                        self.column = self.column.min(other.column);
                        self.inactive = self.inactive.max(other.inactive);
                    }
                    Ordering::Greater => {
                        self.active = self.active.max(other.active);
                        self.column = self.column.max(other.column);
                        self.inactive = self.inactive.min(other.inactive);
                    }
                    Ordering::Equal => unreachable!(),
                }
                true
            }
            _ => false,
        }
    }

    pub fn apply_diff(&mut self, diff: &Diff, local: bool) {
        if local {
            self.active.apply_diff(diff, true);
            self.clear();
        } else {
            self.active.apply_diff(diff, false);
            self.inactive.apply_diff(diff, false);
        }
    }
}

#[derive(Clone, Debug)]
pub struct Spans<'a> {
    pos: Pos,
    iter: Peekable<Iter<'a>>,
}

impl<'a> Iterator for Spans<'a> {
    type Item = Span;

    fn next(&mut self) -> Option<Self::Item> {
        let range = self.iter.peek().copied()?;
        Some(if self.pos < range.start() {
            let span = Span {
                len: range.start() - self.pos,
                is_sel: false,
            };
            self.pos = range.start();
            span
        } else {
            let span = Span {
                len: range.len(),
                is_sel: true,
            };
            self.pos = range.end();
            self.iter.next().unwrap();
            span
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Span {
    pub len: Len,
    pub is_sel: bool,
}
