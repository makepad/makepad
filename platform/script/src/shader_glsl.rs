use crate::pod::{ScriptPodTy, ScriptPodTypeInline};
use crate::shader::{ShaderIoKind, ShaderOutput, TextureType};
use crate::value::ScriptPodType;
use crate::vm::ScriptVm;
use makepad_live_id::{id, LiveId};
use std::collections::BTreeSet;
use std::fmt::Write;

/// GLSL ES 3.00's minimum MAX_VERTEX_ATTRIBS: the vertex inputs every GL ES
/// 3 and WebGL 2 device takes. Instance data past it are read from the
/// instance data texture (`mp_inst_data`).
pub const GLSL_MAX_VERTEX_ATTRIBS: usize = 16;
/// WebGL's largest vertex attribute stride, in bytes.
pub const WEBGL_MAX_ATTRIB_STRIDE: usize = 255;
/// GL ES 3.0's minimum MAX_VERTEX_ATTRIB_STRIDE, in bytes.
pub const GLES_MAX_ATTRIB_STRIDE: usize = 2048;

#[derive(Clone, Copy, PartialEq, Eq)]
enum GlslPackedFormat {
    Float,
    UInt,
    SInt,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GlslPackedSource {
    NumericFloat,
    BitPackedFloat,
}

#[derive(Clone)]
struct GlslPackedField {
    name: String,
    ty: ScriptPodType,
    slots: usize,
    offset: usize,
}

struct GlslTypedAttr {
    name: String,
    ty_name: String,
}

impl ShaderOutput {
    pub fn glsl_create_vertex_shader(&self, vm: &ScriptVm, shared_defs: &str, out: &mut String) {
        let geometry_fields = self.glsl_collect_geometry_fields(vm);
        let instance_fields = self.glsl_collect_instance_fields(vm);
        let varying_fields = self.glsl_collect_varying_pack_fields(vm);
        let instance_fetch_from = self.glsl_instance_fetch_from(vm, &geometry_fields, &instance_fields);

        let varying_slots = varying_fields
            .last()
            .map(|field| field.offset + field.slots)
            .unwrap_or(0);

        out.push_str(shared_defs);
        // Manual unpack4u8: `unpackUnorm4x8` is GLSL ES 3.10+ / GL 4.0+, but we
        // compile as `#version 300 es` for GLES 3.0 (Linux/Android/WebGL2).
        // `_mp_iter`: the invocation's shared loop-pass counter
        // (shader_control::SHADER_ITERATION_BUDGET).
        out.push_str(
            "uint _mp_iter = 0u;\n\
vec2 _mp_unpack2f16(float x){ return unpackHalf2x16(floatBitsToUint(x)); }\n\
vec4 _mp_unpack4u8(float x){ uint u = floatBitsToUint(x); return vec4(float(u & 0xffu), float((u >> 8u) & 0xffu), float((u >> 16u) & 0xffu), float((u >> 24u) & 0xffu)) * (1.0 / 255.0); }\n",
        );

        self.glsl_write_uniform_blocks(vm, out);
        // Vertex never samples video; omit TextureVideo so Adreno does not see
        // `samplerExternalOES` + `camera_projection[int(VIEW_ID)]` (driver ICE).
        self.glsl_write_texture_uniforms(out, /*include_video_external*/ false);
        self.glsl_write_vertex_globals(vm, out);
        self.glsl_write_vertex_input_attrs(vm, &geometry_fields, &instance_fields, instance_fetch_from, out);
        self.glsl_write_varying_interface(varying_slots, true, out);
        let vertex_entry = self.backend.map_function_name("io_vertex");
        self.glsl_write_functions_for_entries(out, &[vertex_entry.as_str()]);
        self.glsl_write_vertex_main(vm, &geometry_fields, &instance_fields, instance_fetch_from, &varying_fields, out);
    }

    pub fn glsl_create_fragment_shader(&self, vm: &ScriptVm, shared_defs: &str, out: &mut String) {
        let varying_fields = self.glsl_collect_varying_pack_fields(vm);
        let varying_slots = varying_fields
            .last()
            .map(|field| field.offset + field.slots)
            .unwrap_or(0);

        out.push_str(shared_defs);
        // Manual unpack4u8: `unpackUnorm4x8` is GLSL ES 3.10+ / GL 4.0+, but we
        // compile as `#version 300 es` for GLES 3.0 (Linux/Android/WebGL2).
        // `_mp_iter`: the invocation's shared loop-pass counter
        // (shader_control::SHADER_ITERATION_BUDGET).
        out.push_str(
            "uint _mp_iter = 0u;\n\
vec2 _mp_unpack2f16(float x){ return unpackHalf2x16(floatBitsToUint(x)); }\n\
vec4 _mp_unpack4u8(float x){ uint u = floatBitsToUint(x); return vec4(float(u & 0xffu), float((u >> 8u) & 0xffu), float((u >> 16u) & 0xffu), float((u >> 24u) & 0xffu)) * (1.0 / 255.0); }\n",
        );

        self.glsl_write_uniform_blocks(vm, out);
        self.glsl_write_texture_uniforms(out, /*include_video_external*/ true);
        self.glsl_write_fragment_globals(vm, out);
        self.glsl_write_varying_interface(varying_slots, false, out);
        self.glsl_write_fragment_outputs(vm, out);
        let fragment_entry = self.backend.map_function_name("io_fragment");
        self.glsl_write_functions_for_entries(out, &[fragment_entry.as_str()]);
        self.glsl_write_fragment_main(vm, &varying_fields, out);
    }

    fn glsl_write_uniform_blocks(&self, vm: &ScriptVm, out: &mut String) {
        for io in &self.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                let block_name = self.glsl_uniform_block_name(io.name);
                let io_name = self.backend.map_io_name(io.name);
                if io_name == "draw_pass"
                    && self.glsl_write_draw_pass_uniform_block(
                        vm,
                        io.ty,
                        &block_name,
                        &io_name,
                        out,
                    )
                {
                    continue;
                }
                let type_name = self.glsl_type_name_from_ty(vm, io.ty);
                writeln!(out, "layout(std140) uniform {} {{", block_name).ok();
                writeln!(out, "    {} unibuf_{};", type_name, io_name).ok();
                writeln!(out, "}};").ok();
            }
        }

        let mut has_uniforms = false;
        for io in &self.io {
            if matches!(io.kind, ShaderIoKind::Uniform) {
                if !has_uniforms {
                    writeln!(out, "layout(std140) uniform userUniforms {{").ok();
                    has_uniforms = true;
                }
                let type_name = self.glsl_type_name_from_ty(vm, io.ty);
                let io_name = self.backend.map_io_name(io.name);
                writeln!(out, "    {} uni_{};", type_name, io_name).ok();
            }
        }
        if has_uniforms {
            writeln!(out, "}};").ok();
        }

        let mut has_scope_uniforms = false;
        for io in &self.io {
            if matches!(io.kind, ShaderIoKind::ScopeUniform) {
                if !has_scope_uniforms {
                    writeln!(out, "layout(std140) uniform liveUniforms {{").ok();
                    has_scope_uniforms = true;
                }
                let type_name = self.glsl_type_name_from_ty(vm, io.ty);
                let io_name = self.backend.map_io_name(io.name);
                writeln!(out, "    {} su_{};", type_name, io_name).ok();
            }
        }
        if has_scope_uniforms {
            writeln!(out, "}};").ok();
        }
    }

    fn glsl_write_draw_pass_uniform_block(
        &self,
        vm: &ScriptVm,
        ty: ScriptPodType,
        block_name: &str,
        io_name: &str,
        out: &mut String,
    ) -> bool {
        let pod_ty = vm.bx.heap.pod_type_ref(ty);
        let ScriptPodTy::Struct { fields, .. } = &pod_ty.ty else {
            return false;
        };

        writeln!(out, "layout(std140) uniform {} {{", block_name).ok();
        for field in fields {
            let field_name = self.backend.map_field_name(field.name);
            let type_name = self.glsl_type_name_inline(&field.ty);
            match field_name.as_str() {
                "camera_projection" | "camera_view" | "depth_projection" | "depth_view"
                | "camera_inv" => {
                    writeln!(out, "    {} {}[2];", type_name, field_name).ok();
                }
                "camera_projection_r"
                | "camera_view_r"
                | "depth_projection_r"
                | "depth_view_r"
                | "camera_inv_r" => {}
                _ => {
                    writeln!(out, "    {} {};", type_name, field_name).ok();
                }
            }
        }
        writeln!(out, "}} unibuf_{};", io_name).ok();
        true
    }

    fn glsl_write_texture_uniforms(&self, out: &mut String, include_video_external: bool) {
        for io in &self.io {
            if let ShaderIoKind::Texture(tex_type) = io.kind {
                if !include_video_external && matches!(tex_type, TextureType::TextureVideo) {
                    continue;
                }
                let tex_name = self.backend.map_io_name(io.name);
                writeln!(
                    out,
                    "uniform {} tex_{};",
                    self.glsl_sampler_type(tex_type),
                    tex_name
                )
                .ok();
            }
        }
    }

    fn glsl_write_vertex_globals(&self, vm: &ScriptVm, out: &mut String) {
        // instance_index() lowers to uint(gl_InstanceID), also on WebGL 2.
        // Keep the vertex position register available as a global for shaders
        // that write `self.pos` directly instead of returning a vec4 from io_vertex().
        writeln!(out, "vec4 vtx_pos;").ok();
        // The platform's Y law for render targets stored bottom-up (WebGL's
        // framebuffers): the host sets this to 1 for a texture pass, and the
        // clip position's Y is inverted after the shader's own vertex code,
        // so every shader lands texels in the top-left row order Metal and
        // D3D produce, whether it projects through the pass camera or writes
        // clip space itself. Never set (0) it changes nothing.
        writeln!(out, "uniform float mp_clip_y_flip;").ok();
        for io in &self.io {
            let type_name = self.glsl_type_name_from_ty(vm, io.ty);
            let io_name = self.backend.map_io_name(io.name);
            match io.kind {
                ShaderIoKind::VertexBuffer => {
                    writeln!(out, "{} vb_{};", type_name, io_name).ok();
                }
                ShaderIoKind::DynInstance => {
                    writeln!(out, "{} dyninst_{};", type_name, io_name).ok();
                }
                ShaderIoKind::RustInstance => {
                    writeln!(out, "{} rustinst_{};", type_name, io_name).ok();
                }
                ShaderIoKind::Varying => {
                    writeln!(out, "{} var_{};", type_name, io_name).ok();
                }
                _ => {}
            }
        }
    }

    fn glsl_write_fragment_globals(&self, vm: &ScriptVm, out: &mut String) {
        for io in &self.io {
            let type_name = self.glsl_type_name_from_ty(vm, io.ty);
            let io_name = self.backend.map_io_name(io.name);
            match io.kind {
                ShaderIoKind::DynInstance => {
                    writeln!(out, "{} dyninst_{};", type_name, io_name).ok();
                }
                ShaderIoKind::RustInstance => {
                    writeln!(out, "{} rustinst_{};", type_name, io_name).ok();
                }
                ShaderIoKind::Varying => {
                    writeln!(out, "{} var_{};", type_name, io_name).ok();
                }
                ShaderIoKind::FragmentOutput(index) => {
                    writeln!(out, "{} frag_fb{};", type_name, index).ok();
                }
                _ => {}
            }
        }
    }

    fn glsl_vertex_fetch_is_f32(&self, vm: &ScriptVm) -> bool {
        self.io.iter().all(|io| match io.kind {
            ShaderIoKind::VertexBuffer | ShaderIoKind::DynInstance | ShaderIoKind::RustInstance => {
                !vm.bx.heap.pod_type_ref(io.ty).ty.has_compact_format()
            }
            _ => true,
        })
    }

    fn glsl_packed_slots(fields: &[GlslPackedField]) -> usize {
        fields
            .last()
            .map(|field| field.offset + field.slots)
            .unwrap_or(0)
    }

    /// The first packed instance vec4 read from the instance data texture
    /// (`mp_inst_data`) instead of an attribute, when the geometry and
    /// instance vec4s together are more vertex inputs than every GL ES 3 /
    /// WebGL 2 device has. An instance record wider than an attribute stride
    /// can be (WebGL's 255 bytes, GL ES's 2048) is read from the texture
    /// whole. `None`: all instance data are attributes.
    fn glsl_instance_fetch_from(
        &self,
        vm: &ScriptVm,
        geometry_fields: &[GlslPackedField],
        instance_fields: &[GlslPackedField],
    ) -> Option<usize> {
        if !self.glsl_vertex_fetch_is_f32(vm) {
            return None;
        }
        let geometry = Self::glsl_num_packed_vec4s(Self::glsl_packed_slots(geometry_fields));
        let instance_slots = Self::glsl_packed_slots(instance_fields);
        let instance = Self::glsl_num_packed_vec4s(instance_slots);
        if geometry + instance <= GLSL_MAX_VERTEX_ATTRIBS {
            return None;
        }
        let max_stride = if self.glsl_webgl2 { WEBGL_MAX_ATTRIB_STRIDE } else { GLES_MAX_ATTRIB_STRIDE };
        if instance_slots * 4 > max_stride {
            return Some(0);
        }
        Some(GLSL_MAX_VERTEX_ATTRIBS.saturating_sub(geometry))
    }

    fn glsl_write_vertex_input_attrs(
        &self,
        vm: &ScriptVm,
        geometry_fields: &[GlslPackedField],
        instance_fields: &[GlslPackedField],
        instance_fetch_from: Option<usize>,
        out: &mut String,
    ) {
        if self.glsl_vertex_fetch_is_f32(vm) {
            let geometry_slots = Self::glsl_packed_slots(geometry_fields);
            let instance_slots = Self::glsl_packed_slots(instance_fields);
            for idx in 0..Self::glsl_num_packed_vec4s(geometry_slots) {
                writeln!(out, "in vec4 packed_geometry_{};", idx).ok();
            }
            for idx in 0..Self::glsl_num_packed_vec4s(instance_slots) {
                if instance_fetch_from.is_some_and(|from| idx >= from) {
                    writeln!(out, "vec4 packed_instance_{};", idx).ok();
                } else {
                    writeln!(out, "in vec4 packed_instance_{};", idx).ok();
                }
            }
            if instance_fetch_from.is_some() {
                // The instance buffer's words as an R32UI texture, row-major
                // at the texture's width: word `slot` of this instance.
                writeln!(out, "uniform highp usampler2D mp_inst_data;").ok();
                writeln!(
                    out,
                    "float _mp_inst_word(int slot) {{ int i = gl_InstanceID * {} + slot; int w = textureSize(mp_inst_data, 0).x; return uintBitsToFloat(texelFetch(mp_inst_data, ivec2(i % w, i / w), 0).r); }}",
                    instance_slots
                )
                .ok();
            }
            return;
        }
        for attr in self.glsl_collect_typed_attrs(vm, true) {
            writeln!(out, "in {} {};", attr.ty_name, attr.name).ok();
        }
        for attr in self.glsl_collect_typed_attrs(vm, false) {
            writeln!(out, "in {} {};", attr.ty_name, attr.name).ok();
        }
    }

    fn glsl_collect_typed_attrs(&self, vm: &ScriptVm, geometry: bool) -> Vec<GlslTypedAttr> {
        let mut out = Vec::new();
        for io in &self.io {
            let want = if geometry {
                matches!(io.kind, ShaderIoKind::VertexBuffer)
            } else {
                matches!(
                    io.kind,
                    ShaderIoKind::DynInstance | ShaderIoKind::RustInstance
                )
            };
            if !want {
                continue;
            }
            let prefix = if geometry { "geom_" } else { "inst_" };
            let pod_ty = &vm.bx.heap.pod_type_ref(io.ty).ty;
            if let ScriptPodTy::Struct { .. } = pod_ty {
                self.glsl_collect_typed_attrs_ty(
                    vm,
                    pod_ty,
                    prefix.trim_end_matches('_'),
                    &mut out,
                );
            } else {
                let io_name = self.backend.map_io_name(io.name);
                self.glsl_collect_typed_attrs_ty(
                    vm,
                    pod_ty,
                    &format!("{}{}", prefix, io_name),
                    &mut out,
                );
            }
        }
        out
    }

    fn glsl_collect_typed_attrs_ty(
        &self,
        vm: &ScriptVm,
        ty: &ScriptPodTy,
        name: &str,
        out: &mut Vec<GlslTypedAttr>,
    ) {
        match ty {
            ScriptPodTy::Struct { fields, .. } => {
                let prefix = if name.is_empty() || name == "geom" || name == "inst" {
                    if name == "inst" {
                        "inst"
                    } else {
                        "geom"
                    }
                } else {
                    name
                };
                for field in fields {
                    let field_name = self.backend.map_field_name(field.name);
                    self.glsl_collect_typed_attrs_ty(
                        vm,
                        &field.ty.data.ty,
                        &format!("{}_{}", prefix, field_name),
                        out,
                    );
                }
            }
            _ => {
                let ty_name = Self::glsl_logical_attr_type(ty);
                out.push(GlslTypedAttr {
                    name: name.to_string(),
                    ty_name,
                });
            }
        }
    }

    fn glsl_write_varying_interface(
        &self,
        varying_slots: usize,
        is_vertex: bool,
        out: &mut String,
    ) {
        let qualifier = if is_vertex { "out" } else { "in" };
        for idx in 0..Self::glsl_num_packed_vec4s(varying_slots) {
            writeln!(out, "{} vec4 packed_varying_{};", qualifier, idx).ok();
        }
    }

    fn glsl_write_fragment_outputs(&self, vm: &ScriptVm, out: &mut String) {
        for io in &self.io {
            if let ShaderIoKind::FragmentOutput(index) = io.kind {
                let type_name = self.glsl_type_name_from_ty(vm, io.ty);
                writeln!(
                    out,
                    "layout(location = {}) out {} _mp_frag_{};",
                    index, type_name, index
                )
                .ok();
            }
        }
    }

    fn glsl_logical_attr_type(ty: &ScriptPodTy) -> String {
        match ty {
            ScriptPodTy::Packed(p) if p.is_vec4() => "vec4".to_string(),
            ScriptPodTy::Packed(_) => "vec2".to_string(),
            ScriptPodTy::F32 | ScriptPodTy::F16 => "float".to_string(),
            ScriptPodTy::U32 | ScriptPodTy::AtomicU32 | ScriptPodTy::Bool => "uint".to_string(),
            ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => "int".to_string(),
            ScriptPodTy::Vec(v) => match v {
                crate::pod::ScriptPodVec::Vec2f | crate::pod::ScriptPodVec::Vec2h => {
                    "vec2".to_string()
                }
                crate::pod::ScriptPodVec::Vec3f | crate::pod::ScriptPodVec::Vec3h => {
                    "vec3".to_string()
                }
                crate::pod::ScriptPodVec::Vec4f | crate::pod::ScriptPodVec::Vec4h => {
                    "vec4".to_string()
                }
                crate::pod::ScriptPodVec::Vec2u | crate::pod::ScriptPodVec::Vec2b => {
                    "uvec2".to_string()
                }
                crate::pod::ScriptPodVec::Vec3u | crate::pod::ScriptPodVec::Vec3b => {
                    "uvec3".to_string()
                }
                crate::pod::ScriptPodVec::Vec4u | crate::pod::ScriptPodVec::Vec4b => {
                    "uvec4".to_string()
                }
                crate::pod::ScriptPodVec::Vec2i => "ivec2".to_string(),
                crate::pod::ScriptPodVec::Vec3i => "ivec3".to_string(),
                crate::pod::ScriptPodVec::Vec4i => "ivec4".to_string(),
            },
            _ => "float".to_string(),
        }
    }

    fn glsl_write_vertex_main(
        &self,
        vm: &ScriptVm,
        geometry_fields: &[GlslPackedField],
        instance_fields: &[GlslPackedField],
        instance_fetch_from: Option<usize>,
        varying_fields: &[GlslPackedField],
        out: &mut String,
    ) {
        writeln!(out, "void main() {{").ok();
        if let Some(from) = instance_fetch_from {
            let instance_slots = Self::glsl_packed_slots(instance_fields);
            for idx in from..Self::glsl_num_packed_vec4s(instance_slots) {
                let words: Vec<String> = (idx * 4..idx * 4 + 4)
                    .map(|slot| {
                        if slot < instance_slots {
                            format!("_mp_inst_word({})", slot)
                        } else {
                            "0.0".to_string()
                        }
                    })
                    .collect();
                writeln!(out, "    packed_instance_{} = vec4({});", idx, words.join(", ")).ok();
            }
        }
        if self.glsl_vertex_fetch_is_f32(vm) {
            for field in geometry_fields {
                self.glsl_unpack_field_to_statements(
                    vm,
                    field,
                    "packed_geometry_",
                    GlslPackedSource::BitPackedFloat,
                    out,
                );
            }
            for field in instance_fields {
                self.glsl_unpack_field_to_statements(
                    vm,
                    field,
                    "packed_instance_",
                    GlslPackedSource::BitPackedFloat,
                    out,
                );
            }
        } else {
            self.glsl_assign_typed_vertex_inputs(vm, out);
        }
        writeln!(out, "    vtx_pos = vec4(0.0, 0.0, 0.0, 1.0);").ok();

        let vertex_returns_vec4f = self
            .functions
            .iter()
            .find(|f| f.name == id!(vertex))
            .map(|f| f.ret == vm.bx.code.builtins.pod.pod_vec4f)
            .unwrap_or(false);
        let vertex_fn_name = self.backend.map_function_name("io_vertex");

        if vertex_returns_vec4f {
            writeln!(out, "    vtx_pos = {}();", vertex_fn_name).ok();
        } else {
            writeln!(out, "    {}();", vertex_fn_name).ok();
        }

        for field in varying_fields {
            let mut scalars = Vec::new();
            self.glsl_flatten_exprs(vm, field.ty, &field.name, &mut scalars);
            for slot in 0..field.slots {
                let src = scalars
                    .get(slot)
                    .cloned()
                    .unwrap_or_else(|| "0.0".to_string());
                let dst = Self::glsl_packed_component("packed_varying_", field.offset + slot);
                writeln!(out, "    {} = {};", dst, src).ok();
            }
        }
        // One clip-z convention on every backend: clip z runs 0..w, as
        // Metal, D3D, Vulkan and WebGPU take it, and a depth buffer holds
        // ndc z. OpenGL clips z at -w..w and stores (ndc + 1) / 2, so its
        // z is moved there: the same draws are clipped and the same depth
        // values written as everywhere else.
        let z = if self.use_vulkan { "vtx_pos.z" } else { "2.0 * vtx_pos.z - vtx_pos.w" };
        writeln!(
            out,
            "    gl_Position = vec4(vtx_pos.x, mp_clip_y_flip > 0.5 ? -vtx_pos.y : vtx_pos.y, {z}, vtx_pos.w);"
        )
        .ok();
        writeln!(out, "}}").ok();
    }

    fn glsl_write_fragment_main(
        &self,
        vm: &ScriptVm,
        varying_fields: &[GlslPackedField],
        out: &mut String,
    ) {
        writeln!(out, "void main() {{").ok();
        for field in varying_fields {
            self.glsl_unpack_field_to_statements(
                vm,
                field,
                "packed_varying_",
                GlslPackedSource::NumericFloat,
                out,
            );
        }
        let fragment_fn_name = self.backend.map_function_name("io_fragment");
        writeln!(out, "    {}();", fragment_fn_name).ok();
        for io in &self.io {
            if let ShaderIoKind::FragmentOutput(index) = io.kind {
                writeln!(out, "    _mp_frag_{} = frag_fb{};", index, index).ok();
            }
        }
        writeln!(out, "}}").ok();
    }

    fn glsl_write_functions_for_entries(&self, out: &mut String, entries: &[&str]) {
        let reachable = self.glsl_collect_reachable_functions(entries);
        for (index, fns) in self.functions.iter().enumerate() {
            if !reachable.contains(&index) {
                continue;
            }
            writeln!(out, "{}{{", fns.call_sig).ok();
            writeln!(out, "{}", fns.out).ok();
            writeln!(out, "}}\n").ok();
        }
    }

    /// The source size of the fragment entry once every call is inlined, as a compiler
    /// that inlines all calls sees it (FXC under ANGLE on D3D, and the D3D
    /// drivers after it): a function called at k sites, each inside a caller
    /// itself inlined m times, appears k * m times. The heaviest functions by
    /// inlined bytes come with it: (name, copies, bytes of one copy).
    ///
    /// Compile time on D3D follows this size, not the emitted one: a pass
    /// whose raymarch helpers are called from several sites (a normal from
    /// four samples, two rays a pixel) inlines to hundreds of KB and takes
    /// seconds to compile there, which stalls the frame that first draws it.
    pub fn glsl_fragment_inline_estimate(&self) -> (usize, Vec<(String, usize, usize)>) {
        let entry = self.backend.map_function_name("io_fragment");
        let names: Vec<Option<String>> = self.functions.iter().map(|f| Self::glsl_function_name_from_sig(&f.call_sig)).collect();
        let calls: Vec<Vec<(usize, usize)>> = self
            .functions
            .iter()
            .enumerate()
            .map(|(i, f)| {
                names
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .filter_map(|(j, name)| {
                        let n = Self::glsl_count_calls(&f.out, name.as_deref()?);
                        (n > 0).then_some((j, n))
                    })
                    .collect()
            })
            .collect();
        let Some(root) = names.iter().position(|n| n.as_deref() == Some(entry.as_str())) else {
            return (0, Vec::new());
        };
        // Copies of each function, callers before callees (GLSL has no
        // recursion, so the graph is acyclic; a back edge is ignored).
        let n_fns = self.functions.len();
        let mut order = Vec::with_capacity(n_fns);
        let mut state = vec![0u8; n_fns]; // 0 new, 1 open, 2 done
        let mut stack = vec![(root, 0usize)];
        state[root] = 1;
        while let Some(&mut (i, ref mut next)) = stack.last_mut() {
            if let Some(&(j, _)) = calls[i].get(*next) {
                *next += 1;
                if state[j] == 0 {
                    state[j] = 1;
                    stack.push((j, 0));
                }
            } else {
                state[i] = 2;
                order.push(i);
                stack.pop();
            }
        }
        let mut pos = vec![usize::MAX; n_fns];
        for (k, &i) in order.iter().enumerate() {
            pos[i] = k;
        }
        let mut copies = vec![0usize; n_fns];
        copies[root] = 1;
        for &i in order.iter().rev() {
            for &(j, n) in &calls[i] {
                if pos[j] < pos[i] {
                    copies[j] = copies[j].saturating_add(copies[i].saturating_mul(n));
                }
            }
        }
        let mut total = 0usize;
        let mut top = Vec::new();
        for (i, f) in self.functions.iter().enumerate() {
            if copies[i] == 0 {
                continue;
            }
            total = total.saturating_add(copies[i].saturating_mul(f.out.len()));
            if let Some(name) = &names[i] {
                top.push((name.clone(), copies[i], f.out.len()));
            }
        }
        top.sort_by_key(|(_, c, l)| std::cmp::Reverse(c.saturating_mul(*l)));
        top.truncate(4);
        (total, top)
    }

    /// How many times `body` calls `function_name`.
    fn glsl_count_calls(body: &str, function_name: &str) -> usize {
        let pattern = format!("{}(", function_name);
        let mut count = 0;
        let mut search_start = 0;
        while let Some(pos) = body[search_start..].find(&pattern) {
            let abs = search_start + pos;
            let prev_is_ident = body[..abs].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
            if !prev_is_ident {
                count += 1;
            }
            search_start = abs + pattern.len();
        }
        count
    }

    fn glsl_collect_reachable_functions(&self, entries: &[&str]) -> BTreeSet<usize> {
        let mut reachable = BTreeSet::new();
        let mut work = Vec::new();
        let function_names: Vec<String> = self
            .functions
            .iter()
            .filter_map(|func| Self::glsl_function_name_from_sig(&func.call_sig))
            .collect();

        if function_names.len() != self.functions.len() {
            // If we failed to parse any signature, fall back to including everything.
            return (0..self.functions.len()).collect();
        }

        for entry in entries {
            for (index, name) in function_names.iter().enumerate() {
                if name == entry && reachable.insert(index) {
                    work.push(index);
                }
            }
        }

        while let Some(current) = work.pop() {
            let body = &self.functions[current].out;
            for (index, name) in function_names.iter().enumerate() {
                if reachable.contains(&index) {
                    continue;
                }
                if Self::glsl_body_calls_function(body, name) {
                    reachable.insert(index);
                    work.push(index);
                }
            }
        }

        reachable
    }

    fn glsl_function_name_from_sig(call_sig: &str) -> Option<String> {
        let open_paren = call_sig.find('(')?;
        let head = call_sig[..open_paren].trim_end();
        let name = head.split_whitespace().next_back()?;
        Some(name.to_string())
    }

    /// `ident` appears in `body` as a whole identifier.
    fn glsl_body_mentions(body: &str, ident: &str) -> bool {
        let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
        let mut search_start = 0;
        while let Some(pos) = body[search_start..].find(ident) {
            let abs = search_start + pos;
            let end = abs + ident.len();
            let before = body[..abs].chars().next_back().is_some_and(is_ident);
            let after = body[end..].chars().next().is_some_and(is_ident);
            if !before && !after {
                return true;
            }
            search_start = end;
        }
        false
    }

    fn glsl_body_calls_function(body: &str, function_name: &str) -> bool {
        let pattern = format!("{}(", function_name);
        let mut search_start = 0;
        while let Some(pos) = body[search_start..].find(&pattern) {
            let abs = search_start + pos;
            let prev = body[..abs].chars().next_back();
            let prev_is_ident = prev
                .map(|c| c.is_ascii_alphanumeric() || c == '_')
                .unwrap_or(false);
            if !prev_is_ident {
                return true;
            }
            search_start = abs + pattern.len();
        }
        false
    }

    fn glsl_assign_typed_vertex_inputs(&self, vm: &ScriptVm, out: &mut String) {
        for io in &self.io {
            let (prefix, var_prefix) = match io.kind {
                ShaderIoKind::VertexBuffer => ("geom_", "vb_"),
                ShaderIoKind::DynInstance => ("inst_", "dyninst_"),
                ShaderIoKind::RustInstance => ("inst_", "rustinst_"),
                _ => continue,
            };
            let io_name = self.backend.map_io_name(io.name);
            let dst = format!("{}{}", var_prefix, io_name);
            let src = format!("{}{}", prefix, io_name);
            let pod_ty = &vm.bx.heap.pod_type_ref(io.ty).ty;
            let src = if matches!(pod_ty, ScriptPodTy::Struct { .. }) {
                prefix.trim_end_matches('_').to_string()
            } else {
                src
            };
            self.glsl_assign_typed_ty(vm, pod_ty, &dst, &src, out);
        }
    }

    fn glsl_assign_typed_ty(
        &self,
        vm: &ScriptVm,
        ty: &ScriptPodTy,
        dst: &str,
        src: &str,
        out: &mut String,
    ) {
        match ty {
            ScriptPodTy::Struct { fields, .. } => {
                for field in fields {
                    let field_name = self.backend.map_field_name(field.name);
                    self.glsl_assign_typed_ty(
                        vm,
                        &field.ty.data.ty,
                        &format!("{}.{}", dst, field_name),
                        &format!("{}_{}", src, field_name),
                        out,
                    );
                }
            }
            _ => {
                writeln!(out, "    {} = {};", dst, src).ok();
            }
        }
    }

    fn glsl_collect_geometry_fields(&self, vm: &ScriptVm) -> Vec<GlslPackedField> {
        let mut out = Vec::new();
        let mut offset = 0;
        for io in &self.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                self.glsl_push_field(vm, io, "vb_", true, &mut offset, &mut out);
            }
        }
        out
    }

    fn glsl_collect_instance_fields(&self, vm: &ScriptVm) -> Vec<GlslPackedField> {
        let mut out = Vec::new();
        let mut offset = 0;
        for io in &self.io {
            if let ShaderIoKind::DynInstance = io.kind {
                self.glsl_push_field(vm, io, "dyninst_", true, &mut offset, &mut out);
            }
        }
        for io in &self.io {
            if let ShaderIoKind::RustInstance = io.kind {
                self.glsl_push_field(vm, io, "rustinst_", true, &mut offset, &mut out);
            }
        }
        out
    }

    /// The varyings: the instance fields the fragment stage reads, then the
    /// shader's own varyings (the rule every emitter follows).
    fn glsl_collect_varying_pack_fields(&self, vm: &ScriptVm) -> Vec<GlslPackedField> {
        let entry = self.backend.map_function_name("io_fragment");
        let reachable = self.glsl_collect_reachable_functions(&[entry.as_str()]);
        let fragment_reads = |name: &str| {
            reachable
                .iter()
                .any(|&index| Self::glsl_body_mentions(&self.functions[index].out, name))
        };
        self.glsl_pack_varying_fields(vm, &fragment_reads)
    }

    fn glsl_pack_varying_fields(
        &self,
        vm: &ScriptVm,
        keep_instance: &dyn Fn(&str) -> bool,
    ) -> Vec<GlslPackedField> {
        let mut out = Vec::new();
        let mut offset = 0;
        for rust_instance in [false, true] {
            let prefix = if rust_instance { "rustinst_" } else { "dyninst_" };
            for io in &self.io {
                let wanted = match io.kind {
                    ShaderIoKind::DynInstance => !rust_instance,
                    ShaderIoKind::RustInstance => rust_instance,
                    _ => false,
                };
                if !wanted {
                    continue;
                }
                let name = format!("{}{}", prefix, self.backend.map_io_name(io.name));
                if keep_instance(&name) {
                    self.glsl_push_field(vm, io, prefix, false, &mut offset, &mut out);
                }
            }
        }
        for io in &self.io {
            if let ShaderIoKind::Varying = io.kind {
                self.glsl_push_field(vm, io, "var_", false, &mut offset, &mut out);
            }
        }
        out
    }

    fn glsl_push_field(
        &self,
        vm: &ScriptVm,
        io: &crate::shader::ShaderIo,
        prefix: &str,
        attribute_packing: bool,
        offset: &mut usize,
        out: &mut Vec<GlslPackedField>,
    ) {
        let io_name = self.backend.map_io_name(io.name);
        let pod_ty = vm.bx.heap.pod_type_ref(io.ty);
        let slots = pod_ty.ty.slots();
        let attr_format = if attribute_packing {
            Self::glsl_attr_format_from_pod_ty(&pod_ty.ty)
        } else {
            GlslPackedFormat::Float
        };

        if attribute_packing
            && attr_format != GlslPackedFormat::Float
            && slots > 1
            && (*offset & 3) != 0
        {
            *offset += 4 - (*offset & 3);
        }
        out.push(GlslPackedField {
            name: format!("{}{}", prefix, io_name),
            ty: io.ty,
            slots,
            offset: *offset,
        });
        *offset += slots;
        if attribute_packing
            && attr_format != GlslPackedFormat::Float
            && slots > 1
            && (*offset & 3) != 0
        {
            *offset += 4 - (*offset & 3);
        }
    }

    fn glsl_unpack_expr_for_field(
        &self,
        vm: &ScriptVm,
        field: &GlslPackedField,
        prefix: &str,
        source: GlslPackedSource,
    ) -> String {
        let scalars = (0..field.slots)
            .map(|slot| Self::glsl_packed_component(prefix, field.offset + slot))
            .collect::<Vec<_>>();
        let mut scalar_index = 0usize;
        self.glsl_reconstruct_from_scalars(vm, field.ty, source, &scalars, &mut scalar_index)
    }

    /// Emit assignment statements to unpack a packed field into a variable.
    ///
    /// For struct-typed fields, this generates per-sub-field assignments instead
    /// of using GLSL struct constructor syntax, which some GLES drivers (notably
    /// the Android emulator's ANGLE/SwiftShader) reject.
    ///
    /// For example, instead of `vb_geom = QuadVertex(vec2(x, y));` this emits:
    ///   `vb_geom.pos = vec2(x, y);`
    fn glsl_unpack_field_to_statements(
        &self,
        vm: &ScriptVm,
        field: &GlslPackedField,
        prefix: &str,
        source: GlslPackedSource,
        out: &mut String,
    ) {
        let pod_ty = vm.bx.heap.pod_type_ref(field.ty);
        if let ScriptPodTy::Struct {
            fields: sub_fields, ..
        } = &pod_ty.ty
        {
            let scalars = (0..field.slots)
                .map(|slot| Self::glsl_packed_component(prefix, field.offset + slot))
                .collect::<Vec<_>>();
            let mut scalar_index = 0usize;
            for sub_field in sub_fields {
                let sub_field_name = self.backend.map_field_name(sub_field.name);
                let value_expr = self.glsl_reconstruct_inline(
                    vm,
                    &sub_field.ty,
                    source,
                    &scalars,
                    &mut scalar_index,
                );
                writeln!(
                    out,
                    "    {}.{} = {};",
                    field.name, sub_field_name, value_expr
                )
                .ok();
            }
        } else {
            let value_expr = self.glsl_unpack_expr_for_field(vm, field, prefix, source);
            writeln!(out, "    {} = {};", field.name, value_expr).ok();
        }
    }

    fn glsl_reconstruct_from_scalars(
        &self,
        vm: &ScriptVm,
        ty: ScriptPodType,
        source: GlslPackedSource,
        scalars: &[String],
        scalar_index: &mut usize,
    ) -> String {
        let pod_ty = vm.bx.heap.pod_type_ref(ty);
        let inline = ScriptPodTypeInline {
            self_ref: ty,
            data: pod_ty.clone(),
        };
        self.glsl_reconstruct_inline(vm, &inline, source, scalars, scalar_index)
    }

    fn glsl_reconstruct_inline(
        &self,
        vm: &ScriptVm,
        ty: &ScriptPodTypeInline,
        source: GlslPackedSource,
        scalars: &[String],
        scalar_index: &mut usize,
    ) -> String {
        match &ty.data.ty {
            ScriptPodTy::Struct { fields, .. } => {
                let mut field_exprs = Vec::new();
                for field in fields {
                    field_exprs.push(self.glsl_reconstruct_inline(
                        vm,
                        &field.ty,
                        source,
                        scalars,
                        scalar_index,
                    ));
                }
                format!(
                    "{}({})",
                    self.glsl_type_name_inline(ty),
                    field_exprs.join(", ")
                )
            }
            ScriptPodTy::Vec(vec_ty) => {
                let dims = vec_ty.dims();
                let mut comps = Vec::with_capacity(dims);
                let elem_ty = vec_ty.elem_ty();
                for _ in 0..dims {
                    let scalar = Self::glsl_take_scalar_or_zero(scalars, scalar_index);
                    comps.push(Self::glsl_convert_scalar_expr(source, &elem_ty, &scalar));
                }
                format!("{}({})", self.glsl_type_name_inline(ty), comps.join(", "))
            }
            ScriptPodTy::Mat(mat_ty) => {
                let mut comps = Vec::new();
                for _ in 0..mat_ty.dim() {
                    let scalar = Self::glsl_take_scalar_or_zero(scalars, scalar_index);
                    comps.push(Self::glsl_convert_scalar_expr(
                        source,
                        &ScriptPodTy::F32,
                        &scalar,
                    ));
                }
                format!("{}({})", self.glsl_type_name_inline(ty), comps.join(", "))
            }
            scalar_ty => {
                let scalar = Self::glsl_take_scalar_or_zero(scalars, scalar_index);
                Self::glsl_convert_scalar_expr(source, scalar_ty, &scalar)
            }
        }
    }

    fn glsl_take_scalar_or_zero(scalars: &[String], scalar_index: &mut usize) -> String {
        let value = scalars
            .get(*scalar_index)
            .cloned()
            .unwrap_or_else(|| "0.0".to_string());
        *scalar_index += 1;
        value
    }

    fn glsl_flatten_exprs(
        &self,
        vm: &ScriptVm,
        ty: ScriptPodType,
        expr: &str,
        out: &mut Vec<String>,
    ) {
        let pod_ty = vm.bx.heap.pod_type_ref(ty);
        let inline = ScriptPodTypeInline {
            self_ref: ty,
            data: pod_ty.clone(),
        };
        self.glsl_flatten_inline(&inline, expr, out);
    }

    fn glsl_flatten_inline(&self, ty: &ScriptPodTypeInline, expr: &str, out: &mut Vec<String>) {
        match &ty.data.ty {
            ScriptPodTy::Struct { fields, .. } => {
                for field in fields {
                    let field_name = self.backend.map_field_name(field.name);
                    let field_expr = format!("({}).{}", expr, field_name);
                    self.glsl_flatten_inline(&field.ty, &field_expr, out);
                }
            }
            ScriptPodTy::Vec(vec_ty) => {
                let elem_ty = vec_ty.elem_ty();
                for comp in 0..vec_ty.dims() {
                    let swizzle = Self::glsl_swizzle_component(comp);
                    let comp_expr = format!("({}).{}", expr, swizzle);
                    out.push(Self::glsl_to_float_scalar_expr(&elem_ty, &comp_expr));
                }
            }
            ScriptPodTy::Mat(mat_ty) => {
                let (cols, rows) = mat_ty.dims();
                for col in 0..cols {
                    for row in 0..rows {
                        out.push(format!("({})[{}][{}]", expr, col, row));
                    }
                }
            }
            scalar_ty => {
                out.push(Self::glsl_to_float_scalar_expr(scalar_ty, expr));
            }
        }
    }

    fn glsl_to_float_scalar_expr(ty: &ScriptPodTy, expr: &str) -> String {
        match ty {
            ScriptPodTy::F32 | ScriptPodTy::F16 => expr.to_string(),
            ScriptPodTy::U32
            | ScriptPodTy::I32
            | ScriptPodTy::AtomicU32
            | ScriptPodTy::AtomicI32 => format!("float({})", expr),
            ScriptPodTy::Bool => format!("(({}) ? 1.0 : 0.0)", expr),
            _ => format!("float({})", expr),
        }
    }

    fn glsl_convert_scalar_expr(
        source: GlslPackedSource,
        target: &ScriptPodTy,
        expr: &str,
    ) -> String {
        match source {
            GlslPackedSource::NumericFloat => match target {
                ScriptPodTy::F32 | ScriptPodTy::F16 => expr.to_string(),
                ScriptPodTy::U32 | ScriptPodTy::AtomicU32 => format!("uint({})", expr),
                ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => format!("int({})", expr),
                ScriptPodTy::Bool => format!("({} != 0.0)", expr),
                _ => format!("float({})", expr),
            },
            GlslPackedSource::BitPackedFloat => match target {
                ScriptPodTy::F32 | ScriptPodTy::F16 => expr.to_string(),
                ScriptPodTy::U32 | ScriptPodTy::AtomicU32 => format!("floatBitsToUint({})", expr),
                ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => format!("floatBitsToInt({})", expr),
                ScriptPodTy::Bool => format!("(floatBitsToUint({}) != 0u)", expr),
                _ => expr.to_string(),
            },
        }
    }

    fn glsl_attr_format_from_pod_ty(ty: &ScriptPodTy) -> GlslPackedFormat {
        match ty {
            ScriptPodTy::U32 | ScriptPodTy::AtomicU32 | ScriptPodTy::Bool => GlslPackedFormat::UInt,
            ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => GlslPackedFormat::SInt,
            ScriptPodTy::Vec(v) => match v {
                crate::pod::ScriptPodVec::Vec2u
                | crate::pod::ScriptPodVec::Vec3u
                | crate::pod::ScriptPodVec::Vec4u
                | crate::pod::ScriptPodVec::Vec2b
                | crate::pod::ScriptPodVec::Vec3b
                | crate::pod::ScriptPodVec::Vec4b => GlslPackedFormat::UInt,
                crate::pod::ScriptPodVec::Vec2i
                | crate::pod::ScriptPodVec::Vec3i
                | crate::pod::ScriptPodVec::Vec4i => GlslPackedFormat::SInt,
                _ => GlslPackedFormat::Float,
            },
            _ => GlslPackedFormat::Float,
        }
    }

    fn glsl_uniform_block_name(&self, name: LiveId) -> String {
        let io_name = self.backend.map_io_name(name);
        match io_name.as_str() {
            "draw_pass" => "passUniforms".to_string(),
            "draw_list" => "draw_listUniforms".to_string(),
            "draw_call" => "draw_callUniforms".to_string(),
            _ => format!("{}_Uniforms", io_name),
        }
    }

    fn glsl_sampler_type(&self, tex_type: TextureType) -> &'static str {
        match tex_type {
            TextureType::Texture1d => "sampler2D",
            TextureType::Texture1dArray => "sampler2DArray",
            TextureType::Texture2d => "sampler2D",
            TextureType::Texture2dArray => "sampler2DArray",
            TextureType::Texture3d => "sampler3D",
            TextureType::Texture3dArray => "sampler3D",
            TextureType::TextureCube => "samplerCube",
            TextureType::TextureCubeArray => "samplerCubeArray",
            TextureType::TextureDepth => "sampler2DShadow",
            TextureType::TextureDepthArray => "sampler2DArray",
            TextureType::TextureVideo => {
                // Android SurfaceTexture and Linux DMA-Buf NV12 planes use EXTERNAL_OES.
                if cfg!(any(target_os = "android", target_os = "linux")) && !self.use_vulkan {
                    "samplerExternalOES"
                } else {
                    "sampler2D"
                }
            }
        }
    }

    fn glsl_num_packed_vec4s(slots: usize) -> usize {
        if slots == 0 {
            0
        } else {
            (slots + 3) / 4
        }
    }

    fn glsl_packed_component(prefix: &str, slot: usize) -> String {
        let vec_idx = slot / 4;
        let comp = Self::glsl_swizzle_component(slot & 3);
        format!("{}{}.{}", prefix, vec_idx, comp)
    }

    fn glsl_swizzle_component(index: usize) -> &'static str {
        match index {
            0 => "x",
            1 => "y",
            2 => "z",
            _ => "w",
        }
    }

    fn glsl_type_name_from_ty(&self, vm: &ScriptVm, ty: ScriptPodType) -> String {
        let mut out = String::new();
        self.backend
            .pod_type_name_from_ty(&vm.bx.heap, ty, &mut out);
        out
    }

    fn glsl_type_name_inline(&self, ty: &ScriptPodTypeInline) -> String {
        if matches!(ty.data.ty, ScriptPodTy::Struct { .. }) {
            if let Some(name) = ty.data.name {
                return format!("{}", self.backend.map_pod_name(name));
            }
            return format!("S{}", ty.self_ref.index);
        }
        let mut out = String::new();
        self.backend.pod_type_name(ty, &mut out);
        out
    }
}

#[cfg(test)]
mod typed_vertex_tests {
    use super::*;
    use crate::{
        pod::ScriptPodField,
        shader::ShaderIo,
        shader_backend::ShaderBackend,
        value::NIL,
        vm::{ScriptVmBase, ScriptVmHost},
    };

    fn field(vm: &ScriptVmBase, name: LiveId, ty: ScriptPodType) -> ScriptPodField {
        ScriptPodField {
            name,
            default: NIL,
            ty: ScriptPodTypeInline {
                self_ref: ty,
                data: vm.heap.pod_type_ref(ty).clone(),
            },
        }
    }

    fn vertex_io(name: LiveId, ty: ScriptPodType) -> ShaderIo {
        ShaderIo {
            kind: ShaderIoKind::VertexBuffer,
            name,
            ty,
            buffer_index: None,
        }
    }

    #[test]
    fn compact_glsl_uses_typed_attributes_and_legacy_stays_packed() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: &mut host,
            bx: Box::new(ScriptVmBase::new()),
        };
        let half2 = vm.bx.code.builtins.pod.pod_f16x2;
        let color = vm.bx.code.builtins.pod.pod_unorm8x4;
        let object = vm.bx.heap.new_object();
        let compact_ty = vm.bx.heap.new_pod_type(
            object,
            Some(id!(GlslCompactVertex)),
            ScriptPodTy::new_struct(vec![
                field(&vm.bx, id!(off), half2),
                field(&vm.bx, id!(color), color),
            ]),
            NIL,
        );
        let compact = ShaderOutput {
            backend: ShaderBackend::Glsl,
            io: vec![vertex_io(id!(vertex), compact_ty)],
            ..Default::default()
        };
        let mut source = String::new();
        compact.glsl_write_vertex_input_attrs(&vm, &[], &[], None, &mut source);
        compact.glsl_assign_typed_vertex_inputs(&vm, &mut source);
        let off_name = compact.backend.map_field_name(id!(off));
        let color_name = compact.backend.map_field_name(id!(color));
        let vertex_name = compact.backend.map_io_name(id!(vertex));
        assert!(
            source.contains(&format!("in vec2 geom_{off_name};")),
            "{source}"
        );
        assert!(
            source.contains(&format!("in vec4 geom_{color_name};")),
            "{source}"
        );
        assert!(
            source.contains(&format!("vb_{vertex_name}.{off_name} = geom_{off_name};")),
            "{source}"
        );
        assert!(
            source.contains(&format!(
                "vb_{vertex_name}.{color_name} = geom_{color_name};"
            )),
            "{source}"
        );

        let vec4f = vm.bx.code.builtins.pod.pod_vec4f;
        let object = vm.bx.heap.new_object();
        let legacy_ty = vm.bx.heap.new_pod_type(
            object,
            Some(id!(LegacyTwelveFloats)),
            ScriptPodTy::new_struct(vec![
                field(&vm.bx, id!(a), vec4f),
                field(&vm.bx, id!(b), vec4f),
                field(&vm.bx, id!(c), vec4f),
            ]),
            NIL,
        );
        let legacy = ShaderOutput {
            backend: ShaderBackend::Glsl,
            io: vec![vertex_io(id!(legacy), legacy_ty)],
            ..Default::default()
        };
        let geometry_fields = legacy.glsl_collect_geometry_fields(&vm);
        let mut source = String::new();
        legacy.glsl_write_vertex_input_attrs(&vm, &geometry_fields, &[], None, &mut source);
        assert!(source.contains("in vec4 packed_geometry_0;"), "{source}");
        assert!(source.contains("in vec4 packed_geometry_1;"), "{source}");
        assert!(source.contains("in vec4 packed_geometry_2;"), "{source}");
        assert!(!source.contains("geom_a"), "{source}");
    }

    fn instance_io(name: LiveId, ty: ScriptPodType) -> ShaderIo {
        ShaderIo {
            kind: ShaderIoKind::RustInstance,
            name,
            ty,
            buffer_index: None,
        }
    }

    /// GL ES 3 and WebGL 2: instance data past the vertex inputs every
    /// device has are read from `mp_inst_data`; a record wider than an
    /// attribute stride is read whole from it; a shader that fits is
    /// emitted unchanged.
    #[test]
    fn instance_data_past_the_attribute_limit_comes_from_a_texture() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: &mut host,
            bx: Box::new(ScriptVmBase::new()),
        };
        let vec4f = vm.bx.code.builtins.pod.pod_vec4f;
        let make = |vm: &mut ScriptVm, geometry_vec4s: usize, instance_vec4s: usize, webgl2: bool| {
            let names = [id!(a), id!(b), id!(c), id!(d), id!(e), id!(f), id!(g), id!(h), id!(i), id!(j), id!(k), id!(l), id!(m), id!(n), id!(o), id!(p), id!(q), id!(r), id!(s), id!(t)];
            let mut io = Vec::new();
            let object = vm.bx.heap.new_object();
            let geometry_ty = vm.bx.heap.new_pod_type(
                object,
                None,
                ScriptPodTy::new_struct((0..geometry_vec4s).map(|n| field(&vm.bx, names[n], vec4f)).collect()),
                NIL,
            );
            io.push(vertex_io(id!(geom), geometry_ty));
            for n in 0..instance_vec4s {
                io.push(instance_io(names[n], vec4f));
            }
            ShaderOutput {
                backend: ShaderBackend::Glsl,
                glsl_webgl2: webgl2,
                io,
                ..Default::default()
            }
        };
        let emit = |vm: &ScriptVm, output: &ShaderOutput| {
            let geometry = output.glsl_collect_geometry_fields(vm);
            let instance = output.glsl_collect_instance_fields(vm);
            let from = output.glsl_instance_fetch_from(vm, &geometry, &instance);
            let mut source = String::new();
            output.glsl_write_vertex_input_attrs(vm, &geometry, &instance, from, &mut source);
            output.glsl_write_vertex_main(vm, &geometry, &instance, from, &[], &mut source);
            (from, source)
        };

        // 2 + 14 inputs: fits, unchanged.
        let fits = make(&mut vm, 2, 14, true);
        let (from, source) = emit(&vm, &fits);
        assert_eq!(from, None);
        assert!(source.contains("in vec4 packed_instance_13;"), "{source}");
        assert!(!source.contains("mp_inst_data"), "{source}");

        // 2 + 15 inputs (a 240-byte record): the 15th instance vec4 is fetched.
        let over = make(&mut vm, 2, 15, true);
        let (from, source) = emit(&vm, &over);
        assert_eq!(from, Some(14));
        assert!(source.contains("in vec4 packed_instance_13;"), "{source}");
        assert!(source.contains("\nvec4 packed_instance_14;"), "{source}");
        assert!(source.contains("uniform highp usampler2D mp_inst_data;"), "{source}");
        assert!(source.contains("gl_InstanceID * 60 + slot"), "{source}");
        assert!(
            source.contains("packed_instance_14 = vec4(_mp_inst_word(56), _mp_inst_word(57), _mp_inst_word(58), _mp_inst_word(59));"),
            "{source}"
        );

        // A 320-byte record cannot be an attribute stride: all of it is fetched.
        let wide = make(&mut vm, 1, 20, true);
        let (from, source) = emit(&vm, &wide);
        assert_eq!(from, Some(0));
        assert!(!source.contains("in vec4 packed_instance_"), "{source}");
        assert!(source.contains("packed_instance_0 = vec4(_mp_inst_word(0)"), "{source}");

        // Native GL ES: the same rule, with GL ES's 2048-byte stride.
        let native = make(&mut vm, 2, 15, false);
        let (from, source) = emit(&vm, &native);
        assert_eq!(from, Some(14));
        assert!(source.contains("\nvec4 packed_instance_14;"), "{source}");
        let native_wide = make(&mut vm, 1, 20, false);
        assert_eq!(emit(&vm, &native_wide).0, Some(15));
    }

    #[test]
    fn body_mentions_matches_whole_identifiers_only() {
        assert!(ShaderOutput::glsl_body_mentions("x = rustinst_fur.x;", "rustinst_fur"));
        assert!(!ShaderOutput::glsl_body_mentions("x = rustinst_fur_layer.x;", "rustinst_fur"));
        assert!(!ShaderOutput::glsl_body_mentions("x = my_rustinst_fur;", "rustinst_fur"));
        assert!(ShaderOutput::glsl_body_mentions("rustinst_fur_layer + rustinst_fur", "rustinst_fur"));
    }
}
