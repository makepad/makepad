use crate::ty::Ty;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TyLit {
    Bool,
    Int,
    Float,
    Bvec2,
    Bvec3,
    Bvec4,
    Ivec2,
    Ivec3,
    Ivec4,
    Vec2,
    Vec3,
    Vec4,
    Mat2,
    Mat3,
    Mat4,
}

impl TyLit {
    pub fn to_ty(self) -> Ty {
        match self {
            TyLit::Bool => Ty::Bool,
            TyLit::Int => Ty::Int,
            TyLit::Float => Ty::Float,
            TyLit::Bvec2 => Ty::Bvec2,
            TyLit::Bvec3 => Ty::Bvec3,
            TyLit::Bvec4 => Ty::Bvec4,
            TyLit::Ivec2 => Ty::Ivec2,
            TyLit::Ivec3 => Ty::Ivec3,
            TyLit::Ivec4 => Ty::Ivec4,
            TyLit::Vec2 => Ty::Vec2,
            TyLit::Vec3 => Ty::Vec3,
            TyLit::Vec4 => Ty::Vec4,
            TyLit::Mat2 => Ty::Mat2,
            TyLit::Mat3 => Ty::Mat3,
            TyLit::Mat4 => Ty::Mat4,
        }
    }
}

impl fmt::Display for TyLit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TyLit::Bool => write!(f, "bool"),
            TyLit::Int => write!(f, "int"),
            TyLit::Float => write!(f, "float"),
            TyLit::Bvec2 => write!(f, "bvec2"),
            TyLit::Bvec3 => write!(f, "bvec3"),
            TyLit::Bvec4 => write!(f, "bvec4"),
            TyLit::Ivec2 => write!(f, "ivec2"),
            TyLit::Ivec3 => write!(f, "ivec3"),
            TyLit::Ivec4 => write!(f, "ivec4"),
            TyLit::Vec2 => write!(f, "vec2"),
            TyLit::Vec3 => write!(f, "vec3"),
            TyLit::Vec4 => write!(f, "vec4"),
            TyLit::Mat2 => write!(f, "mat2"),
            TyLit::Mat3 => write!(f, "mat3"),
            TyLit::Mat4 => write!(f, "mat4"),
        }
    }
}
