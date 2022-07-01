use {crate::cursor::char, makepad_ucd::WordBreak};

pub struct Cursor<C> {
    cursor: C,
    prev_word_break: Option<WordBreak>,
    next_word_break: Option<WordBreak>,
    prev_prev_word_break_skip_ignore: Option<Option<WordBreak>>,
    prev_word_break_skip_ignore: Option<Option<WordBreak>>,
    next_next_word_break_skip_ignore: Option<Option<WordBreak>>,
    regional_indicator_count: Option<usize>,
}

impl<C: char::Cursor> Cursor<C> {
    pub(super) fn new(char_cursor: C) -> Self {
        Self {
            cursor: char_cursor,
            prev_word_break: None,
            next_word_break: None,
            prev_prev_word_break_skip_ignore: None,
            prev_word_break_skip_ignore: None,
            next_next_word_break_skip_ignore: None,
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
        use makepad_ucd::{Ucd, WordBreak::*};

        if self.is_at_start() {
            return true;
        }
        if self.is_at_end() {
            return true;
        }
        if !self.cursor.is_at_boundary() {
            return false;
        }
        let x = match (self.prev_word_break(), self.next_word_break()) {
            (CR, LF) => false,
            (Newline | CR | LF, _) => true,
            (_, Newline | CR | LF) => true,
            (ZWJ, _) if self.cursor.current().extended_pictographic() => false,
            (WSegSpace, WSegSpace) => false,
            (_, Format | Extend | ZWJ) => false,
            _ => match (
                self.prev_prev_word_break_skip_ignore(),
                self.prev_word_break_skip_ignore(),
                self.next_word_break(),
                self.next_next_word_break_skip_ignore(),
            ) {
                (_, Some(ALetter | HebrewLetter), ALetter | HebrewLetter, _) => false,
                (
                    _,
                    Some(ALetter | HebrewLetter),
                    MidLetter | MidNumLet | SingleQuote,
                    Some(ALetter | HebrewLetter),
                ) => false,
                (
                    Some(ALetter | HebrewLetter),
                    Some(MidLetter | MidNumLet | SingleQuote),
                    ALetter | HebrewLetter,
                    _,
                ) => false,
                (_, Some(HebrewLetter), SingleQuote, _) => false,
                (_, Some(HebrewLetter), DoubleQuote, Some(HebrewLetter)) => false,
                (Some(HebrewLetter), Some(DoubleQuote), HebrewLetter, _) => false,
                (_, Some(Numeric), Numeric, _) => false,
                (_, Some(ALetter | HebrewLetter), Numeric, _) => false,
                (_, Some(Numeric), ALetter | HebrewLetter, _) => false,
                (Some(Numeric), Some(MidNum | MidNumLet | SingleQuote), Numeric, _) => false,
                (_, Some(Numeric), MidNum | MidNumLet | SingleQuote, Some(Numeric)) => false,
                (_, Some(Katakana), Katakana, _) => false,
                (
                    _,
                    Some(ALetter | HebrewLetter | Numeric | Katakana | ExtendNumLet),
                    ExtendNumLet,
                    _,
                ) => false,
                (_, Some(ExtendNumLet), ALetter | HebrewLetter | Numeric | Katakana, _) => false,
                (_, Some(RegionalIndicator), RegionalIndicator, _) => {
                    self.regional_indicator_count() % 2 == 0
                }
                _ => true,
            }
        };
        println!("{:?}", x);
        x
    }

    pub fn position(&self) -> usize {
        self.cursor.position()
    }

    pub fn move_next(&mut self) {
        loop {
            self.cursor.move_next();
            self.prev_word_break = None;
            self.next_word_break = None;
            self.prev_prev_word_break_skip_ignore = None;
            self.prev_word_break_skip_ignore = None;
            self.next_next_word_break_skip_ignore = None;
            self.next_next_word_break_skip_ignore = None;
            self.regional_indicator_count = None;
            if self.is_at_boundary() {
                break;
            }
        }
    }

    pub fn move_prev(&mut self) {
        loop {
            self.cursor.move_prev();
            self.prev_word_break = None;
            self.next_word_break = None;
            self.prev_prev_word_break_skip_ignore = None;
            self.prev_word_break_skip_ignore = None;
            self.next_next_word_break_skip_ignore = None;
            self.regional_indicator_count = None;
            if self.is_at_boundary() {
                break;
            }
        }
    }

    pub fn set_position(&mut self, position: usize) {
        self.cursor.set_position(position);
        self.prev_word_break = None;
        self.next_word_break = None;
        self.prev_prev_word_break_skip_ignore = None;
        self.prev_word_break_skip_ignore = None;
        self.next_next_word_break_skip_ignore = None;
        self.regional_indicator_count = None;
    }

    fn prev_word_break(&mut self) -> WordBreak {
        use makepad_ucd::Ucd;

        *self.prev_word_break.get_or_insert_with(|| {
            self.cursor.move_prev();
            let word_break = self.cursor.current().word_break();
            self.cursor.move_next();
            word_break
        })
    }

    fn next_word_break(&mut self) -> WordBreak {
        use makepad_ucd::Ucd;

        *self.next_word_break.get_or_insert_with(|| {
            self.cursor.current().word_break()
        })
    }

    fn prev_prev_word_break_skip_ignore(&mut self) -> Option<WordBreak> {
        use makepad_ucd::Ucd;
        
        *self.prev_prev_word_break_skip_ignore.get_or_insert_with(|| {
            let position = self.cursor.position();
            if !self.cursor.move_prev_skip_ignore() {
                self.cursor.set_position(position);
                return None
            }
            if !self.cursor.move_prev_skip_ignore() {
                self.cursor.set_position(position);
                return None
            }
            let word_break = self.cursor.current().word_break();
            self.cursor.set_position(position);
            Some(word_break)
        })
    }

    fn prev_word_break_skip_ignore(&mut self) -> Option<WordBreak> {
        use makepad_ucd::Ucd;
        
        *self.prev_word_break_skip_ignore.get_or_insert_with(|| {
            let position = self.cursor.position();
            if !self.cursor.move_prev_skip_ignore() {
                self.cursor.set_position(position);
                return None
            }
            let word_break = self.cursor.current().word_break();
            self.cursor.set_position(position);
            Some(word_break)
        })
    }

    fn next_next_word_break_skip_ignore(&mut self) -> Option<WordBreak> {
        use makepad_ucd::Ucd;

        *self.next_next_word_break_skip_ignore.get_or_insert_with(|| {
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

    fn regional_indicator_count(&mut self) -> usize {
        use makepad_ucd::{GraphemeClusterBreak::RegionalIndicator, Ucd};

        *self.regional_indicator_count.get_or_insert_with(|| {
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
            return false
        }
        loop {
            self.move_next();
            if self.is_at_end() {
                return false;
            }
            match self.current().word_break() {
                Extend | Format | ZWJ => {}
                _ => break,
            }
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
