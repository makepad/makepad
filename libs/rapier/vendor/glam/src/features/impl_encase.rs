use crate::{IVec2, IVec3, IVec4, Mat2, Mat3, Mat4, UVec2, UVec3, UVec4, Vec2, Vec3, Vec4};
use encase::{
    matrix::{impl_matrix, AsMutMatrixParts, AsRefMatrixParts, FromMatrixParts, MatrixScalar},
    vector::impl_vector,
};

impl_vector!(2, Vec2, f32; using AsRef AsMut From);
impl_vector!(2, UVec2, u32; using AsRef AsMut From);
impl_vector!(2, IVec2, i32; using AsRef AsMut From);

impl_vector!(3, Vec3, f32; using AsRef AsMut From);
impl_vector!(3, UVec3, u32; using AsRef AsMut From);
impl_vector!(3, IVec3, i32; using AsRef AsMut From);

impl_vector!(4, Vec4, f32; using AsRef AsMut From);
impl_vector!(4, UVec4, u32; using AsRef AsMut From);
impl_vector!(4, IVec4, i32; using AsRef AsMut From);

macro_rules! impl_matrix_traits {
    ($c:literal, $r:literal, $type:ty, $el_ty:ty) => {
        impl AsRefMatrixParts<$el_ty, $c, $r> for $type
        where
            Self: AsRef<[$el_ty; $r * $c]>,
            $el_ty: MatrixScalar,
        {
            fn as_ref_parts(&self) -> &[[$el_ty; $r]; $c] {
                unsafe {
                    ::core::mem::transmute::<&[$el_ty; $r * $c], &[[$el_ty; $r]; $c]>(self.as_ref())
                }
            }
        }

        impl AsMutMatrixParts<$el_ty, $c, $r> for $type
        where
            Self: AsMut<[$el_ty; $r * $c]>,
            $el_ty: MatrixScalar,
        {
            fn as_mut_parts(&mut self) -> &mut [[$el_ty; $r]; $c] {
                unsafe {
                    ::core::mem::transmute::<&mut [$el_ty; $r * $c], &mut [[$el_ty; $r]; $c]>(
                        self.as_mut(),
                    )
                }
            }
        }

        impl FromMatrixParts<$el_ty, $c, $r> for $type {
            fn from_parts(parts: [[$el_ty; $r]; $c]) -> Self {
                Self::from_cols_array_2d(&parts)
            }
        }
    };
}

impl_matrix_traits!(2, 2, Mat2, f32);
impl_matrix_traits!(3, 3, Mat3, f32);
impl_matrix_traits!(4, 4, Mat4, f32);

impl_matrix!(2, 2, Mat2, f32);
impl_matrix!(3, 3, Mat3, f32);
impl_matrix!(4, 4, Mat4, f32);
