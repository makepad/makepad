use {
    crate::{
        makepad_live_id::{live_id, LiveId},
        shader_ast::{Ident, ShaderTy},
    },
    std::collections::HashMap,
};
type Ty = ShaderTy;

macro_rules! builtin {
    ($f:ident, [$(($($a:path),*) -> $b:path),*]) => {
        (
            Builtin2 {
                id: live_id!($f),
                maps: &[$(
                    (
                        &[
                            $($a),*
                        ],
                        $b
                    )
                ),*]
            }
        )
    }
}

pub struct Builtin2<'a> {
    id: LiveId,
    maps: &'a [(&'a [Ty], Ty)],
}

#[derive(Clone, Debug)]
pub struct Builtin {
    pub return_tys: HashMap<Vec<Ty>, Ty>,
}

pub fn generate_builtins() -> HashMap<Ident, Builtin> {
    fn generate_builtins(x: &[Builtin2]) -> HashMap<Ident, Builtin> {
        let mut map = HashMap::new();
        for b in x {
            let mut map2 = HashMap::new();
            for item in b.maps {
                map2.insert(item.0.to_vec(), item.1.clone());
            }
            map.insert(Ident(b.id), Builtin { return_tys: map2 });
        }
        map
    }
    let x = [
        builtin!(abs, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f,
            (Ty::Int) -> Ty::Int,
            (Ty::Ivec2) -> Ty::Ivec2,
            (Ty::Ivec3) -> Ty::Ivec3,
            (Ty::Ivec4) -> Ty::Ivec4
        ]),
        builtin!(acos, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(acos, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
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
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(atan, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f,
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2f, Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(ceil, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(clamp, [
            (Ty::Float, Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2f, Ty::Vec2f, Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Vec3f, Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Vec4f, Ty::Vec4f) -> Ty::Vec4f,
            (Ty::Vec2f, Ty::Float, Ty::Float) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Float, Ty::Float) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Float, Ty::Float) -> Ty::Vec4f
        ]),
        builtin!(cos, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(cross, [
            (Ty::Vec3f, Ty::Vec3f) -> Ty::Vec3f
        ]),
        builtin!(degrees, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(dFdx, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(dFdy, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(distance, [
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2f, Ty::Vec2f) -> Ty::Float,
            (Ty::Vec3f, Ty::Vec3f) -> Ty::Float,
            (Ty::Vec4f, Ty::Vec4f) -> Ty::Float
        ]),
        builtin!(dot, [
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2f, Ty::Vec2f) -> Ty::Float,
            (Ty::Vec3f, Ty::Vec3f) -> Ty::Float,
            (Ty::Vec4f, Ty::Vec4f) -> Ty::Float
        ]),
        builtin!(equal, [
            (Ty::Bvec2, Ty::Bvec2) -> Ty::Bvec2,
            (Ty::Bvec3, Ty::Bvec3) -> Ty::Bvec3,
            (Ty::Bvec4, Ty::Bvec4) -> Ty::Bvec4,
            (Ty::Ivec2, Ty::Ivec2) -> Ty::Bvec2,
            (Ty::Ivec3, Ty::Ivec3) -> Ty::Bvec3,
            (Ty::Ivec4, Ty::Ivec4) -> Ty::Bvec4,
            (Ty::Vec2f, Ty::Vec2f) -> Ty::Bvec2,
            (Ty::Vec3f, Ty::Vec3f) -> Ty::Bvec3,
            (Ty::Vec4f, Ty::Vec4f) -> Ty::Bvec4
        ]),
        builtin!(exp, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(exp2, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(faceforward, [
            (Ty::Float, Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2f, Ty::Vec2f, Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Vec3f, Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Vec4f, Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(floor, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(fract, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(greaterThan, [
            (Ty::Ivec2, Ty::Ivec2) -> Ty::Bvec2,
            (Ty::Ivec3, Ty::Ivec3) -> Ty::Bvec3,
            (Ty::Ivec4, Ty::Ivec4) -> Ty::Bvec4,
            (Ty::Vec2f, Ty::Vec2f) -> Ty::Bvec2,
            (Ty::Vec3f, Ty::Vec3f) -> Ty::Bvec3,
            (Ty::Vec4f, Ty::Vec4f) -> Ty::Bvec4
        ]),
        builtin!(greaterThanEqual, [
            (Ty::Ivec2, Ty::Ivec2) -> Ty::Bvec2,
            (Ty::Ivec3, Ty::Ivec3) -> Ty::Bvec3,
            (Ty::Ivec4, Ty::Ivec4) -> Ty::Bvec4,
            (Ty::Vec2f, Ty::Vec2f) -> Ty::Bvec2,
            (Ty::Vec3f, Ty::Vec3f) -> Ty::Bvec3,
            (Ty::Vec4f, Ty::Vec4f) -> Ty::Bvec4
        ]),
        builtin!(inversesqrt, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(inverse, [
            (Ty::Mat4f) -> Ty::Mat4f
        ]),
        builtin!(length, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Float,
            (Ty::Vec3f) -> Ty::Float,
            (Ty::Vec4f) -> Ty::Float
        ]),
        builtin!(lessThan, [
            (Ty::Ivec2, Ty::Ivec2) -> Ty::Bvec2,
            (Ty::Ivec3, Ty::Ivec3) -> Ty::Bvec3,
            (Ty::Ivec4, Ty::Ivec4) -> Ty::Bvec4,
            (Ty::Vec2f, Ty::Vec2f) -> Ty::Bvec2,
            (Ty::Vec3f, Ty::Vec3f) -> Ty::Bvec3,
            (Ty::Vec4f, Ty::Vec4f) -> Ty::Bvec4
        ]),
        builtin!(lessThanEqual, [
            (Ty::Ivec2, Ty::Ivec2) -> Ty::Bvec2,
            (Ty::Ivec3, Ty::Ivec3) -> Ty::Bvec3,
            (Ty::Ivec4, Ty::Ivec4) -> Ty::Bvec4,
            (Ty::Vec2f, Ty::Vec2f) -> Ty::Bvec2,
            (Ty::Vec3f, Ty::Vec3f) -> Ty::Bvec3,
            (Ty::Vec4f, Ty::Vec4f) -> Ty::Bvec4
        ]),
        builtin!(log, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(log2, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(matrixCompMult, [
            (Ty::Mat2, Ty::Mat2) -> Ty::Mat2,
            (Ty::Mat3, Ty::Mat3) -> Ty::Mat3,
            (Ty::Mat4f, Ty::Mat4f) -> Ty::Mat4f
        ]),
        builtin!(max, [
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2f, Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Vec4f) -> Ty::Vec4f,
            (Ty::Vec2f, Ty::Float) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Float) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Float) -> Ty::Vec4f
        ]),
        builtin!(min, [
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2f, Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Vec4f) -> Ty::Vec4f,
            (Ty::Vec2f, Ty::Float) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Float) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Float) -> Ty::Vec4f
        ]),
        builtin!(mix, [
            (Ty::Float, Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2f, Ty::Vec2f, Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Vec3f, Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Vec4f, Ty::Vec4f) -> Ty::Vec4f,
            (Ty::Vec2f, Ty::Vec2f, Ty::Float) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Vec3f, Ty::Float) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Vec4f, Ty::Float) -> Ty::Vec4f
        ]),
        builtin!(mod, [
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2f, Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Vec4f) -> Ty::Vec4f,
            (Ty::Vec2f, Ty::Float) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Float) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Float) -> Ty::Vec4f
        ]),
        builtin!(normalize, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
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
            (Ty::Vec2f, Ty::Vec2f) -> Ty::Bvec2,
            (Ty::Vec3f, Ty::Vec3f) -> Ty::Bvec3,
            (Ty::Vec4f, Ty::Vec4f) -> Ty::Bvec4
        ]),
        builtin!(pow, [
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2f, Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(radians, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(reflect, [
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2f, Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(refract, [
            (Ty::Float, Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2f, Ty::Vec2f, Ty::Float) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Vec3f, Ty::Float) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Vec4f, Ty::Float) -> Ty::Vec4f
        ]),
        builtin!(sample2d, [
            (Ty::Texture2D, Ty::Vec2f) -> Ty::Vec4f
        ]),
        builtin!(sample2d_rt, [
            (Ty::Texture2D, Ty::Vec2f) -> Ty::Vec4f
        ]),
        builtin!(sample2dOES, [
            (Ty::TextureOES, Ty::Vec2f) -> Ty::Vec4f
        ]),
        builtin!(depth_clip, [
            (Ty::Vec4f, Ty::Vec4f, Ty::Float) -> Ty::Vec4f
        ]),
        builtin!(sign, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(sin, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(smoothstep, [
            (Ty::Float, Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Vec2f, Ty::Vec2f, Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Vec3f, Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Vec4f, Ty::Vec4f) -> Ty::Vec4f,
            (Ty::Float, Ty::Float, Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Float, Ty::Float, Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Float, Ty::Float, Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(sqrt, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(step, [
            (Ty::Float, Ty::Float) -> Ty::Float,
            (Ty::Float, Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Float, Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Float, Ty::Vec4f) -> Ty::Vec4f,
            (Ty::Vec2f, Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f, Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f, Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(tan, [
            (Ty::Float) -> Ty::Float,
            (Ty::Vec2f) -> Ty::Vec2f,
            (Ty::Vec3f) -> Ty::Vec3f,
            (Ty::Vec4f) -> Ty::Vec4f
        ]),
        builtin!(transpose, [
            (Ty::Mat4f) -> Ty::Mat4f,
            (Ty::Mat3) -> Ty::Mat3
        ]),
    ];
    generate_builtins(&x)
}
