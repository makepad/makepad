use crate::math::{Pose, Vector};

/// Extra operations with isometries.
pub trait PoseOps {
    /// Transform a vector by the absolute value of the homogeneous matrix
    /// equivalent to `self`.
    // TODO: move this to the rotation
    fn absolute_transform_vector(&self, v: Vector) -> Vector;
}

impl PoseOps for Pose {
    #[inline]
    fn absolute_transform_vector(&self, v: Vector) -> Vector {
        #[cfg(feature = "dim3")]
        {
            let rot_matrix = crate::math::Matrix::from_quat(self.rotation);
            rot_matrix.abs() * v
        }
        #[cfg(feature = "dim2")]
        {
            let rot_matrix = self.rotation.to_mat();
            rot_matrix.abs() * v
        }
    }
}

/// Various operations usable with `Option<Pose>` and `Option<&Pose>`
/// where `None` is assumed to be equivalent to the identity.
pub trait PoseOpt {
    /// Computes `self.inverse() * rhs`.
    fn inv_mul(self, rhs: &Pose) -> Pose;
    /// Computes `rhs * self`.
    fn prepend_to(self, rhs: &Pose) -> Pose;
    /// Computes `self * p`.
    fn transform_point(self, p: Vector) -> Vector;
    /// Computes `self.inverse() * p`.
    fn inverse_transform_point(self, p: Vector) -> Vector;
}

impl PoseOpt for Option<&Pose> {
    #[inline]
    fn inv_mul(self, rhs: &Pose) -> Pose {
        if let Some(iso) = self {
            iso.inv_mul(rhs)
        } else {
            *rhs
        }
    }

    #[inline]
    fn prepend_to(self, rhs: &Pose) -> Pose {
        if let Some(iso) = self {
            *rhs * *iso
        } else {
            *rhs
        }
    }

    #[inline]
    fn transform_point(self, p: Vector) -> Vector {
        if let Some(iso) = self {
            iso * p
        } else {
            p
        }
    }

    #[inline]
    fn inverse_transform_point(self, p: Vector) -> Vector {
        if let Some(iso) = self {
            iso.inverse_transform_point(p)
        } else {
            p
        }
    }
}

impl PoseOpt for Option<Pose> {
    #[inline]
    fn inv_mul(self, rhs: &Pose) -> Pose {
        if let Some(iso) = self {
            iso.inv_mul(rhs)
        } else {
            *rhs
        }
    }

    #[inline]
    fn prepend_to(self, rhs: &Pose) -> Pose {
        if let Some(iso) = self {
            *rhs * iso
        } else {
            *rhs
        }
    }

    #[inline]
    fn transform_point(self, p: Vector) -> Vector {
        if let Some(iso) = self {
            iso * p
        } else {
            p
        }
    }

    #[inline]
    fn inverse_transform_point(self, p: Vector) -> Vector {
        if let Some(iso) = self {
            iso.inverse_transform_point(p)
        } else {
            p
        }
    }
}
