use std::fmt::Write;
use crate::vm::ScriptVm;
use crate::shader::{ShaderOutput, ShaderIoKind, TextureType};

impl ShaderOutput {
    /// Emit HLSL helper functions that are needed by the shader
    pub fn hlsl_create_helpers(&self, _vm: &ScriptVm, out: &mut String) {
        if self.hlsl_needs_tex_size {
            writeln!(out, "float2 _mpTexSize2D(Texture2D tex) {{ uint w, h; tex.GetDimensions(w, h); return float2(w, h); }}").ok();
        }
    }

    pub fn hlsl_create_instance_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoInstance {{").ok();
        
        // 1. Output Dyn instance fields first (order doesn't matter, just output as encountered)
        for io in &self.io {
            if let ShaderIoKind::DynInstance = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " {};", io.name).ok();
            }
        }
        
        // 2. Output Rust instance fields last (already in correct order from pre_collect_rust_instance_io)
        for io in &self.io {
            if let ShaderIoKind::RustInstance = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " {};", io.name).ok();
            }
        }
        
        writeln!(out, "}};").ok();
    }

    pub fn hlsl_create_uniform_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "cbuffer IoUniform : register(b2) {{").ok();
        for io in &self.io {
            match &io.kind {
                ShaderIoKind::Uniform => {
                    write!(out, "    ").ok();
                    self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                    writeln!(out, " u_{};", io.name).ok();
                }
                _=>()
            }
        }
        writeln!(out, "}};").ok();
    }

    pub fn hlsl_create_uniform_buffer_cbuffers(&self, vm: &ScriptVm, out: &mut String) {
        // Create cbuffer declarations for each uniform buffer using pre-assigned buffer indices
        for io in &self.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                let buf_idx = io.buffer_index.expect("UniformBuffer must have buffer_index assigned");
                write!(out, "cbuffer cb_{} : register(b{}) {{ ", io.name, buf_idx).ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " u_{}; }};", io.name).ok();
            }
        }
    }

    pub fn hlsl_create_varying_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoVarying {{").ok();
        for io in &self.io {
            match io.kind {
                ShaderIoKind::Varying => {
                    write!(out, "    ").ok();
                    self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                    writeln!(out, " {};", io.name).ok();
                }
                _=>()
            }
        }
        writeln!(out, "    float4 _position : SV_POSITION;").ok();
        writeln!(out, "    uint _iid : TEXCOORD0;").ok();
        writeln!(out, "}};").ok();
    }

    pub fn hlsl_create_vertex_buffer_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoVertexBuffer {{").ok();
        for io in &self.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " {};", io.name).ok();
            }
        }
        writeln!(out, "}};").ok();
    }

    pub fn hlsl_create_vertex_input_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct VertexInput {{").ok();
        
        // Vertex buffer fields
        let mut semantic_idx = 0;
        for io in &self.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " vb_{} : GEOM{};", io.name, index_to_char(semantic_idx)).ok();
                semantic_idx += 1;
            }
        }
        
        // Instance fields
        semantic_idx = 0;
        // Dyn instance fields first
        for io in &self.io {
            if let ShaderIoKind::DynInstance = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " i_{} : INST{};", io.name, index_to_char(semantic_idx)).ok();
                semantic_idx += 1;
            }
        }
        // Rust instance fields
        for io in &self.io {
            if let ShaderIoKind::RustInstance = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " i_{} : INST{};", io.name, index_to_char(semantic_idx)).ok();
                semantic_idx += 1;
            }
        }
        
        writeln!(out, "    uint vid : SV_VertexID;").ok();
        writeln!(out, "    uint iid : SV_InstanceID;").ok();
        writeln!(out, "}};").ok();
    }

    pub fn hlsl_create_io_structs(&self, _vm: &ScriptVm, out: &mut String) {
        // IoV for vertex shader
        writeln!(out, "struct IoV {{").ok();
        writeln!(out, "    IoVarying v;").ok();
        writeln!(out, "    uint vid;").ok();
        writeln!(out, "    uint iid;").ok();
        writeln!(out, "}};").ok();
        writeln!(out).ok();
        
        // IoF for fragment shader
        writeln!(out, "struct IoF {{").ok();
        writeln!(out, "    IoVarying v;").ok();
        writeln!(out, "}};").ok();
        writeln!(out).ok();
        
        // Io for passing to shader functions
        writeln!(out, "struct Io {{").ok();
        writeln!(out, "}};").ok();
    }

    pub fn hlsl_create_fragment_output_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoFb {{").ok();
        for io in &self.io {
            if let ShaderIoKind::FragmentOutput(index) = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " fb{} : SV_TARGET{};", index, index).ok();
            }
        }
        writeln!(out, "}};").ok();
    }

    pub fn hlsl_create_texture_samplers(&self, _vm: &ScriptVm, out: &mut String) {
        let mut tex_idx = 0;
        let mut samp_idx = 0;
        for io in &self.io {
            match &io.kind {
                ShaderIoKind::Texture(tex_type) => {
                    let hlsl_type = match tex_type {
                        TextureType::Texture1d => "Texture1D",
                        TextureType::Texture1dArray => "Texture1DArray",
                        TextureType::Texture2d => "Texture2D",
                        TextureType::Texture2dArray => "Texture2DArray",
                        TextureType::Texture3d => "Texture3D",
                        TextureType::Texture3dArray => "Texture3D", // HLSL doesn't support 3D array textures
                        TextureType::TextureCube => "TextureCube",
                        TextureType::TextureCubeArray => "TextureCubeArray",
                        TextureType::TextureDepth => "Texture2D",
                        TextureType::TextureDepthArray => "Texture2DArray",
                    };
                    writeln!(out, "{} {} : register(t{});", hlsl_type, io.name, tex_idx).ok();
                    tex_idx += 1;
                }
                ShaderIoKind::Sampler(_) => {
                    writeln!(out, "SamplerState {} : register(s{});", io.name, samp_idx).ok();
                    samp_idx += 1;
                }
                _=>()
            }
        }
    }

    pub fn hlsl_create_vertex_fn(&self, _vm: &ScriptVm, out: &mut String) {
        writeln!(out, "IoVarying vertex_main(VertexInput input) {{").ok();
        writeln!(out, "    Io _io;").ok();
        writeln!(out, "    IoV _iov;").ok();
        writeln!(out, "    _iov.vid = input.vid;").ok();
        writeln!(out, "    _iov.iid = input.iid;").ok();
        writeln!(out).ok();
        
        // Copy vertex buffer fields to local struct
        for io in &self.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                // Vertex buffer fields are accessed directly via input.vb_name
            }
        }
        
        writeln!(out, "    io_vertex(_io, _iov);").ok();
        writeln!(out, "    return _iov.v;").ok();
        writeln!(out, "}}").ok();
    }

    pub fn hlsl_create_fragment_fn(&self, _vm: &ScriptVm, out: &mut String) {
        writeln!(out, "IoFb pixel_main(IoVarying v) {{").ok();
        writeln!(out, "    Io _io;").ok();
        writeln!(out, "    IoF _iof;").ok();
        writeln!(out, "    _iof.v = v;").ok();
        writeln!(out, "    IoFb _iofb;").ok();
        writeln!(out, "    io_fragment(_io, _iof);").ok();
        writeln!(out, "    return _iofb;").ok();
        writeln!(out, "}}").ok();
    }
}

/// Convert index to HLSL semantic character (A, B, C, ...)
pub fn index_to_char(index: usize) -> char {
    std::char::from_u32(index as u32 + 65).unwrap_or('?')
}
