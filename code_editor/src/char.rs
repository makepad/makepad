pub trait CharExt {
    fn width(self, tab_width: usize) -> usize;
}

impl CharExt for char {
    fn width(self, tab_width: usize) -> usize {
        match self {
            '\t' => tab_width,
            _ => 1,
        }
    }
}
