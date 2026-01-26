use std::fmt::Write;
use crate::vm::ScriptVm;
use crate::shader::{ShaderOutput, ShaderIoKind, TextureType};

impl ShaderOutput {
    pub fn metal_create_io_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct Io {{").ok();
        writeln!(out, "    constant IoUniform *u;").ok();
        writeln!(out, "    constant IoInstance *i;").ok();
        
        // Add scope uniforms buffer pointer if we have any scope uniforms
        let has_scope_uniforms = self.io.iter().any(|io| matches!(io.kind, ShaderIoKind::ScopeUniform));
        if has_scope_uniforms {
            writeln!(out, "    constant IoScopeUniform *su;").ok();
        }
        
        for io in &self.io {
            match &io.kind {
                ShaderIoKind::Texture(tex_type) => {
                    let metal_type = match tex_type {
                        TextureType::Texture1d => "texture1d<float>",
                        TextureType::Texture1dArray => "texture1d_array<float>",
                        TextureType::Texture2d => "texture2d<float>",
                        TextureType::Texture2dArray => "texture2d_array<float>",
                        TextureType::Texture3d => "texture3d<float>",
                        TextureType::Texture3dArray => "texture3d<float>", // Metal doesn't support 3D array textures
                        TextureType::TextureCube => "texturecube<float>",
                        TextureType::TextureCubeArray => "texturecube_array<float>",
                        TextureType::TextureDepth => "depth2d<float>",
                        TextureType::TextureDepthArray => "depth2d_array<float>",
                    };
                    writeln!(out, "    {} {};", metal_type, io.name).ok();
                }
                ShaderIoKind::Sampler(_) => {
                    writeln!(out, "    sampler {};", io.name).ok();
                }
                ShaderIoKind::UniformBuffer => {
                    write!(out, "    constant ").ok();
                    self.backend.pod_type_name_from_ty(vm.heap, io.ty, out);
                    writeln!(out, " *u_{};", io.name).ok();
                }
                _=>()
            }
        }
        
        let mut have_vb = false;
        for io in &self.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                if !have_vb{
                    writeln!(out, "    constant IoVertexBuffer *vb;").ok();
                    have_vb = true;
                }
            }
        }
        writeln!(out, "}};").ok();
    }
    
    /// Creates the IoScopeUniform struct that holds values read from the script scope.
    /// This struct is populated by reading values from scope_uniforms sources before drawing.
    pub fn metal_create_scope_uniform_struct(&self, vm: &ScriptVm, out: &mut String) {
        // Only create the struct if there are scope uniforms
        let has_scope_uniforms = self.io.iter().any(|io| matches!(io.kind, ShaderIoKind::ScopeUniform));
        if !has_scope_uniforms {
            return;
        }
        
        writeln!(out, "struct IoScopeUniform {{").ok();
        for io in &self.io {
            if let ShaderIoKind::ScopeUniform = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(vm.heap, io.ty, out);
                writeln!(out, " {};", io.name).ok();
            }
        }
        writeln!(out, "}};").ok();
    }

    pub fn metal_create_instance_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoInstance {{").ok();
        
        // 1. Output Dyn instance fields first (order doesn't matter, just output as encountered)
        // Use packed types to match CPU-side repr(C) struct alignment
        for io in &self.io {
            if let ShaderIoKind::DynInstance = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_packed_from_ty(vm.heap, io.ty, out);
                writeln!(out, " {};", io.name).ok();
            }
        }
        
        // 2. Output Rust instance fields last (already in correct order from pre_collect_rust_instance_io)
        // Use packed types to match CPU-side repr(C) struct alignment
        for io in &self.io {
            if let ShaderIoKind::RustInstance = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_packed_from_ty(vm.heap, io.ty, out);
                writeln!(out, " {};", io.name).ok();
            }
        }
        
        writeln!(out, "}};").ok();
    }

    pub fn metal_create_uniform_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoUniform {{").ok();
        for io in &self.io {
            match &io.kind {
                ShaderIoKind::Uniform => {
                    write!(out, "    ").ok();
                    self.backend.pod_type_name_from_ty(vm.heap, io.ty, out);
                    writeln!(out, " {};", io.name).ok();
                }
                _=>()
            }
        }
        writeln!(out, "}};").ok();
    }

    pub fn metal_create_varying_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoVarying {{").ok();
        // Put _iid first to ensure consistent offset regardless of other varyings
        writeln!(out, "    uint _iid [[flat]];").ok();
        for io in &self.io {
            match io.kind {
                ShaderIoKind::Varying => {
                    write!(out, "    ").ok();
                    self.backend.pod_type_name_from_ty(vm.heap, io.ty, out);
                    writeln!(out, " {};", io.name).ok();
                }
                _=>()
            }
        }
        writeln!(out, "    float4 _position [[position]];").ok();
        writeln!(out, "}};").ok();
    }

    pub fn metal_create_vertex_buffer_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoVertexBuffer {{").ok();
        // Use packed types to match CPU-side repr(C) struct alignment
        for io in &self.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_packed_from_ty(vm.heap, io.ty, out);
                writeln!(out, " {};", io.name).ok();
            }
        }
        writeln!(out, "}};").ok();
    }

    pub fn metal_create_io_vertex_struct(&self, _vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoV {{").ok();
        writeln!(out, "    thread IoVarying *v;").ok();
        writeln!(out, "    uint vid;").ok();
        writeln!(out, "    uint iid;").ok();
        writeln!(out, "}};").ok();
    }

    pub fn metal_create_vertex_fn(&self, vm: &ScriptVm, out: &mut String) {
        let has_scope_uniforms = self.io.iter().any(|io| matches!(io.kind, ShaderIoKind::ScopeUniform));
        
        writeln!(out, "vertex IoVarying vertex_main(").ok();
        writeln!(out, "    constant IoVertexBuffer *vb [[buffer(0)]],").ok();
        writeln!(out, "    constant IoInstance *i [[buffer(1)]],").ok();
        writeln!(out, "    constant IoUniform *u [[buffer(2)]],").ok();
        
        // Use pre-assigned buffer indices from assign_uniform_buffer_indices()
        for io in &self.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                let buf_idx = io.buffer_index.expect("UniformBuffer must have buffer_index assigned");
                write!(out, "    constant ").ok();
                self.backend.pod_type_name_from_ty(vm.heap, io.ty, out);
                writeln!(out, " *u_{} [[buffer({})]],", io.name, buf_idx).ok();
            }
        }
        
        // Add scope uniforms buffer parameter if we have any
        if has_scope_uniforms {
            // Use a fixed buffer index for scope uniforms (after uniform buffers)
            let scope_uniform_buffer_idx = self.io.iter()
                .filter_map(|io| io.buffer_index)
                .max()
                .map(|m| m + 1)
                .unwrap_or(3);
            writeln!(out, "    constant IoScopeUniform *su [[buffer({})]],", scope_uniform_buffer_idx).ok();
        }
        
        let mut tex_idx = 0;
        let mut samp_idx = 0;
        for io in &self.io {
            match &io.kind {
                ShaderIoKind::Texture(tex_type) => {
                    let metal_type = match tex_type {
                        TextureType::Texture1d => "texture1d<float>",
                        TextureType::Texture1dArray => "texture1d_array<float>",
                        TextureType::Texture2d => "texture2d<float>",
                        TextureType::Texture2dArray => "texture2d_array<float>",
                        TextureType::Texture3d => "texture3d<float>",
                        TextureType::Texture3dArray => "texture3d<float>",
                        TextureType::TextureCube => "texturecube<float>",
                        TextureType::TextureCubeArray => "texturecube_array<float>",
                        TextureType::TextureDepth => "depth2d<float>",
                        TextureType::TextureDepthArray => "depth2d_array<float>",
                    };
                    writeln!(out, "    {} {} [[texture({})]],", metal_type, io.name, tex_idx).ok();
                    tex_idx += 1;
                }
                ShaderIoKind::Sampler(_) => {
                    writeln!(out, "    sampler {} [[sampler({})]],", io.name, samp_idx).ok();
                    samp_idx += 1;
                }
                _=>()
            }
        }
        
        writeln!(out, "    uint vid [[vertex_id]],").ok();
        writeln!(out, "    uint iid [[instance_id]]").ok();
        writeln!(out, ") {{").ok();
        
        writeln!(out, "    Io _io;").ok();
        writeln!(out, "    _io.vb = vb;").ok();
        writeln!(out, "    _io.i = i;").ok();
        writeln!(out, "    _io.u = u;").ok();
        
        if has_scope_uniforms {
            writeln!(out, "    _io.su = su;").ok();
        }
        
        for io in &self.io {
            match &io.kind {
                ShaderIoKind::UniformBuffer => {
                    writeln!(out, "    _io.u_{} = u_{};", io.name, io.name).ok();
                }
                ShaderIoKind::Texture(_) | ShaderIoKind::Sampler(_) => {
                    writeln!(out, "    _io.{} = {};", io.name, io.name).ok();
                }
                _=>()
            }
        }
        
        writeln!(out, "    IoVarying _v = {{}};").ok();  // Local varying struct, zero-initialized
        writeln!(out, "    IoV _iov;").ok();
        writeln!(out, "    _iov.v = &_v;").ok();  // Point to local varying (like fragment shader)
        writeln!(out, "    _iov.vid = vid;").ok();
        writeln!(out, "    _iov.iid = iid;").ok();
        writeln!(out, "    _iov.v->_iid = iid;").ok();  // Set before io_vertex so user can read it
        writeln!(out, "    io_vertex(_io, _iov);").ok();
        // Ensure instance id is set after user code in case they modified it
        writeln!(out, "    _iov.v->_iid = iid;").ok();
        writeln!(out, "    return _v;").ok();
        writeln!(out, "}}").ok();
    }

    pub fn metal_create_fragment_main_fn(&self, vm: &ScriptVm, out: &mut String) {
        let has_scope_uniforms = self.io.iter().any(|io| matches!(io.kind, ShaderIoKind::ScopeUniform));
        
        writeln!(out, "fragment IoFb fragment_main(").ok();
        writeln!(out, "    IoVarying v [[stage_in]],").ok();
        writeln!(out, "    constant IoVertexBuffer *vb [[buffer(0)]],").ok();
        writeln!(out, "    constant IoInstance *i [[buffer(1)]],").ok();
        write!(out, "    constant IoUniform *u [[buffer(2)]]").ok();
        
        // Use pre-assigned buffer indices from assign_uniform_buffer_indices()
        for io in &self.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                let buf_idx = io.buffer_index.expect("UniformBuffer must have buffer_index assigned");
                writeln!(out, ",").ok();
                write!(out, "    constant ").ok();
                self.backend.pod_type_name_from_ty(vm.heap, io.ty, out);
                write!(out, " *u_{} [[buffer({})]]", io.name, buf_idx).ok();
            }
        }
        
        // Add scope uniforms buffer parameter if we have any
        if has_scope_uniforms {
            let scope_uniform_buffer_idx = self.io.iter()
                .filter_map(|io| io.buffer_index)
                .max()
                .map(|m| m + 1)
                .unwrap_or(3);
            writeln!(out, ",").ok();
            write!(out, "    constant IoScopeUniform *su [[buffer({})]]", scope_uniform_buffer_idx).ok();
        }
        
        let mut tex_idx = 0;
        let mut samp_idx = 0;
        for io in &self.io {
            match &io.kind {
                ShaderIoKind::Texture(tex_type) => {
                    let metal_type = match tex_type {
                        TextureType::Texture1d => "texture1d<float>",
                        TextureType::Texture1dArray => "texture1d_array<float>",
                        TextureType::Texture2d => "texture2d<float>",
                        TextureType::Texture2dArray => "texture2d_array<float>",
                        TextureType::Texture3d => "texture3d<float>",
                        TextureType::Texture3dArray => "texture3d<float>",
                        TextureType::TextureCube => "texturecube<float>",
                        TextureType::TextureCubeArray => "texturecube_array<float>",
                        TextureType::TextureDepth => "depth2d<float>",
                        TextureType::TextureDepthArray => "depth2d_array<float>",
                    };
                    writeln!(out, ",").ok();
                    write!(out, "    {} {} [[texture({})]]", metal_type, io.name, tex_idx).ok();
                    tex_idx += 1;
                }
                ShaderIoKind::Sampler(_) => {
                    writeln!(out, ",").ok();
                    write!(out, "    sampler {} [[sampler({})]]", io.name, samp_idx).ok();
                    samp_idx += 1;
                }
                _=>()
            }
        }
        
        writeln!(out, ") {{").ok();
        
        writeln!(out, "    Io _io;").ok();
        writeln!(out, "    _io.vb = vb;").ok();
        writeln!(out, "    _io.i = i;").ok();
        writeln!(out, "    _io.u = u;").ok();
        
        if has_scope_uniforms {
            writeln!(out, "    _io.su = su;").ok();
        }
        
        for io in &self.io {
            match &io.kind {
                ShaderIoKind::UniformBuffer => {
                    writeln!(out, "    _io.u_{} = u_{};", io.name, io.name).ok();
                }
                ShaderIoKind::Texture(_) | ShaderIoKind::Sampler(_) => {
                    writeln!(out, "    _io.{} = {};", io.name, io.name).ok();
                }
                _=>()
            }
        }
        
        writeln!(out, "    IoFb _iofb;").ok();
        writeln!(out, "    IoF _iof;").ok();
        writeln!(out, "    _iof.v = &v;").ok();
        writeln!(out, "    _iof.fb = &_iofb;").ok();
        writeln!(out, "    io_fragment(_io, _iof);").ok();
        writeln!(out, "    return _iofb;").ok();
        writeln!(out, "}}").ok();
    }

    pub fn metal_create_io_fragment_struct(&self, _vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoF {{").ok();
        writeln!(out, "    thread IoVarying *v;").ok();
        writeln!(out, "    thread IoFb *fb;").ok();
        writeln!(out, "}};").ok();
    }
    
    pub fn metal_create_io_framebuffer_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoFb {{").ok();
        for io in &self.io {
            if let ShaderIoKind::FragmentOutput(index) = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(vm.heap, io.ty, out);
                writeln!(out, " fb{} [[color({})]];", index, index).ok();
            }
        }
        writeln!(out, "}};").ok();
    }
    
    pub fn metal_create_sampler_decls(&self, out: &mut String) {
        use crate::shader::{SamplerFilter, SamplerAddress, SamplerCoord};
        
        for (idx, sampler) in self.samplers.iter().enumerate() {
            let filter = match sampler.filter {
                SamplerFilter::Nearest => "nearest",
                SamplerFilter::Linear => "linear",
            };
            let address = match sampler.address {
                SamplerAddress::Repeat => "repeat",
                SamplerAddress::ClampToEdge => "clamp_to_edge",
                SamplerAddress::ClampToZero => "clamp_to_zero",
                SamplerAddress::MirroredRepeat => "mirrored_repeat",
            };
            let coord = match sampler.coord {
                SamplerCoord::Normalized => "normalized",
                SamplerCoord::Pixel => "pixel",
            };
            writeln!(out, "constexpr sampler _s{}(filter::{}, address::{}, coord::{});", 
                idx, filter, address, coord).ok();
        }
    }
}
