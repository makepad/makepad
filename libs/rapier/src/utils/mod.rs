//! Miscellaneous utilities.

mod angular_inertia_ops;
mod component_mul;
mod copysign;
mod cross_product;
mod cross_product_matrix;
mod dot_product;
mod fp_flags;
mod index_mut2;
mod matrix_column;
mod orthonormal_basis;
mod pos_ops;
mod rotation_ops;
mod scalar_type;
mod simd_real_copy;
mod simd_select;

pub use component_mul::ComponentMul;
pub use copysign::CopySign;
pub use index_mut2::IndexMut2;
pub use matrix_column::MatrixColumn;
pub use orthonormal_basis::OrthonormalBasis;
pub use pos_ops::PoseOps;
pub use rotation_ops::RotationOps;
pub use scalar_type::ScalarType;
pub use simd_real_copy::SimdRealCopy;
pub use simd_select::SimdSelect;

pub use angular_inertia_ops::AngularInertiaOps;
pub use cross_product::CrossProduct;
pub use cross_product_matrix::CrossProductMatrix;
pub use dot_product::{DotProduct, SimdLength};
pub(crate) use fp_flags::{DisableFloatingPointExceptionsFlags, FlushToZeroDenormalsAreZeroFlags};

#[cfg(feature = "simd-is-enabled")]
use crate::math::SIMD_WIDTH;
use crate::math::{Real, SimdVector, Vector};
#[cfg(feature = "dim2")]
use na::Matrix2;
#[cfg(feature = "dim3")]
use na::Matrix3;

/// Dimension minus one (1 for 2D, 2 for 3D).
#[cfg(feature = "dim2")]
pub const DIM_MINUS_ONE: usize = 1;
/// Dimension minus one (1 for 2D, 2 for 3D).
#[cfg(feature = "dim3")]
pub const DIM_MINUS_ONE: usize = 2;

/// Try to normalize a vector and return both the normalized vector and the original length.
///
/// Returns `None` if the vector's length is below the threshold.
/// This is the glam equivalent of nalgebra's `Unit::try_new_and_get`.
pub fn try_normalize_and_get_length(v: Vector, threshold: Real) -> Option<(Vector, Real)> {
    let len = v.length();
    if len > threshold {
        Some((v / len, len))
    } else {
        None
    }
}

/// Convert glam Vector to nalgebra `SimdVector<Real>`
#[inline]
pub fn vect_to_na(v: Vector) -> SimdVector<Real> {
    #[cfg(feature = "dim2")]
    {
        na::Vector2::new(v.x, v.y)
    }

    #[cfg(feature = "dim3")]
    {
        na::Vector3::new(v.x, v.y, v.z)
    }
}

use crate::math::Matrix;

/// Convert glam Matrix to nalgebra `Matrix2<Real>` (2D matrix)
#[cfg(feature = "dim2")]
#[inline]
pub fn mat_to_na(m: Matrix) -> Matrix2<Real> {
    m.into()
}

/// Convert glam Matrix to nalgebra `Matrix3<Real>` (3D matrix)
#[cfg(feature = "dim3")]
#[inline]
pub fn mat_to_na(m: Matrix) -> Matrix3<Real> {
    Matrix3::new(
        m.x_axis.x, m.y_axis.x, m.z_axis.x, m.x_axis.y, m.y_axis.y, m.z_axis.y, m.x_axis.z,
        m.y_axis.z, m.z_axis.z,
    )
}

const INV_EPSILON: Real = 1.0e-20;

pub(crate) fn inv(val: Real) -> Real {
    if (-INV_EPSILON..=INV_EPSILON).contains(&val) {
        0.0
    } else {
        1.0 / val
    }
}

pub(crate) fn simd_inv<N: SimdRealCopy>(val: N) -> N {
    let eps = N::splat(INV_EPSILON);
    N::zero().select(val.simd_gt(-eps) & val.simd_lt(eps), N::one() / val)
}

pub(crate) fn select_other<T: PartialEq>(pair: (T, T), elt: T) -> T {
    if pair.0 == elt { pair.1 } else { pair.0 }
}

/// Calculate the difference with smallest absolute value between the two given values.
pub fn smallest_abs_diff_between_sin_angles<N: SimdRealCopy>(a: N, b: N) -> N {
    // Select the smallest path among the two angles to reach the target.
    let s_err = a - b;
    let sgn = s_err.simd_signum();
    let s_err_complement = s_err - sgn * N::splat(2.0);
    let s_err_is_smallest = s_err.simd_abs().simd_lt(s_err_complement.simd_abs());
    s_err.select(s_err_is_smallest, s_err_complement)
}

/// Calculate the difference with smallest absolute value between the two given angles.
pub fn smallest_abs_diff_between_angles<N: SimdRealCopy>(a: N, b: N) -> N {
    // Select the smallest path among the two angles to reach the target.
    let s_err = a - b;
    let sgn = s_err.simd_signum();
    let s_err_complement = s_err - sgn * N::simd_two_pi();
    let s_err_is_smallest = s_err.simd_abs().simd_lt(s_err_complement.simd_abs());
    s_err.select(s_err_is_smallest, s_err_complement)
}

#[cfg(feature = "simd-nightly")]
#[inline(always)]
pub(crate) fn transmute_to_wide(val: [std::simd::f32x4; SIMD_WIDTH]) -> [wide::f32x4; SIMD_WIDTH] {
    unsafe { std::mem::transmute(val) }
}

#[cfg(feature = "simd-stable")]
#[inline(always)]
pub(crate) fn transmute_to_wide(val: [wide::f32x4; SIMD_WIDTH]) -> [wide::f32x4; SIMD_WIDTH] {
    val
}

/// Helpers around serialization.
#[cfg(feature = "serde-serialize")]
pub mod serde {
    use serde::{Deserialize, Serialize};
    use std::iter::FromIterator;

    /// Serializes to a `Vec<(K, V)>`.
    ///
    /// Useful for [`std::collections::HashMap`] with a non-string key,
    /// which is unsupported by [`serde_json`](https://docs.rs/serde_json/).
    pub fn serialize_to_vec_tuple<
        'a,
        S: serde::Serializer,
        T: IntoIterator<Item = (&'a K, &'a V)>,
        K: Serialize + 'a,
        V: Serialize + 'a,
    >(
        target: T,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        let container: Vec<_> = target.into_iter().collect();
        serde::Serialize::serialize(&container, s)
    }

    /// Deserializes from a `Vec<(K, V)>`.
    ///
    /// Useful for [`std::collections::HashMap`] with a non-string key,
    /// which is unsupported by [`serde_json`](https://docs.rs/serde_json/).
    pub fn deserialize_from_vec_tuple<
        'de,
        D: serde::Deserializer<'de>,
        T: FromIterator<(K, V)>,
        K: Deserialize<'de>,
        V: Deserialize<'de>,
    >(
        d: D,
    ) -> Result<T, D::Error> {
        let hashmap_as_vec: Vec<(K, V)> = Deserialize::deserialize(d)?;
        Ok(T::from_iter(hashmap_as_vec))
    }

    #[cfg(test)]
    mod test {
        use std::collections::HashMap;

        /// This test uses serde_json because json doesn't support non string
        /// keys in hashmaps, which requires a custom serialization.
        #[test]
        fn serde_json_hashmap() {
            #[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
            struct Test {
                #[cfg_attr(
                    feature = "serde-serialize",
                    serde(
                        serialize_with = "crate::utils::serde::serialize_to_vec_tuple",
                        deserialize_with = "crate::utils::serde::deserialize_from_vec_tuple"
                    )
                )]
                pub map: HashMap<usize, String>,
            }

            let s = Test {
                map: [(42, "Forty-Two".to_string())].into(),
            };
            let j = serde_json::to_string(&s).unwrap();
            assert_eq!(&j, "{\"map\":[[42,\"Forty-Two\"]]}");
            let p: Test = serde_json::from_str(&j).unwrap();
            assert_eq!(&p, &s);
        }
    }
}
