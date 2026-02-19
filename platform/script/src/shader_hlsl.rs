use crate::pod::ScriptPodTy;
use crate::shader::{SamplerAddress, SamplerFilter, ShaderIoKind, ShaderOutput, TextureType};
use crate::vm::ScriptVm;
use makepad_live_id::{id, LiveId};
use std::fmt::Write;

impl ShaderOutput {
    fn hlsl_is_integer_like_input(vm: &ScriptVm, ty: crate::ScriptPodType) -> bool {
        let pod_ty = vm.bx.heap.pod_type_ref(ty);
        match pod_ty.ty {
            ScriptPodTy::U32
            | ScriptPodTy::I32
            | ScriptPodTy::Bool
            | ScriptPodTy::AtomicU32
            | ScriptPodTy::AtomicI32 => true,
            ScriptPodTy::Vec(vec_ty) => matches!(
                vec_ty,
                crate::pod::ScriptPodVec::Vec2u
                    | crate::pod::ScriptPodVec::Vec3u
                    | crate::pod::ScriptPodVec::Vec4u
                    | crate::pod::ScriptPodVec::Vec2i
                    | crate::pod::ScriptPodVec::Vec3i
                    | crate::pod::ScriptPodVec::Vec4i
                    | crate::pod::ScriptPodVec::Vec2b
                    | crate::pod::ScriptPodVec::Vec3b
                    | crate::pod::ScriptPodVec::Vec4b
            ),
            _ => false,
        }
    }

    fn hlsl_slot_chunks(slots: usize) -> Vec<usize> {
        match slots {
            0 => Vec::new(),
            // Keep matrix layouts aligned with D3D input layout chunking.
            9 => vec![3, 3, 3],
            16 => vec![4, 4, 4, 4],
            _ => {
                let mut rem = slots;
                let mut chunks = Vec::new();
                while rem > 0 {
                    let chunk = rem.min(4);
                    chunks.push(chunk);
                    rem -= chunk;
                }
                chunks
            }
        }
    }

    fn hlsl_chunk_ty(slots: usize) -> &'static str {
        match slots {
            1 => "float",
            2 => "float2",
            3 => "float3",
            4 => "float4",
            _ => "float4",
        }
    }

    fn hlsl_input_needs_chunks(vm: &ScriptVm, ty: crate::ScriptPodType) -> bool {
        let pod_ty = vm.bx.heap.pod_type_ref(ty);
        let slots = pod_ty.ty.slots();
        slots > 4 || matches!(pod_ty.ty, ScriptPodTy::Struct { .. })
    }

    fn hlsl_reconstruct_from_scalars(
        &self,
        vm: &ScriptVm,
        ty: &crate::pod::ScriptPodTypeInline,
        scalars: &[String],
        scalar_idx: &mut usize,
    ) -> String {
        match &ty.data.ty {
            ScriptPodTy::Struct { fields, .. } => {
                let struct_name = if let Some(name) = vm.bx.heap.pod_type_name(ty.self_ref) {
                    format!("{}", self.backend.map_pod_name(name))
                } else {
                    format!("S{}", ty.self_ref.index)
                };
                let mut field_exprs = Vec::new();
                for field in fields {
                    field_exprs.push(self.hlsl_reconstruct_from_scalars(
                        vm,
                        &field.ty,
                        scalars,
                        scalar_idx,
                    ));
                }
                format!("consfn_{}({})", struct_name, field_exprs.join(", "))
            }
            _ => {
                let slot_count = ty.data.ty.slots();
                if slot_count <= 1 {
                    let v = scalars
                        .get(*scalar_idx)
                        .cloned()
                        .unwrap_or_else(|| "0.0".to_string());
                    *scalar_idx += 1;
                    return v;
                }
                let start = *scalar_idx;
                let end = (start + slot_count).min(scalars.len());
                *scalar_idx = end;
                let args = scalars[start..end].join(", ");
                let mut ty_name = String::new();
                self.backend.pod_type_name(ty, &mut ty_name);
                if matches!(ty.data.ty, ScriptPodTy::Mat(_)) {
                    // Mat4f/MatNxM are stored column-major in CPU slot streams.
                    // HLSL scalar constructors consume row-major order, so transpose.
                    format!("transpose({}({}))", ty_name, args)
                } else {
                    format!("{}({})", ty_name, args)
                }
            }
        }
    }

    fn hlsl_reconstruct_input_value(
        &self,
        vm: &ScriptVm,
        ty: crate::ScriptPodType,
        input_prefix: &str,
        io_name: LiveId,
    ) -> String {
        let pod_ty = vm.bx.heap.pod_type_ref(ty);
        let slots = pod_ty.ty.slots();
        let io_name = self.backend.map_io_name(io_name);

        if !Self::hlsl_input_needs_chunks(vm, ty) {
            return format!("input.{}_{}", input_prefix, io_name);
        }

        let mut args: Vec<String> = Vec::new();
        for (chunk_idx, chunk_slots) in Self::hlsl_slot_chunks(slots).into_iter().enumerate() {
            let base = format!("input.{}_{}_{}", input_prefix, io_name, chunk_idx);
            if chunk_slots == 1 {
                args.push(base);
            } else {
                for comp in 0..chunk_slots {
                    let swiz = match comp {
                        0 => "x",
                        1 => "y",
                        2 => "z",
                        3 => "w",
                        _ => "x",
                    };
                    args.push(format!("{}.{}", base, swiz));
                }
            }
        }
        let inline = crate::pod::ScriptPodTypeInline {
            self_ref: ty,
            data: pod_ty.clone(),
        };
        let mut scalar_idx = 0usize;
        self.hlsl_reconstruct_from_scalars(vm, &inline, &args, &mut scalar_idx)
    }

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
                let io_name = self.backend.map_io_name(io.name);
                writeln!(out, " {};", io_name).ok();
            }
        }

        // 2. Output Rust instance fields last (already in correct order from pre_collect_rust_instance_io)
        for io in &self.io {
            if let ShaderIoKind::RustInstance = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                let io_name = self.backend.map_io_name(io.name);
                writeln!(out, " {};", io_name).ok();
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
                    let io_name = self.backend.map_io_name(io.name);
                    writeln!(out, " u_{};", io_name).ok();
                }
                _ => (),
            }
        }
        writeln!(out, "}};").ok();
    }

    pub fn hlsl_create_scope_uniform_cbuffer(&self, vm: &ScriptVm, out: &mut String) {
        let has_scope_uniforms = self
            .io
            .iter()
            .any(|io| matches!(io.kind, ShaderIoKind::ScopeUniform));
        if !has_scope_uniforms {
            return;
        }

        let scope_uniform_buffer_idx = self
            .io
            .iter()
            .filter_map(|io| io.buffer_index)
            .max()
            .map(|m| m + 1)
            .unwrap_or(3);

        writeln!(
            out,
            "cbuffer IoScopeUniform : register(b{}) {{",
            scope_uniform_buffer_idx
        )
        .ok();
        for io in &self.io {
            if let ShaderIoKind::ScopeUniform = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                let io_name = self.backend.map_io_name(io.name);
                writeln!(out, " su_{};", io_name).ok();
            }
        }
        writeln!(out, "}};").ok();
    }

    pub fn hlsl_create_uniform_buffer_cbuffers(&self, vm: &ScriptVm, out: &mut String) {
        // Create cbuffer declarations for each uniform buffer using pre-assigned buffer indices
        for io in &self.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                let buf_idx = io
                    .buffer_index
                    .expect("UniformBuffer must have buffer_index assigned");
                let io_name = self.backend.map_io_name(io.name);
                write!(out, "cbuffer cb_{} : register(b{}) {{ ", io_name, buf_idx).ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " u_{}; }};", io_name).ok();
            }
        }
    }

    pub fn hlsl_create_varying_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoVarying {{").ok();
        let mut semantic_idx = 0usize;
        for io in &self.io {
            match io.kind {
                ShaderIoKind::DynInstance | ShaderIoKind::RustInstance | ShaderIoKind::Varying => {
                    write!(out, "    ").ok();
                    if Self::hlsl_is_integer_like_input(vm, io.ty) {
                        write!(out, "nointerpolation ").ok();
                    }
                    self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                    writeln!(
                        out,
                        " {} : VARY{};",
                        self.backend.map_io_name(io.name),
                        index_to_semantic(semantic_idx)
                    )
                    .ok();
                    semantic_idx += 1;
                }
                _ => (),
            }
        }
        writeln!(out, "    nointerpolation uint _iid : TEXCOORD0;").ok();
        writeln!(out, "    float4 _position : SV_POSITION;").ok();
        writeln!(out, "}};").ok();
    }

    pub fn hlsl_create_vertex_buffer_struct(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoVertexBuffer {{").ok();
        for io in &self.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                let io_name = self.backend.map_io_name(io.name);
                writeln!(out, " {};", io_name).ok();
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
                let pod_ty = vm.bx.heap.pod_type_ref(io.ty);
                let slots = pod_ty.ty.slots();
                let io_name = self.backend.map_io_name(io.name);
                if Self::hlsl_input_needs_chunks(vm, io.ty) {
                    for (chunk_idx, chunk_slots) in Self::hlsl_slot_chunks(slots).into_iter().enumerate()
                    {
                        writeln!(
                            out,
                            "    {} vb_{}_{} : GEOM{}{};",
                            Self::hlsl_chunk_ty(chunk_slots),
                            io_name,
                            chunk_idx,
                            index_to_semantic(semantic_idx),
                            chunk_idx
                        )
                        .ok();
                    }
                } else {
                    write!(out, "    ").ok();
                    self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                    writeln!(
                        out,
                        " vb_{} : GEOM{};",
                        io_name,
                        index_to_semantic(semantic_idx)
                    )
                    .ok();
                }
                semantic_idx += 1;
            }
        }

        // Instance fields
        semantic_idx = 0;
        // Dyn instance fields first
        for io in &self.io {
            if let ShaderIoKind::DynInstance = io.kind {
                let pod_ty = vm.bx.heap.pod_type_ref(io.ty);
                let slots = pod_ty.ty.slots();
                let io_name = self.backend.map_io_name(io.name);
                if Self::hlsl_input_needs_chunks(vm, io.ty) {
                    for (chunk_idx, chunk_slots) in Self::hlsl_slot_chunks(slots).into_iter().enumerate()
                    {
                        writeln!(
                            out,
                            "    {} i_{}_{} : INST{}{};",
                            Self::hlsl_chunk_ty(chunk_slots),
                            io_name,
                            chunk_idx,
                            index_to_semantic(semantic_idx),
                            chunk_idx
                        )
                        .ok();
                    }
                } else {
                    write!(out, "    ").ok();
                    self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                    writeln!(
                        out,
                        " i_{} : INST{};",
                        io_name,
                        index_to_semantic(semantic_idx)
                    )
                    .ok();
                }
                semantic_idx += 1;
            }
        }
        // Rust instance fields
        for io in &self.io {
            if let ShaderIoKind::RustInstance = io.kind {
                let pod_ty = vm.bx.heap.pod_type_ref(io.ty);
                let slots = pod_ty.ty.slots();
                let io_name = self.backend.map_io_name(io.name);
                if Self::hlsl_input_needs_chunks(vm, io.ty) {
                    for (chunk_idx, chunk_slots) in Self::hlsl_slot_chunks(slots).into_iter().enumerate()
                    {
                        writeln!(
                            out,
                            "    {} i_{}_{} : INST{}{};",
                            Self::hlsl_chunk_ty(chunk_slots),
                            io_name,
                            chunk_idx,
                            index_to_semantic(semantic_idx),
                            chunk_idx
                        )
                        .ok();
                    }
                } else {
                    write!(out, "    ").ok();
                    self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                    writeln!(
                        out,
                        " i_{} : INST{};",
                        io_name,
                        index_to_semantic(semantic_idx)
                    )
                    .ok();
                }
                semantic_idx += 1;
            }
        }

        writeln!(out, "    uint vid : SV_VertexID;").ok();
        writeln!(out, "    uint iid : SV_InstanceID;").ok();
        writeln!(out, "}};").ok();
    }

    pub fn hlsl_create_io_structs(&self, vm: &ScriptVm, out: &mut String) {
        // IoV for vertex shader
        writeln!(out, "struct IoV {{").ok();
        writeln!(out, "    IoVarying v;").ok();
        writeln!(out, "    IoVertexBuffer vb;").ok();
        writeln!(out, "    IoInstance i;").ok();
        writeln!(out, "    uint vid;").ok();
        writeln!(out, "    uint iid;").ok();
        writeln!(out, "}};").ok();
        writeln!(out).ok();

        // IoF for fragment shader
        writeln!(out, "struct IoF {{").ok();
        writeln!(out, "    IoVarying v;").ok();
        for io in &self.io {
            if let ShaderIoKind::FragmentOutput(index) = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " fb{};", index).ok();
            }
        }
        writeln!(out, "}};").ok();
        writeln!(out).ok();

        // Io for passing to shader functions
        writeln!(out, "struct Io {{").ok();
        writeln!(out, "}};").ok();
        writeln!(out, "static Io _mp_io;").ok();
        writeln!(out, "static IoV _mp_iov;").ok();
        writeln!(out, "static IoF _mp_iof;").ok();
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
                        TextureType::TextureVideo => "Texture2D", // Video textures are standard Texture2D on HLSL
                    };
                    let io_name = self.backend.map_io_name(io.name);
                    writeln!(out, "{} {} : register(t{});", hlsl_type, io_name, tex_idx).ok();
                    tex_idx += 1;
                }
                ShaderIoKind::Sampler(_) => {
                    let io_name = self.backend.map_io_name(io.name);
                    writeln!(out, "SamplerState {} : register(s{});", io_name, samp_idx).ok();
                    samp_idx += 1;
                }
                _ => (),
            }
        }

        for (idx, sampler) in self.samplers.iter().enumerate() {
            let filter = match sampler.filter {
                SamplerFilter::Nearest => "MIN_MAG_MIP_POINT",
                SamplerFilter::Linear => "MIN_MAG_MIP_LINEAR",
            };
            let address = match sampler.address {
                SamplerAddress::Repeat => "Wrap",
                SamplerAddress::ClampToEdge => "Clamp",
                SamplerAddress::ClampToZero => "Border",
                SamplerAddress::MirroredRepeat => "Mirror",
            };
            writeln!(
                out,
                "SamplerState _s{} {{ Filter = {}; AddressU = {}; AddressV = {}; AddressW = {}; }};",
                idx, filter, address, address, address
            )
            .ok();
        }
    }

    pub fn hlsl_create_vertex_fn(&self, vm: &ScriptVm, out: &mut String) {
        writeln!(out, "IoVarying vertex_main(VertexInput input) {{").ok();
        writeln!(out, "    _mp_iov.vid = input.vid;").ok();
        writeln!(out, "    _mp_iov.iid = input.iid;").ok();
        writeln!(out, "    _mp_iov.v._iid = input.iid;").ok();
        writeln!(out).ok();

        // Copy vertex/instance input into IoV so helper functions can access it.
        for io in &self.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                let expr = self.hlsl_reconstruct_input_value(vm, io.ty, "vb", io.name);
                let io_name = self.backend.map_io_name(io.name);
                writeln!(out, "    _mp_iov.vb.{0} = {1};", io_name, expr).ok();
            }
        }
        for io in &self.io {
            if let ShaderIoKind::DynInstance = io.kind {
                let expr = self.hlsl_reconstruct_input_value(vm, io.ty, "i", io.name);
                let io_name = self.backend.map_io_name(io.name);
                writeln!(out, "    _mp_iov.i.{0} = {1};", io_name, expr).ok();
                writeln!(out, "    _mp_iov.v.{0} = _mp_iov.i.{0};", io_name).ok();
            }
        }
        for io in &self.io {
            if let ShaderIoKind::RustInstance = io.kind {
                let expr = self.hlsl_reconstruct_input_value(vm, io.ty, "i", io.name);
                let io_name = self.backend.map_io_name(io.name);
                writeln!(out, "    _mp_iov.i.{0} = {1};", io_name, expr).ok();
                writeln!(out, "    _mp_iov.v.{0} = _mp_iov.i.{0};", io_name).ok();
            }
        }

        // Check if vertex shader returns Vec4f - if so, assign to _position automatically
        let vertex_returns_vec4f = self
            .functions
            .iter()
            .find(|f| f.name == id!(vertex))
            .map(|f| f.ret == vm.bx.code.builtins.pod.pod_vec4f)
            .unwrap_or(false);

        let vertex_fn_name = self.backend.map_function_name("io_vertex");
        if vertex_returns_vec4f {
            writeln!(out, "    _mp_iov.v._position = {}();", vertex_fn_name).ok();
        } else {
            writeln!(out, "    {}();", vertex_fn_name).ok();
        }
        writeln!(out, "    _mp_iov.v._iid = input.iid;").ok();
        writeln!(out, "    return _mp_iov.v;").ok();
        writeln!(out, "}}").ok();
    }

    pub fn hlsl_create_fragment_fn(&self, _vm: &ScriptVm, out: &mut String) {
        writeln!(out, "IoFb pixel_main(IoVarying v) {{").ok();
        writeln!(out, "    _mp_iof.v = v;").ok();
        let fragment_fn_name = self.backend.map_function_name("io_fragment");
        writeln!(out, "    {}();", fragment_fn_name).ok();
        writeln!(out, "    IoFb _iofb;").ok();
        for io in &self.io {
            if let ShaderIoKind::FragmentOutput(index) = io.kind {
                writeln!(out, "    _iofb.fb{0} = _mp_iof.fb{0};", index).ok();
            }
        }
        writeln!(out, "    return _iofb;").ok();
        writeln!(out, "}}").ok();
    }
}

/// Convert index to HLSL semantic suffix (A..Z, AA..AZ, BA..).
pub fn index_to_semantic(index: usize) -> String {
    const LETTERS: &[u8; 26] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut value = index;
    let mut out = Vec::new();
    loop {
        let rem = value % 26;
        out.push(LETTERS[rem] as char);
        if value < 26 {
            break;
        }
        value = value / 26 - 1;
    }
    out.iter().rev().collect()
}
