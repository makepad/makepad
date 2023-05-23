pub trait CharExt {
    fn column_width(self) -> usize;
}

impl CharExt for char {
    fn column_width(self) -> usize {
        1
    }
}
