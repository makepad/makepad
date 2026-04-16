pub trait CharExt {
    fn is_opening_delimiter(self) -> bool;
    fn is_closing_delimiter(self) -> bool;
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

    fn column_count(self) -> usize {
        // Unicode East Asian Width: characters in East Asian Wide or Fullwidth
        // categories occupy two display columns in a monospace grid. Without
        // this, CJK text in the code editor overlaps because each glyph draws
        // at 2× advance but the layouter only reserves 1 column.
        match self as u32 {
            // CJK Symbols and Punctuation, Hiragana, Katakana
            0x3000..=0x30FF
            // CJK Unified Ideographs Extension A
            | 0x3400..=0x4DBF
            // CJK Unified Ideographs (main block)
            | 0x4E00..=0x9FFF
            // Hangul Syllables
            | 0xAC00..=0xD7AF
            // CJK Compatibility Ideographs
            | 0xF900..=0xFAFF
            // Fullwidth forms + Halfwidth/Fullwidth punctuation
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            // CJK Unified Ideographs Extensions B..F
            | 0x20000..=0x2FFFF
            // Emoticons, misc symbols & pictographs, transport, supplemental symbols
            | 0x1F300..=0x1F9FF => 2,
            _ => 1,
        }
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
