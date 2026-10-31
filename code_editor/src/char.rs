pub trait CharExt {
    fn is_opening_delimiter(self) -> bool;
    fn is_closing_delimiter(self) -> bool;
    fn column_count_at(self, column: usize, tab_column_count: usize) -> usize;
    fn column_count(self) -> usize;
    fn opposite_delimiter(&self) -> Option<char>;
}

impl CharExt for char {
    fn is_opening_delimiter(self) -> bool {
        match self {
            '(' | '[' | '{' => true,
            _ => false,
        }
    }

    fn is_closing_delimiter(self) -> bool {
        match self {
            ')' | ']' | '}' => true,
            _ => false,
        }
    }

    fn column_count_at(self, column: usize, tab_column_count: usize) -> usize {
        if self == '\t' {
            let tab_column_count = tab_column_count.max(1);
            tab_column_count - (column % tab_column_count)
        } else {
            self.column_count()
        }
    }

    fn column_count(self) -> usize {
        // The terminal's cell width: East Asian Wide/Fullwidth and emoji
        // presentation take two columns, so a CJK glyph (drawn at twice the
        // Latin advance) gets two grid columns for the cursor, selection and
        // hit testing. The editor steps through text a char at a time, so a
        // zero-width char (a combining mark, a joiner) keeps one column of
        // its own: visible, and a place the cursor can stand.
        (makepad_cell_width::char_width(self as u32) as usize).max(1)
    }

    fn opposite_delimiter(&self) -> Option<char> {
        Some(match self {
            '(' => ')',
            ')' => '(',
            '[' => ']',
            ']' => '[',
            '{' => '}',
            '}' => '{',
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CharExt;
    use crate::str::StrExt;

    /// Wide characters take two grid columns, the terminal's widths: the
    /// cursor after `"大型"` stands at column 4, and selection and hit
    /// testing walk the same columns.
    #[test]
    fn wide_characters_take_two_columns() {
        for (ch, columns) in [
            ('a', 1),
            ('大', 2),
            ('，', 2),
            ('ア', 2),
            ('한', 2),
            ('ㄅ', 2),
            ('🚀', 2),
            ('✨', 2),
            ('✅', 2),
            ('→', 1),
            ('\u{301}', 1),
            ('\u{200d}', 1),
        ] {
            assert_eq!(ch.column_count(), columns, "{ch:?}");
        }
        let line = "zh: \"大型代码库\"";
        assert_eq!(line.column_count(), 6 + 5 * 2);
        assert_eq!("\t大".column_count_at(0, 4), 4 + 2);
        assert_eq!("大\t".column_count_at(0, 4), 2 + 2);
    }
}
