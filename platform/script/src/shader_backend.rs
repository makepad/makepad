pub use makepad_shader::shader_backend::*;
use crate::{mod_shader::*, shader_output::*};
pub trait ScriptShaderBackend {
    fn get_shader_io_kind_and_prefix(&self, mode: ShaderMode, io_type: ShaderIoType) -> (ShaderIoKind, ShaderIoPrefix);
}
impl ScriptShaderBackend for ShaderBackend {
    fn get_shader_io_kind_and_prefix(
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
                        SHADER_IO_DYN_INSTANCE => {
                            (ShaderIoKind::DynInstance, ShaderIoPrefix::Prefix("_io.i->"))
                        }
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
                            ShaderIoPrefix::Prefix("_io.g->"),
                        ),
                        SHADER_IO_FRAGMENT_OUTPUT_0 => {
                            (ShaderIoKind::Varying, ShaderIoPrefix::Prefix(""))
                        }
                        SHADER_IO_TEXTURE_1D => (
                            ShaderIoKind::Texture(TextureType::Texture1d),
                            ShaderIoPrefix::Prefix("_io.t_"),
                        ),
                        SHADER_IO_TEXTURE_1D_ARRAY => (
                            ShaderIoKind::Texture(TextureType::Texture1dArray),
                            ShaderIoPrefix::Prefix("_io.t_"),
                        ),
                        SHADER_IO_TEXTURE_2D => (
                            ShaderIoKind::Texture(TextureType::Texture2d),
                            ShaderIoPrefix::Prefix("_io.t_"),
                        ),
                        SHADER_IO_TEXTURE_2D_ARRAY => (
                            ShaderIoKind::Texture(TextureType::Texture2dArray),
                            ShaderIoPrefix::Prefix("_io.t_"),
                        ),
                        SHADER_IO_TEXTURE_3D => (
                            ShaderIoKind::Texture(TextureType::Texture3d),
                            ShaderIoPrefix::Prefix("_io.t_"),
                        ),
                        SHADER_IO_TEXTURE_3D_ARRAY => (
                            ShaderIoKind::Texture(TextureType::Texture3dArray),
                            ShaderIoPrefix::Prefix("_io.t_"),
                        ),
                        SHADER_IO_TEXTURE_CUBE => (
                            ShaderIoKind::Texture(TextureType::TextureCube),
                            ShaderIoPrefix::Prefix("_io.t_"),
                        ),
                        SHADER_IO_TEXTURE_CUBE_ARRAY => (
                            ShaderIoKind::Texture(TextureType::TextureCubeArray),
                            ShaderIoPrefix::Prefix("_io.t_"),
                        ),
                        SHADER_IO_TEXTURE_DEPTH => (
                            ShaderIoKind::Texture(TextureType::TextureDepth),
                            ShaderIoPrefix::Prefix("_io.t_"),
                        ),
                        SHADER_IO_TEXTURE_DEPTH_ARRAY => (
                            ShaderIoKind::Texture(TextureType::TextureDepthArray),
                            ShaderIoPrefix::Prefix("_io.t_"),
                        ),
                        SHADER_IO_TEXTURE_VIDEO => (
                            ShaderIoKind::Texture(TextureType::TextureVideo),
                            ShaderIoPrefix::Prefix("_io.t_"),
                        ),
                        SHADER_IO_SAMPLER => (
                            ShaderIoKind::Sampler(ShaderSamplerOptions::default()),
                            ShaderIoPrefix::Prefix("_io."),
                        ),
                        SHADER_IO_SCOPE_UNIFORM => (
                            ShaderIoKind::ScopeUniform,
                            ShaderIoPrefix::Prefix("_io.su->"),
                        ),

                        _ => unsupported_shader_io(self, mode, io_type),
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
                            SHADER_IO_DYN_INSTANCE => {
                                (ShaderIoKind::DynInstance, ShaderIoPrefix::Prefix("_io.i->"))
                            }
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
                                ShaderIoPrefix::Prefix("_io.t_"),
                            ),
                            SHADER_IO_TEXTURE_1D_ARRAY => (
                                ShaderIoKind::Texture(TextureType::Texture1dArray),
                                ShaderIoPrefix::Prefix("_io.t_"),
                            ),
                            SHADER_IO_TEXTURE_2D => (
                                ShaderIoKind::Texture(TextureType::Texture2d),
                                ShaderIoPrefix::Prefix("_io.t_"),
                            ),
                            SHADER_IO_TEXTURE_2D_ARRAY => (
                                ShaderIoKind::Texture(TextureType::Texture2dArray),
                                ShaderIoPrefix::Prefix("_io.t_"),
                            ),
                            SHADER_IO_TEXTURE_3D => (
                                ShaderIoKind::Texture(TextureType::Texture3d),
                                ShaderIoPrefix::Prefix("_io.t_"),
                            ),
                            SHADER_IO_TEXTURE_3D_ARRAY => (
                                ShaderIoKind::Texture(TextureType::Texture3dArray),
                                ShaderIoPrefix::Prefix("_io.t_"),
                            ),
                            SHADER_IO_TEXTURE_CUBE => (
                                ShaderIoKind::Texture(TextureType::TextureCube),
                                ShaderIoPrefix::Prefix("_io.t_"),
                            ),
                            SHADER_IO_TEXTURE_CUBE_ARRAY => (
                                ShaderIoKind::Texture(TextureType::TextureCubeArray),
                                ShaderIoPrefix::Prefix("_io.t_"),
                            ),
                            SHADER_IO_TEXTURE_DEPTH => (
                                ShaderIoKind::Texture(TextureType::TextureDepth),
                                ShaderIoPrefix::Prefix("_io.t_"),
                            ),
                            SHADER_IO_TEXTURE_DEPTH_ARRAY => (
                                ShaderIoKind::Texture(TextureType::TextureDepthArray),
                                ShaderIoPrefix::Prefix("_io.t_"),
                            ),
                            SHADER_IO_TEXTURE_VIDEO => (
                                ShaderIoKind::Texture(TextureType::TextureVideo),
                                ShaderIoPrefix::Prefix("_io.t_"),
                            ),
                            SHADER_IO_SAMPLER => (
                                ShaderIoKind::Sampler(ShaderSamplerOptions::default()),
                                ShaderIoPrefix::Prefix("_io."),
                            ),
                            SHADER_IO_SCOPE_UNIFORM => (
                                ShaderIoKind::ScopeUniform,
                                ShaderIoPrefix::Prefix("_io.su->"),
                            ),
                            _ => unsupported_shader_io(self, mode, io_type),
                        }
                    }
                    _ => unsupported_shader_io(self, mode, io_type),
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
                            _ => unsupported_shader_io(self, mode, io_type),
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
                            _ => unsupported_shader_io(self, mode, io_type),
                        }
                    }
                    _ => unsupported_shader_io(self, mode, io_type),
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
                    _ => unsupported_shader_io(self, mode, io_type),
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
                    _ => unsupported_shader_io(self, mode, io_type),
                }
            }
        }
    }
}
#[cfg(test)]
mod typed_vertex_tests {
    use makepad_live_id::{id, LiveId};
    use crate::{
        pod::{ScriptPodTy, ScriptPodTypeInline},
        shader::{ShaderIoKind, ShaderOutput},
        value::NIL,
        vm::ScriptVmBase,
    };

    #[test]
    fn compact_instances_and_nested_aggregates_are_compile_errors() {
        let mut vm = ScriptVmBase::new();
        let half2 = vm.code.builtins.pod.pod_f16x2;
        let mut output = ShaderOutput::default();
        output.validate_vertex_fetch_io(
            &vm.heap,
            &ShaderIoKind::DynInstance,
            id!(inst),
            half2.into(),
        );
        assert!(output.error_report().contains("instance buffers are f32-backed"));

        let half_inline = ScriptPodTypeInline {
            self_ref: half2,
            data: vm.heap.pod_type_ref(half2).clone(),
        };
        let array_object = vm.heap.new_object();
        let array_ty = vm.heap.new_pod_type(
            array_object,
            Some(id!(NestedCompactVertex)),
            ScriptPodTy::FixedArray {
                align_of: 2,
                size_of: 8,
                len: 2,
                ty: Box::new(half_inline),
            },
            NIL,
        );
        output.validate_vertex_fetch_io(
            &vm.heap,
            &ShaderIoKind::VertexBuffer,
            id!(geom),
            array_ty.into(),
        );
        assert!(output.error_report().contains("cannot contain nested structs, arrays, matrices"));

        let matrix_ty = vm.code.builtins.pod.pod_mat4x4f;
        let mut matrix_output = ShaderOutput::default();
        matrix_output.validate_vertex_fetch_io(
            &vm.heap,
            &ShaderIoKind::VertexBuffer,
            id!(matrix_geom),
            matrix_ty.into(),
        );
        assert!(
            matrix_output
                .error_report()
                .contains("cannot contain nested structs, arrays, matrices")
        );
    }

    // `if a { f() } else { g() }` as a statement leaves its phi as a value
    // nothing uses. WGSL rejects `_phi_1;` ("expected assignment or
    // increment/decrement"); the other backends accept a bare expression.
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
#[cold]
fn unsupported_shader_io(backend: &ShaderBackend, mode: ShaderMode, io_type: ShaderIoType) -> ! {
    if io_type == SHADER_IO_VERTEX_BUFFER && matches!(mode, ShaderMode::Fragment) {
        panic!(
            "shader: a geometry attribute (self.geom.*) was read in `pixel:`, but vertex \
             attributes only exist in the vertex stage.\n\
             Pass it through a varying instead:\n\
             \n\
             \x20   v_uv: varying(vec2f)\n\
             \x20   vertex: fn() {{ self.v_uv = self.geom.geom_uv  ... }}\n\
             \x20   pixel:  fn() {{ ... self.v_uv ... }}\n\
             \n\
             (backend {backend:?})"
        )
    }
    panic!(
        "shader backend {backend:?}: no lowering for io type {io_type:?} in {mode:?} stage.\n\
         This is a gap in the shader compiler, not in the shader that hit it — a declaration the \
         language accepts must lower on every backend, or fail with a message that names it."
    )
}
