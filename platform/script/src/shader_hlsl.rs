use crate::pod::ScriptPodTy;
use crate::shader::{ShaderIoKind, ShaderOutput, TextureType};
use crate::vm::ScriptVm;
use makepad_live_id::{id, LiveId};
use std::collections::BTreeSet;
use std::fmt::Write;

/// Registers a vs_5_0 vertex shader reads (its input elements and
/// system values) and its interface to the pixel stage holds (vs_5_0
/// outputs, ps_5_0 inputs).
const HLSL_STAGE_REGISTERS: usize = 32;

/// The declaration of the buffer a vertex shader reads its instance records
/// from ([`ShaderOutput::hlsl_instances_from_buffer`]); its register follows.
const HLSL_INSTANCE_BUFFER_DECL: &str = "Buffer<uint> _mp_inst : register(t";

/// The t register of the instance buffer `hlsl` reads its instance records
/// from, or None when its instances arrive as input elements.
pub fn hlsl_instance_buffer_register(hlsl: &str) -> Option<u32> {
    let start = hlsl.find(HLSL_INSTANCE_BUFFER_DECL)? + HLSL_INSTANCE_BUFFER_DECL.len();
    let end = start + hlsl[start..].find(')')?;
    hlsl[start..end].parse().ok()
}

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

    /// Input elements `hlsl_create_vertex_input_struct` declares for one field.
    fn hlsl_input_elements(vm: &ScriptVm, ty: crate::ScriptPodType) -> usize {
        if Self::hlsl_input_needs_chunks(vm, ty) {
            Self::hlsl_slot_chunks(vm.bx.heap.pod_type_ref(ty).ty.slots()).len()
        } else {
            1
        }
    }

    /// Made of 32-bit float lanes only, which the instance layout packs back
    /// to back.
    fn hlsl_is_f32_lanes(ty: &ScriptPodTy) -> bool {
        match ty {
            ScriptPodTy::F32 | ScriptPodTy::Mat(_) => true,
            ScriptPodTy::Vec(vec_ty) => matches!(
                vec_ty,
                crate::pod::ScriptPodVec::Vec2f
                    | crate::pod::ScriptPodVec::Vec3f
                    | crate::pod::ScriptPodVec::Vec4f
            ),
            ScriptPodTy::Struct { fields, .. } => {
                !ty.has_compact_format()
                    && fields.iter().all(|f| Self::hlsl_is_f32_lanes(&f.ty.data.ty))
            }
            _ => false,
        }
    }

    fn hlsl_is_instance(kind: &ShaderIoKind) -> bool {
        matches!(kind, ShaderIoKind::DynInstance | ShaderIoKind::RustInstance)
    }

    /// The vertex stage reads the instance record from a buffer by
    /// SV_InstanceID instead of from input elements when one element per
    /// instance field, the geometry's and the two system values would exceed
    /// vs_5_0's 32 input registers. Only for records of float lanes: those
    /// sit back to back in the instance buffer, so a field's offset is the
    /// sum of the slots before it.
    pub fn hlsl_instances_from_buffer(&self, vm: &ScriptVm) -> bool {
        let mut elements = 2;
        let mut instances = 0;
        for io in &self.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                elements += Self::hlsl_input_elements(vm, io.ty);
            } else if Self::hlsl_is_instance(&io.kind) {
                if !Self::hlsl_is_f32_lanes(&vm.bx.heap.pod_type_ref(io.ty).ty) {
                    return false;
                }
                elements += Self::hlsl_input_elements(vm, io.ty);
                instances += 1;
            }
        }
        instances > 0 && elements > HLSL_STAGE_REGISTERS
    }

    /// The instance fields in record order (dyn first, then Rust) with their
    /// f32 offsets, and the record's stride in f32s.
    fn hlsl_instance_offsets(&self, vm: &ScriptVm) -> (Vec<(usize, usize)>, usize) {
        let mut offsets = Vec::new();
        let mut offset = 0;
        for rust in [false, true] {
            for (index, io) in self.io.iter().enumerate() {
                let wanted = match io.kind {
                    ShaderIoKind::DynInstance => !rust,
                    ShaderIoKind::RustInstance => rust,
                    _ => false,
                };
                if wanted {
                    offsets.push((index, offset));
                    offset += vm.bx.heap.pod_type_ref(io.ty).ty.slots();
                }
            }
        }
        (offsets, offset)
    }

    /// The instance fields the stage interface carries: every one, unless
    /// that could exceed 32 registers (one per scalar or vector, one per
    /// matrix column, before the compiler packs them); then only those the
    /// pixel stage reads (`_mp_iof.v.<field>`), the rule WebGL 2 follows.
    /// The vertex stage reads instance fields from `_mp_iov.i` and never
    /// writes them, so a field the pixel stage does not read only cost a
    /// register. None keeps every field.
    fn hlsl_varying_instances(&self, vm: &ScriptVm) -> Option<BTreeSet<LiveId>> {
        let mut registers = 2;
        for io in &self.io {
            if Self::hlsl_is_instance(&io.kind) || matches!(io.kind, ShaderIoKind::Varying) {
                registers += vm.bx.heap.pod_type_ref(io.ty).ty.slots().div_ceil(4).max(1);
            }
        }
        if registers <= HLSL_STAGE_REGISTERS {
            return None;
        }
        let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
        let pixel_reads = |field: &str| {
            let needle = format!("_mp_iof.v.{}", field);
            self.functions.iter().any(|f| {
                f.out.match_indices(&needle).any(|(at, _)| {
                    !f.out[at + needle.len()..].chars().next().is_some_and(is_ident)
                })
            })
        };
        Some(
            self.io
                .iter()
                .filter(|io| Self::hlsl_is_instance(&io.kind))
                .filter(|io| pixel_reads(&self.backend.map_io_name(io.name)))
                .map(|io| io.name)
                .collect(),
        )
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
                    field_exprs.push(
                        self.hlsl_reconstruct_from_scalars(vm, &field.ty, scalars, scalar_idx),
                    );
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
        // The invocation's shared loop-pass counter
        // (shader_control::SHADER_ITERATION_BUDGET).
        writeln!(out, "static uint _mp_iter = 0;").ok();
        // Packed vertex attribute unpackers (map / vector geometry).
        // Two f16s or four unorm8s are bitcast into one f32 geometry slot.
        writeln!(out, "float2 _mp_unpack2f16(float x) {{").ok();
        writeln!(out, "    uint u = asuint(x);").ok();
        writeln!(
            out,
            "    return float2(f16tof32(u & 0xffff), f16tof32(u >> 16));"
        )
        .ok();
        writeln!(out, "}}").ok();
        writeln!(out, "float4 _mp_unpack4u8(float x) {{").ok();
        writeln!(out, "    uint u = asuint(x);").ok();
        writeln!(
            out,
            "    return float4(float(u & 0xff), float((u >> 8) & 0xff), float((u >> 16) & 0xff), float((u >> 24) & 0xff)) * (1.0 / 255.0);"
        )
        .ok();
        writeln!(out, "}}").ok();
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
        let keep = self.hlsl_varying_instances(vm);
        let mut semantic_idx = 0usize;
        for io in &self.io {
            let dropped = keep.as_ref().is_some_and(|k| !k.contains(&io.name));
            if Self::hlsl_is_instance(&io.kind) && dropped {
                continue;
            }
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
                    for (chunk_idx, chunk_slots) in
                        Self::hlsl_slot_chunks(slots).into_iter().enumerate()
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

        // Instance fields, unless the vertex stage reads them from the
        // instance buffer.
        let from_buffer = self.hlsl_instances_from_buffer(vm);
        semantic_idx = 0;
        // Dyn instance fields first
        for io in self.io.iter().filter(|_| !from_buffer) {
            if let ShaderIoKind::DynInstance = io.kind {
                let pod_ty = vm.bx.heap.pod_type_ref(io.ty);
                let slots = pod_ty.ty.slots();
                let io_name = self.backend.map_io_name(io.name);
                if Self::hlsl_input_needs_chunks(vm, io.ty) {
                    for (chunk_idx, chunk_slots) in
                        Self::hlsl_slot_chunks(slots).into_iter().enumerate()
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
        for io in self.io.iter().filter(|_| !from_buffer) {
            if let ShaderIoKind::RustInstance = io.kind {
                let pod_ty = vm.bx.heap.pod_type_ref(io.ty);
                let slots = pod_ty.ty.slots();
                let io_name = self.backend.map_io_name(io.name);
                if Self::hlsl_input_needs_chunks(vm, io.ty) {
                    for (chunk_idx, chunk_slots) in
                        Self::hlsl_slot_chunks(slots).into_iter().enumerate()
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
        // Portable instance_index(); retained buffers bind their complete prefix.
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

    pub fn hlsl_create_texture_samplers(&self, vm: &ScriptVm, out: &mut String) {
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
                        TextureType::TextureDepth => "Texture2D<float>",
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
        // The instance buffer takes the register after the textures.
        if self.hlsl_instances_from_buffer(vm) {
            writeln!(out, "{}{});", HLSL_INSTANCE_BUFFER_DECL, tex_idx).ok();
        }

        for (idx, sampler) in self.samplers.iter().enumerate() {
            writeln!(
                out,
                "{} _s{} : register(s{});",
                if sampler.compare {
                    "SamplerComparisonState"
                } else {
                    "SamplerState"
                },
                idx,
                idx
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
        let from_buffer = self.hlsl_instances_from_buffer(vm);
        let keep = self.hlsl_varying_instances(vm);
        let (offsets, stride) = self.hlsl_instance_offsets(vm);
        if from_buffer {
            writeln!(out, "    uint _mp_ib = input.iid * {};", stride).ok();
        }
        for (index, offset) in offsets {
            let io = &self.io[index];
            let expr = if from_buffer {
                let pod_ty = vm.bx.heap.pod_type_ref(io.ty);
                let scalars: Vec<String> = (0..pod_ty.ty.slots())
                    .map(|slot| format!("asfloat(_mp_inst[_mp_ib + {}])", offset + slot))
                    .collect();
                let inline = crate::pod::ScriptPodTypeInline {
                    self_ref: io.ty,
                    data: pod_ty.clone(),
                };
                self.hlsl_reconstruct_from_scalars(vm, &inline, &scalars, &mut 0)
            } else {
                self.hlsl_reconstruct_input_value(vm, io.ty, "i", io.name)
            };
            let io_name = self.backend.map_io_name(io.name);
            writeln!(out, "    _mp_iov.i.{0} = {1};", io_name, expr).ok();
            if keep.as_ref().is_none_or(|k| k.contains(&io.name)) {
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
