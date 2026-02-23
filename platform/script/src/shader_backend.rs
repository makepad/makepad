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
    Rust,
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
                            ShaderIoPrefix::Prefix("_io.i->"),
                        ),
                        SHADER_IO_DYN_INSTANCE => (
                            ShaderIoKind::DynInstance,
                            ShaderIoPrefix::Prefix("_io.i->"),
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
                        SHADER_IO_TEXTURE_VIDEO => (
                            ShaderIoKind::Texture(TextureType::TextureVideo),
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
                                ShaderIoPrefix::Prefix("_io.i->"),
                            ),
                            SHADER_IO_DYN_INSTANCE => (
                                ShaderIoKind::DynInstance,
                                ShaderIoPrefix::Prefix("_io.i->"),
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
                            SHADER_IO_TEXTURE_VIDEO => (
                                ShaderIoKind::Texture(TextureType::TextureVideo),
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
                                ShaderIoPrefix::Prefix("_mp_iov.i."),
                            ),
                            SHADER_IO_DYN_INSTANCE => (
                                ShaderIoKind::DynInstance,
                                ShaderIoPrefix::Prefix("_mp_iov.i."),
                            ),
                            SHADER_IO_DYN_UNIFORM => {
                                (ShaderIoKind::Uniform, ShaderIoPrefix::Prefix("u_"))
                            }
                            SHADER_IO_UNIFORM_BUFFER => {
                                (ShaderIoKind::UniformBuffer, ShaderIoPrefix::Prefix("u_"))
                            }
                            SHADER_IO_VARYING => {
                                (ShaderIoKind::Varying, ShaderIoPrefix::Prefix("_mp_iov.v."))
                            }
                            SHADER_IO_VERTEX_POSITION => (
                                ShaderIoKind::VertexPosition,
                                ShaderIoPrefix::Full("_mp_iov.v._position"),
                            ),
                            SHADER_IO_VERTEX_BUFFER => (
                                ShaderIoKind::VertexBuffer,
                                ShaderIoPrefix::Prefix("_mp_iov.vb."),
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
                            SHADER_IO_TEXTURE_VIDEO => (
                                ShaderIoKind::Texture(TextureType::TextureVideo),
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
                                ShaderIoPrefix::FullOwned(format!("_mp_iof.fb{}", index)),
                            );
                        }
                        match io_type {
                            SHADER_IO_RUST_INSTANCE => (
                                ShaderIoKind::RustInstance,
                                ShaderIoPrefix::Prefix("_mp_iof.v."),
                            ),
                            SHADER_IO_DYN_INSTANCE => (
                                ShaderIoKind::DynInstance,
                                ShaderIoPrefix::Prefix("_mp_iof.v."),
                            ),
                            SHADER_IO_DYN_UNIFORM => {
                                (ShaderIoKind::Uniform, ShaderIoPrefix::Prefix("u_"))
                            }
                            SHADER_IO_UNIFORM_BUFFER => {
                                (ShaderIoKind::UniformBuffer, ShaderIoPrefix::Prefix("u_"))
                            }
                            SHADER_IO_VARYING => {
                                (ShaderIoKind::Varying, ShaderIoPrefix::Prefix("_mp_iof.v."))
                            }
                            SHADER_IO_VERTEX_POSITION => (
                                ShaderIoKind::VertexPosition,
                                ShaderIoPrefix::Full("_mp_iof.v._position"),
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
                            SHADER_IO_TEXTURE_VIDEO => (
                                ShaderIoKind::Texture(TextureType::TextureVideo),
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
            Self::Rust => {
                // Check for fragment output range first
                if io_type.0 >= SHADER_IO_FRAGMENT_OUTPUT_0.0
                    && io_type.0 <= SHADER_IO_FRAGMENT_OUTPUT_MAX.0
                {
                    let index = io_type.0 - SHADER_IO_FRAGMENT_OUTPUT_0.0;
                    return (
                        ShaderIoKind::FragmentOutput(index as u8),
                        ShaderIoPrefix::FullOwned(format!("rcx.frag_fb{}", index)),
                    );
                }
                match io_type {
                    SHADER_IO_RUST_INSTANCE => (
                        ShaderIoKind::RustInstance,
                        ShaderIoPrefix::Prefix("rcx.rustinst_"),
                    ),
                    SHADER_IO_DYN_INSTANCE => (
                        ShaderIoKind::DynInstance,
                        ShaderIoPrefix::Prefix("rcx.dyninst_"),
                    ),
                    SHADER_IO_DYN_UNIFORM => {
                        (ShaderIoKind::Uniform, ShaderIoPrefix::Prefix("rcx.uni_"))
                    }
                    SHADER_IO_UNIFORM_BUFFER => (
                        ShaderIoKind::UniformBuffer,
                        ShaderIoPrefix::Prefix("rcx.unibuf_"),
                    ),
                    SHADER_IO_VARYING => {
                        (ShaderIoKind::Varying, ShaderIoPrefix::Prefix("rcx.var_"))
                    }
                    SHADER_IO_VERTEX_POSITION => (
                        ShaderIoKind::VertexPosition,
                        ShaderIoPrefix::Full("rcx.vtx_pos"),
                    ),
                    SHADER_IO_VERTEX_BUFFER => (
                        ShaderIoKind::VertexBuffer,
                        ShaderIoPrefix::Prefix("rcx.vb_"),
                    ),
                    SHADER_IO_TEXTURE_1D => (
                        ShaderIoKind::Texture(TextureType::Texture1d),
                        ShaderIoPrefix::Prefix("rcx.tex_"),
                    ),
                    SHADER_IO_TEXTURE_1D_ARRAY => (
                        ShaderIoKind::Texture(TextureType::Texture1dArray),
                        ShaderIoPrefix::Prefix("rcx.tex_"),
                    ),
                    SHADER_IO_TEXTURE_2D => (
                        ShaderIoKind::Texture(TextureType::Texture2d),
                        ShaderIoPrefix::Prefix("rcx.tex_"),
                    ),
                    SHADER_IO_TEXTURE_2D_ARRAY => (
                        ShaderIoKind::Texture(TextureType::Texture2dArray),
                        ShaderIoPrefix::Prefix("rcx.tex_"),
                    ),
                    SHADER_IO_TEXTURE_3D => (
                        ShaderIoKind::Texture(TextureType::Texture3d),
                        ShaderIoPrefix::Prefix("rcx.tex_"),
                    ),
                    SHADER_IO_TEXTURE_3D_ARRAY => (
                        ShaderIoKind::Texture(TextureType::Texture3dArray),
                        ShaderIoPrefix::Prefix("rcx.tex_"),
                    ),
                    SHADER_IO_TEXTURE_CUBE => (
                        ShaderIoKind::Texture(TextureType::TextureCube),
                        ShaderIoPrefix::Prefix("rcx.tex_"),
                    ),
                    SHADER_IO_TEXTURE_CUBE_ARRAY => (
                        ShaderIoKind::Texture(TextureType::TextureCubeArray),
                        ShaderIoPrefix::Prefix("rcx.tex_"),
                    ),
                    SHADER_IO_TEXTURE_DEPTH => (
                        ShaderIoKind::Texture(TextureType::TextureDepth),
                        ShaderIoPrefix::Prefix("rcx.tex_"),
                    ),
                    SHADER_IO_TEXTURE_DEPTH_ARRAY => (
                        ShaderIoKind::Texture(TextureType::TextureDepthArray),
                        ShaderIoPrefix::Prefix("rcx.tex_"),
                    ),
                    SHADER_IO_TEXTURE_VIDEO => (
                        ShaderIoKind::Texture(TextureType::TextureVideo),
                        ShaderIoPrefix::Prefix("rcx.tex_"),
                    ),
                    SHADER_IO_SAMPLER => (
                        ShaderIoKind::Sampler(ShaderSamplerOptions::default()),
                        ShaderIoPrefix::Prefix("rcx.sampler_"),
                    ),
                    SHADER_IO_SCOPE_UNIFORM => (
                        ShaderIoKind::ScopeUniform,
                        ShaderIoPrefix::Prefix("rcx.su_"),
                    ),
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
                        ShaderIoPrefix::Full("vtx_pos"),
                    ),
                    SHADER_IO_VERTEX_BUFFER => {
                        (ShaderIoKind::VertexBuffer, ShaderIoPrefix::Prefix("vb_"))
                    }
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
                    SHADER_IO_TEXTURE_VIDEO => (
                        ShaderIoKind::Texture(TextureType::TextureVideo),
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
            Self::Hlsl => "",
            Self::Rust => "rcx",
            _ => "",
        }
    }

    pub fn get_io_all_decl(&self, _mode: ShaderMode) -> &'static str {
        match self {
            Self::Metal => "thread Io &_io",
            Self::Hlsl => "",
            Self::Rust => "rcx: &mut RenderCx",
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
                ShaderMode::Vertex => "",
                ShaderMode::Fragment => "",
                _ => "",
            },
            Self::Rust => "",
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
                ShaderMode::Vertex => "",
                ShaderMode::Fragment => "",
                _ => "",
            },
            Self::Rust => "",
            _ => "",
        }
    }

    pub fn map_local_name(&self, id: LiveId, shadow: usize) -> String {
        match self {
            Self::Hlsl => {
                if shadow > 0 {
                    format!("l_{}_{}", id, shadow)
                } else {
                    format!("l_{}", id)
                }
            }
            Self::Glsl => {
                let base = if id == id!(self) {
                    "_self".to_string()
                } else {
                    format!("{}", id)
                };
                if shadow > 0 {
                    format!("l_{}_{}", base, shadow)
                } else {
                    format!("l_{}", base)
                }
            }
            Self::Rust => {
                let base = if id == id!(self) {
                    "_self".to_string()
                } else if id == id!(type)
                    || id == id!(match)
                    || id == id!(fn)
                    || id == id!(let)
                    || id == id!(mut)
                    || id == id!(ref)
                    || id == id!(loop)
                    || id == id!(move)
                    || id == id!(pub)
                    || id == id!(use)
                    || id == id!(mod)
                    || id == id!(impl)
                    || id == id!(where)
                    || id == id!(as)
                    || id == id!(in)
                    || id == id!(for)
                    || id == id!(if)
                    || id == id!(else)
                    || id == id!(while)
                    || id == id!(return)
                    || id == id!(break)
                    || id == id!(continue)
                    || id == id!(struct)
                    || id == id!(enum)
                    || id == id!(trait)
                    || id == id!(super)
                    || id == id!(crate)
                {
                    format!("r#{}", id)
                } else {
                    format!("{}", id)
                };
                if shadow > 0 {
                    format!("{}_{}", base, shadow)
                } else {
                    base
                }
            }
            _ => {
                if shadow > 0 {
                    format!("_s{}{}", shadow, id)
                } else if id == id!(self) {
                    "_self".to_string()
                } else {
                    format!("{}", id)
                }
            }
        }
    }

    pub fn map_param_name(&self, id: LiveId, shadow: usize) -> String {
        if id == id!(self) {
            // Rust and WGSL self params are pointers, so dereference for field access.
            if matches!(self, Self::Rust | Self::Wgsl) {
                return "(*_self)".to_string();
            }
            return "_self".to_string();
        }
        match self {
            Self::Hlsl | Self::Glsl => {
                if shadow > 0 {
                    format!("p_{}_{}", id, shadow)
                } else {
                    format!("p_{}", id)
                }
            }
            Self::Rust => self.map_local_name(id, shadow),
            _ => self.map_local_name(id, shadow),
        }
    }

    pub fn map_function_name(&self, name: &str) -> String {
        match self {
            Self::Hlsl => format!("f_{}", name),
            Self::Rust => name.to_string(),
            _ => name.to_string(),
        }
    }

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
            Self::Rust => {
                let zero = self.zero_literal(ty_name);
                write!(out, "let mut {}: {} = {};\n", var_name, ty_name, zero).ok();
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
            Self::Rust => {
                let zero = self.zero_literal(ty_name);
                write!(out, "let mut {}: {} = {};\n", var_name, ty_name, zero).ok();
            }
        }
    }

    /// Returns the zero literal for a given backend type name.
    fn zero_literal(&self, ty_name: LiveId) -> String {
        match self {
            Self::Metal => {
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
            Self::Hlsl => {
                fn join_n(lit: &str, n: usize) -> String {
                    std::iter::repeat(lit)
                        .take(n)
                        .collect::<Vec<_>>()
                        .join(", ")
                }
                match ty_name {
                    // Scalars
                    id!(float) => "0.0".to_string(),
                    id!(half) => "0.0h".to_string(),
                    id!(uint) => "0".to_string(),
                    id!(int) => "0".to_string(),
                    id!(bool) => "false".to_string(),
                    // Float/half vectors
                    id!(float2) => "float2(0.0, 0.0)".to_string(),
                    id!(float3) => "float3(0.0, 0.0, 0.0)".to_string(),
                    id!(float4) => "float4(0.0, 0.0, 0.0, 0.0)".to_string(),
                    id!(half2) => "half2(0.0h, 0.0h)".to_string(),
                    id!(half3) => "half3(0.0h, 0.0h, 0.0h)".to_string(),
                    id!(half4) => "half4(0.0h, 0.0h, 0.0h, 0.0h)".to_string(),
                    // Int/uint vectors
                    id!(uint2) => "uint2(0, 0)".to_string(),
                    id!(uint3) => "uint3(0, 0, 0)".to_string(),
                    id!(uint4) => "uint4(0, 0, 0, 0)".to_string(),
                    id!(int2) => "int2(0, 0)".to_string(),
                    id!(int3) => "int3(0, 0, 0)".to_string(),
                    id!(int4) => "int4(0, 0, 0, 0)".to_string(),
                    // Bool vectors
                    id!(bool2) => "bool2(false, false)".to_string(),
                    id!(bool3) => "bool3(false, false, false)".to_string(),
                    id!(bool4) => "bool4(false, false, false, false)".to_string(),
                    // Matrices
                    id!(float2x2) => format!("float2x2({})", join_n("0.0", 4)),
                    id!(float2x3) => format!("float2x3({})", join_n("0.0", 6)),
                    id!(float2x4) => format!("float2x4({})", join_n("0.0", 8)),
                    id!(float3x2) => format!("float3x2({})", join_n("0.0", 6)),
                    id!(float3x3) => format!("float3x3({})", join_n("0.0", 9)),
                    id!(float3x4) => format!("float3x4({})", join_n("0.0", 12)),
                    id!(float4x2) => format!("float4x2({})", join_n("0.0", 8)),
                    id!(float4x3) => format!("float4x3({})", join_n("0.0", 12)),
                    id!(float4x4) => format!("float4x4({})", join_n("0.0", 16)),
                    // Default: use constructor with no args
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
            Self::Rust => {
                match ty_name {
                    // Scalars
                    id!(f32) => "0.0f32".to_string(),
                    id!(f16) => "0.0f32".to_string(), // f16 maps to f32 in Rust runtime
                    id!(u32) => "0u32".to_string(),
                    id!(i32) => "0i32".to_string(),
                    id!(bool) => "false".to_string(),
                    // Vectors - use constructor functions
                    id!(vec2f) => "vec2(0.0, 0.0)".to_string(),
                    id!(vec3f) => "vec3(0.0, 0.0, 0.0)".to_string(),
                    id!(vec4f) => "vec4(0.0, 0.0, 0.0, 0.0)".to_string(),
                    // Matrices
                    id!(mat4x4f) => "Mat4f::default()".to_string(),
                    // Default: use Default trait
                    _ => format!("{}::default()", ty_name),
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
                id_lut!(frac);
                id_lut!(lerp);
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
                id_lut!(mod);
            }
            Self::Wgsl => {
                // Builtin function names
                id_lut!(dpdx);
                id_lut!(dpdy);
            }
            Self::Rust => {
                // Rust uses canonical names from makepad_math - no remapping needed
                // Register builtin function names used in Rust backend
                id_lut!(inverseSqrt);
                id_lut!(modf);
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
                id!(fract) => id!(frac),
                id!(mix) => id!(lerp),
                x => x,
            },
            Self::Glsl => {
                match name_in {
                    // GLSL uses dFdx/dFdy natively, mod is native
                    id!(inverseSqrt) => id!(inversesqrt),
                    id!(modf) => id!(mod),
                    id!(atan2) => id!(atan),
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
            Self::Rust => {
                match name_in {
                    // Rust backend maps to makepad_math::shader_runtime functions
                    id!(inverseSqrt) => id!(inverseSqrt), // maps to inverse_sqrt in codegen
                    id!(modf) => id!(modf),               // maps to modf in shader_runtime
                    id!(dFdx) => id!(dFdx),               // no-op in CPU (returns 0)
                    id!(dFdy) => id!(dFdy),               // no-op in CPU (returns 0)
                    id!(discard) => id!(discard),         // no-op in CPU
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
            Self::Wgsl | Self::Rust => name_in,
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
            if matches!(self, Self::Rust) {
                writeln!(out, "#[derive(Default, Clone, Copy)]").ok();
                writeln!(out, "#[repr(C)]").ok();
            }
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
                            let field_name = self.map_field_name(field.name);
                            writeln!(out, " {};", field_name).ok();
                        }
                    }
                    Self::Wgsl => {
                        write!(out, "    {}: ", field.name).ok();
                        self.pod_type_name_referenced(&field.ty, referenced, out);
                        writeln!(out, ",").ok();
                    }
                    Self::Rust => {
                        write!(out, "    pub {}: ", field.name).ok();
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
                Self::Rust => {
                    writeln!(out, "}}").ok();
                }
            }

            if matches!(self, Self::Hlsl) {
                let struct_name = if let Some(name) = pod_type.name {
                    format!("{}", self.map_pod_name(name))
                } else {
                    format!("S{}", pod_ty.index)
                };

                write!(out, "{} consfn_{}(", struct_name, struct_name).ok();
                for (index, field) in fields.iter().enumerate() {
                    if index > 0 {
                        write!(out, ", ").ok();
                    }
                    self.pod_type_name_referenced(&field.ty, referenced, out);
                    let field_param = self.map_param_name(field.name, 0);
                    write!(out, " {}", field_param).ok();
                }
                writeln!(out, "){{").ok();
                writeln!(out, "    {} r;", struct_name).ok();
                for field in fields {
                    let field_name = self.map_field_name(field.name);
                    let field_param = self.map_param_name(field.name, 0);
                    writeln!(out, "    r.{0} = {1};", field_name, field_param).ok();
                }
                writeln!(out, "    return r;").ok();
                writeln!(out, "}}").ok();
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
        let mapped = self.map_field_name(*name);
        writeln!(out, " {}{};", mapped, dims).ok();
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
