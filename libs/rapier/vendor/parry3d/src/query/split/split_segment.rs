use crate::math::{Real, Vector, VectorExt};
use crate::query::SplitResult;
use crate::shape::Segment;

impl Segment {
    /// Splits this segment along the given canonical axis.
    ///
    /// This will split the segment by a plane with a normal with it’s `axis`-th component set to 1.
    /// The splitting plane is shifted wrt. the origin by the `bias` (i.e. it passes through the point
    /// equal to `normal * bias`).
    ///
    /// # Result
    /// Returns the result of the split. The first shape returned is the piece lying on the negative
    /// half-space delimited by the splitting plane. The second shape returned is the piece lying on the
    /// positive half-space delimited by the splitting plane.
    pub fn canonical_split(&self, axis: usize, bias: Real, epsilon: Real) -> SplitResult<Self> {
        // TODO: optimize this.
        self.local_split(Vector::ith(axis, 1.0), bias, epsilon)
    }

    /// Splits this segment by a plane identified by its normal `local_axis` and
    /// the `bias` (i.e. the plane passes through the point equal to `normal * bias`).
    pub fn local_split(&self, local_axis: Vector, bias: Real, epsilon: Real) -> SplitResult<Self> {
        self.local_split_and_get_intersection(local_axis, bias, epsilon)
            .0
    }

    /// Split a segment with a plane.
    ///
    /// This returns the result of the splitting operation, as well as
    /// the intersection point (and barycentric coordinate of this point)
    /// with the plane. The intersection point is `None` if the plane is
    /// parallel or near-parallel to the segment.
    pub fn local_split_and_get_intersection(
        &self,
        local_axis: Vector,
        bias: Real,
        epsilon: Real,
    ) -> (SplitResult<Self>, Option<(Vector, Real)>) {
        let dir = self.b - self.a;
        let a = bias - local_axis.dot(self.a);
        let b = local_axis.dot(dir);
        let bcoord = a / b;
        let dir_norm = dir.length();

        if relative_eq!(b, 0.0)
            || bcoord * dir_norm <= epsilon
            || bcoord * dir_norm >= dir_norm - epsilon
        {
            if a >= 0.0 {
                (SplitResult::Negative, None)
            } else {
                (SplitResult::Positive, None)
            }
        } else {
            let intersection = self.a + dir * bcoord;
            let s1 = Segment::new(self.a, intersection);
            let s2 = Segment::new(intersection, self.b);
            if a >= 0.0 {
                (SplitResult::Pair(s1, s2), Some((intersection, bcoord)))
            } else {
                (SplitResult::Pair(s2, s1), Some((intersection, bcoord)))
            }
        }
    }
}
