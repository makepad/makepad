pub trait CharExt {
    fn column_len(self) -> usize;
}

impl CharExt for char {
    fn column_len(self) -> usize {
        1
    }
}
