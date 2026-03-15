use crate::bounding_volume::Aabb;
use crate::math::Vector;
use alloc::vec::Vec;

impl Aabb {
    /// Computes the intersections between this Aabb and the given polygon.
    ///
    /// The results is written into `points` directly. The input points are
    /// assumed to form a convex polygon where all points lie on the same plane.
    /// In order to avoid internal allocations, uses `self.clip_polygon_with_workspace`
    /// instead.
    #[inline]
    pub fn clip_polygon(&self, points: &mut Vec<Vector>) {
        let mut workspace = Vec::new();
        self.clip_polygon_with_workspace(points, &mut workspace)
    }

    /// Computes the intersections between this Aabb and the given polygon.
    ///
    /// The results is written into `points` directly. The input points are
    /// assumed to form a convex polygon where all points lie on the same plane.
    #[inline]
    pub fn clip_polygon_with_workspace(
        &self,
        points: &mut Vec<Vector>,
        workspace: &mut Vec<Vector>,
    ) {
        super::clip_halfspace_polygon(self.mins, -Vector::X, points, workspace);
        super::clip_halfspace_polygon(self.maxs, Vector::X, workspace, points);

        super::clip_halfspace_polygon(self.mins, -Vector::Y, points, workspace);
        super::clip_halfspace_polygon(self.maxs, Vector::Y, workspace, points);

        #[cfg(feature = "dim3")]
        {
            super::clip_halfspace_polygon(self.mins, -Vector::Z, points, workspace);
            super::clip_halfspace_polygon(self.maxs, Vector::Z, workspace, points);
        }
    }
}
