use {
    crate::{Cursor, Diff, Len, Pos},
    std::{iter::Peekable, slice},
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CursorSet {
    latest: Cursor,
    earlier: Vec<Cursor>,
}

impl CursorSet {
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

    pub fn update_latest(&mut self, mut f: impl FnMut(Cursor) -> Cursor) {
        self.latest = f(self.latest);
        self.normalize_latest();
    }

    pub fn update_all(&mut self, mut f: impl FnMut(Cursor) -> Cursor) {
        for cursor in &mut self.earlier {
            *cursor = f(*cursor);
        }
        self.normalize_earlier();
        self.update_latest(f);
    }

    pub fn push_cursor(&mut self, cursor: Cursor) {
        self.earlier.push(self.latest);
        self.latest = cursor;
        self.normalize_latest();
    }

    pub fn apply_diff(&mut self, diff: &Diff, local: bool) {
        for cursor in &mut self.earlier {
            *cursor = cursor.apply_diff(diff, local);
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
            if let Some(cursor) = self.latest.merge(self.earlier[prev_index]) {
                self.latest = cursor;
                self.earlier.remove(prev_index);
                index = prev_index;
            } else {
                break;
            }
        }
        while index < self.earlier.len() {
            if let Some(cursor) = self.latest.merge(self.earlier[index]) {
                self.latest = cursor;
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
            if let Some(cursor) = self.earlier[index].merge(self.earlier[index + 1]) {
                self.earlier[index] = cursor;
                self.earlier.remove(index + 1);
            } else {
                index += 1;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a> {
    latest: Option<&'a Cursor>,
    earlier: Peekable<slice::Iter<'a, Cursor>>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = Cursor;

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
