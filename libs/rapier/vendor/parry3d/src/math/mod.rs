//! Math types and utilities for parry, wrapping glam types.

mod vector_ext;

/// The scalar type used throughout this crate.
#[cfg(feature = "f64")]
pub use f64 as Real;
#[cfg(feature = "f64")]
pub use i64 as Int;

/// The scalar type used throughout this crate.
#[cfg(feature = "f32")]
pub use f32 as Real;
#[cfg(feature = "f32")]
pub use i32 as Int;

// Re-export ComplexField and RealField from simba for math operations
pub use simba::scalar::{ComplexField, RealField};

pub use vector_ext::*;

// Re-export SIMD types from parent module
pub use crate::simd::*;

// Re-export types from glamx based on precision
#[cfg(feature = "f64")]
pub use glamx::{
    DPose2 as Pose2, DPose3 as Pose3, DRot2 as Rot2, DRot3 as Rot3,
    DSymmetricEigen2 as SymmetricEigen2, DSymmetricEigen3 as SymmetricEigen3, MatExt,
};
#[cfg(feature = "f32")]
pub use glamx::{MatExt, Pose2, Pose3, Rot2, Rot3, SymmetricEigen2, SymmetricEigen3};

// Re-export glam types used directly
#[cfg(feature = "f64")]
pub use glamx::{
    DMat2 as Mat2, DMat3 as Mat3, DVec2 as Vec2, DVec3 as Vec3, DVec3 as Vec3A, DVec4 as Vec4,
};
#[cfg(feature = "f32")]
pub use glamx::{Mat2, Mat3, Vec2, Vec3, Vec3A, Vec4};

/// The default tolerance used for geometric operations.
pub const DEFAULT_EPSILON: Real = Real::EPSILON;

/// The dimension of the space.
#[cfg(feature = "dim2")]
pub const DIM: usize = 2;
/// The dimension of the space.
#[cfg(feature = "dim3")]
pub const DIM: usize = 3;

/// The dimension of the space multiplied by two.
pub const TWO_DIM: usize = DIM * 2;

// ==================== DIMENSION-DEPENDENT TYPE ALIASES ====================

// --- 3D Type Aliases ---

#[cfg(feature = "dim3")]
mod dim3_types {
    use super::*;
    #[cfg(feature = "f64")]
    use glamx::{DMat3, DVec3};

    /// The vector type.
    #[cfg(feature = "f32")]
    pub type Vector = Vec3;
    /// The vector type.
    #[cfg(feature = "f64")]
    pub type Vector = DVec3;

    /// The integer vector type.
    #[cfg(feature = "f32")]
    pub type IVector = glamx::IVec3;
    /// The integer vector type.
    #[cfg(feature = "f64")]
    pub type IVector = glamx::I64Vec3;

    /// The angular vector type.
    #[cfg(feature = "f32")]
    pub type AngVector = Vec3;
    /// The angular vector type.
    #[cfg(feature = "f64")]
    pub type AngVector = DVec3;

    /// The matrix type.
    #[cfg(feature = "f32")]
    pub type Matrix = Mat3;
    /// The matrix type.
    #[cfg(feature = "f64")]
    pub type Matrix = DMat3;

    /// The transformation matrix type (pose = rotation + translation).
    pub type Pose = Pose3;

    /// The rotation type.
    pub type Rotation = Rot3;

    /// The orientation type.
    #[cfg(feature = "f32")]
    pub type Orientation = Vec3;
    /// The orientation type.
    #[cfg(feature = "f64")]
    pub type Orientation = DVec3;

    /// A vector with a dimension equal to the maximum number of degrees of freedom of a rigid body.
    #[cfg(feature = "f32")]
    pub type SpatialVector = [f32; 6];
    /// A vector with a dimension equal to the maximum number of degrees of freedom of a rigid body.
    #[cfg(feature = "f64")]
    pub type SpatialVector = [f64; 6];

    /// The angular inertia of a rigid body.
    pub type AngularInertia = crate::utils::SdpMatrix3<Real>;

    /// The principal angular inertia of a rigid body.
    #[cfg(feature = "f32")]
    pub type PrincipalAngularInertia = Vec3;
    /// The principal angular inertia of a rigid body.
    #[cfg(feature = "f64")]
    pub type PrincipalAngularInertia = DVec3;

    /// A matrix that represent the cross product with a given vector.
    #[cfg(feature = "f32")]
    pub type CrossMatrix = Mat3;
    /// A matrix that represent the cross product with a given vector.
    #[cfg(feature = "f64")]
    pub type CrossMatrix = DMat3;

    /// A 3D symmetric-definite-positive matrix.
    pub type SdpMatrix = crate::utils::SdpMatrix3<Real>;

    /// The result of eigendecomposition of a symmetric matrix.
    #[cfg(feature = "f32")]
    pub type SymmetricEigen = SymmetricEigen3;
    /// The result of eigendecomposition of a symmetric matrix.
    #[cfg(feature = "f64")]
    pub type SymmetricEigen = glamx::DSymmetricEigen3;
}

#[cfg(feature = "dim3")]
pub use dim3_types::*;

// --- 2D Type Aliases ---

#[cfg(feature = "dim2")]
mod dim2_types {
    use super::*;
    #[cfg(feature = "f64")]
    use glamx::{DMat2, DVec2, DVec3};

    /// The vector type.
    #[cfg(feature = "f32")]
    pub type Vector = Vec2;
    /// The vector type.
    #[cfg(feature = "f64")]
    pub type Vector = DVec2;

    /// The integer vector type.
    #[cfg(feature = "f32")]
    pub type IVector = glamx::IVec2;
    /// The integer vector type.
    #[cfg(feature = "f64")]
    pub type IVector = glamx::I64Vec2;

    /// The angular vector type (scalar for 2D).
    pub type AngVector = Real;

    /// The matrix type.
    #[cfg(feature = "f32")]
    pub type Matrix = Mat2;
    /// The matrix type.
    #[cfg(feature = "f64")]
    pub type Matrix = DMat2;

    /// The transformation matrix type (pose = rotation + translation).
    pub type Pose = Pose2;

    /// The rotation type.
    pub type Rotation = Rot2;

    /// The orientation type (scalar angle for 2D).
    pub type Orientation = Real;

    /// A vector with a dimension equal to the maximum number of degrees of freedom of a rigid body.
    #[cfg(feature = "f32")]
    pub type SpatialVector = Vec3;
    /// A vector with a dimension equal to the maximum number of degrees of freedom of a rigid body.
    #[cfg(feature = "f64")]
    pub type SpatialVector = DVec3;

    /// The angular inertia of a rigid body (scalar for 2D).
    pub type AngularInertia = Real;

    /// The principal angular inertia of a rigid body (scalar for 2D).
    pub type PrincipalAngularInertia = Real;

    /// A matrix that represent the cross product with a given vector.
    #[cfg(feature = "f32")]
    pub type CrossMatrix = Vec2;
    /// A matrix that represent the cross product with a given vector.
    #[cfg(feature = "f64")]
    pub type CrossMatrix = DVec2;

    /// A 2D symmetric-definite-positive matrix.
    pub type SdpMatrix = crate::utils::SdpMatrix2<Real>;

    /// The result of eigendecomposition of a symmetric matrix.
    #[cfg(feature = "f32")]
    pub type SymmetricEigen = SymmetricEigen2;
    /// The result of eigendecomposition of a symmetric matrix.
    #[cfg(feature = "f64")]
    pub type SymmetricEigen = glamx::DSymmetricEigen2;
}

#[cfg(feature = "dim2")]
pub use dim2_types::*;

// ==================== ORTHONORMAL BASIS COMPUTATION ====================

/// Computes an orthonormal basis for the subspace orthogonal to the given vectors.
/// Calls the callback `f` with each basis vector.
///
/// For 3D: given 1 vector, produces 2 orthonormal vectors perpendicular to it.
#[cfg(feature = "dim3")]
pub fn orthonormal_subspace_basis<F>(vs: &[Vector], mut f: F)
where
    F: FnMut(Vector) -> bool,
{
    if vs.is_empty() {
        return;
    }

    // Normalize the input vector
    let v = vs[0].normalize_or_zero();

    if v == Vector::ZERO {
        return;
    }

    // Find a vector that's not parallel to v
    let orth = if v.x.abs() > v.z.abs() {
        Vector::new(-v.y, v.x, 0.0)
    } else {
        Vector::new(0.0, -v.z, v.y)
    };

    // First orthonormal vector
    let orth1 = orth.normalize();
    if !f(orth1) {
        return;
    }

    // Second orthonormal vector (cross product)
    let orth2 = v.cross(orth1);
    let _ = f(orth2);
}

// ==================== 2D TYPES FOR 3D CONTEXTS ====================
// These are needed for algorithms like spiral intersection that use 2D math in 3D contexts.

/// A 2D vector type for use in any dimension context.
pub type Vector2 = Vec2;

/// A 2x2 matrix type for use in any dimension context.
pub type Matrix2 = Mat2;

/// A 3D vector type for use in any dimension context.
pub type Vector3 = Vec3;

/// A 3x3 matrix type for use in any dimension context.
pub type Matrix3 = Mat3;

/// Converts an integer vector to a floating-point vector.
#[cfg(all(feature = "dim2", feature = "f32"))]
pub fn ivect_to_vect(p: IVector) -> Vector {
    p.as_vec2()
}
/// Converts an integer vector to a floating-point vector.
#[cfg(all(feature = "dim2", feature = "f64"))]
pub fn ivect_to_vect(p: IVector) -> Vector {
    p.as_dvec2()
}
/// Converts an integer vector to a floating-point vector.
#[cfg(all(feature = "dim3", feature = "f32"))]
pub fn ivect_to_vect(p: IVector) -> Vector {
    p.as_vec3()
}
/// Converts an integer vector to a floating-point vector.
#[cfg(all(feature = "dim3", feature = "f64"))]
pub fn ivect_to_vect(p: IVector) -> Vector {
    p.as_dvec3()
}

/// Converts a floating-point vector to an integer vector.
#[cfg(all(feature = "dim2", feature = "f32"))]
pub fn vect_to_ivect(p: Vector) -> IVector {
    p.as_ivec2()
}
/// Converts a floating-point vector to an integer vector.
#[cfg(all(feature = "dim2", feature = "f64"))]
pub fn vect_to_ivect(p: Vector) -> IVector {
    p.as_i64vec2()
}
/// Converts a floating-point vector to an integer vector.
#[cfg(all(feature = "dim3", feature = "f32"))]
pub fn vect_to_ivect(p: Vector) -> IVector {
    p.as_ivec3()
}
/// Converts a floating-point vector to an integer vector.
#[cfg(all(feature = "dim3", feature = "f64"))]
pub fn vect_to_ivect(p: Vector) -> IVector {
    p.as_i64vec3()
}
