#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Range<T> {
    pub(crate) start: T,
    pub(crate) end: T,
}

impl<T> Range<T> {
    pub(crate) fn new(start: T, end: T) -> Self {
        Self { start, end }
    }

    pub(crate) fn contains(&self, value: &T) -> bool
    where
        T: Ord,
    {
        &self.start <= value && value <= &self.end
    }
}
