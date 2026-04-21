use crate::pod::{ScriptPodMat, ScriptPodTy};
use crate::shader::{ShaderIoKind, ShaderOutput, TextureType};
use crate::vm::ScriptVm;
use makepad_live_id::{id, LiveId};
use std::fmt::Write;

impl ShaderOutput {
    pub fn metal_create_helpers(&self, out: &mut String) {
        writeln!(out, "inline float4x4 _mp_inverse(float4x4 m) {{").ok();
        writeln!(out, "    float a00 = m[0][0];").ok();
        writeln!(out, "    float a01 = m[0][1];").ok();
        writeln!(out, "    float a02 = m[0][2];").ok();
        writeln!(out, "    float a03 = m[0][3];").ok();
        writeln!(out, "    float a10 = m[1][0];").ok();
        writeln!(out, "    float a11 = m[1][1];").ok();
        writeln!(out, "    float a12 = m[1][2];").ok();
        writeln!(out, "    float a13 = m[1][3];").ok();
        writeln!(out, "    float a20 = m[2][0];").ok();
        writeln!(out, "    float a21 = m[2][1];").ok();
        writeln!(out, "    float a22 = m[2][2];").ok();
        writeln!(out, "    float a23 = m[2][3];").ok();
        writeln!(out, "    float a30 = m[3][0];").ok();
        writeln!(out, "    float a31 = m[3][1];").ok();
        writeln!(out, "    float a32 = m[3][2];").ok();
        writeln!(out, "    float a33 = m[3][3];").ok();
        writeln!(out, "    float b00 = a00 * a11 - a01 * a10;").ok();
        writeln!(out, "    float b01 = a00 * a12 - a02 * a10;").ok();
        writeln!(out, "    float b02 = a00 * a13 - a03 * a10;").ok();
        writeln!(out, "    float b03 = a01 * a12 - a02 * a11;").ok();
        writeln!(out, "    float b04 = a01 * a13 - a03 * a11;").ok();
        writeln!(out, "    float b05 = a02 * a13 - a03 * a12;").ok();
        writeln!(out, "    float b06 = a20 * a31 - a21 * a30;").ok();
        writeln!(out, "    float b07 = a20 * a32 - a22 * a30;").ok();
        writeln!(out, "    float b08 = a20 * a33 - a23 * a30;").ok();
        writeln!(out, "    float b09 = a21 * a32 - a22 * a31;").ok();
        writeln!(out, "    float b10 = a21 * a33 - a23 * a31;").ok();
        writeln!(out, "    float b11 = a22 * a33 - a23 * a32;").ok();
        writeln!(
            out,
            "    float det = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;"
        )
        .ok();
        writeln!(out, "    if (det == 0.0) {{").ok();
        writeln!(
            out,
            "        return float4x4(float4(1.0, 0.0, 0.0, 0.0), float4(0.0, 1.0, 0.0, 0.0), float4(0.0, 0.0, 1.0, 0.0), float4(0.0, 0.0, 0.0, 1.0));"
        )
        .ok();
        writeln!(out, "    }}").ok();
        writeln!(out, "    float idet = 1.0 / det;").ok();
        writeln!(out, "    return float4x4(").ok();
        writeln!(
            out,
            "        float4((a11 * b11 - a12 * b10 + a13 * b09) * idet, (a02 * b10 - a01 * b11 - a03 * b09) * idet, (a31 * b05 - a32 * b04 + a33 * b03) * idet, (a22 * b04 - a21 * b05 - a23 * b03) * idet),"
        )
        .ok();
        writeln!(
            out,
            "        float4((a12 * b08 - a10 * b11 - a13 * b07) * idet, (a00 * b11 - a02 * b08 + a03 * b07) * idet, (a32 * b02 - a30 * b05 - a33 * b01) * idet, (a20 * b05 - a22 * b02 + a23 * b01) * idet),"
        )
        .ok();
        writeln!(
            out,
            "        float4((a10 * b10 - a11 * b08 + a13 * b06) * idet, (a01 * b08 - a00 * b10 - a03 * b06) * idet, (a30 * b04 - a31 * b02 + a33 * b00) * idet, (a21 * b02 - a20 * b04 - a23 * b00) * idet),"
        )
        .ok();
        writeln!(
            out,
            "        float4((a11 * b07 - a10 * b09 - a12 * b06) * idet, (a00 * b09 - a01 * b07 + a02 * b06) * idet, (a31 * b01 - a30 * b03 - a32 * b00) * idet, (a20 * b03 - a21 * b01 + a22 * b00) * idet)"
        )
        .ok();
        writeln!(out, "    );").ok();
        writeln!(out, "}}").ok();
    }

    pub fn metal_create_io_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct Io {{").ok();
        writeln!(out, "    constant IoUniform *u;").ok();
        writeln!(out, "    thread IoInstance *i;").ok();

        // Add scope uniforms buffer pointer if we have any scope uniforms
        let has_scope_uniforms = self
            .io
            .iter()
            .any(|io| matches!(io.kind, ShaderIoKind::ScopeUniform));
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
                        TextureType::TextureVideo => "texture2d<float>", // Video textures are standard texture2d on Metal
                    };
                    writeln!(out, "    {} {};", metal_type, io.name).ok();
                }
                ShaderIoKind::Sampler(_) => {
                    writeln!(out, "    sampler {};", io.name).ok();
                }
                ShaderIoKind::UniformBuffer => {
                    write!(out, "    constant ").ok();
                    self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                    writeln!(out, " *u_{};", io.name).ok();
                }
                _ => (),
            }
        }

        let mut have_vb = false;
        for io in &self.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                if !have_vb {
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
        let has_scope_uniforms = self
            .io
            .iter()
            .any(|io| matches!(io.kind, ShaderIoKind::ScopeUniform));
        if !has_scope_uniforms {
            return;
        }

        writeln!(out, "struct IoScopeUniform {{").ok();
        for io in &self.io {
            if let ShaderIoKind::ScopeUniform = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " {};", io.name).ok();
            }
        }
        writeln!(out, "}};").ok();
    }

    pub fn metal_create_instance_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoInstanceRaw {{").ok();

        // 1. Output Dyn instance fields first (order doesn't matter, just output as encountered)
        // Use packed types to match CPU-side repr(C) struct alignment
        for io in &self.io {
            if let ShaderIoKind::DynInstance = io.kind {
                let pod_ty = vm.bx.heap.pod_type_ref(io.ty);
                if matches!(pod_ty.ty, ScriptPodTy::Mat(ScriptPodMat::Mat4x4f)) {
                    for col in 0..4 {
                        writeln!(out, "    packed_float4 {}_{};", io.name, col).ok();
                    }
                } else {
                    write!(out, "    ").ok();
                    self.backend
                        .pod_type_name_packed_from_ty(&vm.bx.heap, io.ty, out);
                    writeln!(out, " {};", io.name).ok();
                }
            }
        }

        // 2. Output Rust instance fields last (already in correct order from pre_collect_rust_instance_io)
        // Use packed types to match CPU-side repr(C) struct alignment
        for io in &self.io {
            if let ShaderIoKind::RustInstance = io.kind {
                let pod_ty = vm.bx.heap.pod_type_ref(io.ty);
                if matches!(pod_ty.ty, ScriptPodTy::Mat(ScriptPodMat::Mat4x4f)) {
                    for col in 0..4 {
                        writeln!(out, "    packed_float4 {}_{};", io.name, col).ok();
                    }
                } else {
                    write!(out, "    ").ok();
                    self.backend
                        .pod_type_name_packed_from_ty(&vm.bx.heap, io.ty, out);
                    writeln!(out, " {};", io.name).ok();
                }
            }
        }

        writeln!(out, "}};").ok();

        writeln!(out, "struct IoInstance {{").ok();
        for io in &self.io {
            if let ShaderIoKind::DynInstance = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " {};", io.name).ok();
            }
        }
        for io in &self.io {
            if let ShaderIoKind::RustInstance = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " {};", io.name).ok();
            }
        }
        writeln!(out, "}};").ok();

        writeln!(
            out,
            "inline IoInstance _mp_decode_instance(constant IoInstanceRaw &raw) {{"
        )
        .ok();
        writeln!(out, "    IoInstance out_instance;").ok();
        for io in &self.io {
            if let ShaderIoKind::DynInstance = io.kind {
                let pod_ty = vm.bx.heap.pod_type_ref(io.ty);
                if matches!(pod_ty.ty, ScriptPodTy::Mat(ScriptPodMat::Mat4x4f)) {
                    writeln!(
                        out,
                        "    out_instance.{0} = float4x4(float4(raw.{0}_0), float4(raw.{0}_1), float4(raw.{0}_2), float4(raw.{0}_3));",
                        io.name
                    )
                    .ok();
                } else {
                    writeln!(out, "    out_instance.{0} = raw.{0};", io.name).ok();
                }
            }
        }
        for io in &self.io {
            if let ShaderIoKind::RustInstance = io.kind {
                let pod_ty = vm.bx.heap.pod_type_ref(io.ty);
                if matches!(pod_ty.ty, ScriptPodTy::Mat(ScriptPodMat::Mat4x4f)) {
                    writeln!(
                        out,
                        "    out_instance.{0} = float4x4(float4(raw.{0}_0), float4(raw.{0}_1), float4(raw.{0}_2), float4(raw.{0}_3));",
                        io.name
                    )
                    .ok();
                } else {
                    writeln!(out, "    out_instance.{0} = raw.{0};", io.name).ok();
                }
            }
        }
        writeln!(out, "    return out_instance;").ok();
        writeln!(out, "}}").ok();
    }

    pub fn metal_create_uniform_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoUniform {{").ok();
        for io in &self.io {
            match &io.kind {
                ShaderIoKind::Uniform => {
                    write!(out, "    ").ok();
                    self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                    writeln!(out, " {};", io.name).ok();
                }
                _ => (),
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
                    self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                    writeln!(out, " {};", io.name).ok();
                }
                _ => (),
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
                self.backend
                    .pod_type_name_packed_from_ty(&vm.bx.heap, io.ty, out);
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
        let has_scope_uniforms = self
            .io
            .iter()
            .any(|io| matches!(io.kind, ShaderIoKind::ScopeUniform));

        writeln!(out, "vertex IoVarying vertex_main(").ok();
        writeln!(out, "    constant IoVertexBuffer *vb [[buffer(0)]],").ok();
        writeln!(out, "    constant IoInstanceRaw *i_raw [[buffer(1)]],").ok();
        writeln!(out, "    constant IoUniform *u [[buffer(2)]],").ok();

        // Use pre-assigned buffer indices from assign_uniform_buffer_indices()
        for io in &self.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                let buf_idx = io
                    .buffer_index
                    .expect("UniformBuffer must have buffer_index assigned");
                write!(out, "    constant ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " *u_{} [[buffer({})]],", io.name, buf_idx).ok();
            }
        }

        // Add scope uniforms buffer parameter if we have any
        if has_scope_uniforms {
            // Use a fixed buffer index for scope uniforms (after uniform buffers)
            let scope_uniform_buffer_idx = self
                .io
                .iter()
                .filter_map(|io| io.buffer_index)
                .max()
                .map(|m| m + 1)
                .unwrap_or(3);
            writeln!(
                out,
                "    constant IoScopeUniform *su [[buffer({})]],",
                scope_uniform_buffer_idx
            )
            .ok();
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
                        TextureType::TextureVideo => "texture2d<float>", // Video textures are standard texture2d on Metal
                    };
                    writeln!(
                        out,
                        "    {} {} [[texture({})]],",
                        metal_type, io.name, tex_idx
                    )
                    .ok();
                    tex_idx += 1;
                }
                ShaderIoKind::Sampler(_) => {
                    writeln!(out, "    sampler {} [[sampler({})]],", io.name, samp_idx).ok();
                    samp_idx += 1;
                }
                _ => (),
            }
        }

        writeln!(out, "    uint vid [[vertex_id]],").ok();
        writeln!(out, "    uint iid [[instance_id]]").ok();
        writeln!(out, ") {{").ok();

        writeln!(out, "    Io _io;").ok();
        writeln!(
            out,
            "    IoInstance _inst = _mp_decode_instance(i_raw[iid]);"
        )
        .ok();
        writeln!(out, "    _io.vb = vb;").ok();
        writeln!(out, "    _io.i = &_inst;").ok();
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
                _ => (),
            }
        }

        writeln!(out, "    IoVarying _v = {{}};").ok(); // Local varying struct, zero-initialized
        writeln!(out, "    IoV _iov;").ok();
        writeln!(out, "    _iov.v = &_v;").ok(); // Point to local varying (like fragment shader)
        writeln!(out, "    _iov.vid = vid;").ok();
        writeln!(out, "    _iov.iid = iid;").ok();
        writeln!(out, "    _iov.v->_iid = iid;").ok(); // Set before io_vertex so user can read it

        // Check if vertex shader returns Vec4f - if so, assign to _position automatically
        let vertex_returns_vec4f = self
            .functions
            .iter()
            .find(|f| f.name == id!(vertex))
            .map(|f| f.ret == vm.bx.code.builtins.pod.pod_vec4f)
            .unwrap_or(false);

        if vertex_returns_vec4f {
            writeln!(out, "    _v._position = io_vertex(_io, _iov);").ok();
        } else {
            writeln!(out, "    io_vertex(_io, _iov);").ok();
        }
        // Ensure instance id is set after user code in case they modified it
        writeln!(out, "    _iov.v->_iid = iid;").ok();
        writeln!(out, "    return _v;").ok();
        writeln!(out, "}}").ok();
    }

    pub fn metal_create_fragment_main_fn(&self, vm: &ScriptVm, out: &mut String) {
        let has_scope_uniforms = self
            .io
            .iter()
            .any(|io| matches!(io.kind, ShaderIoKind::ScopeUniform));

        writeln!(out, "fragment IoFb fragment_main(").ok();
        writeln!(out, "    IoVarying v [[stage_in]],").ok();
        writeln!(out, "    constant IoVertexBuffer *vb [[buffer(0)]],").ok();
        writeln!(out, "    constant IoInstanceRaw *i_raw [[buffer(1)]],").ok();
        write!(out, "    constant IoUniform *u [[buffer(2)]]").ok();

        // Use pre-assigned buffer indices from assign_uniform_buffer_indices()
        for io in &self.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                let buf_idx = io
                    .buffer_index
                    .expect("UniformBuffer must have buffer_index assigned");
                writeln!(out, ",").ok();
                write!(out, "    constant ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                write!(out, " *u_{} [[buffer({})]]", io.name, buf_idx).ok();
            }
        }

        // Add scope uniforms buffer parameter if we have any
        if has_scope_uniforms {
            let scope_uniform_buffer_idx = self
                .io
                .iter()
                .filter_map(|io| io.buffer_index)
                .max()
                .map(|m| m + 1)
                .unwrap_or(3);
            writeln!(out, ",").ok();
            write!(
                out,
                "    constant IoScopeUniform *su [[buffer({})]]",
                scope_uniform_buffer_idx
            )
            .ok();
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
                        TextureType::TextureVideo => "texture2d<float>", // Video textures are standard texture2d on Metal
                    };
                    writeln!(out, ",").ok();
                    write!(
                        out,
                        "    {} {} [[texture({})]]",
                        metal_type, io.name, tex_idx
                    )
                    .ok();
                    tex_idx += 1;
                }
                ShaderIoKind::Sampler(_) => {
                    writeln!(out, ",").ok();
                    write!(out, "    sampler {} [[sampler({})]]", io.name, samp_idx).ok();
                    samp_idx += 1;
                }
                _ => (),
            }
        }

        writeln!(out, ") {{").ok();

        writeln!(out, "    Io _io;").ok();
        writeln!(
            out,
            "    IoInstance _inst = _mp_decode_instance(i_raw[v._iid]);"
        )
        .ok();
        writeln!(out, "    _io.vb = vb;").ok();
        writeln!(out, "    _io.i = &_inst;").ok();
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
                _ => (),
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
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " fb{} [[color({})]];", index, index).ok();
            }
        }
        writeln!(out, "}};").ok();
    }

    pub fn metal_create_sampler_decls(&self, out: &mut String) {
        use crate::shader::{SamplerAddress, SamplerCoord, SamplerFilter};

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
            writeln!(
                out,
                "constexpr sampler _s{}(filter::{}, mip_filter::linear, address::{}, coord::{});",
                idx, filter, address, coord
            )
            .ok();
        }
    }
}
