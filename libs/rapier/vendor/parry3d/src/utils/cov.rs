use crate::math::{Matrix, Real, Vector, VectorExt};

/// Computes the covariance matrix of a set of points.
pub fn cov(pts: &[Vector]) -> Matrix {
    center_cov(pts).1
}

/// Computes the center and the covariance matrix of a set of points.
pub fn center_cov(pts: &[Vector]) -> (Vector, Matrix) {
    let center = crate::utils::center(pts);
    let mut cov = Matrix::ZERO;
    let normalizer: Real = 1.0 / (pts.len() as Real);

    for p in pts.iter() {
        let cp = *p - center;
        let cp_scaled = cp * normalizer;
        // Compute outer product: cp * cp_scaled^T
        cov += cp.kronecker(cp_scaled);
    }

    (center, cov)
}
