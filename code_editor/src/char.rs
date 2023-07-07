pub trait CharExt {
    fn is_opening_delimiter(self) -> bool;
    fn is_closing_delimiter(self) -> bool;
    fn column_count(self, tab_column_count: usize) -> usize;
}

impl CharExt for char {
    fn is_opening_delimiter(self) -> bool {
        match self {
            '(' | '<' | '[' | '{' => true,
            _ => false,
        }
    }

    fn is_closing_delimiter(self) -> bool {
        match self {
            ')' | '>' | ']' | '}' => true,
            _ => false,
        }
    }

    fn column_count(self, tab_column_count: usize) -> usize {
        match self {
            '\t' => tab_column_count,
            _ => 1,
        }
    }
}
