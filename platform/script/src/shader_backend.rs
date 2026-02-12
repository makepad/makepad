use crate::heap::*;
use crate::mod_shader::*;
use crate::pod::*;
use crate::shader::ShaderIoKind;
use crate::shader::ShaderMode;
use crate::shader::ShaderSamplerOptions;
use crate::shader::TextureType;
use crate::value::*;
use makepad_live_id::*;
use std::collections::BTreeSet;
use std::fmt::Write;

#[derive(Default, Debug)]
pub enum ShaderBackend {
    #[default]
    Metal,
    Wgsl,
    Hlsl,
    Glsl,
}

#[derive(Debug, Clone)]
pub enum ShaderIoPrefix {
    Prefix(&'static str),
    Full(&'static str),
    FullOwned(String),
}

impl ShaderBackend {
    pub fn get_shader_io_kind_and_prefix(
        &self,
        mode: ShaderMode,
        io_type: ShaderIoType,
    ) -> (ShaderIoKind, ShaderIoPrefix) {
        match self {
            Self::Metal => {
                match mode {
                    ShaderMode::Vertex => match io_type {
                        SHADER_IO_RUST_INSTANCE => (
                            ShaderIoKind::RustInstance,
                            ShaderIoPrefix::Prefix("_io.i[_iov.iid]."),
                        ),
                        SHADER_IO_DYN_INSTANCE => (
                            ShaderIoKind::DynInstance,
                            ShaderIoPrefix::Prefix("_io.i[_iov.iid]."),
                        ),
                        SHADER_IO_DYN_UNIFORM => {
                            (ShaderIoKind::Uniform, ShaderIoPrefix::Prefix("_io.u->"))
                        }
                        SHADER_IO_UNIFORM_BUFFER => (
                            ShaderIoKind::UniformBuffer,
                            ShaderIoPrefix::Prefix("_io.u_"),
                        ),
                        SHADER_IO_VARYING => {
                            (ShaderIoKind::Varying, ShaderIoPrefix::Prefix("_iov.v->"))
                        }
                        SHADER_IO_VERTEX_POSITION => (
                            ShaderIoKind::VertexPosition,
                            ShaderIoPrefix::Full("_iov.v->_position"),
                        ),
                        SHADER_IO_VERTEX_BUFFER => (
                            ShaderIoKind::VertexBuffer,
                            ShaderIoPrefix::Prefix("_io.vb[_iov.vid]."),
                        ),
                        SHADER_IO_FRAGMENT_OUTPUT_0 => {
                            (ShaderIoKind::Varying, ShaderIoPrefix::Prefix(""))
                        }
                        SHADER_IO_TEXTURE_1D => (
                            ShaderIoKind::Texture(TextureType::Texture1d),
                            ShaderIoPrefix::Prefix("_io."),
                        ),
                        SHADER_IO_TEXTURE_1D_ARRAY => (
                            ShaderIoKind::Texture(TextureType::Texture1dArray),
                            ShaderIoPrefix::Prefix("_io."),
                        ),
                        SHADER_IO_TEXTURE_2D => (
                            ShaderIoKind::Texture(TextureType::Texture2d),
                            ShaderIoPrefix::Prefix("_io."),
                        ),
                        SHADER_IO_TEXTURE_2D_ARRAY => (
                            ShaderIoKind::Texture(TextureType::Texture2dArray),
                            ShaderIoPrefix::Prefix("_io."),
                        ),
                        SHADER_IO_TEXTURE_3D => (
                            ShaderIoKind::Texture(TextureType::Texture3d),
                            ShaderIoPrefix::Prefix("_io."),
                        ),
                        SHADER_IO_TEXTURE_3D_ARRAY => (
                            ShaderIoKind::Texture(TextureType::Texture3dArray),
                            ShaderIoPrefix::Prefix("_io."),
                        ),
                        SHADER_IO_TEXTURE_CUBE => (
                            ShaderIoKind::Texture(TextureType::TextureCube),
                            ShaderIoPrefix::Prefix("_io."),
                        ),
                        SHADER_IO_TEXTURE_CUBE_ARRAY => (
                            ShaderIoKind::Texture(TextureType::TextureCubeArray),
                            ShaderIoPrefix::Prefix("_io."),
                        ),
                        SHADER_IO_TEXTURE_DEPTH => (
                            ShaderIoKind::Texture(TextureType::TextureDepth),
                            ShaderIoPrefix::Prefix("_io."),
                        ),
                        SHADER_IO_TEXTURE_DEPTH_ARRAY => (
                            ShaderIoKind::Texture(TextureType::TextureDepthArray),
                            ShaderIoPrefix::Prefix("_io."),
                        ),
                        SHADER_IO_SAMPLER => (
                            ShaderIoKind::Sampler(ShaderSamplerOptions::default()),
                            ShaderIoPrefix::Prefix("_io."),
                        ),
                        SHADER_IO_SCOPE_UNIFORM => (
                            ShaderIoKind::ScopeUniform,
                            ShaderIoPrefix::Prefix("_io.su->"),
                        ),

                        _ => panic!(),
                    },
                    ShaderMode::Fragment => {
                        // Check for fragment output range first
                        if io_type.0 >= SHADER_IO_FRAGMENT_OUTPUT_0.0
                            && io_type.0 <= SHADER_IO_FRAGMENT_OUTPUT_MAX.0
                        {
                            let index = io_type.0 - SHADER_IO_FRAGMENT_OUTPUT_0.0;
                            return (
                                ShaderIoKind::FragmentOutput(index as u8),
                                ShaderIoPrefix::FullOwned(format!("_iof.fb->fb{}", index)),
                            );
                        }
                        match io_type {
                            SHADER_IO_RUST_INSTANCE => (
                                ShaderIoKind::RustInstance,
                                ShaderIoPrefix::Prefix("_io.i[_iof.v->_iid]."),
                            ),
                            SHADER_IO_DYN_INSTANCE => (
                                ShaderIoKind::DynInstance,
                                ShaderIoPrefix::Prefix("_io.i[_iof.v->_iid]."),
                            ),
                            SHADER_IO_DYN_UNIFORM => {
                                (ShaderIoKind::Uniform, ShaderIoPrefix::Prefix("_io.u->"))
                            }
                            SHADER_IO_UNIFORM_BUFFER => (
                                ShaderIoKind::UniformBuffer,
                                ShaderIoPrefix::Prefix("_io.u_"),
                            ),
                            SHADER_IO_VARYING => {
                                (ShaderIoKind::Varying, ShaderIoPrefix::Prefix("_iof.v->"))
                            }
                            SHADER_IO_VERTEX_POSITION => (
                                ShaderIoKind::VertexPosition,
                                ShaderIoPrefix::Full("_iof.v->_position"),
                            ),
                            SHADER_IO_TEXTURE_1D => (
                                ShaderIoKind::Texture(TextureType::Texture1d),
                                ShaderIoPrefix::Prefix("_io."),
                            ),
                            SHADER_IO_TEXTURE_1D_ARRAY => (
                                ShaderIoKind::Texture(TextureType::Texture1dArray),
                                ShaderIoPrefix::Prefix("_io."),
                            ),
                            SHADER_IO_TEXTURE_2D => (
                                ShaderIoKind::Texture(TextureType::Texture2d),
                                ShaderIoPrefix::Prefix("_io."),
                            ),
                            SHADER_IO_TEXTURE_2D_ARRAY => (
                                ShaderIoKind::Texture(TextureType::Texture2dArray),
                                ShaderIoPrefix::Prefix("_io."),
                            ),
                            SHADER_IO_TEXTURE_3D => (
                                ShaderIoKind::Texture(TextureType::Texture3d),
                                ShaderIoPrefix::Prefix("_io."),
                            ),
                            SHADER_IO_TEXTURE_3D_ARRAY => (
                                ShaderIoKind::Texture(TextureType::Texture3dArray),
                                ShaderIoPrefix::Prefix("_io."),
                            ),
                            SHADER_IO_TEXTURE_CUBE => (
                                ShaderIoKind::Texture(TextureType::TextureCube),
                                ShaderIoPrefix::Prefix("_io."),
                            ),
                            SHADER_IO_TEXTURE_CUBE_ARRAY => (
                                ShaderIoKind::Texture(TextureType::TextureCubeArray),
                                ShaderIoPrefix::Prefix("_io."),
                            ),
                            SHADER_IO_TEXTURE_DEPTH => (
                                ShaderIoKind::Texture(TextureType::TextureDepth),
                                ShaderIoPrefix::Prefix("_io."),
                            ),
                            SHADER_IO_TEXTURE_DEPTH_ARRAY => (
                                ShaderIoKind::Texture(TextureType::TextureDepthArray),
                                ShaderIoPrefix::Prefix("_io."),
                            ),
                            SHADER_IO_SAMPLER => (
                                ShaderIoKind::Sampler(ShaderSamplerOptions::default()),
                                ShaderIoPrefix::Prefix("_io."),
                            ),
                            SHADER_IO_SCOPE_UNIFORM => (
                                ShaderIoKind::ScopeUniform,
                                ShaderIoPrefix::Prefix("_io.su->"),
                            ),
                            _ => panic!(),
                        }
                    }
                    _ => panic!(),
                }
            }
            Self::Hlsl => {
                match mode {
                    ShaderMode::Vertex => {
                        // Check for fragment output range first
                        if io_type.0 >= SHADER_IO_FRAGMENT_OUTPUT_0.0
                            && io_type.0 <= SHADER_IO_FRAGMENT_OUTPUT_MAX.0
                        {
                            let index = io_type.0 - SHADER_IO_FRAGMENT_OUTPUT_0.0;
                            return (
                                ShaderIoKind::FragmentOutput(index as u8),
                                ShaderIoPrefix::FullOwned(format!("_iofb.fb{}", index)),
                            );
                        }
                        match io_type {
                            SHADER_IO_RUST_INSTANCE => (
                                ShaderIoKind::RustInstance,
                                ShaderIoPrefix::Prefix("input.i_"),
                            ),
                            SHADER_IO_DYN_INSTANCE => (
                                ShaderIoKind::DynInstance,
                                ShaderIoPrefix::Prefix("input.i_"),
                            ),
                            SHADER_IO_DYN_UNIFORM => {
                                (ShaderIoKind::Uniform, ShaderIoPrefix::Prefix("u_"))
                            }
                            SHADER_IO_UNIFORM_BUFFER => {
                                (ShaderIoKind::UniformBuffer, ShaderIoPrefix::Prefix("u_"))
                            }
                            SHADER_IO_VARYING => {
                                (ShaderIoKind::Varying, ShaderIoPrefix::Prefix("_iov.v->"))
                            }
                            SHADER_IO_VERTEX_POSITION => (
                                ShaderIoKind::VertexPosition,
                                ShaderIoPrefix::Full("_iov.v->_position"),
                            ),
                            SHADER_IO_VERTEX_BUFFER => (
                                ShaderIoKind::VertexBuffer,
                                ShaderIoPrefix::Prefix("input.vb_"),
                            ),
                            SHADER_IO_TEXTURE_1D => (
                                ShaderIoKind::Texture(TextureType::Texture1d),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_1D_ARRAY => (
                                ShaderIoKind::Texture(TextureType::Texture1dArray),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_2D => (
                                ShaderIoKind::Texture(TextureType::Texture2d),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_2D_ARRAY => (
                                ShaderIoKind::Texture(TextureType::Texture2dArray),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_3D => (
                                ShaderIoKind::Texture(TextureType::Texture3d),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_3D_ARRAY => (
                                ShaderIoKind::Texture(TextureType::Texture3dArray),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_CUBE => (
                                ShaderIoKind::Texture(TextureType::TextureCube),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_CUBE_ARRAY => (
                                ShaderIoKind::Texture(TextureType::TextureCubeArray),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_DEPTH => (
                                ShaderIoKind::Texture(TextureType::TextureDepth),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_DEPTH_ARRAY => (
                                ShaderIoKind::Texture(TextureType::TextureDepthArray),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_SAMPLER => (
                                ShaderIoKind::Sampler(ShaderSamplerOptions::default()),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_SCOPE_UNIFORM => {
                                (ShaderIoKind::ScopeUniform, ShaderIoPrefix::Prefix("su_"))
                            }
                            _ => panic!(),
                        }
                    }
                    ShaderMode::Fragment => {
                        // Check for fragment output range first
                        if io_type.0 >= SHADER_IO_FRAGMENT_OUTPUT_0.0
                            && io_type.0 <= SHADER_IO_FRAGMENT_OUTPUT_MAX.0
                        {
                            let index = io_type.0 - SHADER_IO_FRAGMENT_OUTPUT_0.0;
                            return (
                                ShaderIoKind::FragmentOutput(index as u8),
                                ShaderIoPrefix::FullOwned(format!("_iofb.fb{}", index)),
                            );
                        }
                        match io_type {
                            SHADER_IO_RUST_INSTANCE => (
                                ShaderIoKind::RustInstance,
                                ShaderIoPrefix::Prefix("_iof.v."),
                            ),
                            SHADER_IO_DYN_INSTANCE => {
                                (ShaderIoKind::DynInstance, ShaderIoPrefix::Prefix("_iof.v."))
                            }
                            SHADER_IO_DYN_UNIFORM => {
                                (ShaderIoKind::Uniform, ShaderIoPrefix::Prefix("u_"))
                            }
                            SHADER_IO_UNIFORM_BUFFER => {
                                (ShaderIoKind::UniformBuffer, ShaderIoPrefix::Prefix("u_"))
                            }
                            SHADER_IO_VARYING => {
                                (ShaderIoKind::Varying, ShaderIoPrefix::Prefix("_iof.v."))
                            }
                            SHADER_IO_VERTEX_POSITION => (
                                ShaderIoKind::VertexPosition,
                                ShaderIoPrefix::Full("_iof.v._position"),
                            ),
                            SHADER_IO_TEXTURE_1D => (
                                ShaderIoKind::Texture(TextureType::Texture1d),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_1D_ARRAY => (
                                ShaderIoKind::Texture(TextureType::Texture1dArray),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_2D => (
                                ShaderIoKind::Texture(TextureType::Texture2d),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_2D_ARRAY => (
                                ShaderIoKind::Texture(TextureType::Texture2dArray),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_3D => (
                                ShaderIoKind::Texture(TextureType::Texture3d),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_3D_ARRAY => (
                                ShaderIoKind::Texture(TextureType::Texture3dArray),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_CUBE => (
                                ShaderIoKind::Texture(TextureType::TextureCube),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_CUBE_ARRAY => (
                                ShaderIoKind::Texture(TextureType::TextureCubeArray),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_DEPTH => (
                                ShaderIoKind::Texture(TextureType::TextureDepth),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_TEXTURE_DEPTH_ARRAY => (
                                ShaderIoKind::Texture(TextureType::TextureDepthArray),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_SAMPLER => (
                                ShaderIoKind::Sampler(ShaderSamplerOptions::default()),
                                ShaderIoPrefix::Prefix(""),
                            ),
                            SHADER_IO_SCOPE_UNIFORM => {
                                (ShaderIoKind::ScopeUniform, ShaderIoPrefix::Prefix("su_"))
                            }
                            _ => panic!(),
                        }
                    }
                    _ => panic!(),
                }
            }
            Self::Glsl | Self::Wgsl => {
                // Check for fragment output range first
                if io_type.0 >= SHADER_IO_FRAGMENT_OUTPUT_0.0
                    && io_type.0 <= SHADER_IO_FRAGMENT_OUTPUT_MAX.0
                {
                    let index = io_type.0 - SHADER_IO_FRAGMENT_OUTPUT_0.0;
                    return (
                        ShaderIoKind::FragmentOutput(index as u8),
                        ShaderIoPrefix::FullOwned(format!("frag_fb{}", index)),
                    );
                }
                match io_type {
                    SHADER_IO_RUST_INSTANCE => (
                        ShaderIoKind::RustInstance,
                        ShaderIoPrefix::Prefix("rustinst_"),
                    ),
                    SHADER_IO_DYN_INSTANCE => (
                        ShaderIoKind::DynInstance,
                        ShaderIoPrefix::Prefix("dyninst_"),
                    ),
                    SHADER_IO_DYN_UNIFORM => {
                        (ShaderIoKind::Uniform, ShaderIoPrefix::Prefix("uni_"))
                    }
                    SHADER_IO_UNIFORM_BUFFER => (
                        ShaderIoKind::UniformBuffer,
                        ShaderIoPrefix::Prefix("unibuf_"),
                    ),
                    SHADER_IO_VARYING => (ShaderIoKind::Varying, ShaderIoPrefix::Prefix("var_")),
                    SHADER_IO_VERTEX_POSITION => (
                        ShaderIoKind::VertexPosition,
                        ShaderIoPrefix::Prefix("vtx_pos"),
                    ),
                    SHADER_IO_TEXTURE_1D => (
                        ShaderIoKind::Texture(TextureType::Texture1d),
                        ShaderIoPrefix::Prefix("tex_"),
                    ),
                    SHADER_IO_TEXTURE_1D_ARRAY => (
                        ShaderIoKind::Texture(TextureType::Texture1dArray),
                        ShaderIoPrefix::Prefix("tex_"),
                    ),
                    SHADER_IO_TEXTURE_2D => (
                        ShaderIoKind::Texture(TextureType::Texture2d),
                        ShaderIoPrefix::Prefix("tex_"),
                    ),
                    SHADER_IO_TEXTURE_2D_ARRAY => (
                        ShaderIoKind::Texture(TextureType::Texture2dArray),
                        ShaderIoPrefix::Prefix("tex_"),
                    ),
                    SHADER_IO_TEXTURE_3D => (
                        ShaderIoKind::Texture(TextureType::Texture3d),
                        ShaderIoPrefix::Prefix("tex_"),
                    ),
                    SHADER_IO_TEXTURE_3D_ARRAY => (
                        ShaderIoKind::Texture(TextureType::Texture3dArray),
                        ShaderIoPrefix::Prefix("tex_"),
                    ),
                    SHADER_IO_TEXTURE_CUBE => (
                        ShaderIoKind::Texture(TextureType::TextureCube),
                        ShaderIoPrefix::Prefix("tex_"),
                    ),
                    SHADER_IO_TEXTURE_CUBE_ARRAY => (
                        ShaderIoKind::Texture(TextureType::TextureCubeArray),
                        ShaderIoPrefix::Prefix("tex_"),
                    ),
                    SHADER_IO_TEXTURE_DEPTH => (
                        ShaderIoKind::Texture(TextureType::TextureDepth),
                        ShaderIoPrefix::Prefix("tex_"),
                    ),
                    SHADER_IO_TEXTURE_DEPTH_ARRAY => (
                        ShaderIoKind::Texture(TextureType::TextureDepthArray),
                        ShaderIoPrefix::Prefix("tex_"),
                    ),
                    SHADER_IO_SAMPLER => (
                        ShaderIoKind::Sampler(ShaderSamplerOptions::default()),
                        ShaderIoPrefix::Prefix("sampler_"),
                    ),
                    SHADER_IO_SCOPE_UNIFORM => {
                        (ShaderIoKind::ScopeUniform, ShaderIoPrefix::Prefix("su_"))
                    }
                    _ => panic!(),
                }
            }
        }
    }

    pub fn get_io_all(&self, _mode: ShaderMode) -> &'static str {
        match self {
            Self::Metal => "_io",
            Self::Hlsl => "_io",
            _ => "",
        }
    }

    pub fn get_io_all_decl(&self, _mode: ShaderMode) -> &'static str {
        match self {
            Self::Metal => "thread Io &_io",
            Self::Hlsl => "Io _io",
            _ => "",
        }
    }

    pub fn get_io_self(&self, mode: ShaderMode) -> &'static str {
        match self {
            Self::Metal => match mode {
                ShaderMode::Vertex => "_iov",
                ShaderMode::Fragment => "_iof",
                _ => "",
            },
            Self::Hlsl => match mode {
                ShaderMode::Vertex => "_iov",
                ShaderMode::Fragment => "_iof",
                _ => "",
            },
            _ => "",
        }
    }

    pub fn get_io_self_decl(&self, mode: ShaderMode) -> &'static str {
        match self {
            Self::Metal => match mode {
                ShaderMode::Vertex => "thread IoV &_iov",
                ShaderMode::Fragment => "thread IoF &_iof",
                _ => "",
            },
            Self::Hlsl => match mode {
                ShaderMode::Vertex => "inout IoV _iov",
                ShaderMode::Fragment => "inout IoF _iof",
                _ => "",
            },
            _ => "",
        }
    }

    /// Generate a variable declaration statement for the backend.
    /// For C-style backends (Metal, HLSL, GLSL): `type_name var_name;\n`
    /// For WGSL: `var var_name:type_name;\n`
    pub fn write_var_decl(&self, out: &mut String, ty_name: LiveId, var_name: &str) {
        match self {
            Self::Metal | Self::Hlsl | Self::Glsl => {
                write!(out, "{} {};\n", ty_name, var_name).ok();
            }
            Self::Wgsl => {
                write!(out, "var {}:{};\n", var_name, ty_name).ok();
            }
        }
    }

    /// Generate a variable declaration with zero initialization for the backend.
    /// For C-style backends (Metal, HLSL, GLSL): `type_name var_name = type_name(0);\n`
    /// For WGSL: `var var_name:type_name = type_name();\n` (zero-initialized)
    pub fn write_var_decl_zero_init(&self, out: &mut String, ty_name: LiveId, var_name: &str) {
        match self {
            Self::Metal | Self::Hlsl => {
                // Use constructor with zero for compound types, literal for scalars
                let zero = self.zero_literal(ty_name);
                write!(out, "{} {} = {};\n", ty_name, var_name, zero).ok();
            }
            Self::Glsl => {
                let zero = self.zero_literal(ty_name);
                write!(out, "{} {} = {};\n", ty_name, var_name, zero).ok();
            }
            Self::Wgsl => {
                let zero = self.zero_literal(ty_name);
                write!(out, "var {}:{} = {};\n", var_name, ty_name, zero).ok();
            }
        }
    }

    /// Returns the zero literal for a given backend type name.
    fn zero_literal(&self, ty_name: LiveId) -> String {
        match self {
            Self::Metal | Self::Hlsl => {
                match ty_name {
                    // Scalars
                    id!(float) => "0.0".to_string(),
                    id!(half) => "0.0h".to_string(),
                    id!(uint) => "0".to_string(),
                    id!(int) => "0".to_string(),
                    id!(bool) => "false".to_string(),
                    // Vectors - use constructor syntax
                    id!(float2)
                    | id!(float3)
                    | id!(float4)
                    | id!(half2)
                    | id!(half3)
                    | id!(half4)
                    | id!(uint2)
                    | id!(uint3)
                    | id!(uint4)
                    | id!(int2)
                    | id!(int3)
                    | id!(int4)
                    | id!(bool2)
                    | id!(bool3)
                    | id!(bool4) => format!("{}(0)", ty_name),
                    // Matrices - use constructor syntax
                    id!(float2x2)
                    | id!(float2x3)
                    | id!(float2x4)
                    | id!(float3x2)
                    | id!(float3x3)
                    | id!(float3x4)
                    | id!(float4x2)
                    | id!(float4x3)
                    | id!(float4x4) => format!("{}(0.0)", ty_name),
                    // Default: use constructor with zero
                    _ => format!("{}()", ty_name),
                }
            }
            Self::Glsl => {
                match ty_name {
                    // Scalars
                    id!(float) => "0.0".to_string(),
                    id!(uint) => "0u".to_string(),
                    id!(int) => "0".to_string(),
                    id!(bool) => "false".to_string(),
                    // Vectors - use constructor syntax
                    id!(vec2) | id!(vec3) | id!(vec4) => format!("{}(0.0)", ty_name),
                    id!(uvec2) | id!(uvec3) | id!(uvec4) => format!("{}(0u)", ty_name),
                    id!(ivec2) | id!(ivec3) | id!(ivec4) => format!("{}(0)", ty_name),
                    id!(bvec2) | id!(bvec3) | id!(bvec4) => format!("{}(false)", ty_name),
                    // Matrices - use constructor syntax
                    id!(mat2) | id!(mat3) | id!(mat4) => format!("{}(0.0)", ty_name),
                    // Default: use constructor with zero
                    _ => format!("{}(0)", ty_name),
                }
            }
            Self::Wgsl => {
                match ty_name {
                    // Scalars
                    id!(f32) => "0.0".to_string(),
                    id!(f16) => "0.0h".to_string(),
                    id!(u32) => "0u".to_string(),
                    id!(i32) => "0i".to_string(),
                    id!(bool) => "false".to_string(),
                    // Vectors - use constructor syntax (WGSL allows empty constructor for zero)
                    id!(vec2f)
                    | id!(vec3f)
                    | id!(vec4f)
                    | id!(vec2h)
                    | id!(vec3h)
                    | id!(vec4h)
                    | id!(vec2u)
                    | id!(vec3u)
                    | id!(vec4u)
                    | id!(vec2i)
                    | id!(vec3i)
                    | id!(vec4i)
                    | id!(vec2b)
                    | id!(vec3b)
                    | id!(vec4b) => format!("{}()", ty_name),
                    // Matrices - empty constructor for zero
                    id!(mat2x2f)
                    | id!(mat2x3f)
                    | id!(mat2x4f)
                    | id!(mat3x2f)
                    | id!(mat3x3f)
                    | id!(mat3x4f)
                    | id!(mat4x2f)
                    | id!(mat4x3f)
                    | id!(mat4x4f) => format!("{}()", ty_name),
                    // Default: use empty constructor
                    _ => format!("{}()", ty_name),
                }
            }
        }
    }

    pub fn register_ids(&self) {
        match self {
            Self::Metal | Self::Hlsl => {
                id_lut!(float);
                id_lut!(half);
                id_lut!(uint);
                id_lut!(int);
                id_lut!(float2);
                id_lut!(float3);
                id_lut!(float4);
                id_lut!(half2);
                id_lut!(half3);
                id_lut!(half4);
                id_lut!(uint2);
                id_lut!(uint3);
                id_lut!(uint4);
                id_lut!(int2);
                id_lut!(int3);
                id_lut!(int4);
                id_lut!(bool2);
                id_lut!(bool3);
                id_lut!(bool4);
                id_lut!(float2x2);
                id_lut!(float2x3);
                id_lut!(float2x4);
                id_lut!(float3x2);
                id_lut!(float3x3);
                id_lut!(float3x4);
                id_lut!(float4x2);
                id_lut!(float4x3);
                id_lut!(float4x4);
                id_lut!(atomic_uint);
                id_lut!(atomic_int);
                // Packed types for Metal instance/vertex buffers
                id_lut!(packed_float2);
                id_lut!(packed_float3);
                id_lut!(packed_float4);
                id_lut!(packed_half2);
                id_lut!(packed_half3);
                id_lut!(packed_half4);
                id_lut!(packed_uint2);
                id_lut!(packed_uint3);
                id_lut!(packed_uint4);
                id_lut!(packed_int2);
                id_lut!(packed_int3);
                id_lut!(packed_int4);
                id_lut!(packed_bool2);
                id_lut!(packed_bool3);
                id_lut!(packed_bool4);
                id_lut!(packed_float2x2);
                id_lut!(packed_float2x3);
                id_lut!(packed_float2x4);
                id_lut!(packed_float3x2);
                id_lut!(packed_float3x3);
                id_lut!(packed_float3x4);
                id_lut!(packed_float4x2);
                id_lut!(packed_float4x3);
                id_lut!(packed_float4x4);
                // Builtin function names
                id_lut!(dfdx);
                id_lut!(dfdy);
                id_lut!(ddx);
                id_lut!(ddy);
                id_lut!(rsqrt);
                id_lut!(fmod);
                id_lut!(discard_fragment);
            }
            Self::Glsl => {
                id_lut!(float);
                id_lut!(uint);
                id_lut!(int);
                id_lut!(vec2);
                id_lut!(vec3);
                id_lut!(vec4);
                id_lut!(uvec2);
                id_lut!(uvec3);
                id_lut!(uvec4);
                id_lut!(ivec2);
                id_lut!(ivec3);
                id_lut!(ivec4);
                id_lut!(bvec2);
                id_lut!(bvec3);
                id_lut!(bvec4);
                id_lut!(mat2);
                id_lut!(mat3);
                id_lut!(mat4);
                // Builtin function names
                id_lut!(dFdx);
                id_lut!(dFdy);
                id_lut!(inversesqrt);
            }
            Self::Wgsl => {
                // Builtin function names
                id_lut!(dpdx);
                id_lut!(dpdy);
            }
        }
    }

    pub fn map_builtin_name(&self, name_in: LiveId) -> LiveId {
        match self {
            Self::Metal => match name_in {
                id!(dFdx) => id!(dfdx),
                id!(dFdy) => id!(dfdy),
                id!(inverseSqrt) => id!(rsqrt),
                id!(modf) => id!(fmod),
                id!(discard) => id!(discard_fragment),
                x => x,
            },
            Self::Hlsl => match name_in {
                id!(dFdx) => id!(ddx),
                id!(dFdy) => id!(ddy),
                id!(inverseSqrt) => id!(rsqrt),
                id!(modf) => id!(fmod),
                x => x,
            },
            Self::Glsl => {
                match name_in {
                    // GLSL uses dFdx/dFdy natively, mod is native
                    id!(inverseSqrt) => id!(inversesqrt),
                    x => x,
                }
            }
            Self::Wgsl => {
                match name_in {
                    // WGSL uses dpdx/dpdy, mod is native (%)
                    id!(dFdx) => id!(dpdx),
                    id!(dFdy) => id!(dpdy),
                    id!(inverseSqrt) => id!(inverseSqrt),
                    x => x,
                }
            }
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
                    // Matrices use packed column vectors
                    id!(mat2x2f) => id!(packed_float2x2),
                    id!(mat2x3f) => id!(packed_float2x3),
                    id!(mat2x4f) => id!(packed_float2x4),
                    id!(mat3x2f) => id!(packed_float3x2),
                    id!(mat3x3f) => id!(packed_float3x3),
                    id!(mat3x4f) => id!(packed_float3x4),
                    id!(mat4x2f) => id!(packed_float4x2),
                    id!(mat4x3f) => id!(packed_float4x3),
                    id!(mat4x4f) => id!(packed_float4x4),
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
            Self::Wgsl => name_in,
        }
    }

    pub fn pod_struct_defs(
        &self,
        heap: &ScriptHeap,
        root_structs: &BTreeSet<ScriptPodType>,
        out: &mut String,
    ) {
        let mut visited = BTreeSet::new();
        let mut order = Vec::new();

        for root in root_structs {
            self.pod_struct_visit(heap, *root, &mut visited, &mut order);
        }

        for ty in order {
            let pod_type = heap.pod_type_ref(ty);
            if let ScriptPodTy::Struct { .. } = &pod_type.ty {
                let mut referenced = BTreeSet::new();
                self.pod_type_def(heap, ty, &mut referenced, out);
            }
        }
    }

    fn pod_struct_visit(
        &self,
        heap: &ScriptHeap,
        ty: ScriptPodType,
        visited: &mut BTreeSet<ScriptPodType>,
        order: &mut Vec<ScriptPodType>,
    ) {
        if visited.contains(&ty) {
            return;
        }
        visited.insert(ty);

        let pod_type = heap.pod_type_ref(ty);
        let mut referenced = BTreeSet::new();
        let mut dummy = String::new();

        match &pod_type.ty {
            ScriptPodTy::Struct { fields, .. } => {
                for field in fields {
                    self.pod_type_name_referenced(&field.ty, &mut referenced, &mut dummy);
                }
            }
            ScriptPodTy::FixedArray { ty: inner, .. }
            | ScriptPodTy::VariableArray { ty: inner, .. } => {
                self.pod_type_name_referenced(inner, &mut referenced, &mut dummy);
            }
            _ => {}
        }

        for ref_ty in referenced {
            self.pod_struct_visit(heap, ref_ty, visited, order);
        }

        order.push(ty);
    }

    pub fn pod_type_def(
        &self,
        heap: &ScriptHeap,
        pod_ty: ScriptPodType,
        referenced: &mut BTreeSet<ScriptPodType>,
        out: &mut String,
    ) {
        let pod_type = heap.pod_type_ref(pod_ty);
        if let ScriptPodTy::Struct { fields, .. } = &pod_type.ty {
            if let Some(name) = pod_type.name {
                writeln!(out, "struct {} {{", self.map_pod_name(name)).ok();
            } else {
                writeln!(out, "struct S{} {{", pod_ty.index).ok();
            };
            for field in fields {
                match self {
                    Self::Metal | Self::Hlsl | Self::Glsl => {
                        write!(out, "    ").ok();
                        if let ScriptPodTy::FixedArray { .. } = &field.ty.data.ty {
                            self.pod_type_def_metal_array(&field.ty, &field.name, referenced, out);
                        } else {
                            self.pod_type_name_referenced(&field.ty, referenced, out);
                            writeln!(out, " {};", field.name).ok();
                        }
                    }
                    Self::Wgsl => {
                        write!(out, "    {}: ", field.name).ok();
                        self.pod_type_name_referenced(&field.ty, referenced, out);
                        writeln!(out, ",").ok();
                    }
                }
            }
            match self {
                Self::Metal | Self::Hlsl | Self::Glsl => {
                    writeln!(out, "}};").ok();
                }
                Self::Wgsl => {
                    writeln!(out, "}}").ok();
                }
            }
        }
    }

    fn pod_type_def_metal_array(
        &self,
        ty: &ScriptPodTypeInline,
        name: &LiveId,
        referenced: &mut BTreeSet<ScriptPodType>,
        out: &mut String,
    ) {
        let mut dims = String::new();
        let mut curr = ty;
        loop {
            match &curr.data.ty {
                ScriptPodTy::FixedArray { ty: inner, len, .. } => {
                    write!(dims, "[{}]", len).ok();
                    curr = inner;
                }
                _ => break,
            }
        }
        self.pod_type_name_referenced(curr, referenced, out);
        writeln!(out, " {}{};", name, dims).ok();
    }

    fn pod_type_name_referenced(
        &self,
        ty: &ScriptPodTypeInline,
        referenced: &mut BTreeSet<ScriptPodType>,
        out: &mut String,
    ) {
        match &ty.data.ty {
            ScriptPodTy::Struct { .. } => {
                referenced.insert(ty.self_ref);
                let name = ty.data.name.unwrap();
                let name = self.map_pod_name(name);
                write!(out, "{}", name).ok();
            }
            ScriptPodTy::FixedArray { ty: inner, len, .. } => {
                out.push_str("array<");
                self.pod_type_name_referenced(inner, referenced, out);
                write!(out, ", {}>", len).ok();
            }
            ScriptPodTy::VariableArray { ty: inner, .. } => {
                out.push_str("array<");
                self.pod_type_name_referenced(inner, referenced, out);
                out.push_str(">");
            }
            _ => self.pod_type_name(ty, out),
        }
    }

    pub fn pod_type_name_from_ty(&self, heap: &ScriptHeap, ty: ScriptPodType, out: &mut String) {
        let pod_ty = heap.pod_type_ref(ty);
        let inline = ScriptPodTypeInline {
            self_ref: ty,
            data: pod_ty.clone(),
        };
        self.pod_type_name(&inline, out);
    }

    /// Output packed type name for instance buffer structs (Metal only).
    /// Uses packed_float2, packed_float3, etc. to match CPU struct alignment.
    pub fn pod_type_name_packed_from_ty(
        &self,
        heap: &ScriptHeap,
        ty: ScriptPodType,
        out: &mut String,
    ) {
        let pod_ty = heap.pod_type_ref(ty);
        let inline = ScriptPodTypeInline {
            self_ref: ty,
            data: pod_ty.clone(),
        };
        self.pod_type_name_packed(&inline, out);
    }

    /// Output packed type name (for instance buffer structs in Metal).
    pub fn pod_type_name_packed(&self, ty: &ScriptPodTypeInline, out: &mut String) {
        match &ty.data.ty {
            ScriptPodTy::F32 => write!(out, "{}", self.map_packed_pod_name(id!(f32)))
                .ok()
                .unwrap_or(()),
            ScriptPodTy::F16 => write!(out, "{}", self.map_packed_pod_name(id!(f16)))
                .ok()
                .unwrap_or(()),
            ScriptPodTy::U32 => write!(out, "{}", self.map_packed_pod_name(id!(u32)))
                .ok()
                .unwrap_or(()),
            ScriptPodTy::I32 => write!(out, "{}", self.map_packed_pod_name(id!(i32)))
                .ok()
                .unwrap_or(()),
            ScriptPodTy::Bool => write!(out, "{}", self.map_packed_pod_name(id!(bool)))
                .ok()
                .unwrap_or(()),
            ScriptPodTy::Vec(v) => write!(out, "{}", self.map_packed_pod_name(v.name()))
                .ok()
                .unwrap_or(()),
            ScriptPodTy::Mat(m) => write!(out, "{}", self.map_packed_pod_name(m.name()))
                .ok()
                .unwrap_or(()),
            // For other types, fall back to regular type names
            _ => self.pod_type_name(ty, out),
        }
    }

    pub fn pod_type_name(&self, ty: &ScriptPodTypeInline, out: &mut String) {
        match &ty.data.ty {
            ScriptPodTy::F32 => write!(out, "{}", self.map_pod_name(id!(f32)))
                .ok()
                .unwrap_or(()),
            ScriptPodTy::F16 => write!(out, "{}", self.map_pod_name(id!(f16)))
                .ok()
                .unwrap_or(()),
            ScriptPodTy::U32 => write!(out, "{}", self.map_pod_name(id!(u32)))
                .ok()
                .unwrap_or(()),
            ScriptPodTy::I32 => write!(out, "{}", self.map_pod_name(id!(i32)))
                .ok()
                .unwrap_or(()),
            ScriptPodTy::Bool => write!(out, "{}", self.map_pod_name(id!(bool)))
                .ok()
                .unwrap_or(()),
            ScriptPodTy::AtomicU32 => write!(out, "atomic<{}>", self.map_pod_name(id!(u32)))
                .ok()
                .unwrap_or(()),
            ScriptPodTy::AtomicI32 => write!(out, "atomic<{}>", self.map_pod_name(id!(i32)))
                .ok()
                .unwrap_or(()),
            ScriptPodTy::Vec(v) => write!(out, "{}", self.map_pod_name(v.name()))
                .ok()
                .unwrap_or(()),
            ScriptPodTy::Mat(m) => write!(out, "{}", self.map_pod_name(m.name()))
                .ok()
                .unwrap_or(()),
            ScriptPodTy::Struct { .. } => {
                let name = ty.data.name.unwrap();
                let name = self.map_pod_name(name);
                write!(out, "{}", name).ok().unwrap_or(());
            }
            ScriptPodTy::FixedArray { ty: inner, len, .. } => {
                out.push_str("array<");
                self.pod_type_name(inner, out);
                write!(out, ", {}>", len).ok();
            }
            ScriptPodTy::VariableArray { ty: inner, .. } => {
                out.push_str("array<");
                self.pod_type_name(inner, out);
                out.push_str(">");
            }
            _ => out.push_str("unknown"),
        }
    }
}
