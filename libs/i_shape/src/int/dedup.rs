use crate::int::shape::IntContour;
use i_float::int::number::int::IntNumber;

/// Trait for removing redundant points from a contour.
pub trait DedupContour {
    /// Removes consecutive duplicate points and a duplicated closing point
    /// (if the last point is equal to the first).
    ///
    /// Returns `true` if the contour was modified, `false` otherwise.
    fn dedup_contour(&mut self) -> bool;
}

impl<I: IntNumber> DedupContour for IntContour<I> {
    fn dedup_contour(&mut self) -> bool {
        let n = self.len();
        self.dedup();

        if let (Some(&first), Some(&last)) = (self.first(), self.last())
            && last == first
        {
            self.pop();
        }

        self.len() < n
    }
}

#[cfg(test)]
mod tests {
    use crate::int::dedup::DedupContour;
    use crate::int_path;

    #[test]
    fn test_0() {
        let mut contour = int_path![[0, 0], [1, 0],];

        let modified = contour.dedup_contour();

        assert_eq!(contour.len(), 2);
        assert_eq!(modified, false);
    }

    #[test]
    fn test_1() {
        let mut contour = int_path![[0, 0], [1, 0], [0, 0],];

        let modified = contour.dedup_contour();

        assert_eq!(contour.len(), 2);
        assert_eq!(modified, true);
    }

    #[test]
    fn test_2() {
        let mut contour = int_path![[0, 0], [0, 0], [1, 0],];

        let modified = contour.dedup_contour();

        assert_eq!(contour.len(), 2);
        assert_eq!(modified, true);
    }
}
