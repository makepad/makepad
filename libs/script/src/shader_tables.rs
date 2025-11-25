use crate::value::NIL;
use crate::trap::ScriptTrap;
use crate::mod_pod::ScriptPodBuiltins;
use crate::shader::ShaderType;

pub fn type_table_float_arithmetic(lhs: ShaderType, rhs: ShaderType, trap:&ScriptTrap, builtins:&ScriptPodBuiltins )->ShaderType{
    let r = match lhs{
        ShaderType::AbstractFloat => match rhs{
            ShaderType::AbstractFloat=>ShaderType::AbstractFloat,
            ShaderType::AbstractInt=>ShaderType::AbstractFloat,
            ShaderType::Pod(x) if x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_f32),
            ShaderType::Pod(x) if x == builtins.pod_f16=>ShaderType::Pod(builtins.pod_f16),
            ShaderType::Pod(x) if x == builtins.pod_vec2f=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::Pod(x) if x == builtins.pod_vec3f=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::Pod(x) if x == builtins.pod_vec4f=>ShaderType::Pod(builtins.pod_vec4f),
            ShaderType::Pod(x) if x == builtins.pod_vec2h=>ShaderType::Pod(builtins.pod_vec2h),
            ShaderType::Pod(x) if x == builtins.pod_vec3h=>ShaderType::Pod(builtins.pod_vec3h),
            ShaderType::Pod(x) if x == builtins.pod_vec4h=>ShaderType::Pod(builtins.pod_vec4h),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::AbstractInt => match rhs{
            ShaderType::AbstractFloat=>ShaderType::AbstractFloat,
            ShaderType::AbstractInt=>ShaderType::AbstractInt,
            ShaderType::Pod(x) if x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_u32),
            ShaderType::Pod(x) if x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_i32),
            ShaderType::Pod(x) if x == builtins.pod_vec2f=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::Pod(x) if x == builtins.pod_vec3f=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::Pod(x) if x == builtins.pod_vec4f=>ShaderType::Pod(builtins.pod_vec4f),
            ShaderType::Pod(x) if x == builtins.pod_vec2h=>ShaderType::Pod(builtins.pod_vec2h),
            ShaderType::Pod(x) if x == builtins.pod_vec3h=>ShaderType::Pod(builtins.pod_vec3h),
            ShaderType::Pod(x) if x == builtins.pod_vec4h=>ShaderType::Pod(builtins.pod_vec4h),
            ShaderType::Pod(x) if x == builtins.pod_vec2u=>ShaderType::Pod(builtins.pod_vec2u),
            ShaderType::Pod(x) if x == builtins.pod_vec3u=>ShaderType::Pod(builtins.pod_vec3u),
            ShaderType::Pod(x) if x == builtins.pod_vec4u=>ShaderType::Pod(builtins.pod_vec4u),
            ShaderType::Pod(x) if x == builtins.pod_vec2i=>ShaderType::Pod(builtins.pod_vec2i),
            ShaderType::Pod(x) if x == builtins.pod_vec3i=>ShaderType::Pod(builtins.pod_vec3i),
            ShaderType::Pod(x) if x == builtins.pod_vec4i=>ShaderType::Pod(builtins.pod_vec4i),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_f32=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_f32),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_f32),
            ShaderType::Pod(x) if x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_f32),
            ShaderType::Pod(x) if x == builtins.pod_vec2f=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::Pod(x) if x == builtins.pod_vec3f=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::Pod(x) if x == builtins.pod_vec4f=>ShaderType::Pod(builtins.pod_vec4f),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_f16=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_f16),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_f16),
            ShaderType::Pod(x) if x == builtins.pod_f16=>ShaderType::Pod(builtins.pod_f16),
            ShaderType::Pod(x) if x == builtins.pod_vec2h=>ShaderType::Pod(builtins.pod_vec2h),
            ShaderType::Pod(x) if x == builtins.pod_vec3h=>ShaderType::Pod(builtins.pod_vec3h),
            ShaderType::Pod(x) if x == builtins.pod_vec4h=>ShaderType::Pod(builtins.pod_vec4h),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_u32=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_u32),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_u32),
            ShaderType::Pod(x) if x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_u32),
            ShaderType::Pod(x) if x == builtins.pod_vec2u=>ShaderType::Pod(builtins.pod_vec2u),
            ShaderType::Pod(x) if x == builtins.pod_vec3u=>ShaderType::Pod(builtins.pod_vec3u),
            ShaderType::Pod(x) if x == builtins.pod_vec4u=>ShaderType::Pod(builtins.pod_vec4u),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_i32=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_i32),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_i32),
            ShaderType::Pod(x) if x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_i32),
            ShaderType::Pod(x) if x == builtins.pod_vec2i=>ShaderType::Pod(builtins.pod_vec2i),
            ShaderType::Pod(x) if x == builtins.pod_vec3i=>ShaderType::Pod(builtins.pod_vec3i),
            ShaderType::Pod(x) if x == builtins.pod_vec4i=>ShaderType::Pod(builtins.pod_vec4i),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec2f=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::Pod(x) if x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_vec2f),
            ShaderType::Pod(x) if x == builtins.pod_vec2f=>ShaderType::Pod(builtins.pod_vec2f),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec3f=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::Pod(x) if x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_vec3f),
            ShaderType::Pod(x) if x == builtins.pod_vec3f=>ShaderType::Pod(builtins.pod_vec3f),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec4f=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_vec4f),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec4f),
            ShaderType::Pod(x) if x == builtins.pod_f32=>ShaderType::Pod(builtins.pod_vec4f),
            ShaderType::Pod(x) if x == builtins.pod_vec4f=>ShaderType::Pod(builtins.pod_vec4f),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec2h=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_vec2h),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec2h),
            ShaderType::Pod(x) if x == builtins.pod_f16=>ShaderType::Pod(builtins.pod_vec2h),
            ShaderType::Pod(x) if x == builtins.pod_vec2h=>ShaderType::Pod(builtins.pod_vec2h),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec3h=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_vec3h),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec3h),
            ShaderType::Pod(x) if x == builtins.pod_f16=>ShaderType::Pod(builtins.pod_vec3h),
            ShaderType::Pod(x) if x == builtins.pod_vec3h=>ShaderType::Pod(builtins.pod_vec3h),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec4h=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_vec4h),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec4h),
            ShaderType::Pod(x) if x == builtins.pod_f16=>ShaderType::Pod(builtins.pod_vec4h),
            ShaderType::Pod(x) if x == builtins.pod_vec4h=>ShaderType::Pod(builtins.pod_vec4h),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec2u=> match rhs{
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec2u),
            ShaderType::Pod(x) if x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_vec2u),
            ShaderType::Pod(x) if x == builtins.pod_vec2u=>ShaderType::Pod(builtins.pod_vec2u),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec3u=> match rhs{
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec3u),
            ShaderType::Pod(x) if x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_vec3u),
            ShaderType::Pod(x) if x == builtins.pod_vec3u=>ShaderType::Pod(builtins.pod_vec3u),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec4u=> match rhs{
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec4u),
            ShaderType::Pod(x) if x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_vec4u),
            ShaderType::Pod(x) if x == builtins.pod_vec4u=>ShaderType::Pod(builtins.pod_vec4u),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec2i=> match rhs{
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec2i),
            ShaderType::Pod(x) if x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_vec2i),
            ShaderType::Pod(x) if x == builtins.pod_vec2i=>ShaderType::Pod(builtins.pod_vec2i),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec3i=> match rhs{
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec3i),
            ShaderType::Pod(x) if x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_vec3i),
            ShaderType::Pod(x) if x == builtins.pod_vec3i=>ShaderType::Pod(builtins.pod_vec3i),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec4i=> match rhs{
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_vec4i),
            ShaderType::Pod(x) if x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_vec4i),
            ShaderType::Pod(x) if x == builtins.pod_vec4i=>ShaderType::Pod(builtins.pod_vec4i),
            _=>ShaderType::Error(NIL),
        }
        _=>ShaderType::Error(NIL),
    };
    if let ShaderType::Error(_) = r{
        trap.err_no_wgsl_conversion_available();
    }
    r
}
    
pub fn type_table_int_arithmetic(lhs: ShaderType, rhs: ShaderType, trap:&ScriptTrap, builtins:&ScriptPodBuiltins )->ShaderType{
    let r = match lhs{
        ShaderType::AbstractFloat => match rhs{
            _=>ShaderType::Error(NIL),
        }
        ShaderType::AbstractInt => match rhs{
            ShaderType::AbstractInt=>ShaderType::AbstractInt,
            ShaderType::Pod(x) if x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_u32),
            ShaderType::Pod(x) if x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_i32),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_u32=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_u32),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_u32),
            ShaderType::Pod(x) if x == builtins.pod_u32=>ShaderType::Pod(builtins.pod_u32),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_i32=> match rhs{
            ShaderType::AbstractFloat=>ShaderType::Pod(builtins.pod_i32),
            ShaderType::AbstractInt=>ShaderType::Pod(builtins.pod_i32),
            ShaderType::Pod(x) if x == builtins.pod_i32=>ShaderType::Pod(builtins.pod_i32),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec2u=> match rhs{
            ShaderType::Pod(x) if x == builtins.pod_vec2u=>ShaderType::Pod(builtins.pod_vec2u),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec3u=> match rhs{
            ShaderType::Pod(x) if x == builtins.pod_vec3u=>ShaderType::Pod(builtins.pod_vec3u),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec4u=> match rhs{
            ShaderType::Pod(x) if x == builtins.pod_vec4u=>ShaderType::Pod(builtins.pod_vec4u),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec2i=> match rhs{
            ShaderType::Pod(x) if x == builtins.pod_vec2i=>ShaderType::Pod(builtins.pod_vec2i),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec3i=> match rhs{
            ShaderType::Pod(x) if x == builtins.pod_vec3i=>ShaderType::Pod(builtins.pod_vec3i),
            _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec4i=> match rhs{
            ShaderType::Pod(x) if x == builtins.pod_vec4i=>ShaderType::Pod(builtins.pod_vec4i),
            _=>ShaderType::Error(NIL),
        }
        _=>ShaderType::Error(NIL),
    };
    if let ShaderType::Error(_) = r{
        trap.err_no_wgsl_conversion_available();
    }
    r
}

pub fn type_table_logic(lhs: ShaderType, rhs: ShaderType, trap:&ScriptTrap, builtins:&ScriptPodBuiltins )->ShaderType{
    let bool_ty = ShaderType::Pod(builtins.pod_bool);
    let r = match lhs{
        ShaderType::Pod(x) if x == builtins.pod_bool => match rhs{
             ShaderType::Pod(y) if y == builtins.pod_bool => bool_ty,
             _=>ShaderType::Error(NIL),
        }
        _=>ShaderType::Error(NIL),
    };
    if let ShaderType::Error(_) = r{
        trap.err_no_wgsl_conversion_available();
    }
    r
}

pub fn type_table_eq(lhs: ShaderType, rhs: ShaderType, trap:&ScriptTrap, builtins:&ScriptPodBuiltins )->ShaderType{
    let bool_ty = ShaderType::Pod(builtins.pod_bool);
    let vec2b_ty = ShaderType::Pod(builtins.pod_vec2b);
    let vec3b_ty = ShaderType::Pod(builtins.pod_vec3b);
    let vec4b_ty = ShaderType::Pod(builtins.pod_vec4b);

    let r = match lhs{
        ShaderType::AbstractFloat | ShaderType::AbstractInt => match rhs{
             ShaderType::AbstractFloat | ShaderType::AbstractInt => bool_ty,
             ShaderType::Pod(x) if x == builtins.pod_f32 || x == builtins.pod_f16 || x == builtins.pod_u32 || x == builtins.pod_i32 => bool_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_f32 => match rhs {
             ShaderType::AbstractFloat | ShaderType::AbstractInt => bool_ty,
             ShaderType::Pod(y) if y == builtins.pod_f32 => bool_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_f16 => match rhs {
             ShaderType::AbstractFloat | ShaderType::AbstractInt => bool_ty,
             ShaderType::Pod(y) if y == builtins.pod_f16 => bool_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_u32 => match rhs {
             ShaderType::AbstractFloat | ShaderType::AbstractInt => bool_ty,
             ShaderType::Pod(y) if y == builtins.pod_u32 => bool_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_i32 => match rhs {
             ShaderType::AbstractFloat | ShaderType::AbstractInt => bool_ty,
             ShaderType::Pod(y) if y == builtins.pod_i32 => bool_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_bool => match rhs {
             ShaderType::Pod(y) if y == builtins.pod_bool => bool_ty,
             _=>ShaderType::Error(NIL),
        }
        // Vec2
        ShaderType::Pod(x) if x == builtins.pod_vec2f => match rhs {
             ShaderType::Pod(y) if y == builtins.pod_vec2f => vec2b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec2h => match rhs {
             ShaderType::Pod(y) if y == builtins.pod_vec2h => vec2b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec2u => match rhs {
             ShaderType::Pod(y) if y == builtins.pod_vec2u => vec2b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec2i => match rhs {
             ShaderType::Pod(y) if y == builtins.pod_vec2i => vec2b_ty,
             _=>ShaderType::Error(NIL),
        }
        // Vec3
        ShaderType::Pod(x) if x == builtins.pod_vec3f => match rhs {
             ShaderType::Pod(y) if y == builtins.pod_vec3f => vec3b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec3h => match rhs {
             ShaderType::Pod(y) if y == builtins.pod_vec3h => vec3b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec3u => match rhs {
             ShaderType::Pod(y) if y == builtins.pod_vec3u => vec3b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec3i => match rhs {
             ShaderType::Pod(y) if y == builtins.pod_vec3i => vec3b_ty,
             _=>ShaderType::Error(NIL),
        }
        // Vec4
        ShaderType::Pod(x) if x == builtins.pod_vec4f => match rhs {
             ShaderType::Pod(y) if y == builtins.pod_vec4f => vec4b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec4h => match rhs {
             ShaderType::Pod(y) if y == builtins.pod_vec4h => vec4b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec4u => match rhs {
             ShaderType::Pod(y) if y == builtins.pod_vec4u => vec4b_ty,
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) if x == builtins.pod_vec4i => match rhs {
             ShaderType::Pod(y) if y == builtins.pod_vec4i => vec4b_ty,
             _=>ShaderType::Error(NIL),
        }
        _=>ShaderType::Error(NIL),
    };
    if let ShaderType::Error(_) = r{
        trap.err_no_wgsl_conversion_available();
    }
    r
}

pub fn type_table_if_else(lhs: ShaderType, rhs: ShaderType, trap:&ScriptTrap, builtins:&ScriptPodBuiltins )->ShaderType{
    let r = match lhs{
        ShaderType::AbstractFloat => match rhs{
             ShaderType::AbstractFloat => ShaderType::AbstractFloat,
             ShaderType::AbstractInt => ShaderType::AbstractFloat,
             ShaderType::Pod(x) if x == builtins.pod_f32 || x == builtins.pod_f16 => ShaderType::Pod(x),
             _=>ShaderType::Error(NIL),
        }
        ShaderType::AbstractInt => match rhs{
             ShaderType::AbstractFloat => ShaderType::AbstractFloat,
             ShaderType::AbstractInt => ShaderType::AbstractInt,
             ShaderType::Pod(x) if x == builtins.pod_f32 || x == builtins.pod_f16 || x == builtins.pod_i32 || x == builtins.pod_u32 => ShaderType::Pod(x),
             _=>ShaderType::Error(NIL),
        }
        ShaderType::Pod(x) => match rhs{
             ShaderType::AbstractFloat if x == builtins.pod_f32 || x == builtins.pod_f16 => ShaderType::Pod(x),
             ShaderType::AbstractInt if x == builtins.pod_f32 || x == builtins.pod_f16 || x == builtins.pod_i32 || x == builtins.pod_u32 => ShaderType::Pod(x),
             ShaderType::Pod(y) if x == y => ShaderType::Pod(x),
             _=>ShaderType::Error(NIL),
        }
        _=>ShaderType::Error(NIL),
    };
    if let ShaderType::Error(_) = r{
        trap.err_if_else_type_different();
    }
    r
}
