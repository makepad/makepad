pub trait Info: Copy {
    fn empty() -> Self;

    fn combine(self, other: Self) -> Self;

    fn len(&self) -> usize;
}
