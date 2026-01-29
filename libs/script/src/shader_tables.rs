use crate::mod_pod::ScriptPodBuiltins;
use crate::value::*;
use crate::pod::*;
use crate::shader::ShaderType;
use crate::trap::ScriptTrap;
use crate::*;

pub fn type_table_neg(val: &ShaderType, trap:ScriptTrap, builtins:&ScriptPodBuiltins )->ShaderType{
    let r = match val{
        ShaderType::AbstractInt => ShaderType::AbstractInt,
        ShaderType::AbstractFloat => ShaderType::AbstractFloat,
        ShaderType::Pod(x) if *x == builtins.pod_f32 => ShaderType::Pod(builtins.pod_f32),
        ShaderType::Pod(x) if *x == builtins.pod_f16 => ShaderType::Pod(builtins.pod_f16),
        ShaderType::Pod(x) if *x == builtins.pod_i32 => ShaderType::Pod(builtins.pod_i32),
        ShaderType::Pod(x) if *x == builtins.pod_vec2f => ShaderType::Pod(builtins.pod_vec2f),
        ShaderType::Pod(x) if *x == builtins.pod_vec3f => ShaderType::Pod(builtins.pod_vec3f),
        ShaderType::Pod(x) if *x == builtins.pod_vec4f => ShaderType::Pod(builtins.pod_vec4f),
        ShaderType::Pod(x) if *x == builtins.pod_vec2h => ShaderType::Pod(builtins.pod_vec2h),
        ShaderType::Pod(x) if *x == builtins.pod_vec3h => ShaderType::Pod(builtins.pod_vec3h),
        ShaderType::Pod(x) if *x == builtins.pod_vec4h => ShaderType::Pod(builtins.pod_vec4h),
        ShaderType::Pod(x) if *x == builtins.pod_vec2i => ShaderType::Pod(builtins.pod_vec2i),
        ShaderType::Pod(x) if *x == builtins.pod_vec3i => ShaderType::Pod(builtins.pod_vec3i),
        ShaderType::Pod(x) if *x == builtins.pod_vec4i => ShaderType::Pod(builtins.pod_vec4i),
        _=>ShaderType::Error(NIL),
    };
    if let ShaderType::Error(_) = r{
        err_opcode_not_defined_for_shader_type!(trap);
    }
    r
}

pub fn type_table_float_arithmetic(lhs: &ShaderType, rhs: &ShaderType, trap:ScriptTrap, builtins:&ScriptPodBuiltins )->ShaderType{
    let r = match lhs{
        ShaderType::AbstractFloat => match rhs{
            ShaderType::AbstractFloat=>ShaderType::AbstractFloat,
            ShaderType::AbstractInt=>ShaderType::AbstractFloat,
            ShaderType::Pod(x) if *x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_f32),
            ShaderType::Pod(x) if *x == builtins.pod_f16=>ShaderType::Pod(builtins.pod_f16),
            ShaderType::Pod(x) if *x == builtins.pod_vec2f=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::Pod(x) if *x == builtins.pod_vec3f=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::Pod(x) if *x == builtins.pod_vec4f=>ShaderType::Pod(builtins.pod_vec4f),
            ShaderType::Pod(x) if *x == builtins.pod_vec2h=>ShaderType::Pod(builtins.pod_vec2h),
            ShaderType::Pod(x) if *x == builtins.pod_vec3h=>ShaderType::Pod(builtins.pod_vec3h),
            ShaderType::Pod(x) if *x == builtins.pod_vec4h=>ShaderType::Pod(builtins.pod_vec4h),
            // abstract float * matrix -> matrix
            ShaderType::Pod(x) if *x == builtins.pod_mat2x2f=>ShaderType::Pod(builtins.pod_mat2x2f),
            ShaderType::Pod(x) if *x == builtins.pod_mat2x3f=>ShaderType::Pod(builtins.pod_mat2x3f),
            ShaderType::Pod(x) if *x == builtins.pod_mat2x4f=>ShaderType::Pod(builtins.pod_mat2x4f),
            ShaderType::Pod(x) if *x == builtins.pod_mat3x2f=>ShaderType::Pod(builtins.pod_mat3x2f),
            ShaderType::Pod(x) if *x == builtins.pod_mat3x3f=>ShaderType::Pod(builtins.pod_mat3x3f),
            ShaderType::Pod(x) if *x == builtins.pod_mat3x4f=>ShaderType::Pod(builtins.pod_mat3x4f),
            ShaderType::Pod(x) if *x == builtins.pod_mat4x2f=>ShaderType::Pod(builtins.pod_mat4x2f),
            ShaderType::Pod(x) if *x == builtins.pod_mat4x3f=>ShaderType::Pod(builtins.pod_mat4x3f),
            ShaderType::Pod(x) if *x == builtins.pod_mat4x4f=>ShaderType::Pod(builtins.pod_mat4x4f),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::AbstractInt => match rhs{
            ShaderType::AbstractFloat=>ShaderType::AbstractFloat,
            ShaderType::AbstractInt=>ShaderType::AbstractInt,
            ShaderType::Pod(x) if *x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_u32),
            ShaderType::Pod(x) if *x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_i32),
            ShaderType::Pod(x) if *x == builtins.pod_vec2f=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::Pod(x) if *x == builtins.pod_vec3f=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::Pod(x) if *x == builtins.pod_vec4f=>ShaderType::Pod(builtins.pod_vec4f),
            ShaderType::Pod(x) if *x == builtins.pod_vec2h=>ShaderType::Pod(builtins.pod_vec2h),
            ShaderType::Pod(x) if *x == builtins.pod_vec3h=>ShaderType::Pod(builtins.pod_vec3h),
            ShaderType::Pod(x) if *x == builtins.pod_vec4h=>ShaderType::Pod(builtins.pod_vec4h),
            ShaderType::Pod(x) if *x == builtins.pod_vec2u=>ShaderType::Pod(builtins.pod_vec2u),
            ShaderType::Pod(x) if *x == builtins.pod_vec3u=>ShaderType::Pod(builtins.pod_vec3u),
            ShaderType::Pod(x) if *x == builtins.pod_vec4u=>ShaderType::Pod(builtins.pod_vec4u),
            ShaderType::Pod(x) if *x == builtins.pod_vec2i=>ShaderType::Pod(builtins.pod_vec2i),
            ShaderType::Pod(x) if *x == builtins.pod_vec3i=>ShaderType::Pod(builtins.pod_vec3i),
            ShaderType::Pod(x) if *x == builtins.pod_vec4i=>ShaderType::Pod(builtins.pod_vec4i),
            // abstract int * matrix -> matrix
            ShaderType::Pod(x) if *x == builtins.pod_mat2x2f=>ShaderType::Pod(builtins.pod_mat2x2f),
            ShaderType::Pod(x) if *x == builtins.pod_mat2x3f=>ShaderType::Pod(builtins.pod_mat2x3f),
            ShaderType::Pod(x) if *x == builtins.pod_mat2x4f=>ShaderType::Pod(builtins.pod_mat2x4f),
            ShaderType::Pod(x) if *x == builtins.pod_mat3x2f=>ShaderType::Pod(builtins.pod_mat3x2f),
            ShaderType::Pod(x) if *x == builtins.pod_mat3x3f=>ShaderType::Pod(builtins.pod_mat3x3f),
            ShaderType::Pod(x) if *x == builtins.pod_mat3x4f=>ShaderType::Pod(builtins.pod_mat3x4f),
            ShaderType::Pod(x) if *x == builtins.pod_mat4x2f=>ShaderType::Pod(builtins.pod_mat4x2f),
            ShaderType::Pod(x) if *x == builtins.pod_mat4x3f=>ShaderType::Pod(builtins.pod_mat4x3f),
            ShaderType::Pod(x) if *x == builtins.pod_mat4x4f=>ShaderType::Pod(builtins.pod_mat4x4f),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_f32=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_f32),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_f32),
            ShaderType::Pod(x) if *x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_f32),
            ShaderType::Pod(x) if *x == builtins.pod_vec2f=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::Pod(x) if *x == builtins.pod_vec3f=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::Pod(x) if *x == builtins.pod_vec4f=>ShaderType::Pod(builtins.pod_vec4f),
            // scalar * matrix -> matrix
            ShaderType::Pod(x) if *x == builtins.pod_mat2x2f=>ShaderType::Pod(builtins.pod_mat2x2f),
            ShaderType::Pod(x) if *x == builtins.pod_mat2x3f=>ShaderType::Pod(builtins.pod_mat2x3f),
            ShaderType::Pod(x) if *x == builtins.pod_mat2x4f=>ShaderType::Pod(builtins.pod_mat2x4f),
            ShaderType::Pod(x) if *x == builtins.pod_mat3x2f=>ShaderType::Pod(builtins.pod_mat3x2f),
            ShaderType::Pod(x) if *x == builtins.pod_mat3x3f=>ShaderType::Pod(builtins.pod_mat3x3f),
            ShaderType::Pod(x) if *x == builtins.pod_mat3x4f=>ShaderType::Pod(builtins.pod_mat3x4f),
            ShaderType::Pod(x) if *x == builtins.pod_mat4x2f=>ShaderType::Pod(builtins.pod_mat4x2f),
            ShaderType::Pod(x) if *x == builtins.pod_mat4x3f=>ShaderType::Pod(builtins.pod_mat4x3f),
            ShaderType::Pod(x) if *x == builtins.pod_mat4x4f=>ShaderType::Pod(builtins.pod_mat4x4f),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_f16=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_f16),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_f16),
            ShaderType::Pod(x) if *x == builtins.pod_f16=>ShaderType::Pod(builtins.pod_f16),
            ShaderType::Pod(x) if *x == builtins.pod_vec2h=>ShaderType::Pod(builtins.pod_vec2h),
            ShaderType::Pod(x) if *x == builtins.pod_vec3h=>ShaderType::Pod(builtins.pod_vec3h),
            ShaderType::Pod(x) if *x == builtins.pod_vec4h=>ShaderType::Pod(builtins.pod_vec4h),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_u32=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_u32),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_u32),
            ShaderType::Pod(x) if *x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_u32),
            ShaderType::Pod(x) if *x == builtins.pod_vec2u=>ShaderType::Pod(builtins.pod_vec2u),
            ShaderType::Pod(x) if *x == builtins.pod_vec3u=>ShaderType::Pod(builtins.pod_vec3u),
            ShaderType::Pod(x) if *x == builtins.pod_vec4u=>ShaderType::Pod(builtins.pod_vec4u),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_i32=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_i32),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_i32),
            ShaderType::Pod(x) if *x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_i32),
            ShaderType::Pod(x) if *x == builtins.pod_vec2i=>ShaderType::Pod(builtins.pod_vec2i),
            ShaderType::Pod(x) if *x == builtins.pod_vec3i=>ShaderType::Pod(builtins.pod_vec3i),
            ShaderType::Pod(x) if *x == builtins.pod_vec4i=>ShaderType::Pod(builtins.pod_vec4i),
            _=>ShaderType::Error(NIL),
        }
        // vec2f * matCx2 -> vecC (vector * matrix multiplication)
        ShaderType::Pod(x) if *x == builtins.pod_vec2f=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::Pod(x) if *x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::Pod(x) if *x == builtins.pod_vec2f=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::Pod(x) if *x == builtins.pod_mat2x2f=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::Pod(x) if *x == builtins.pod_mat3x2f=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::Pod(x) if *x == builtins.pod_mat4x2f=>ShaderType::Pod(builtins.pod_vec4f),
            _=>ShaderType::Error(NIL),
        }
        // vec3f * matCx3 -> vecC
        ShaderType::Pod(x) if *x == builtins.pod_vec3f=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::Pod(x) if *x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::Pod(x) if *x == builtins.pod_vec3f=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::Pod(x) if *x == builtins.pod_mat2x3f=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::Pod(x) if *x == builtins.pod_mat3x3f=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::Pod(x) if *x == builtins.pod_mat4x3f=>ShaderType::Pod(builtins.pod_vec4f),
            _=>ShaderType::Error(NIL),
        }
        // vec4f * matCx4 -> vecC
        ShaderType::Pod(x) if *x == builtins.pod_vec4f=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_vec4f),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec4f),
            ShaderType::Pod(x) if *x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_vec4f),
            ShaderType::Pod(x) if *x == builtins.pod_vec4f=>ShaderType::Pod(builtins.pod_vec4f),
            ShaderType::Pod(x) if *x == builtins.pod_mat2x4f=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::Pod(x) if *x == builtins.pod_mat3x4f=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::Pod(x) if *x == builtins.pod_mat4x4f=>ShaderType::Pod(builtins.pod_vec4f),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec2h=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_vec2h),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec2h),
            ShaderType::Pod(x) if *x == builtins.pod_f16=>ShaderType::Pod(builtins.pod_vec2h),
            ShaderType::Pod(x) if *x == builtins.pod_vec2h=>ShaderType::Pod(builtins.pod_vec2h),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec3h=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_vec3h),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec3h),
            ShaderType::Pod(x) if *x == builtins.pod_f16=>ShaderType::Pod(builtins.pod_vec3h),
            ShaderType::Pod(x) if *x == builtins.pod_vec3h=>ShaderType::Pod(builtins.pod_vec3h),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec4h=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_vec4h),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec4h),
            ShaderType::Pod(x) if *x == builtins.pod_f16=>ShaderType::Pod(builtins.pod_vec4h),
            ShaderType::Pod(x) if *x == builtins.pod_vec4h=>ShaderType::Pod(builtins.pod_vec4h),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec2u=> match rhs{
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec2u),
            ShaderType::Pod(x) if *x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_vec2u),
            ShaderType::Pod(x) if *x == builtins.pod_vec2u=>ShaderType::Pod(builtins.pod_vec2u),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec3u=> match rhs{
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec3u),
            ShaderType::Pod(x) if *x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_vec3u),
            ShaderType::Pod(x) if *x == builtins.pod_vec3u=>ShaderType::Pod(builtins.pod_vec3u),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec4u=> match rhs{
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec4u),
            ShaderType::Pod(x) if *x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_vec4u),
            ShaderType::Pod(x) if *x == builtins.pod_vec4u=>ShaderType::Pod(builtins.pod_vec4u),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec2i=> match rhs{
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec2i),
            ShaderType::Pod(x) if *x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_vec2i),
            ShaderType::Pod(x) if *x == builtins.pod_vec2i=>ShaderType::Pod(builtins.pod_vec2i),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec3i=> match rhs{
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec3i),
            ShaderType::Pod(x) if *x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_vec3i),
            ShaderType::Pod(x) if *x == builtins.pod_vec3i=>ShaderType::Pod(builtins.pod_vec3i),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec4i=> match rhs{
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec4i),
            ShaderType::Pod(x) if *x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_vec4i),
            ShaderType::Pod(x) if *x == builtins.pod_vec4i=>ShaderType::Pod(builtins.pod_vec4i),
            _=>ShaderType::Error(NIL),
        }
        // Matrix multiplication: matCxR * vecR -> vecC
        // mat2x2f * vec2f -> vec2f, mat2x2f * scalar -> mat2x2f
        ShaderType::Pod(x) if *x == builtins.pod_mat2x2f => match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_mat2x2f),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_mat2x2f),
            ShaderType::Pod(x) if *x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_mat2x2f),
            ShaderType::Pod(x) if *x == builtins.pod_vec2f=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::Pod(x) if *x == builtins.pod_mat2x2f=>ShaderType::Pod(builtins.pod_mat2x2f),
            _=>ShaderType::Error(NIL),
        }
        // mat2x3f * vec3f -> vec2f
        ShaderType::Pod(x) if *x == builtins.pod_mat2x3f => match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_mat2x3f),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_mat2x3f),
            ShaderType::Pod(x) if *x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_mat2x3f),
            ShaderType::Pod(x) if *x == builtins.pod_vec3f=>ShaderType::Pod(builtins.pod_vec2f),
            _=>ShaderType::Error(NIL),
        }
        // mat2x4f * vec4f -> vec2f
        ShaderType::Pod(x) if *x == builtins.pod_mat2x4f => match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_mat2x4f),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_mat2x4f),
            ShaderType::Pod(x) if *x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_mat2x4f),
            ShaderType::Pod(x) if *x == builtins.pod_vec4f=>ShaderType::Pod(builtins.pod_vec2f),
            _=>ShaderType::Error(NIL),
        }
        // mat3x2f * vec2f -> vec3f
        ShaderType::Pod(x) if *x == builtins.pod_mat3x2f => match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_mat3x2f),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_mat3x2f),
            ShaderType::Pod(x) if *x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_mat3x2f),
            ShaderType::Pod(x) if *x == builtins.pod_vec2f=>ShaderType::Pod(builtins.pod_vec3f),
            _=>ShaderType::Error(NIL),
        }
        // mat3x3f * vec3f -> vec3f
        ShaderType::Pod(x) if *x == builtins.pod_mat3x3f => match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_mat3x3f),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_mat3x3f),
            ShaderType::Pod(x) if *x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_mat3x3f),
            ShaderType::Pod(x) if *x == builtins.pod_vec3f=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::Pod(x) if *x == builtins.pod_mat3x3f=>ShaderType::Pod(builtins.pod_mat3x3f),
            _=>ShaderType::Error(NIL),
        }
        // mat3x4f * vec4f -> vec3f
        ShaderType::Pod(x) if *x == builtins.pod_mat3x4f => match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_mat3x4f),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_mat3x4f),
            ShaderType::Pod(x) if *x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_mat3x4f),
            ShaderType::Pod(x) if *x == builtins.pod_vec4f=>ShaderType::Pod(builtins.pod_vec3f),
            _=>ShaderType::Error(NIL),
        }
        // mat4x2f * vec2f -> vec4f
        ShaderType::Pod(x) if *x == builtins.pod_mat4x2f => match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_mat4x2f),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_mat4x2f),
            ShaderType::Pod(x) if *x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_mat4x2f),
            ShaderType::Pod(x) if *x == builtins.pod_vec2f=>ShaderType::Pod(builtins.pod_vec4f),
            _=>ShaderType::Error(NIL),
        }
        // mat4x3f * vec3f -> vec4f
        ShaderType::Pod(x) if *x == builtins.pod_mat4x3f => match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_mat4x3f),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_mat4x3f),
            ShaderType::Pod(x) if *x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_mat4x3f),
            ShaderType::Pod(x) if *x == builtins.pod_vec3f=>ShaderType::Pod(builtins.pod_vec4f),
            _=>ShaderType::Error(NIL),
        }
        // mat4x4f * vec4f -> vec4f
        ShaderType::Pod(x) if *x == builtins.pod_mat4x4f => match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_mat4x4f),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_mat4x4f),
            ShaderType::Pod(x) if *x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_mat4x4f),
            ShaderType::Pod(x) if *x == builtins.pod_vec4f=>ShaderType::Pod(builtins.pod_vec4f),
            ShaderType::Pod(x) if *x == builtins.pod_mat4x4f=>ShaderType::Pod(builtins.pod_mat4x4f),
            _=>ShaderType::Error(NIL),
        }
        _=>ShaderType::Error(NIL),
    };
    if let ShaderType::Error(_) = r{
        err_no_wgsl_conversion_available!(trap);
    }
    r
}
    
pub fn type_table_int_arithmetic(lhs: &ShaderType, rhs: &ShaderType, trap:ScriptTrap, builtins:&ScriptPodBuiltins )->ShaderType{
    let r = match lhs{
        ShaderType::AbstractFloat => match rhs{
            _=>ShaderType::Error(NIL),
        }
        ShaderType::AbstractInt => match rhs{
            ShaderType::AbstractInt=>ShaderType::AbstractInt,
            ShaderType::Pod(x) if *x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_u32),
            ShaderType::Pod(x) if *x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_i32),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_u32=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_u32),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_u32),
            ShaderType::Pod(x) if *x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_u32),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_i32=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_i32),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_i32),
            ShaderType::Pod(x) if *x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_i32),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec2u=> match rhs{
            ShaderType::Pod(x) if *x == builtins.pod_vec2u=>ShaderType::Pod(builtins.pod_vec2u),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec3u=> match rhs{
            ShaderType::Pod(x) if *x == builtins.pod_vec3u=>ShaderType::Pod(builtins.pod_vec3u),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec4u=> match rhs{
            ShaderType::Pod(x) if *x == builtins.pod_vec4u=>ShaderType::Pod(builtins.pod_vec4u),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec2i=> match rhs{
            ShaderType::Pod(x) if *x == builtins.pod_vec2i=>ShaderType::Pod(builtins.pod_vec2i),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec3i=> match rhs{
            ShaderType::Pod(x) if *x == builtins.pod_vec3i=>ShaderType::Pod(builtins.pod_vec3i),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec4i=> match rhs{
            ShaderType::Pod(x) if *x == builtins.pod_vec4i=>ShaderType::Pod(builtins.pod_vec4i),
            _=>ShaderType::Error(NIL),
        }
        _=>ShaderType::Error(NIL),
    };
    if let ShaderType::Error(_) = r{
        err_no_wgsl_conversion_available!(trap);
    }
    r
}

pub fn type_table_logic(lhs: &ShaderType, rhs: &ShaderType, trap:ScriptTrap, builtins:&ScriptPodBuiltins )->ShaderType{
    let bool_ty = ShaderType::Pod(builtins.pod_bool);
    let r = match lhs{
        ShaderType::Pod(x) if *x == builtins.pod_bool => match rhs{
             ShaderType::Pod(y) if *y == builtins.pod_bool => bool_ty,
             _=>ShaderType::Error(NIL),
        }
        _=>ShaderType::Error(NIL),
    };
    if let ShaderType::Error(_) = r{
        err_no_wgsl_conversion_available!(trap);
    }
    r
}

pub fn type_table_eq(lhs: &ShaderType, rhs: &ShaderType, trap:ScriptTrap, builtins:&ScriptPodBuiltins )->ShaderType{
    let bool_ty = ShaderType::Pod(builtins.pod_bool);
    let vec2b_ty = ShaderType::Pod(builtins.pod_vec2b);
    let vec3b_ty = ShaderType::Pod(builtins.pod_vec3b);
    let vec4b_ty = ShaderType::Pod(builtins.pod_vec4b);

    let r = match lhs{
        ShaderType::AbstractFloat | ShaderType::AbstractInt => match rhs{
             ShaderType::AbstractFloat | ShaderType::AbstractInt => bool_ty,
             ShaderType::Pod(x) if *x == builtins.pod_f32 || *x == builtins.pod_f16 || *x == builtins.pod_u32 || *x == builtins.pod_i32 => bool_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_f32 => match rhs {
             ShaderType::AbstractFloat | ShaderType::AbstractInt => bool_ty,
             ShaderType::Pod(y) if *y == builtins.pod_f32 => bool_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_f16 => match rhs {
             ShaderType::AbstractFloat | ShaderType::AbstractInt => bool_ty,
             ShaderType::Pod(y) if *y == builtins.pod_f16 => bool_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_u32 => match rhs {
             ShaderType::AbstractFloat | ShaderType::AbstractInt => bool_ty,
             ShaderType::Pod(y) if *y == builtins.pod_u32 => bool_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_i32 => match rhs {
             ShaderType::AbstractFloat | ShaderType::AbstractInt => bool_ty,
             ShaderType::Pod(y) if *y == builtins.pod_i32 => bool_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_bool => match rhs {
             ShaderType::Pod(y) if *y == builtins.pod_bool => bool_ty,
             _=>ShaderType::Error(NIL),
        }
        // Vec2f
        ShaderType::Pod(x) if *x == builtins.pod_vec2f => match rhs {
             ShaderType::Pod(y) if *y == builtins.pod_vec2f => vec2b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec2h => match rhs {
             ShaderType::Pod(y) if *y == builtins.pod_vec2h => vec2b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec2u => match rhs {
             ShaderType::Pod(y) if *y == builtins.pod_vec2u => vec2b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec2i => match rhs {
             ShaderType::Pod(y) if *y == builtins.pod_vec2i => vec2b_ty,
             _=>ShaderType::Error(NIL),
        }
        // Vec3f
        ShaderType::Pod(x) if *x == builtins.pod_vec3f => match rhs {
             ShaderType::Pod(y) if *y == builtins.pod_vec3f => vec3b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec3h => match rhs {
             ShaderType::Pod(y) if *y == builtins.pod_vec3h => vec3b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec3u => match rhs {
             ShaderType::Pod(y) if *y == builtins.pod_vec3u => vec3b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec3i => match rhs {
             ShaderType::Pod(y) if *y == builtins.pod_vec3i => vec3b_ty,
             _=>ShaderType::Error(NIL),
        }
        // Vec4f
        ShaderType::Pod(x) if *x == builtins.pod_vec4f => match rhs {
             ShaderType::Pod(y) if *y == builtins.pod_vec4f => vec4b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec4h => match rhs {
             ShaderType::Pod(y) if *y == builtins.pod_vec4h => vec4b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec4u => match rhs {
             ShaderType::Pod(y) if *y == builtins.pod_vec4u => vec4b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if *x == builtins.pod_vec4i => match rhs {
             ShaderType::Pod(y) if *y == builtins.pod_vec4i => vec4b_ty,
             _=>ShaderType::Error(NIL),
        }
        _=>ShaderType::Error(NIL),
    };
    if let ShaderType::Error(_) = r{
        err_no_wgsl_conversion_available!(trap);
    }
    r
}

pub fn type_table_if_else(lhs: &ShaderType, rhs: &ShaderType, trap:ScriptTrap, builtins:&ScriptPodBuiltins )->ShaderType{
    let r = match lhs{
        ShaderType::AbstractFloat => match rhs{
             ShaderType::AbstractFloat => ShaderType::AbstractFloat,
             ShaderType::AbstractInt => ShaderType::AbstractFloat,
             ShaderType::Pod(x) if *x == builtins.pod_f32 || *x == builtins.pod_f16 => ShaderType::Pod(*x),
             _=>ShaderType::Error(NIL),
        }
        ShaderType::AbstractInt => match rhs{
             ShaderType::AbstractFloat => ShaderType::AbstractFloat,
             ShaderType::AbstractInt => ShaderType::AbstractInt,
             ShaderType::Pod(x) if *x == builtins.pod_f32 || *x == builtins.pod_f16 || *x == builtins.pod_i32 || *x == builtins.pod_u32 => ShaderType::Pod(*x),
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) => match rhs{
             ShaderType::AbstractFloat if *x == builtins.pod_f32 || *x == builtins.pod_f16 => ShaderType::Pod(*x),
             ShaderType::AbstractInt if *x == builtins.pod_f32 || *x == builtins.pod_f16 || *x == builtins.pod_i32 || *x == builtins.pod_u32 => ShaderType::Pod(*x),
             ShaderType::Pod(y) if x == y => ShaderType::Pod(*x),
             _=>ShaderType::Error(NIL),
        }
        _=>ShaderType::Error(NIL),
    };
    if let ShaderType::Error(_) = r{
        err_if_else_type_different!(trap);
    }
    r
}

pub fn type_table_elem_type(ty: &ScriptPodTy, _trap: ScriptTrap, builtins: &ScriptPodBuiltins) -> Option<ScriptPodType> {
    match ty {
        ScriptPodTy::FixedArray{ty, ..} => Some(ty.self_ref),
        ScriptPodTy::VariableArray{ty, ..} => Some(ty.self_ref),
        ScriptPodTy::Vec(v) => match v {
            ScriptPodVec::Vec2f | ScriptPodVec::Vec3f | ScriptPodVec::Vec4f => Some(builtins.pod_f32),
            ScriptPodVec::Vec2h | ScriptPodVec::Vec3h | ScriptPodVec::Vec4h => Some(builtins.pod_f16),
            ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u => Some(builtins.pod_u32),
            ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i => Some(builtins.pod_i32),
            ScriptPodVec::Vec2b | ScriptPodVec::Vec3b | ScriptPodVec::Vec4b => Some(builtins.pod_bool),
        },
        ScriptPodTy::Mat(m) => match m {
            ScriptPodMat::Mat2x2f | ScriptPodMat::Mat3x2f | ScriptPodMat::Mat4x2f => Some(builtins.pod_vec2f),
            ScriptPodMat::Mat2x3f | ScriptPodMat::Mat3x3f | ScriptPodMat::Mat4x3f => Some(builtins.pod_vec3f),
            ScriptPodMat::Mat2x4f | ScriptPodMat::Mat3x4f | ScriptPodMat::Mat4x4f => Some(builtins.pod_vec4f),
        },
        _ => None
    }
}
