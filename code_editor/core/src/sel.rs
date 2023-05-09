use {
    crate::{Diff, Len, Pos},
    std::{iter::Peekable, slice},
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Sel {
    latest_region: Region,
    earlier_regions: Vec<Region>,
}

impl Sel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.earlier_regions.len() + 1
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            latest_region: Some(&self.latest_region),
            earlier_regions: self.earlier_regions.iter().peekable(),
        }
    }

    pub fn spans(&self) -> Spans<'_> {
        Spans {
            pos: Pos::default(),
            iter: self.iter().peekable(),
        }
    }

    pub fn update_latest_region(&mut self, mut f: impl FnMut(Region) -> Region) {
        self.latest_region = f(self.latest_region);
        self.normalize_latest_region();
    }

    pub fn update_all_regions(&mut self, mut f: impl FnMut(Region) -> Region) {
        for earlier_region in &mut self.earlier_regions {
            *earlier_region = f(*earlier_region);
        }
        self.normalize_earlier_regions();
        self.update_latest_region(f);
    }

    pub fn push_region(&mut self, region: Region) {
        self.earlier_regions.push(self.latest_region);
        self.latest_region = region;
        self.normalize_latest_region();
    }

    pub fn apply_diff(&mut self, diff: &Diff, local: bool) {
        for earlier_region in &mut self.earlier_regions {
            *earlier_region = earlier_region.apply_diff(diff, local);
        }
        self.latest_region = self.latest_region.apply_diff(diff, local);
    }

    fn normalize_latest_region(&mut self) {
        let mut index = match self
            .earlier_regions
            .binary_search_by_key(&self.latest_region.start(), |region| region.start())
        {
            Ok(index) => index,
            Err(index) => index,
        };
        while index > 0 {
            let prev_index = index - 1;
            if let Some(merged_region) = self.latest_region.merge(self.earlier_regions[prev_index])
            {
                self.latest_region = merged_region;
                self.earlier_regions.remove(prev_index);
                index = prev_index;
            } else {
                break;
            }
        }
        while index < self.earlier_regions.len() {
            if let Some(merged_region) = self.latest_region.merge(self.earlier_regions[index]) {
                self.latest_region = merged_region;
                self.earlier_regions.remove(index);
            } else {
                break;
            }
        }
    }

    fn normalize_earlier_regions(&mut self) {
        if self.earlier_regions.is_empty() {
            return;
        }
        self.earlier_regions.sort_by_key(|region| region.start());
        let mut index = 0;
        while index + 1 < self.earlier_regions.len() {
            if let Some(merged_region) =
                self.earlier_regions[index].merge(self.earlier_regions[index + 1])
            {
                self.earlier_regions[index] = merged_region;
                self.earlier_regions.remove(index + 1);
            } else {
                index += 1;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a> {
    latest_region: Option<&'a Region>,
    earlier_regions: Peekable<slice::Iter<'a, Region>>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = Region;

    fn next(&mut self) -> Option<Self::Item> {
        match (self.latest_region, self.earlier_regions.next()) {
            (Some(region_0), Some(region_1)) => {
                if region_0.start() <= region_1.start() {
                    self.latest_region.take()
                } else {
                    self.earlier_regions.next()
                }
            }
            (Some(_), _) => self.latest_region.take(),
            (_, Some(_)) => self.earlier_regions.next(),
            _ => None,
        }
        .copied()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Region {
    pub active_end: Pos,
    pub column: Option<usize>,
    pub inactive_end: Pos,
}

impl Region {
    pub fn is_empty(self) -> bool {
        self.active_end == self.inactive_end
    }

    pub fn len(self) -> Len {
        self.end() - self.start()
    }

    pub fn start(self) -> Pos {
        self.active_end.min(self.inactive_end)
    }

    pub fn end(self) -> Pos {
        self.active_end.max(self.inactive_end)
    }

    pub fn apply_move(self, f: impl FnOnce(Pos, Option<usize>) -> (Pos, Option<usize>)) -> Self {
        let (active_end, column) = f(self.active_end, self.column);
        Self {
            active_end,
            column,
            ..self
        }
    }

    pub fn clear(self) -> Self {
        Self {
            inactive_end: self.active_end,
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
            (true, true) if first.active_end == second.active_end => Some(self),
            (false, true) if second.active_end <= first.end() => Some(self),
            (true, false) if first.active_end == second.start() => Some(other),
            (false, false) if first.end() > second.start() => {
                Some(match self.active_end.cmp(&self.inactive_end) {
                    Ordering::Less => Self {
                        active_end: self.active_end.min(other.active_end),
                        column: self.column.min(other.column),
                        inactive_end: self.inactive_end.max(other.inactive_end),
                    },
                    Ordering::Greater => Self {
                        active_end: self.active_end.max(other.active_end),
                        column: self.column.max(other.column),
                        inactive_end: self.inactive_end.min(other.inactive_end),
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
                active_end: self.active_end.apply_diff(diff, true),
                ..self
            }
            .clear()
        } else {
            match self.active_end.cmp(&self.inactive_end) {
                Ordering::Less => Self {
                    active_end: self.active_end.apply_diff(diff, false),
                    inactive_end: self.inactive_end.apply_diff(diff, true),
                    ..self
                },
                Ordering::Equal => Self {
                    active_end: self.active_end.apply_diff(diff, true),
                    ..self
                }
                .clear(),
                Ordering::Greater => Self {
                    active_end: self.active_end.apply_diff(diff, true),
                    inactive_end: self.inactive_end.apply_diff(diff, false),
                    ..self
                },
            }
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
