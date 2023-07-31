pub trait CharExt {
    fn column_count(self, tab_column_count: usize) -> usize;
}

impl CharExt for char {
    fn column_count(self, tab_column_count: usize) -> usize {
        match self {
            '\t' => tab_column_count,
            _ => 1,
        }
    }
}
