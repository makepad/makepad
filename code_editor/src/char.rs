pub trait CharExt {
    fn col_count(self, tab_col_count: usize) -> usize;
}

impl CharExt for char {
    fn col_count(self, tab_col_count: usize) -> usize {
        match self {
            '\t' => tab_col_count,
            _ => 1,
        }
    }
}
