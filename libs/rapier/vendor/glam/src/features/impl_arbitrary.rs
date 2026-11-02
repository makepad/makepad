use crate::{
    BVec2, BVec3, BVec3A, BVec4, BVec4A, DQuat, DVec2, DVec3, DVec4, I16Vec2, I16Vec3, I16Vec4,
    I64Vec2, I64Vec3, I64Vec4, I8Vec2, I8Vec3, I8Vec4, IVec2, IVec3, IVec4, Quat, U16Vec2, U16Vec3,
    U16Vec4, U64Vec2, U64Vec3, U64Vec4, U8Vec2, U8Vec3, U8Vec4, USizeVec2, USizeVec3, USizeVec4,
    UVec2, UVec3, UVec4, Vec2, Vec3, Vec3A, Vec4,
};

macro_rules! arbitrary_vector_impl {
    ($($ty:ty),*) => {
        $(
            impl arbitrary::Arbitrary<'_> for $ty {
                fn arbitrary(u: &mut arbitrary::Unstructured<'_>) -> arbitrary::Result<Self> {
                    u.arbitrary().map(Self::from_array)
                }
            }
        )*
    };
}

arbitrary_vector_impl!(
    BVec2, BVec3, BVec3A, BVec4, BVec4A, DQuat, DVec2, DVec3, DVec4, I16Vec2, I16Vec3, I16Vec4,
    I64Vec2, I64Vec3, I64Vec4, I8Vec2, I8Vec3, I8Vec4, IVec2, IVec3, IVec4, Quat, U16Vec2, U16Vec3,
    U16Vec4, U64Vec2, U64Vec3, U64Vec4, U8Vec2, U8Vec3, U8Vec4, USizeVec2, USizeVec3, USizeVec4,
    UVec2, UVec3, UVec4, Vec2, Vec3, Vec3A, Vec4
);

use crate::{Affine2, Affine3A, DAffine2, DAffine3, DMat2, DMat3, DMat4, Mat2, Mat3, Mat3A, Mat4};

macro_rules! arbitrary_matrix_impl {
    ($($ty:ty),*) => {
        $(
            impl arbitrary::Arbitrary<'_> for $ty {
                fn arbitrary(u: &mut arbitrary::Unstructured<'_>) -> arbitrary::Result<Self> {
                    let array = u.arbitrary()?;
                    Ok(Self::from_cols_array(&array))
                }
            }
        )*
    };
}

arbitrary_matrix_impl!(
    Affine2, Affine3A, DAffine2, DAffine3, DMat2, DMat3, DMat4, Mat2, Mat3, Mat3A, Mat4
);
