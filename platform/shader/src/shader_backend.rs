use makepad_relax_api::relax_api;
use crate::shader_ids::*;
use crate::shader_types::ShaderTypes;
use crate::shader_schema::*;

use makepad_live_id::*;
use std::fmt::Write;

#[relax_api(plain)]
#[repr(u8)]
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub enum ShaderBackend {
    #[default]
    Metal,
    Wgsl,
    Hlsl,
    Glsl,
    Rust,
}

#[derive(Debug, Clone)]
pub enum ShaderIoPrefix {
    Prefix(&'static str),
    Full(&'static str),
    FullOwned(String),
}

/// Why a shader IO declaration could not be lowered.
///
/// Previously eight bare `panic!()`s. A shader that trips one is usually not
/// exotic — the commonest by far is reading a geometry attribute from the
/// fragment stage, which every GPU forbids and which the message now says
/// outright instead of aborting with no text at all.

impl ShaderBackend {

    pub fn map_io_name(&self, id: LiveId) -> String {
        match self {
            Self::Hlsl => format!("io_{}", id),
            Self::Rust => format!("{}", id),
            _ => format!("{}", id),
        }
    }

    pub fn map_field_name(&self, id: LiveId) -> String {
        self.map_field_name_typed(id, true)
    }

    /// Map a field name, with `is_vec_type` indicating whether the parent type is a vec
    /// (where swizzle transformations apply).
    pub fn map_field_name_typed(&self, id: LiveId, is_vec_type: bool) -> String {
        match self {
            Self::Hlsl => {
                let id_str = format!("{}", id);
                let len = id_str.len();
                let is_swizzle = (1..=4).contains(&len)
                    && id_str.bytes().all(|c| {
                        matches!(c, b'x' | b'y' | b'z' | b'w' | b'r' | b'g' | b'b' | b'a')
                    });
                if is_swizzle {
                    id_str
                } else {
                    format!("f_{}", id_str)
                }
            }
            Self::Rust => {
                let id_str = format!("{}", id);
                if !is_vec_type {
                    return id_str;
                }
                let len = id_str.len();
                let is_swizzle_char =
                    |c: u8| matches!(c, b'x' | b'y' | b'z' | b'w' | b'r' | b'g' | b'b' | b'a');
                let all_swizzle = !id_str.is_empty() && id_str.bytes().all(is_swizzle_char);
                if all_swizzle && len >= 2 {
                    // Multi-component swizzles become method calls: .xy() not .xy
                    // Map rgba to xyzw
                    let mapped: String = id_str
                        .chars()
                        .map(|c| match c {
                            'r' => 'x',
                            'g' => 'y',
                            'b' => 'z',
                            'a' => 'w',
                            other => other,
                        })
                        .collect();
                    format!("{}()", mapped)
                } else if all_swizzle && len == 1 {
                    // Single-char field access: map rgba to xyzw
                    match id_str.as_str() {
                        "r" => "x".to_string(),
                        "g" => "y".to_string(),
                        "b" => "z".to_string(),
                        "a" => "w".to_string(),
                        other => other.to_string(),
                    }
                } else {
                    id_str
                }
            }
            _ => format!("{}", id),
        }
    }

    /// Map pod type names to packed versions for Metal instance buffers.
    /// Packed types match CPU-side repr(C) struct alignment.
    pub fn map_packed_pod_name(&self, name_in: LiveId) -> LiveId {
        match self {
            Self::Metal => {
                match name_in {
                    id!(f32) => id!(float),
                    id!(f16) => id!(half),
                    id!(u32) => id!(uint),
                    id!(i32) => id!(int),
                    id!(vec2f) => id!(packed_float2),
                    id!(vec3f) => id!(packed_float3),
                    id!(vec4f) => id!(packed_float4),
                    id!(vec2h) => id!(packed_half2),
                    id!(vec3h) => id!(packed_half3),
                    id!(vec4h) => id!(packed_half4),
                    id!(vec2u) => id!(packed_uint2),
                    id!(vec3u) => id!(packed_uint3),
                    id!(vec4u) => id!(packed_uint4),
                    id!(vec2i) => id!(packed_int2),
                    id!(vec3i) => id!(packed_int3),
                    id!(vec4i) => id!(packed_int4),
                    id!(vec2b) => id!(packed_bool2),
                    id!(vec3b) => id!(packed_bool3),
                    id!(vec4b) => id!(packed_bool4),
                    // Metal exposes packed vector types, but not packed matrix aliases.
                    // Keep matrix names plain and rely on their native column layout.
                    id!(mat2x2f) => id!(float2x2),
                    id!(mat2x3f) => id!(float2x3),
                    id!(mat2x4f) => id!(float2x4),
                    id!(mat3x2f) => id!(float3x2),
                    id!(mat3x3f) => id!(float3x3),
                    id!(mat3x4f) => id!(float3x4),
                    id!(mat4x2f) => id!(float4x2),
                    id!(mat4x3f) => id!(float4x3),
                    id!(mat4x4f) => id!(float4x4),
                    id!(f16x2) => id!(packed_half2),
                    id!(f16x4) => id!(packed_half4),
                    id!(u16x2) => id!(packed_ushort2),
                    id!(i16x2) => id!(packed_short2),
                    id!(unorm16x2) => id!(packed_ushort2),
                    id!(snorm16x2) => id!(packed_short2),
                    id!(unorm8x4) => id!(uchar4),
                    id!(snorm8x4) => id!(char4),
                    x => x,
                }
            }
            _ => self.map_pod_name(name_in),
        }
    }

    pub fn map_pod_name(&self, name_in: LiveId) -> LiveId {
        match self {
            Self::Metal | Self::Hlsl => match name_in {
                id!(f32) => id!(float),
                id!(f16) => id!(half),
                id!(u32) => id!(uint),
                id!(i32) => id!(int),
                id!(vec2f) => id!(float2),
                id!(vec3f) => id!(float3),
                id!(vec4f) => id!(float4),
                id!(vec2h) => id!(half2),
                id!(vec3h) => id!(half3),
                id!(vec4h) => id!(half4),
                id!(vec2u) => id!(uint2),
                id!(vec3u) => id!(uint3),
                id!(vec4u) => id!(uint4),
                id!(vec2i) => id!(int2),
                id!(vec3i) => id!(int3),
                id!(vec4i) => id!(int4),
                id!(vec2b) => id!(bool2),
                id!(vec3b) => id!(bool3),
                id!(vec4b) => id!(bool4),
                id!(mat2x2f) => id!(float2x2),
                id!(mat2x3f) => id!(float2x3),
                id!(mat2x4f) => id!(float2x4),
                id!(mat3x2f) => id!(float3x2),
                id!(mat3x3f) => id!(float3x3),
                id!(mat3x4f) => id!(float3x4),
                id!(mat4x2f) => id!(float4x2),
                id!(mat4x3f) => id!(float4x3),
                id!(mat4x4f) => id!(float4x4),
                id!(atomic_u32) => id!(atomic_uint),
                id!(atomic_i32) => id!(atomic_int),
                x => x,
            },
            Self::Glsl => {
                match name_in {
                    id!(f32) => id!(float),
                    id!(f16) => id!(float), // no half in standard GLSL 300 es, could use mediump float
                    id!(u32) => id!(uint),
                    id!(i32) => id!(int),
                    id!(vec2f) => id!(vec2),
                    id!(vec3f) => id!(vec3),
                    id!(vec4f) => id!(vec4),
                    id!(vec2h) => id!(vec2),
                    id!(vec3h) => id!(vec3),
                    id!(vec4h) => id!(vec4),
                    id!(vec2u) => id!(uvec2),
                    id!(vec3u) => id!(uvec3),
                    id!(vec4u) => id!(uvec4),
                    id!(vec2i) => id!(ivec2),
                    id!(vec3i) => id!(ivec3),
                    id!(vec4i) => id!(ivec4),
                    id!(vec2b) => id!(bvec2),
                    id!(vec3b) => id!(bvec3),
                    id!(vec4b) => id!(bvec4),
                    id!(mat2x2f) => id!(mat2),
                    id!(mat3x3f) => id!(mat3),
                    id!(mat4x4f) => id!(mat4),
                    // TODO more matrices
                    x => x,
                }
            }
            Self::Wgsl => match name_in {
                // Inline uniform structs can inherit their field's name.
                // `shared` is legal Splash, but reserved by WGSL.
                id!(shared) => id!(_mp_type_shared),
                name => name,
            },
            Self::Rust => name_in,
        }
    }

    pub fn pod_type_name_from_ty(&self, types: &impl ShaderTypes, ty: ShaderTypeId, out: &mut String) {
        let pod_ty = types.type_data(ty);
        let inline = ShaderTypeInline {
            id: ty,
            data: pod_ty.clone(),
        };
        self.pod_type_name(&inline, out);
    }

    pub fn pod_type_name(&self, ty: &ShaderTypeInline, out: &mut String) {
        match &ty.data.ty {
            ShaderTypeKind::F32 => write!(out, "{}", self.map_pod_name(id!(f32)))
                .ok()
                .unwrap_or(()),
            ShaderTypeKind::F16 => write!(out, "{}", self.map_pod_name(id!(f16)))
                .ok()
                .unwrap_or(()),
            ShaderTypeKind::U32 => write!(out, "{}", self.map_pod_name(id!(u32)))
                .ok()
                .unwrap_or(()),
            ShaderTypeKind::I32 => write!(out, "{}", self.map_pod_name(id!(i32)))
                .ok()
                .unwrap_or(()),
            ShaderTypeKind::Bool => write!(out, "{}", self.map_pod_name(id!(bool)))
                .ok()
                .unwrap_or(()),
            ShaderTypeKind::AtomicU32 => write!(out, "atomic<{}>", self.map_pod_name(id!(u32)))
                .ok()
                .unwrap_or(()),
            ShaderTypeKind::AtomicI32 => write!(out, "atomic<{}>", self.map_pod_name(id!(i32)))
                .ok()
                .unwrap_or(()),
            ShaderTypeKind::Vec(v) => write!(out, "{}", self.map_pod_name(v.name()))
                .ok()
                .unwrap_or(()),
            ShaderTypeKind::Mat(m) => write!(out, "{}", self.map_pod_name(m.name()))
                .ok()
                .unwrap_or(()),
            ShaderTypeKind::Struct { .. } => {
                let name = ty.data.name.unwrap();
                let name = self.map_pod_name(name);
                write!(out, "{}", name).ok().unwrap_or(());
            }
            ShaderTypeKind::FixedArray { ty: inner, len, .. } => {
                out.push_str("array<");
                self.pod_type_name(inner, out);
                write!(out, ", {}>", len).ok();
            }
            ShaderTypeKind::VariableArray { ty: inner, .. } => {
                out.push_str("array<");
                self.pod_type_name(inner, out);
                out.push_str(">");
            }
            // Compact fetch formats are logical vec2f / vec4f in shader code.
            ShaderTypeKind::Packed(p) => {
                let logical = if p.is_vec4() {
                    id!(vec4f)
                } else {
                    id!(vec2f)
                };
                write!(out, "{}", self.map_pod_name(logical))
                    .ok()
                    .unwrap_or(());
            }
            _ => out.push_str("unknown"),
        }
    }
}
