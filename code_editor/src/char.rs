pub trait CharExt {
    fn is_opening_delimiter(self) -> bool;
    fn is_closing_delimiter(self) -> bool;
    fn opposite_delimiter(&self) -> Option<char>;
    fn column_count(self, tab_column_count: usize) -> usize;
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

    fn column_count(self, tab_column_count: usize) -> usize {
        match self {
            '\t' => tab_column_count,
            _ => 1,
        }
    }
}
