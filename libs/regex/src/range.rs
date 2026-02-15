/// A range bounded inclusively from below and above.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Range<T> {
    /// The lower bound of the range (inclusive).
    pub start: T,
    /// The upper bound of the range (inclusive).
    pub end: T,
}

impl<T> Range<T> {
    /// Creates a new inclusive range with the given `start` and `end` bounds.
    pub const fn new(start: T, end: T) -> Self {
        Self { start, end }
    }

    /// Returns `true` if `value` is contained in the range.
    pub fn contains(&self, value: &T) -> bool
    where
        T: Ord,
    {
        &self.start <= value && value <= &self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains() {
        let range = Range::new(10, 20);
        assert!(!range.contains(&9));
        assert!(range.contains(&10));
        assert!(range.contains(&15));
        assert!(range.contains(&20));
        assert!(!range.contains(&21));
    }
}
