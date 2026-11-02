use mint::IntoMint;

use crate::{
    DMat2, DMat3, DMat4, DQuat, DVec2, DVec3, DVec4, I16Vec2, I16Vec3, I16Vec4, I64Vec2, I64Vec3,
    I64Vec4, I8Vec2, I8Vec3, I8Vec4, IVec2, IVec3, IVec4, Mat2, Mat3, Mat3A, Mat4, Quat, U16Vec2,
    U16Vec3, U16Vec4, U64Vec2, U64Vec3, U64Vec4, U8Vec2, U8Vec3, U8Vec4, USizeVec2, USizeVec3,
    USizeVec4, UVec2, UVec3, UVec4, Vec2, Vec3, Vec3A, Vec4,
};

macro_rules! impl_vec_types {
    ($t:ty, $vec2:ty, $vec3:ty, $vec4:ty) => {
        impl From<mint::Point2<$t>> for $vec2 {
            fn from(v: mint::Point2<$t>) -> Self {
                Self::new(v.x, v.y)
            }
        }

        impl From<$vec2> for mint::Point2<$t> {
            fn from(v: $vec2) -> Self {
                Self { x: v.x, y: v.y }
            }
        }

        impl From<mint::Vector2<$t>> for $vec2 {
            fn from(v: mint::Vector2<$t>) -> Self {
                Self::new(v.x, v.y)
            }
        }

        impl From<$vec2> for mint::Vector2<$t> {
            fn from(v: $vec2) -> Self {
                Self { x: v.x, y: v.y }
            }
        }

        impl IntoMint for $vec2 {
            type MintType = mint::Vector2<$t>;
        }

        impl From<mint::Point3<$t>> for $vec3 {
            fn from(v: mint::Point3<$t>) -> Self {
                Self::new(v.x, v.y, v.z)
            }
        }

        impl From<$vec3> for mint::Point3<$t> {
            fn from(v: $vec3) -> Self {
                Self {
                    x: v.x,
                    y: v.y,
                    z: v.z,
                }
            }
        }

        impl From<mint::Vector3<$t>> for $vec3 {
            fn from(v: mint::Vector3<$t>) -> Self {
                Self::new(v.x, v.y, v.z)
            }
        }

        impl From<$vec3> for mint::Vector3<$t> {
            fn from(v: $vec3) -> Self {
                Self {
                    x: v.x,
                    y: v.y,
                    z: v.z,
                }
            }
        }

        impl IntoMint for $vec3 {
            type MintType = mint::Vector3<$t>;
        }

        impl From<mint::Vector4<$t>> for $vec4 {
            fn from(v: mint::Vector4<$t>) -> Self {
                Self::new(v.x, v.y, v.z, v.w)
            }
        }

        impl From<$vec4> for mint::Vector4<$t> {
            fn from(v: $vec4) -> Self {
                Self {
                    x: v.x,
                    y: v.y,
                    z: v.z,
                    w: v.w,
                }
            }
        }

        impl IntoMint for $vec4 {
            type MintType = mint::Vector4<$t>;
        }
    };
}

macro_rules! impl_float_types {
    ($t:ty, $mat2:ty, $mat3:ty, $mat4:ty, $quat:ty, $vec2:ty, $vec3:ty, $vec4:ty) => {
        impl_vec_types!($t, $vec2, $vec3, $vec4);

        impl From<mint::Quaternion<$t>> for $quat {
            fn from(q: mint::Quaternion<$t>) -> Self {
                Self::from_xyzw(q.v.x, q.v.y, q.v.z, q.s)
            }
        }

        impl From<$quat> for mint::Quaternion<$t> {
            fn from(q: $quat) -> Self {
                Self {
                    s: q.w,
                    v: mint::Vector3 {
                        x: q.x,
                        y: q.y,
                        z: q.z,
                    },
                }
            }
        }

        impl IntoMint for $quat {
            type MintType = mint::Quaternion<$t>;
        }

        impl From<mint::RowMatrix2<$t>> for $mat2 {
            fn from(m: mint::RowMatrix2<$t>) -> Self {
                Self::from_cols(m.x.into(), m.y.into()).transpose()
            }
        }

        impl From<$mat2> for mint::RowMatrix2<$t> {
            fn from(m: $mat2) -> Self {
                let mt = m.transpose();
                Self {
                    x: mt.x_axis.into(),
                    y: mt.y_axis.into(),
                }
            }
        }

        impl From<mint::ColumnMatrix2<$t>> for $mat2 {
            fn from(m: mint::ColumnMatrix2<$t>) -> Self {
                Self::from_cols(m.x.into(), m.y.into())
            }
        }

        impl From<$mat2> for mint::ColumnMatrix2<$t> {
            fn from(m: $mat2) -> Self {
                Self {
                    x: m.x_axis.into(),
                    y: m.y_axis.into(),
                }
            }
        }

        impl IntoMint for $mat2 {
            type MintType = mint::ColumnMatrix2<$t>;
        }

        impl From<mint::RowMatrix3<$t>> for $mat3 {
            fn from(m: mint::RowMatrix3<$t>) -> Self {
                Self::from_cols(m.x.into(), m.y.into(), m.z.into()).transpose()
            }
        }

        impl From<$mat3> for mint::RowMatrix3<$t> {
            fn from(m: $mat3) -> Self {
                let mt = m.transpose();
                Self {
                    x: mt.x_axis.into(),
                    y: mt.y_axis.into(),
                    z: mt.z_axis.into(),
                }
            }
        }

        impl From<mint::ColumnMatrix3<$t>> for $mat3 {
            fn from(m: mint::ColumnMatrix3<$t>) -> Self {
                Self::from_cols(m.x.into(), m.y.into(), m.z.into())
            }
        }

        impl From<$mat3> for mint::ColumnMatrix3<$t> {
            fn from(m: $mat3) -> Self {
                Self {
                    x: m.x_axis.into(),
                    y: m.y_axis.into(),
                    z: m.z_axis.into(),
                }
            }
        }

        impl IntoMint for $mat3 {
            type MintType = mint::ColumnMatrix3<$t>;
        }

        impl From<mint::RowMatrix4<$t>> for $mat4 {
            fn from(m: mint::RowMatrix4<$t>) -> Self {
                Self::from_cols(m.x.into(), m.y.into(), m.z.into(), m.w.into()).transpose()
            }
        }

        impl From<$mat4> for mint::RowMatrix4<$t> {
            fn from(m: $mat4) -> Self {
                let mt = m.transpose();
                Self {
                    x: mt.x_axis.into(),
                    y: mt.y_axis.into(),
                    z: mt.z_axis.into(),
                    w: mt.w_axis.into(),
                }
            }
        }

        impl From<mint::ColumnMatrix4<$t>> for $mat4 {
            fn from(m: mint::ColumnMatrix4<$t>) -> Self {
                Self::from_cols(m.x.into(), m.y.into(), m.z.into(), m.w.into())
            }
        }

        impl From<$mat4> for mint::ColumnMatrix4<$t> {
            fn from(m: $mat4) -> Self {
                Self {
                    x: m.x_axis.into(),
                    y: m.y_axis.into(),
                    z: m.z_axis.into(),
                    w: m.w_axis.into(),
                }
            }
        }

        impl IntoMint for $mat4 {
            type MintType = mint::ColumnMatrix4<$t>;
        }
    };
}

impl From<mint::Point3<f32>> for Vec3A {
    fn from(v: mint::Point3<f32>) -> Self {
        Self::new(v.x, v.y, v.z)
    }
}

impl From<Vec3A> for mint::Point3<f32> {
    fn from(v: Vec3A) -> Self {
        Self {
            x: v.x,
            y: v.y,
            z: v.z,
        }
    }
}

impl From<mint::Vector3<f32>> for Vec3A {
    fn from(v: mint::Vector3<f32>) -> Self {
        Self::new(v.x, v.y, v.z)
    }
}

impl From<Vec3A> for mint::Vector3<f32> {
    fn from(v: Vec3A) -> Self {
        Self {
            x: v.x,
            y: v.y,
            z: v.z,
        }
    }
}

impl IntoMint for Vec3A {
    type MintType = mint::Vector3<f32>;
}

impl From<mint::RowMatrix3<f32>> for Mat3A {
    fn from(m: mint::RowMatrix3<f32>) -> Self {
        Self::from_cols(m.x.into(), m.y.into(), m.z.into()).transpose()
    }
}

impl From<Mat3A> for mint::RowMatrix3<f32> {
    fn from(m: Mat3A) -> Self {
        let mt = m.transpose();
        Self {
            x: mt.x_axis.into(),
            y: mt.y_axis.into(),
            z: mt.z_axis.into(),
        }
    }
}

impl From<mint::ColumnMatrix3<f32>> for Mat3A {
    fn from(m: mint::ColumnMatrix3<f32>) -> Self {
        Self::from_cols(m.x.into(), m.y.into(), m.z.into())
    }
}

impl From<Mat3A> for mint::ColumnMatrix3<f32> {
    fn from(m: Mat3A) -> Self {
        Self {
            x: m.x_axis.into(),
            y: m.y_axis.into(),
            z: m.z_axis.into(),
        }
    }
}

impl IntoMint for Mat3A {
    type MintType = mint::ColumnMatrix3<f32>;
}

impl_float_types!(f32, Mat2, Mat3, Mat4, Quat, Vec2, Vec3, Vec4);
impl_float_types!(f64, DMat2, DMat3, DMat4, DQuat, DVec2, DVec3, DVec4);
impl_vec_types!(i8, I8Vec2, I8Vec3, I8Vec4);
impl_vec_types!(u8, U8Vec2, U8Vec3, U8Vec4);
impl_vec_types!(i16, I16Vec2, I16Vec3, I16Vec4);
impl_vec_types!(u16, U16Vec2, U16Vec3, U16Vec4);
impl_vec_types!(i32, IVec2, IVec3, IVec4);
impl_vec_types!(u32, UVec2, UVec3, UVec4);
impl_vec_types!(i64, I64Vec2, I64Vec3, I64Vec4);
impl_vec_types!(u64, U64Vec2, U64Vec3, U64Vec4);
impl_vec_types!(usize, USizeVec2, USizeVec3, USizeVec4);
