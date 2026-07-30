use core::iter::Chain;
use i_float::int::number::int::IntNumber;
use i_float::int::number::wide_int::WideIntNumber;
use i_float::int::point::IntPoint;

pub(super) struct UniqueSegment<I: IntNumber> {
    pub(super) a: IntPoint<I>,
    pub(super) b: IntPoint<I>,
}

pub(super) struct UniqueSegmentsIter<It, I>
where
    It: Iterator<Item = IntPoint<I>>,
    I: IntNumber,
{
    iter: Chain<It, core::array::IntoIter<IntPoint<I>, 2>>,
    p0: IntPoint<I>,
    p1: IntPoint<I>,
}

impl<It, I> UniqueSegmentsIter<It, I>
where
    It: Iterator<Item = IntPoint<I>>,
    I: IntNumber,
{
    #[inline]
    pub(super) fn new(iter: It) -> Option<Self> {
        let mut iter = iter;

        let mut p0 = iter.next()?;
        let mut p1 = iter.find(|p| p0.ne(p))?;

        let q0 = p0;

        for p2 in &mut iter {
            if include_point(p0, p1, p2) {
                p0 = p1;
                p1 = p2;
                break;
            }
            p1 = p2;
        }

        let q1 = p0;

        let chain_iter = iter.chain([q0, q1]);

        Some(Self {
            iter: chain_iter,
            p0,
            p1,
        })
    }
}

impl<It, I> Iterator for UniqueSegmentsIter<It, I>
where
    It: Iterator<Item = IntPoint<I>>,
    I: IntNumber,
{
    type Item = UniqueSegment<I>;
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        for p2 in &mut self.iter {
            if !include_point(self.p0, self.p1, p2) {
                self.p1 = p2;
                continue;
            }
            let s = UniqueSegment {
                a: self.p0,
                b: self.p1,
            };

            self.p0 = self.p1;
            self.p1 = p2;

            return Some(s);
        }

        let add_last = self.p1 != self.p0;
        if add_last {
            let s = UniqueSegment {
                a: self.p0,
                b: self.p1,
            };
            self.p1 = self.p0;
            Some(s)
        } else {
            None
        }
    }
}

#[inline]
fn include_point<I: IntNumber>(p0: IntPoint<I>, p1: IntPoint<I>, p2: IntPoint<I>) -> bool {
    let a = p1 - p0;
    let b = p1 - p2;

    if a.cross_product(b) != I::Wide::ZERO {
        // not collinear
        return true;
    }

    // collinear – keep only if we keep going opposite direction
    a.dot_product(b) > I::Wide::ZERO
}
#[cfg(test)]
mod tests {
    use crate::mesh::outline::uniq_iter::{UniqueSegment, UniqueSegmentsIter};
    use alloc::vec::Vec;
    use i_float::int::point::IntPoint;
    use i_shape::int_path;

    #[test]
    fn test_empty() {
        let uniq_iter = UniqueSegmentsIter::new(core::iter::empty::<IntPoint<i32>>());
        assert!(uniq_iter.is_none());
    }

    #[test]
    fn test_single_point() {
        let path = int_path![[0, 0]];
        let uniq_iter = UniqueSegmentsIter::new(path.iter().copied());
        assert!(uniq_iter.is_none());
    }

    #[test]
    fn test_all_points_equal() {
        let path = int_path![[0, 0], [0, 0], [0, 0]];
        let uniq_iter = UniqueSegmentsIter::new(path.iter().copied());
        assert!(uniq_iter.is_none());
    }

    #[test]
    fn test_line_0() {
        let path = int_path![[0, 0], [10, 0]];
        validate_case_all_rotations(&path, 2);
    }

    #[test]
    fn test_line_1() {
        let path = int_path![[0, 0], [5, 0], [10, 0]];
        validate_case_all_rotations(&path, 2);
    }

    #[test]
    fn test_line_2() {
        let path = int_path![[0, 0], [5, 0], [10, 0], [5, 0]];
        validate_case_all_rotations(&path, 2);
    }

    #[test]
    fn test_square_0() {
        let path = int_path![[0, 10], [0, 0], [10, 0], [10, 10]];
        validate_case_all_rotations(&path, 4);
    }

    #[test]
    fn test_square_1() {
        #[rustfmt::skip]
        let path = int_path![[0, 10], [0, 5], [0, 0], [5, 0], [10, 0], [10, 5], [10, 10], [5, 10]];
        validate_case_all_rotations(&path, 4);
    }

    #[test]
    fn test_square_2() {
        #[rustfmt::skip]
        let path = int_path![
            [0, 10], [0, 8], [0, 5], [0, 2],
            [0, 0], [2, 0], [5, 0], [8, 0],
            [10, 0], [10, 2], [10, 5], [10, 8],
            [10, 10], [8, 10], [5, 10], [2, 10]
        ];
        validate_case_all_rotations(&path, 4);
    }

    fn validate_case_all_rotations(path: &[IntPoint<i32>], expected_segments_count: usize) {
        assert!(!path.is_empty(), "path must not be empty");

        for shift in 0..path.len() {
            let uniq_iter =
                UniqueSegmentsIter::new(path[shift..].iter().chain(path[..shift].iter()).copied()).unwrap();

            let segments: Vec<_> = uniq_iter.collect();

            assert_eq!(
                segments.len(),
                expected_segments_count,
                "unexpected segment count for shift {}",
                shift
            );

            validate_segments(&segments);
        }
    }

    fn validate_segments(segments: &[UniqueSegment<i32>]) {
        assert!(!segments.is_empty(), "expected at least one segment");

        for (i, s) in segments.iter().enumerate() {
            assert_ne!(s.a, s.b, "segment {} is degenerate (a == b)", i);
        }

        for (i, w) in segments.windows(2).enumerate() {
            let s0 = &w[0];
            let s1 = &w[1];

            validate_pair(s0, s1, i, i + 1);
        }

        let last_i = segments.len() - 1;
        validate_pair(&segments[last_i], &segments[0], last_i, 0);
    }

    fn validate_pair(s0: &UniqueSegment<i32>, s1: &UniqueSegment<i32>, i: usize, j: usize) {
        assert_eq!(s0.b, s1.a, "segment {} end does not match segment {} start", i, j);

        let v0 = s0.a - s0.b;
        let v1 = s1.a - s1.b;

        let cross = v0.cross_product(v1);
        if cross == 0 {
            let dot = v0.dot_product(v1);
            assert!(
                dot < 0,
                "segments {} and {} are collinear and same direction",
                i,
                j
            );
        }
    }
}
