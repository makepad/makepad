//! ScalarType trait for generic scalar types in Rapier.

#[cfg(feature = "simd-is-enabled")]
use crate::math::SimdReal;
use crate::math::{AngularInertia, Matrix, Pose, Real, Rotation, Vector};
#[cfg(feature = "dim3")]
use crate::utils::SimdSelect;
use crate::utils::{
    AngularInertiaOps, ComponentMul, CrossProduct, CrossProductMatrix, DIM_MINUS_ONE, DotProduct,
    MatrixColumn, OrthonormalBasis, PoseOps, RotationOps,
};
use na::SimdRealField;
use std::fmt::Debug;
use std::ops::{Add, AddAssign, DivAssign, Index, Mul, MulAssign, Neg, Sub, SubAssign};

/// Trait for types that can be used as scalars in the generic code supporting both
/// the scalar and AoSoA SIMD pattern.
///
/// This trait is mostly for internal use only. No other implementations of this trait
/// is expected.
pub trait ScalarType:
    SimdRealField<Element = Real>
    + Copy
    + CrossProduct<Self::Vector, Result = Self::Vector>
    + DotProduct<Self, Result = Self>
{
    /// The pose type (position + rotation) for this scalar.
    type Pose: Copy + PoseOps<Self> + Mul<Self::Vector, Output = Self::Vector>;
    /// The vector type for this scalar (2D).
    #[cfg(feature = "dim2")]
    type Vector: Copy
        + Debug
        + Default
        + Neg<Output = Self::Vector>
        + Add<Self::Vector, Output = Self::Vector>
        + Sub<Self::Vector, Output = Self::Vector>
        + AddAssign<Self::Vector>
        + SubAssign<Self::Vector>
        + MulAssign<Self>
        + DivAssign<Self>
        + Mul<Self, Output = Self::Vector>
        + Index<usize, Output = Self>
        + DotProduct<Self::Vector, Result = Self>
        + OrthonormalBasis<Basis = [Self::Vector; DIM_MINUS_ONE]>
        + CrossProduct<Self::Vector, Result = Self>
        + CrossProductMatrix<CrossMat = Self::Vector, CrossMatTr = Self::Vector>
        + ComponentMul;
    /// The vector type for this scalar (3D).
    #[cfg(feature = "dim3")]
    type Vector: Copy
        + Debug
        + Default
        + Neg<Output = Self::Vector>
        + Add<Self::Vector, Output = Self::Vector>
        + Sub<Self::Vector, Output = Self::Vector>
        + AddAssign<Self::Vector>
        + SubAssign<Self::Vector>
        + MulAssign<Self>
        + DivAssign<Self>
        + Mul<Self, Output = Self::Vector>
        + Index<usize, Output = Self>
        + DotProduct<Self::Vector, Result = Self>
        + OrthonormalBasis<Basis = [Self::Vector; DIM_MINUS_ONE]>
        + CrossProduct<Self::Vector, Result = Self::Vector>
        + CrossProductMatrix<CrossMat = Self::Matrix, CrossMatTr = Self::Matrix>
        + SimdSelect<Self>
        + ComponentMul
        + Into<Self::AngVector>; // In 3D Vec and Ang are technically the same.
    /// The angular vector type for this scalar (2D: scalar, 3D: vector).
    #[cfg(feature = "dim2")]
    type AngVector: Copy
        + Debug
        + Default
        + Add<Self::AngVector, Output = Self::AngVector>
        + Sub<Self::AngVector, Output = Self::AngVector>
        + AddAssign<Self::AngVector>
        + SubAssign<Self::AngVector>
        + MulAssign<Self>
        + DivAssign<Self>
        + Mul<Self, Output = Self::AngVector>
        + DotProduct<Self::AngVector, Result = Self>
        + num::One
        + From<Self>;
    /// The angular vector type for this scalar (3D).
    #[cfg(feature = "dim3")]
    type AngVector: Copy
        + Debug
        + Default
        + Add<Self::AngVector, Output = Self::AngVector>
        + Sub<Self::AngVector, Output = Self::AngVector>
        + AddAssign<Self::AngVector>
        + SubAssign<Self::AngVector>
        + MulAssign<Self>
        + DivAssign<Self>
        + Mul<Self, Output = Self::AngVector>
        + DotProduct<Self::AngVector, Result = Self>;
    /// The matrix type for this scalar.
    type Matrix: Copy
        + Debug
        + MatrixColumn<Column = Self::Vector>
        + MulAssign<Self>
        + Mul<Self::Matrix, Output = Self::Matrix>;
    /// The angular inertia type for this scalar.
    type AngInertia: AngularInertiaOps<Self, AngVector = Self::AngVector>;
    /// The rotation type for this scalar.
    type Rotation: RotationOps<Self>;
}

impl ScalarType for Real {
    type Pose = Pose;
    type Vector = Vector;
    #[cfg(feature = "dim2")]
    type AngVector = Real;
    #[cfg(feature = "dim3")]
    type AngVector = Vector;
    type Matrix = Matrix;
    type AngInertia = AngularInertia;
    type Rotation = Rotation;
}

#[cfg(all(feature = "dim3", feature = "simd-is-enabled"))]
impl ScalarType for SimdReal {
    type Pose = na::Isometry3<SimdReal>;
    type Vector = na::Vector3<SimdReal>;
    type AngVector = na::Vector3<SimdReal>;
    type Matrix = na::Matrix3<SimdReal>;
    type AngInertia = parry::utils::SdpMatrix3<SimdReal>;
    type Rotation = na::UnitQuaternion<SimdReal>;
}

#[cfg(all(feature = "dim2", feature = "simd-is-enabled"))]
impl ScalarType for SimdReal {
    type Pose = na::Isometry2<SimdReal>;
    type Vector = na::Vector2<SimdReal>;
    type AngVector = SimdReal;
    type Matrix = na::Matrix2<SimdReal>;
    type AngInertia = SimdReal;
    type Rotation = na::UnitComplex<SimdReal>;
}
