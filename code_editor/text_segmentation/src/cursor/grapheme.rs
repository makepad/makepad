use {crate::cursor::char, makepad_ucd::GraphemeClusterBreak};

pub struct Cursor<C> {
    cursor: C,
    prev_grapheme_cluster_break: Option<GraphemeClusterBreak>,
    next_grapheme_cluster_break: Option<GraphemeClusterBreak>,
    regional_indicator_count: Option<usize>,
}

impl<C: char::Cursor> Cursor<C> {
    pub(crate) fn new(cursor: C) -> Self {
        Self {
            cursor,
            prev_grapheme_cluster_break: None,
            next_grapheme_cluster_break: None,
            regional_indicator_count: None,
        }
    }

    pub fn is_at_start(&self) -> bool {
        self.cursor.is_at_start()
    }

    pub fn is_at_end(&self) -> bool {
        self.cursor.is_at_end()
    }

    pub fn is_at_boundary(&mut self) -> bool {
        use makepad_ucd::{GraphemeClusterBreak::*, Ucd};

        if self.is_at_start() {
            return true;
        }
        if self.is_at_end() {
            return true;
        }
        if !self.cursor.is_at_boundary() {
            return false;
        }
        match (
            self.prev_grapheme_cluster_break(),
            self.next_grapheme_cluster_break(),
        ) {
            (CR, LF) => false,
            (Control | CR | LF, _) => true,
            (_, Control | CR | LF) => true,
            (L, L | V | LV | LVT) => false,
            (LV | V, V | T) => false,
            (LVT | T, T) => false,
            (_, Extend | ZWJ) => false,
            (_, SpacingMark) => false,
            (Prepend, _) => false,
            (ZWJ, _) if self.cursor.current().extended_pictographic() => {
                let position = self.cursor.position();
                self.cursor.move_prev();
                let mut is_at_boundary = true;
                while !self.is_at_start() {
                    self.cursor.move_prev();
                    let current = self.cursor.current();
                    if current.extended_pictographic() {
                        is_at_boundary = false;
                        break;
                    }
                    if current.grapheme_cluster_break() != Extend {
                        break;
                    }
                }
                self.cursor.set_position(position);
                is_at_boundary
            }
            (RegionalIndicator, RegionalIndicator) => self.regional_indicator_count() % 2 == 0,
            _ => true,
        }
    }

    pub fn position(&self) -> usize {
        self.cursor.position()
    }

    pub fn move_next(&mut self) {
        use makepad_ucd::GraphemeClusterBreak::RegionalIndicator;

        loop {
            self.cursor.move_next();
            self.prev_grapheme_cluster_break = self.next_grapheme_cluster_break.take();
            self.regional_indicator_count =
                if self.prev_grapheme_cluster_break() == RegionalIndicator {
                    self.regional_indicator_count
                        .map(|regional_indicator_count| regional_indicator_count + 1)
                } else {
                    Some(0)
                };
            if self.is_at_boundary() {
                break;
            }
        }
    }

    pub fn move_prev(&mut self) {
        loop {
            self.cursor.move_prev();
            self.next_grapheme_cluster_break = self.prev_grapheme_cluster_break.take();
            self.regional_indicator_count = match self.regional_indicator_count {
                Some(regional_indicator_count) if regional_indicator_count > 0 => {
                    Some(regional_indicator_count - 1)
                }
                Some(_) | None => None,
            };
            if self.is_at_boundary() {
                break;
            }
        }
    }

    pub fn set_position(&mut self, position: usize) {
        self.cursor.set_position(position);
        self.prev_grapheme_cluster_break = None;
        self.next_grapheme_cluster_break = None;
        self.regional_indicator_count = None;
    }

    fn prev_grapheme_cluster_break(&mut self) -> GraphemeClusterBreak {
        use makepad_ucd::Ucd;

        *self.prev_grapheme_cluster_break.get_or_insert_with(|| {
            self.cursor.move_prev();
            let grapheme_cluster_break = self.cursor.current().grapheme_cluster_break();
            self.cursor.move_next();
            grapheme_cluster_break
        })
    }

    fn next_grapheme_cluster_break(&mut self) -> GraphemeClusterBreak {
        use makepad_ucd::Ucd;

        *self
            .next_grapheme_cluster_break
            .get_or_insert_with(|| self.cursor.current().grapheme_cluster_break())
    }

    fn regional_indicator_count(&mut self) -> usize {
        use makepad_ucd::{GraphemeClusterBreak::RegionalIndicator, Ucd};

        *self.regional_indicator_count.get_or_insert_with(|| {
            let mut count = 0;
            let position = self.cursor.position();
            while !self.cursor.is_at_start() {
                self.cursor.move_prev();
                if self.cursor.current().grapheme_cluster_break() != RegionalIndicator {
                    break;
                }
                count += 1;
            }
            self.cursor.set_position(position);
            count
        })
    }
}
