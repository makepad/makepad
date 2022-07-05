use {crate::cursor::char, makepad_ucd::GraphemeClusterBreak};

/// A cursor over the graphemes in a text.
///
/// A `Cursor` is like an iterator, except that it can freely seek back-and-forth.
pub struct Cursor<C> {
    cursor: C,
    prev_grapheme_cluster_break: Option<GraphemeClusterBreak>,
    next_grapheme_cluster_break: Option<GraphemeClusterBreak>,
    regional_indicator_count: Option<usize>,
}

impl<C: char::Cursor> Cursor<C> {
    /// Returns `true` if this `Cursor` is at the start of the text.
    pub fn is_at_start(&self) -> bool {
        self.cursor.is_at_start()
    }

    /// Returns `true` if this `Cursor` is at the end of the text.
    pub fn is_at_end(&self) -> bool {
        self.cursor.is_at_end()
    }

    /// Returns `true` if this `Cursor` is at a grapheme boundary.
    pub fn is_at_boundary(&mut self) -> bool {
        use makepad_ucd::{GraphemeClusterBreak::*, Ucd};

        if !self.cursor.is_at_boundary() {
            return false;
        }

        // Break at the start and end of text, unless the text is empty.
        if self.is_at_start() {
            // GB1
            return true;
        }
        if self.is_at_end() {
            // GB2
            return true;
        }
        match (
            self.prev_grapheme_cluster_break(),
            self.next_grapheme_cluster_break(),
        ) {
            // Do not break between a CR and LF. Otherwise, break before and after controls.
            (CR, LF) => false,              // GB3
            (Control | CR | LF, _) => true, // GB4
            (_, Control | CR | LF) => true, // GB5

            // Do not break Hangul syllable sequences.
            (L, L | V | LV | LVT) => false, // GB6
            (LV | V, V | T) => false,       // GB7
            (LVT | T, T) => false,          // GB8

            // Do not break before extending characters or ZWJ.
            (_, Extend | ZWJ) => false, // GB9

            // Do not break before SpacingMarks, or after Prepend characters.
            (_, SpacingMark) => false, // GB9a
            (Prepend, _) => false,     // GB9b

            // Do not break within emoji modifier sequences or emoji zwj sequences.
            (ZWJ, _) if self.cursor.current().extended_pictographic() => {
                // GB11
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

            // Do not break within emoji flag sequences. That is, do not break between regional
            // indicator (RI) symbols if there is an odd number of RI characters before the break
            // point.
            (RegionalIndicator, RegionalIndicator) => {
                // GB12 + GB13
                self.regional_indicator_count() % 2 == 0
            }
            // Otherwise, break everywhere.
            _ => {
                // GB999
                true
            }
        }
    }

    /// Returns this position of this `Cursor`.
    pub fn position(&self) -> usize {
        self.cursor.position()
    }

    /// Moves this `Cursor` to the next grapheme boundary.
    ///
    /// # Panics
    ///
    /// Panics if this `Cursor` is at the end of the text.
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

    /// Moves this `Cursor` to the previous grapheme boundary.
    ///
    /// # Panics
    ///
    /// Panics if this `Cursor` is at the start of the text.
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

    /// Sets the `position` of this `Cursor`.
    ///
    /// # Panics
    ///
    /// Panics if `position` is out of bounds.
    pub fn set_position(&mut self, position: usize) {
        self.cursor.set_position(position);
        self.prev_grapheme_cluster_break = None;
        self.next_grapheme_cluster_break = None;
        self.regional_indicator_count = None;
    }

    pub(crate) fn new(cursor: C) -> Self {
        Self {
            cursor,
            prev_grapheme_cluster_break: None,
            next_grapheme_cluster_break: None,
            regional_indicator_count: None,
        }
    }

    // Returns the value of the `Grapheme_Cluster_Break` property for the previous character.
    fn prev_grapheme_cluster_break(&mut self) -> GraphemeClusterBreak {
        use makepad_ucd::Ucd;

        *self.prev_grapheme_cluster_break.get_or_insert_with(|| {
            self.cursor.move_prev();
            let grapheme_cluster_break = self.cursor.current().grapheme_cluster_break();
            self.cursor.move_next();
            grapheme_cluster_break
        })
    }

    // Returns the value of the `Grapheme_Cluster_Break` property for the next character.
    fn next_grapheme_cluster_break(&mut self) -> GraphemeClusterBreak {
        use makepad_ucd::Ucd;

        *self
            .next_grapheme_cluster_break
            .get_or_insert_with(|| self.cursor.current().grapheme_cluster_break())
    }

    // Returns the number of regional indicator (RI) symbols before the break point.
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
