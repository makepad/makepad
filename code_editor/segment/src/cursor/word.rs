use {crate::cursor::char, makepad_ucd::WordBreak};

/// A cursor over the words in a text.
///
/// A `Cursor` is like an iterator, except that it can freely seek back-and-forth.
pub struct Cursor<C> {
    cursor: C,
    prev_word_break: Option<WordBreak>,
    next_word_break: Option<WordBreak>,
    prev_prev_word_break_skip_ignore: Option<Option<WordBreak>>,
    prev_word_break_skip_ignore: Option<Option<WordBreak>>,
    next_word_break_skip_ignore: Option<Option<WordBreak>>,
    next_next_word_break_skip_ignore: Option<Option<WordBreak>>,
    regional_indicator_count_skip_ignore: Option<usize>,
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

    /// Returns `true` if the `Cursor` is at a word boundary.
    pub fn is_at_boundary(&mut self) -> bool {
        use makepad_ucd::{Ucd, WordBreak::*};

        if !self.cursor.is_at_boundary() {
            return false;
        }

        // Break at the start and end of text, unless the text is empty.
        if self.is_at_start() {
            // WB1
            return true;
        }
        if self.is_at_end() {
            // WB2
            return true;
        }
        match (self.prev_word_break(), self.next_word_break()) {
            // Do not break within CRLF.
            (CR, LF) => false, // WB3

            // Otherwise break before and after Newlines (including CR and LF)
            (Newline | CR | LF, _) => true, // WB3a
            (_, Newline | CR | LF) => true, // WB3b

            // Do not break within emoji zwj sequences.
            (ZWJ, _) if self.cursor.current().extended_pictographic() => false, // WB3c

            // Keep horizontal whitespace together.
            (WSegSpace, WSegSpace) => false, // WB3d

            // Ignore Format and Extend characters, except after sot, CR, LF, and Newline. This also
            // has the effect of: Any × (Format | Extend | ZWJ).
            (_, Format | Extend | ZWJ) => false, // WB4

            _ => match (
                self.prev_prev_word_break_skip_ignore(),
                self.prev_word_break_skip_ignore(),
                self.next_word_break_skip_ignore(),
                self.next_next_word_break_skip_ignore(),
            ) {
                // Do not break between most letters.
                (_, Some(ALetter | HebrewLetter), Some(ALetter | HebrewLetter), _) => {
                    // WB5
                    false
                }

                // Do not break letters across certain punctuation.
                (
                    _,
                    Some(ALetter | HebrewLetter),
                    Some(MidLetter | MidNumLet | SingleQuote),
                    Some(ALetter | HebrewLetter),
                ) => false, // WB6
                (
                    Some(ALetter | HebrewLetter),
                    Some(MidLetter | MidNumLet | SingleQuote),
                    Some(ALetter | HebrewLetter),
                    _,
                ) => false, // WB7
                (_, Some(HebrewLetter), Some(SingleQuote), _) => false, // WB7a
                (_, Some(HebrewLetter), Some(DoubleQuote), Some(HebrewLetter)) => false, // WB7b
                (Some(HebrewLetter), Some(DoubleQuote), Some(HebrewLetter), _) => false, // WB7c

                // Do not break within sequences of digits, or digits adjacent to letters (“3a”, or
                // “A3”).
                (_, Some(Numeric), Some(Numeric), _) => false, // WB8
                (_, Some(ALetter | HebrewLetter), Some(Numeric), _) => false, // WB9
                (_, Some(Numeric), Some(ALetter | HebrewLetter), _) => false, // WB10

                // Do not break within sequences, such as “3.2” or “3,456.789”.
                (Some(Numeric), Some(MidNum | MidNumLet | SingleQuote), Some(Numeric), _) => false, // WB11
                (_, Some(Numeric), Some(MidNum | MidNumLet | SingleQuote), Some(Numeric)) => false, // WB12

                // Do not break between Katakana.
                (_, Some(Katakana), Some(Katakana), _) => false, // WB13

                // Do not break from extenders.
                (
                    _,
                    Some(ALetter | HebrewLetter | Numeric | Katakana | ExtendNumLet),
                    Some(ExtendNumLet),
                    _,
                ) => false, // WB13A
                (_, Some(ExtendNumLet), Some(ALetter | HebrewLetter | Numeric | Katakana), _) => {
                    // WB13b
                    false
                }

                // Do not break within emoji flag sequences. That is, do not break between regional
                // indicator (RI) symbols if there is an odd number of RI characters before the break
                // point.
                (_, Some(RegionalIndicator), Some(RegionalIndicator), _) => {
                    // WB15 + WB16
                    self.regional_indicator_count_skip_ignore() % 2 == 0
                }

                // Otherwise, break everywhere (including around ideographs).
                _ => true, // WB999
            },
        }
    }

    /// Returns the position of this `Cursor`.
    pub fn position(&self) -> usize {
        self.cursor.position()
    }

    /// Moves this `Cursor` to the next word boundary.
    ///
    /// # Panics
    ///
    /// Panics if the `Cursor` is at the end of the text.
    pub fn move_next(&mut self) {
        use makepad_ucd::WordBreak::{Extend, Format, RegionalIndicator, ZWJ};

        loop {
            self.cursor.move_next();
            self.prev_word_break = self.next_word_break.take();
            match self.prev_word_break() {
                Extend | Format | ZWJ => {}
                _ => {
                    self.prev_prev_word_break_skip_ignore = self.prev_word_break_skip_ignore.take();
                    self.prev_word_break_skip_ignore = self.next_word_break_skip_ignore.take();
                    self.next_word_break_skip_ignore = self.next_next_word_break_skip_ignore.take();
                    self.regional_indicator_count_skip_ignore =
                        if self.prev_word_break_skip_ignore() == Some(RegionalIndicator) {
                            self.regional_indicator_count_skip_ignore
                                .map(|regional_indicator_count| regional_indicator_count + 1)
                        } else {
                            Some(0)
                        };
                }
            }
            if self.is_at_boundary() {
                break;
            }
        }
    }

    /// Moves this `Cursor` to the previous word boundary.
    ///
    /// # Panics
    ///
    /// Panics if this `Cursor` is at the start of the text.
    pub fn move_prev(&mut self) {
        use makepad_ucd::WordBreak::{Extend, Format, ZWJ};

        loop {
            self.cursor.move_prev();
            self.next_word_break = self.prev_word_break.take();
            match self.next_word_break() {
                Extend | Format | ZWJ => {}
                _ => {
                    self.next_next_word_break_skip_ignore = self.next_word_break_skip_ignore.take();
                    self.next_word_break_skip_ignore = self.prev_word_break_skip_ignore.take();
                    self.prev_word_break_skip_ignore = self.prev_prev_word_break_skip_ignore.take();
                    self.regional_indicator_count_skip_ignore =
                        match self.regional_indicator_count_skip_ignore {
                            Some(regional_indicator_count) if regional_indicator_count > 0 => {
                                Some(regional_indicator_count - 1)
                            }
                            Some(_) | None => None,
                        };
                }
            }
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
        self.prev_word_break = None;
        self.next_word_break = None;
        self.prev_prev_word_break_skip_ignore = None;
        self.prev_word_break_skip_ignore = None;
        self.next_next_word_break_skip_ignore = None;
        self.regional_indicator_count_skip_ignore = None;
    }

    pub(super) fn new(char_cursor: C) -> Self {
        Self {
            cursor: char_cursor,
            prev_word_break: None,
            next_word_break: None,
            prev_prev_word_break_skip_ignore: None,
            prev_word_break_skip_ignore: None,
            next_word_break_skip_ignore: None,
            next_next_word_break_skip_ignore: None,
            regional_indicator_count_skip_ignore: None,
        }
    }

    // Returns the value of the `Word_Break` property for the previous character.
    fn prev_word_break(&mut self) -> WordBreak {
        use makepad_ucd::Ucd;

        *self.prev_word_break.get_or_insert_with(|| {
            self.cursor.move_prev();
            let word_break = self.cursor.current().word_break();
            self.cursor.move_next();
            word_break
        })
    }

    // Returns the value of the `Word_Break` property for the next character.
    fn next_word_break(&mut self) -> WordBreak {
        use makepad_ucd::Ucd;

        *self
            .next_word_break
            .get_or_insert_with(|| self.cursor.current().word_break())
    }

    // Returns the value of the `Word_Break` property for the second previous character, after the
    // ignore rule has been applied.
    fn prev_prev_word_break_skip_ignore(&mut self) -> Option<WordBreak> {
        use makepad_ucd::Ucd;

        *self
            .prev_prev_word_break_skip_ignore
            .get_or_insert_with(|| {
                let position = self.cursor.position();
                if !self.cursor.move_prev_skip_ignore() {
                    self.cursor.set_position(position);
                    return None;
                }
                if !self.cursor.move_prev_skip_ignore() {
                    self.cursor.set_position(position);
                    return None;
                }
                let word_break = self.cursor.current().word_break();
                self.cursor.set_position(position);
                Some(word_break)
            })
    }

    // Returns the value of the `Word_Break` property for the previous character, after the ignore
    // rule has been applied.
    fn prev_word_break_skip_ignore(&mut self) -> Option<WordBreak> {
        use makepad_ucd::Ucd;

        *self.prev_word_break_skip_ignore.get_or_insert_with(|| {
            let position = self.cursor.position();
            if !self.cursor.move_prev_skip_ignore() {
                self.cursor.set_position(position);
                return None;
            }
            let word_break = self.cursor.current().word_break();
            self.cursor.set_position(position);
            Some(word_break)
        })
    }

    // Returns he value of the `Word_Break` property for the next character, after the ignore rule
    // has been applied.
    fn next_word_break_skip_ignore(&mut self) -> Option<WordBreak> {
        use makepad_ucd::Ucd;

        *self.next_word_break_skip_ignore.get_or_insert_with(|| {
            let position = self.cursor.position();
            let word_break = self.cursor.current().word_break();
            self.cursor.set_position(position);
            Some(word_break)
        })
    }

    // Returns value of the `Word_Break` property for the second next character, after the ignore
    // rule has been applied.
    fn next_next_word_break_skip_ignore(&mut self) -> Option<WordBreak> {
        use makepad_ucd::Ucd;

        *self
            .next_next_word_break_skip_ignore
            .get_or_insert_with(|| {
                let position = self.cursor.position();
                if !self.cursor.move_next_skip_ignore() {
                    self.cursor.set_position(position);
                    return None;
                }
                let word_break = self.cursor.current().word_break();
                self.cursor.set_position(position);
                Some(word_break)
            })
    }

    // Returns the number of regional indicator (RI) symbols before the break pointm after the
    // ignore rule has been applied.
    fn regional_indicator_count_skip_ignore(&mut self) -> usize {
        use makepad_ucd::{GraphemeClusterBreak::RegionalIndicator, Ucd};

        *self
            .regional_indicator_count_skip_ignore
            .get_or_insert_with(|| {
                let mut count = 0;
                let position = self.cursor.position();
                while !self.cursor.is_at_start() {
                    self.cursor.move_prev_skip_ignore();
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

trait CharCursorExt {
    fn move_next_skip_ignore(&mut self) -> bool;
    fn move_prev_skip_ignore(&mut self) -> bool;
}

impl<C: char::Cursor> CharCursorExt for C {
    fn move_next_skip_ignore(&mut self) -> bool {
        use makepad_ucd::{
            Ucd,
            WordBreak::{Extend, Format, ZWJ},
        };

        if self.is_at_end() {
            return false;
        }
        self.move_next();
        loop {
            if self.is_at_end() {
                return false;
            }
            match self.current().word_break() {
                Extend | Format | ZWJ => {}
                _ => break,
            }
            self.move_next();
        }
        true
    }

    fn move_prev_skip_ignore(&mut self) -> bool {
        use makepad_ucd::{
            Ucd,
            WordBreak::{Extend, Format, ZWJ},
        };

        loop {
            if self.is_at_start() {
                return false;
            }
            self.move_prev();
            match self.current().word_break() {
                Extend | Format | ZWJ => {}
                _ => break,
            }
        }
        true
    }
}
