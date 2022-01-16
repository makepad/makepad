use{
    std::collections::HashMap,
    crate::{
        makepad_live_compiler::LiveId,
        shader_ast::{Ident, ShaderTy}
    },
};
type Ty = ShaderTy;

macro_rules! builtin {
    ($f:ident, [$(($($a:path),*) -> $b:path),*]) => {
        (
            Ident(LiveId::from_str_unchecked(stringify!($f))),
            Builtin {
                return_tys: [$(
                    (
                        vec![
                            $($a),*
                        ],
                        $b
                    )
                ),*].iter().cloned().collect()
            }
        )
    }
}

#[derive(Clone, Debug)]
pub struct Builtin {
    pub return_tys: HashMap<Vec<Ty>, Ty>,
}

pub fn generate_builtins() -> HashMap<Ident, Builtin> {
    [
        builtin!(abs, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4,
            (Ty::Int) -> Ty::Int,
            (Ty::Ivec2) -> Ty::Ivec2,
            (Ty::Ivec3) -> Ty::Ivec3,
            (Ty::Ivec4) -> Ty::Ivec4 
        ]),
        builtin!(acos, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(acos, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(all, [
            (Ty::Bvec2) -> Ty::Bool,
            (Ty::Bvec3) -> Ty::Bool,
            (Ty::Bvec4) -> Ty::Bool
        ]),
        builtin!(any, [
            (Ty::Bvec2) -> Ty::Bool,
            (Ty::Bvec3) -> Ty::Bool,
            (Ty::Bvec4) -> Ty::Bool
        ]),
        builtin!(asin, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(atan, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4,
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2, Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3, Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4, Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(ceil, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(clamp, [
            (Ty::Float, Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2, Ty::Vec2, Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3, Ty::Vec3, Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4, Ty::Vec4, Ty::Vec4) -> Ty::Vec4,
            (Ty::Vec2, Ty::Float, Ty::Float) -> Ty::Vec2,
            (Ty::Vec3, Ty::Float, Ty::Float) -> Ty::Vec3,
            (Ty::Vec4, Ty::Float, Ty::Float) -> Ty::Vec4
        ]),
        builtin!(cos, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(cross, [
            (Ty::Vec3, Ty::Vec3) -> Ty::Vec3
        ]),
        builtin!(degrees, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(dFdx, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(dFdy, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(distance, [
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2, Ty::Vec2) -> Ty::Float,
            (Ty::Vec3, Ty::Vec3) -> Ty::Float,
            (Ty::Vec4, Ty::Vec4) -> Ty::Float
        ]),
        builtin!(dot, [
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2, Ty::Vec2) -> Ty::Float,
            (Ty::Vec3, Ty::Vec3) -> Ty::Float,
            (Ty::Vec4, Ty::Vec4) -> Ty::Float
        ]),
        builtin!(equal, [
            (Ty::Bvec2, Ty::Bvec2) -> Ty::Bvec2,
            (Ty::Bvec3, Ty::Bvec3) -> Ty::Bvec3,
            (Ty::Bvec4, Ty::Bvec4) -> Ty::Bvec4,
            (Ty::Ivec2, Ty::Ivec2) -> Ty::Bvec2,
            (Ty::Ivec3, Ty::Ivec3) -> Ty::Bvec3,
            (Ty::Ivec4, Ty::Ivec4) -> Ty::Bvec4,
            (Ty::Vec2, Ty::Vec2) -> Ty::Bvec2,
            (Ty::Vec3, Ty::Vec3) -> Ty::Bvec3,
            (Ty::Vec4, Ty::Vec4) -> Ty::Bvec4
        ]),
        builtin!(exp, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(exp2, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(faceforward, [
            (Ty::Float, Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2, Ty::Vec2, Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3, Ty::Vec3, Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4, Ty::Vec4, Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(floor, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(fract, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(greaterThan, [
            (Ty::Ivec2, Ty::Ivec2) -> Ty::Bvec2,
            (Ty::Ivec3, Ty::Ivec3) -> Ty::Bvec3,
            (Ty::Ivec4, Ty::Ivec4) -> Ty::Bvec4,
            (Ty::Vec2, Ty::Vec2) -> Ty::Bvec2,
            (Ty::Vec3, Ty::Vec3) -> Ty::Bvec3,
            (Ty::Vec4, Ty::Vec4) -> Ty::Bvec4
        ]),
        builtin!(greaterThanEqual, [
            (Ty::Ivec2, Ty::Ivec2) -> Ty::Bvec2,
            (Ty::Ivec3, Ty::Ivec3) -> Ty::Bvec3,
            (Ty::Ivec4, Ty::Ivec4) -> Ty::Bvec4,
            (Ty::Vec2, Ty::Vec2) -> Ty::Bvec2,
            (Ty::Vec3, Ty::Vec3) -> Ty::Bvec3,
            (Ty::Vec4, Ty::Vec4) -> Ty::Bvec4
        ]),
        builtin!(inversesqrt, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(inverse, [
            (Ty::Mat4) -> Ty::Mat4
        ]),
        builtin!(length, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Float,
            (Ty::Vec3) -> Ty::Float,
            (Ty::Vec4) -> Ty::Float
        ]),
        builtin!(lessThan, [
            (Ty::Ivec2, Ty::Ivec2) -> Ty::Bvec2,
            (Ty::Ivec3, Ty::Ivec3) -> Ty::Bvec3,
            (Ty::Ivec4, Ty::Ivec4) -> Ty::Bvec4,
            (Ty::Vec2, Ty::Vec2) -> Ty::Bvec2,
            (Ty::Vec3, Ty::Vec3) -> Ty::Bvec3,
            (Ty::Vec4, Ty::Vec4) -> Ty::Bvec4
        ]),
        builtin!(lessThanEqual, [
            (Ty::Ivec2, Ty::Ivec2) -> Ty::Bvec2,
            (Ty::Ivec3, Ty::Ivec3) -> Ty::Bvec3,
            (Ty::Ivec4, Ty::Ivec4) -> Ty::Bvec4,
            (Ty::Vec2, Ty::Vec2) -> Ty::Bvec2,
            (Ty::Vec3, Ty::Vec3) -> Ty::Bvec3,
            (Ty::Vec4, Ty::Vec4) -> Ty::Bvec4
        ]),
        builtin!(log, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(log2, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(matrixCompMult, [
            (Ty::Mat2, Ty::Mat2) -> Ty::Mat2,
            (Ty::Mat3, Ty::Mat3) -> Ty::Mat3,
            (Ty::Mat4, Ty::Mat4) -> Ty::Mat4
        ]),
        builtin!(max, [
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2, Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3, Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4, Ty::Vec4) -> Ty::Vec4,
            (Ty::Vec2, Ty::Float) -> Ty::Vec2,
            (Ty::Vec3, Ty::Float) -> Ty::Vec3,
            (Ty::Vec4, Ty::Float) -> Ty::Vec4
        ]),
        builtin!(min, [
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2, Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3, Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4, Ty::Vec4) -> Ty::Vec4,
            (Ty::Vec2, Ty::Float) -> Ty::Vec2,
            (Ty::Vec3, Ty::Float) -> Ty::Vec3,
            (Ty::Vec4, Ty::Float) -> Ty::Vec4
        ]),
        builtin!(mix, [
            (Ty::Float, Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2, Ty::Vec2, Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3, Ty::Vec3, Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4, Ty::Vec4, Ty::Vec4) -> Ty::Vec4,
            (Ty::Vec2, Ty::Vec2, Ty::Float) -> Ty::Vec2,
            (Ty::Vec3, Ty::Vec3, Ty::Float) -> Ty::Vec3,
            (Ty::Vec4, Ty::Vec4, Ty::Float) -> Ty::Vec4
        ]),
        builtin!(mod, [
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2, Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3, Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4, Ty::Vec4) -> Ty::Vec4,
            (Ty::Vec2, Ty::Float) -> Ty::Vec2,
            (Ty::Vec3, Ty::Float) -> Ty::Vec3,
            (Ty::Vec4, Ty::Float) -> Ty::Vec4
        ]),
        builtin!(normalize, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(not, [
            (Ty::Bvec2) -> Ty::Bvec2,
            (Ty::Bvec3) -> Ty::Bvec3,
            (Ty::Bvec4) -> Ty::Bvec4
        ]),
        builtin!(notEqual, [
            (Ty::Bvec2, Ty::Bvec2) -> Ty::Bvec2,
            (Ty::Bvec3, Ty::Bvec3) -> Ty::Bvec3,
            (Ty::Bvec4, Ty::Bvec4) -> Ty::Bvec4,
            (Ty::Ivec2, Ty::Ivec2) -> Ty::Bvec2,
            (Ty::Ivec3, Ty::Ivec3) -> Ty::Bvec3,
            (Ty::Ivec4, Ty::Ivec4) -> Ty::Bvec4,
            (Ty::Vec2, Ty::Vec2) -> Ty::Bvec2,
            (Ty::Vec3, Ty::Vec3) -> Ty::Bvec3,
            (Ty::Vec4, Ty::Vec4) -> Ty::Bvec4
        ]),
        builtin!(pow, [
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2, Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3, Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4, Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(radians, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(reflect, [
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2, Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3, Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4, Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(refract, [
            (Ty::Float, Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2, Ty::Vec2, Ty::Float) -> Ty::Vec2,
            (Ty::Vec3, Ty::Vec3, Ty::Float) -> Ty::Vec3,
            (Ty::Vec4, Ty::Vec4, Ty::Float) -> Ty::Vec4
        ]),
        builtin!(sample2d, [
            (Ty::Texture2D, Ty::Vec2) -> Ty::Vec4
        ]),
        builtin!(sign, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(sin, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(smoothstep, [
            (Ty::Float, Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2, Ty::Vec2, Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3, Ty::Vec3, Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4, Ty::Vec4, Ty::Vec4) -> Ty::Vec4,
            (Ty::Float, Ty::Float, Ty::Vec2) -> Ty::Vec2,
            (Ty::Float, Ty::Float, Ty::Vec3) -> Ty::Vec3,
            (Ty::Float, Ty::Float, Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(sqrt, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(step, [
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Float, Ty::Vec2) -> Ty::Vec2,
            (Ty::Float, Ty::Vec3) -> Ty::Vec3,
            (Ty::Float, Ty::Vec4) -> Ty::Vec4,
            (Ty::Vec2, Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3, Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4, Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(tan, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2) -> Ty::Vec2,
            (Ty::Vec3) -> Ty::Vec3,
            (Ty::Vec4) -> Ty::Vec4
        ]),
        builtin!(transpose, [
            (Ty::Mat4) -> Ty::Mat4,
            (Ty::Mat3) -> Ty::Mat3
        ]),

    ]
    .iter()
    .cloned()
    .collect()
}
