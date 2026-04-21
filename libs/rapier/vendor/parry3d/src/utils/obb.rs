use crate::math::{MatExt, Pose, Real, Rotation, Vector, DIM};
use crate::shape::Cuboid;

/// Computes an oriented bounding box for the given set of points.
///
/// The returned OBB is not guaranteed to be the smallest enclosing OBB.
/// Though it should be a pretty good on for most purposes.
pub fn obb(pts: &[Vector]) -> (Pose, Cuboid) {
    let cov = crate::utils::cov(pts);
    let mut eigv = cov.symmetric_eigen().eigenvectors;

    if eigv.determinant() < 0.0 {
        eigv = -eigv;
    }

    let mut mins = Vector::splat(Real::MAX);
    let mut maxs = Vector::splat(-Real::MAX);

    for pt in pts {
        for i in 0..DIM {
            let dot = eigv.col(i).dot(*pt);
            mins[i] = mins[i].min(dot);
            maxs[i] = maxs[i].max(dot);
        }
    }

    #[cfg(feature = "dim2")]
    let rot = {
        // Extract rotation from 2x2 matrix
        // Matrix rows are [cos θ, -sin θ; sin θ, cos θ]
        Rotation::from_matrix_unchecked(eigv)
    };
    #[cfg(feature = "dim3")]
    let rot = Rotation::from_mat3(&eigv);

    // Create the isometry with rotation and the rotated center translation
    let center = (maxs + mins) / 2.0;
    let rotated_center = rot * center;
    (
        Pose::from_parts(rotated_center, rot),
        Cuboid::new((maxs - mins) / 2.0),
    )
}
