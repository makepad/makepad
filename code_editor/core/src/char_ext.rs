pub trait CharExt {
    fn width(self) -> usize;
}

impl CharExt for char {
    fn width(self) -> usize {
        1
    }
}
