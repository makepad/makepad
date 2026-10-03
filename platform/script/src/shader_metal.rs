use crate::pod::{ScriptPodMat, ScriptPodTy};
use crate::shader::{ShaderIoKind, ShaderOutput, TextureType};
use crate::vm::ScriptVm;
use makepad_live_id::{id, LiveId};
use std::fmt::Write;

impl ShaderOutput {
    /// The complete Metal library source of a compiled draw shader
    /// (`vertex_main` + `fragment_main`), after its vertex and fragment
    /// functions were compiled into `self` and its uniform buffers were given
    /// indices. The one assembly order the Metal backend and its GPU tests
    /// share.
    pub fn metal_draw_source(&mut self, vm: &ScriptVm) -> String {
        let mut out = String::new();
        writeln!(out, "#include <metal_stdlib>\nusing namespace metal;").ok();
        self.create_struct_defs(vm, &mut out);
        self.metal_create_instance_struct(vm, &mut out);
        self.metal_create_uniform_struct(vm, &mut out);
        self.metal_create_scope_uniform_struct(vm, &mut out);
        self.metal_create_varying_struct(vm, &mut out);
        self.metal_create_vertex_buffer_struct(vm, &mut out);
        self.metal_create_io_struct(vm, &mut out);
        self.metal_create_io_vertex_struct(vm, &mut out);
        self.metal_create_io_framebuffer_struct(vm, &mut out);
        self.metal_create_io_fragment_struct(vm, &mut out);
        self.metal_create_sampler_decls(&mut out);
        self.metal_create_helpers(&mut out);
        self.metal_create_pick_helpers(&mut out);
        self.create_functions(&mut out);
        self.metal_create_vertex_fn(vm, &mut out);
        self.metal_create_fragment_main_fn(vm, &mut out);
        out
    }

    /// The pick variant of the same draw shader ([`Self::pick`]): every
    /// 2D texture it samples has a pick twin (bound after its textures)
    /// read at the same place, and the fragment writes the id of what is
    /// drawn there into every colour target instead of its colour: the id
    /// the most opaque of its samples found, else the draw's own
    /// (`_pk.x`), where its colour covers (alpha over 0.1); nothing
    /// elsewhere. `_pk.y` says which textures' twins hold ids.
    pub fn metal_draw_source_pick(&mut self, vm: &ScriptVm) -> String {
        self.pick_emit = true;
        let out = self.metal_draw_source(vm);
        self.pick_emit = false;
        out
    }

    /// The textures with a pick twin: (index among the shader's textures,
    /// name).
    fn metal_pick_textures(&self) -> Vec<(usize, LiveId)> {
        self.io
            .iter()
            .filter(|io| matches!(io.kind, ShaderIoKind::Texture(_)))
            .enumerate()
            .filter(|(k, io)| *k < 32 && matches!(io.kind, ShaderIoKind::Texture(TextureType::Texture2d)))
            .map(|(k, io)| (k, io.name))
            .collect()
    }

    fn metal_create_pick_helpers(&self, out: &mut String) {
        if !self.pick {
            return;
        }
        if !self.pick_emit {
            writeln!(out, "#define _MP_PS(b, tw, e, uv) (e)").ok();
            return;
        }
        writeln!(out, "#define _MP_PS(b, tw, e, uv) _mp_pick_sample(_io, b, _io.tw, (e), (uv))").ok();
        // How much a colour shows: its alpha, or its light where it adds
        // (premultiplied, alpha 0).
        writeln!(out, "inline float _mp_pick_cover(float4 c) {{ return max(c.w, max(c.x, max(c.y, c.z))); }}").ok();
        writeln!(out, "inline float4 _mp_pick_sample(thread Io &io, uint b, texture2d<float> tw, float4 c, float2 uv) {{").ok();
        writeln!(out, "    float w = _mp_pick_cover(c);").ok();
        writeln!(out, "    if ((io._pick_mask & (1u << b)) != 0u && w > io._pick_w) {{").ok();
        writeln!(out, "        float2 sz = float2(tw.get_width(), tw.get_height());").ok();
        // The texel under the place, else one beside it: a filtered
        // sample sees its neighbours too.
        writeln!(out, "        int2 q = int2(clamp(uv, float2(0.0), float2(0.99999)) * sz);").ok();
        writeln!(out, "        int2 hi = int2(sz) - 1;").ok();
        writeln!(out, "        uint id = 0u;").ok();
        writeln!(out, "        for (int k = 0; k < 5 && id == 0u; k++) {{").ok();
        writeln!(out, "            int2 o = k == 0 ? int2(0) : (k == 1 ? int2(1, 0) : (k == 2 ? int2(-1, 0) : (k == 3 ? int2(0, 1) : int2(0, -1))));").ok();
        writeln!(out, "            float4 e = tw.read(uint2(clamp(q + o, int2(0), hi)));").ok();
        writeln!(out, "            id = uint(e.x * 255.0 + 0.5) | (uint(e.y * 255.0 + 0.5) << 8) | (uint(e.z * 255.0 + 0.5) << 16);").ok();
        writeln!(out, "        }}").ok();
        writeln!(out, "        if (id != 0u) {{ io._pick_w = w; io._pick_id = id; }}").ok();
        writeln!(out, "    }}").ok();
        writeln!(out, "    return c;").ok();
        writeln!(out, "}}").ok();
    }

    pub fn metal_create_helpers(&self, out: &mut String) {
        // Packed vertex attribute unpackers: two f16s / four unorm8s
        // bitcast into one f32 geometry slot (packed map vertex format).
        writeln!(out, "inline float2 _mp_unpack2f16(float x) {{").ok();
        writeln!(out, "    half2 h = as_type<half2>(as_type<uint>(x));").ok();
        writeln!(out, "    return float2(h.x, h.y);").ok();
        writeln!(out, "}}").ok();
        writeln!(out, "inline float4 _mp_unpack4u8(float x) {{").ok();
        writeln!(out, "    uint u = as_type<uint>(x);").ok();
        writeln!(
            out,
            "    return float4(float(u & 0xffu), float((u >> 8) & 0xffu), float((u >> 16) & 0xffu), float((u >> 24) & 0xffu)) * (1.0 / 255.0);"
        )
        .ok();
        writeln!(out, "}}").ok();
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
        // The invocation's shared loop-pass counter
        // (shader_control::SHADER_ITERATION_BUDGET).
        writeln!(out, "    uint _mp_iter;").ok();
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
                    // `t_`: a texture is named by the document (a pass
                    // `half`), and MSL reserves type names.
                    writeln!(out, "    {} t_{};", metal_type, io.name).ok();
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
                    writeln!(out, "    constant IoVertexBufferRaw *vb;").ok();
                    // The decoded (logical) vertex lives on vertex_main's stack; every
                    // shader function reaches it through the context, like the instance.
                    writeln!(out, "    thread IoVertexBuffer *g;").ok();
                    have_vb = true;
                }
            }
        }
        if self.pick_emit {
            for (_, name) in self.metal_pick_textures() {
                writeln!(out, "    texture2d<float> {}__pick;", name).ok();
            }
            writeln!(out, "    uint _pick_mask;").ok();
            writeln!(out, "    uint _pick_id;").ok();
            writeln!(out, "    float _pick_w;").ok();
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

    /// The instance record is read from the instance buffer as 32-bit
    /// words at the offsets of [`ShaderOutput::instance_record`] (the layout
    /// the draw list writes), decoded into `IoInstance`.
    pub fn metal_create_instance_struct(&self, vm: &ScriptVm, out: &mut String) {
        let (fields, stride) = self.instance_record(vm);
        writeln!(out, "struct IoInstance {{").ok();
        for (index, _) in &fields {
            let io = &self.io[*index];
            write!(out, "    ").ok();
            self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
            writeln!(out, " {};", io.name).ok();
        }
        writeln!(out, "}};").ok();
        writeln!(out, "#define MP_INSTANCE_WORDS {}u", stride.max(1)).ok();
        writeln!(out, "inline IoInstance _mp_decode_instance(constant uint *w) {{").ok();
        writeln!(out, "    IoInstance out_instance;").ok();
        for (index, offset) in &fields {
            let io = &self.io[*index];
            let inline = crate::pod::ScriptPodTypeInline { self_ref: io.ty, data: vm.bx.heap.pod_type_ref(io.ty).clone() };
            let mut at = *offset;
            let expr = self.metal_value_from_words(vm, &inline, &mut at);
            writeln!(out, "    out_instance.{} = {};", io.name, expr).ok();
        }
        writeln!(out, "    return out_instance;").ok();
        writeln!(out, "}}").ok();
    }

    /// (columns, rows) of a matrix type.
    fn metal_mat_shape(ty: &ScriptPodTy) -> (usize, usize) {
        match ty {
            ScriptPodTy::Mat(m) => match m {
                ScriptPodMat::Mat2x2f => (2, 2),
                ScriptPodMat::Mat3x2f => (3, 2),
                ScriptPodMat::Mat4x2f => (4, 2),
                ScriptPodMat::Mat2x3f => (2, 3),
                ScriptPodMat::Mat3x3f => (3, 3),
                ScriptPodMat::Mat4x3f => (4, 3),
                ScriptPodMat::Mat2x4f => (2, 4),
                ScriptPodMat::Mat3x4f => (3, 4),
                ScriptPodMat::Mat4x4f => (4, 4),
            },
            _ => (1, 1),
        }
    }

    /// A value of `ty` from the instance words `w[at..]`, tight: scalars
    /// one word each, a matrix its columns in order, a struct its fields.
    fn metal_value_from_words(&self, vm: &ScriptVm, ty: &crate::pod::ScriptPodTypeInline, at: &mut usize) -> String {
        let mut ty_name = String::new();
        self.backend.pod_type_name(ty, &mut ty_name);
        match &ty.data.ty {
            ScriptPodTy::Struct { fields, .. } => {
                let parts: Vec<String> = fields.iter().map(|f| self.metal_value_from_words(vm, &f.ty, at)).collect();
                format!("{}{{{}}}", ty_name, parts.join(", "))
            }
            ScriptPodTy::Packed(p) => {
                let w = format!("w[{}]", *at);
                *at += ty.data.ty.slots();
                match p {
                    crate::pod::ScriptPodPacked::F16x2 => format!("float2(as_type<half2>({w}))"),
                    crate::pod::ScriptPodPacked::F16x4 => format!("float4(as_type<half2>({w}), as_type<half2>(w[{}]))", *at - ty.data.ty.slots() + 1),
                    crate::pod::ScriptPodPacked::U16x2 => format!("float2(as_type<ushort2>({w}))"),
                    crate::pod::ScriptPodPacked::I16x2 => format!("float2(as_type<short2>({w}))"),
                    crate::pod::ScriptPodPacked::U16x2Norm => format!("float2(as_type<ushort2>({w})) / 65535.0"),
                    crate::pod::ScriptPodPacked::I16x2Norm => format!("max(float2(as_type<short2>({w})) / 32767.0, float2(-1.0))"),
                    crate::pod::ScriptPodPacked::U8x4Norm => format!("float4(as_type<uchar4>({w})) / 255.0"),
                    crate::pod::ScriptPodPacked::I8x4Norm => format!("max(float4(as_type<char4>({w})) / 127.0, float4(-1.0))"),
                }
            }
            leaf => {
                let slots = leaf.slots();
                let word = |k: usize| -> String {
                    match leaf {
                        ScriptPodTy::U32 | ScriptPodTy::AtomicU32 => format!("w[{k}]"),
                        ScriptPodTy::Bool => format!("(w[{k}] != 0u)"),
                        ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => format!("as_type<int>(w[{k}])"),
                        ScriptPodTy::Vec(v) if Self::is_integer_word(leaf) => match v {
                            crate::pod::ScriptPodVec::Vec2i | crate::pod::ScriptPodVec::Vec3i | crate::pod::ScriptPodVec::Vec4i => format!("as_type<int>(w[{k}])"),
                            crate::pod::ScriptPodVec::Vec2b | crate::pod::ScriptPodVec::Vec3b | crate::pod::ScriptPodVec::Vec4b => format!("(w[{k}] != 0u)"),
                            _ => format!("w[{k}]"),
                        },
                        _ => format!("as_type<float>(w[{k}])"),
                    }
                };
                let words: Vec<String> = (*at..*at + slots).map(word).collect();
                *at += slots;
                if slots == 1 && !matches!(leaf, ScriptPodTy::Vec(_) | ScriptPodTy::Mat(_)) {
                    words[0].clone()
                } else if matches!(leaf, ScriptPodTy::Mat(_)) {
                    // Columns in order, as the CPU writes them (a column
                    // padded to the record's column stride).
                    let (cols, rows) = Self::metal_mat_shape(leaf);
                    let col_stride = (slots / cols).max(rows);
                    let cols: Vec<String> = words.chunks(col_stride).map(|c| format!("float{}({})", rows, c[..rows.min(c.len())].join(", "))).collect();
                    format!("{}({})", ty_name, cols.join(", "))
                } else {
                    format!("{}({})", ty_name, words.join(", "))
                }
            }
        }
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
        writeln!(out, "struct IoVertexBufferRaw {{").ok();
        for io in &self.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                write!(out, "    ").ok();
                self.backend
                    .pod_type_name_packed_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " {};", io.name).ok();
            }
        }
        writeln!(out, "}};").ok();

        writeln!(out, "struct IoVertexBuffer {{").ok();
        for io in &self.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                write!(out, "    ").ok();
                self.backend.pod_type_name_from_ty(&vm.bx.heap, io.ty, out);
                writeln!(out, " {};", io.name).ok();
            }
        }
        writeln!(out, "}};").ok();

        writeln!(
            out,
            "inline IoVertexBuffer _mp_decode_geometry(constant IoVertexBufferRaw &raw) {{"
        )
        .ok();
        writeln!(out, "    IoVertexBuffer out_geom;").ok();
        for io in &self.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                self.metal_write_decode_assign(
                    vm,
                    io.name.to_string(),
                    &vm.bx.heap.pod_type_ref(io.ty).ty,
                    out,
                );
            }
        }
        writeln!(out, "    return out_geom;").ok();
        writeln!(out, "}}").ok();
    }

    fn metal_write_decode_assign(
        &self,
        vm: &ScriptVm,
        path: String,
        ty: &ScriptPodTy,
        out: &mut String,
    ) {
        match ty {
            ScriptPodTy::Packed(p) => {
                let expr = match p {
                    crate::pod::ScriptPodPacked::F16x2 | crate::pod::ScriptPodPacked::F16x4 => {
                        format!("float{}(raw.{path})", if p.is_vec4() { "4" } else { "2" })
                    }
                    crate::pod::ScriptPodPacked::U16x2 | crate::pod::ScriptPodPacked::I16x2 => {
                        format!("float2(raw.{path})")
                    }
                    crate::pod::ScriptPodPacked::U16x2Norm => {
                        format!("float2(raw.{path}) / 65535.0")
                    }
                    crate::pod::ScriptPodPacked::I16x2Norm => {
                        format!("max(float2(raw.{path}) / 32767.0, float2(-1.0))")
                    }
                    crate::pod::ScriptPodPacked::U8x4Norm => {
                        format!("float4(raw.{path}) / 255.0")
                    }
                    crate::pod::ScriptPodPacked::I8x4Norm => {
                        format!("max(float4(raw.{path}) / 127.0, float4(-1.0))")
                    }
                };
                writeln!(out, "    out_geom.{path} = {expr};").ok();
            }
            ScriptPodTy::Struct { fields, .. } => {
                for field in fields {
                    let field_name = self.backend.map_field_name(field.name);
                    self.metal_write_decode_assign(
                        vm,
                        format!("{path}.{field_name}"),
                        &field.ty.data.ty,
                        out,
                    );
                }
            }
            _ => {
                writeln!(out, "    out_geom.{path} = raw.{path};").ok();
            }
        }
    }

    pub fn metal_create_io_vertex_struct(&self, _vm: &ScriptVm, out: &mut String) {
        writeln!(out, "struct IoV {{").ok();
        writeln!(out, "    thread IoVarying *v;").ok();
        writeln!(out, "    uint vid;").ok();
        // instance_index() reads this draw-local ordinal, including retained appends.
        writeln!(out, "    uint iid;").ok();
        writeln!(out, "}};").ok();
    }

    pub fn metal_create_vertex_fn(&self, vm: &ScriptVm, out: &mut String) {
        let has_scope_uniforms = self
            .io
            .iter()
            .any(|io| matches!(io.kind, ShaderIoKind::ScopeUniform));

        writeln!(out, "vertex IoVarying vertex_main(").ok();
        writeln!(out, "    constant IoVertexBufferRaw *vb [[buffer(0)]],").ok();
        writeln!(out, "    constant uint *i_raw [[buffer(1)]],").ok();
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
                        "    {} t_{} [[texture({})]],",
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
        writeln!(out, "    _io._mp_iter = 0u;").ok();
        if self.pick_emit {
            writeln!(out, "    _io._pick_mask = 0u;").ok();
            writeln!(out, "    _io._pick_id = 0u;").ok();
            writeln!(out, "    _io._pick_w = 0.0;").ok();
        }
        writeln!(
            out,
            "    IoInstance _inst = _mp_decode_instance(i_raw + iid * MP_INSTANCE_WORDS);"
        )
        .ok();
        writeln!(out, "    constant char *_geom_bytes = (constant char *)vb;").ok();
        writeln!(
            out,
            "    constant IoVertexBufferRaw *_geom_raw = (constant IoVertexBufferRaw *)(_geom_bytes + vid * sizeof(IoVertexBufferRaw));"
        )
        .ok();
        writeln!(
            out,
            "    IoVertexBuffer _geom = _mp_decode_geometry(*_geom_raw);"
        )
        .ok();
        writeln!(out, "    _io.vb = vb;").ok();
        writeln!(out, "    _io.g = &_geom;").ok();
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
                ShaderIoKind::Texture(_) => {
                    writeln!(out, "    _io.t_{} = t_{};", io.name, io.name).ok();
                }
                ShaderIoKind::Sampler(_) => {
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

        let has_outputs = self.io.iter().any(|io| matches!(io.kind, ShaderIoKind::FragmentOutput(_)));
        let pick = self.pick_emit && has_outputs;
        if pick {
            writeln!(out, "struct IoPk {{").ok();
            for io in &self.io {
                if let ShaderIoKind::FragmentOutput(index) = io.kind {
                    writeln!(out, "    float4 fb{} [[color({})]];", index, index).ok();
                }
            }
            writeln!(out, "}};").ok();
            writeln!(out, "fragment IoPk fragment_main(").ok();
        } else {
            writeln!(out, "fragment IoFb fragment_main(").ok();
        }
        writeln!(out, "    IoVarying v [[stage_in]],").ok();
        writeln!(out, "    constant IoVertexBufferRaw *vb [[buffer(0)]],").ok();
        writeln!(out, "    constant uint *i_raw [[buffer(1)]],").ok();
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
                        "    {} t_{} [[texture({})]]",
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

        let ntex = self.io.iter().filter(|io| matches!(io.kind, ShaderIoKind::Texture(_))).count();
        if self.pick_emit {
            for (k, name) in self.metal_pick_textures() {
                writeln!(out, ",").ok();
                write!(out, "    texture2d<float> {}__pick [[texture({})]]", name, ntex + k).ok();
            }
            writeln!(out, ",").ok();
            write!(out, "    constant uint4 *_pk [[buffer(30)]]").ok();
        }
        writeln!(out, ") {{").ok();

        writeln!(out, "    Io _io;").ok();
        writeln!(out, "    _io._mp_iter = 0u;").ok();
        if self.pick_emit {
            writeln!(out, "    _io._pick_mask = _pk->y;").ok();
            writeln!(out, "    _io._pick_id = 0u;").ok();
            writeln!(out, "    _io._pick_w = 0.0;").ok();
            for (_, name) in self.metal_pick_textures() {
                writeln!(out, "    _io.{0}__pick = {0}__pick;", name).ok();
            }
        }
        writeln!(
            out,
            "    IoInstance _inst = _mp_decode_instance(i_raw + v._iid * MP_INSTANCE_WORDS);"
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
                ShaderIoKind::Texture(_) => {
                    writeln!(out, "    _io.t_{} = t_{};", io.name, io.name).ok();
                }
                ShaderIoKind::Sampler(_) => {
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
        if pick {
            // The coverage: the first colour target's alpha (a data target
            // of one channel covers).
            let first = self.io.iter().filter_map(|io| match io.kind {
                ShaderIoKind::FragmentOutput(index) => Some((index, io.ty)),
                _ => None,
            }).min_by_key(|(index, _)| *index);
            let alpha = match first {
                Some((index, ty)) if ty == vm.bx.code.builtins.pod.pod_vec4f => format!("_mp_pick_cover(_iofb.fb{})", index),
                _ => "1.0".to_string(),
            };
            writeln!(out, "    uint _pid = _io._pick_w > 0.0 ? _io._pick_id : _pk->x;").ok();
            writeln!(out, "    if ({} < 0.1 || _pid == 0u) discard_fragment();", alpha).ok();
            writeln!(out, "    float4 _pc = float4(float(_pid & 255u), float((_pid >> 8) & 255u), float((_pid >> 16) & 255u), 255.0) / 255.0;").ok();
            writeln!(out, "    IoPk _pko;").ok();
            for io in &self.io {
                if let ShaderIoKind::FragmentOutput(index) = io.kind {
                    writeln!(out, "    _pko.fb{} = _pc;", index).ok();
                }
            }
            writeln!(out, "    return _pko;").ok();
        } else {
            writeln!(out, "    return _iofb;").ok();
        }
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
                "constexpr sampler _s{}(filter::{}, mip_filter::linear, address::{}, coord::{}{});",
                idx,
                filter,
                address,
                coord,
                if sampler.compare {
                    ", compare_func::less_equal"
                } else {
                    ""
                }
            )
            .ok();
        }
    }
}
