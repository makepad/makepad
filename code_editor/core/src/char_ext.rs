pub trait CharExt {
    fn col_count(self) -> usize;
}

impl CharExt for char {
    fn col_count(self) -> usize {
        1
    }
}
