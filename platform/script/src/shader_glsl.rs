use crate::pod::{ScriptPodTy, ScriptPodTypeInline};
use crate::shader::{ShaderIoKind, ShaderOutput, TextureType};
use crate::value::ScriptPodType;
use crate::vm::ScriptVm;
use makepad_live_id::{id, LiveId};
use std::collections::BTreeSet;
use std::fmt::Write;

#[derive(Clone, Copy, PartialEq, Eq)]
enum GlslPackedFormat {
    Float,
    UInt,
    SInt,
}

#[derive(Clone)]
struct GlslPackedField {
    name: String,
    ty: ScriptPodType,
    slots: usize,
    offset: usize,
    attr_format: GlslPackedFormat,
}

impl ShaderOutput {
    pub fn glsl_create_vertex_shader(
        &self,
        vm: &ScriptVm,
        shared_defs: &str,
        out: &mut String,
    ) {
        let geometry_fields = self.glsl_collect_geometry_fields(vm);
        let instance_fields = self.glsl_collect_instance_fields(vm);
        let varying_fields = self.glsl_collect_varying_pack_fields(vm);

        let varying_slots = varying_fields
            .last()
            .map(|field| field.offset + field.slots)
            .unwrap_or(0);

        out.push_str(shared_defs);
        self.glsl_write_uniform_blocks(vm, out);
        self.glsl_write_texture_uniforms(out);
        self.glsl_write_vertex_globals(vm, out);
        self.glsl_write_vertex_input_attrs(&geometry_fields, &instance_fields, out);
        self.glsl_write_varying_interface(varying_slots, true, out);
        let vertex_entry = self.backend.map_function_name("io_vertex");
        self.glsl_write_functions_for_entries(out, &[vertex_entry.as_str()]);
        self.glsl_write_vertex_main(vm, &geometry_fields, &instance_fields, &varying_fields, out);
    }

    pub fn glsl_create_fragment_shader(
        &self,
        vm: &ScriptVm,
        shared_defs: &str,
        out: &mut String,
    ) {
        let varying_fields = self.glsl_collect_varying_pack_fields(vm);
        let varying_slots = varying_fields
            .last()
            .map(|field| field.offset + field.slots)
            .unwrap_or(0);

        out.push_str(shared_defs);
        self.glsl_write_uniform_blocks(vm, out);
        self.glsl_write_texture_uniforms(out);
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
                let type_name = self.glsl_type_name_from_ty(vm, io.ty);
                let io_name = self.backend.map_io_name(io.name);
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

    fn glsl_write_texture_uniforms(&self, out: &mut String) {
        for io in &self.io {
            if let ShaderIoKind::Texture(tex_type) = io.kind {
                let tex_name = self.backend.map_io_name(io.name);
                writeln!(
                    out,
                    "uniform {} tex_{};",
                    Self::glsl_sampler_type(tex_type),
                    tex_name
                )
                .ok();
            }
        }
    }

    fn glsl_write_vertex_globals(&self, vm: &ScriptVm, out: &mut String) {
        // Keep the vertex position register available as a global for shaders
        // that write `self.pos` directly instead of returning a vec4 from io_vertex().
        writeln!(out, "vec4 vtx_pos;").ok();
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

    fn glsl_write_vertex_input_attrs(
        &self,
        geometry_fields: &[GlslPackedField],
        instance_fields: &[GlslPackedField],
        out: &mut String,
    ) {
        for (idx, format) in Self::glsl_collect_chunk_formats(geometry_fields).iter().enumerate() {
            writeln!(
                out,
                "in {} packed_geometry_{};",
                Self::glsl_vertex_attr_vec_type(*format),
                idx
            )
            .ok();
        }
        for (idx, format) in Self::glsl_collect_chunk_formats(instance_fields).iter().enumerate() {
            writeln!(
                out,
                "in {} packed_instance_{};",
                Self::glsl_vertex_attr_vec_type(*format),
                idx
            )
            .ok();
        }
    }

    fn glsl_write_varying_interface(&self, varying_slots: usize, is_vertex: bool, out: &mut String) {
        let qualifier = if is_vertex { "out" } else { "in" };
        for idx in 0..Self::glsl_num_packed_vec4s(varying_slots) {
            writeln!(out, "{} vec4 packed_varying_{};", qualifier, idx).ok();
        }
    }

    fn glsl_write_fragment_outputs(&self, vm: &ScriptVm, out: &mut String) {
        for io in &self.io {
            if let ShaderIoKind::FragmentOutput(index) = io.kind {
                let type_name = self.glsl_type_name_from_ty(vm, io.ty);
                writeln!(out, "layout(location = {}) out {} _mp_frag_{};", index, type_name, index)
                    .ok();
            }
        }
    }

    fn glsl_write_vertex_main(
        &self,
        vm: &ScriptVm,
        geometry_fields: &[GlslPackedField],
        instance_fields: &[GlslPackedField],
        varying_fields: &[GlslPackedField],
        out: &mut String,
    ) {
        writeln!(out, "void main() {{").ok();
        for field in geometry_fields {
            let value_expr = self.glsl_unpack_expr_for_field(vm, field, "packed_geometry_");
            writeln!(out, "    {} = {};", field.name, value_expr).ok();
        }
        for field in instance_fields {
            let value_expr = self.glsl_unpack_expr_for_field(vm, field, "packed_instance_");
            writeln!(out, "    {} = {};", field.name, value_expr).ok();
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
        writeln!(out, "    gl_Position = vtx_pos;").ok();
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
            let value_expr = self.glsl_unpack_expr_for_field(vm, field, "packed_varying_");
            writeln!(out, "    {} = {};", field.name, value_expr).ok();
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

    fn glsl_collect_varying_pack_fields(&self, vm: &ScriptVm) -> Vec<GlslPackedField> {
        let mut out = Vec::new();
        let mut offset = 0;
        for io in &self.io {
            if let ShaderIoKind::DynInstance = io.kind {
                self.glsl_push_field(vm, io, "dyninst_", false, &mut offset, &mut out);
            }
        }
        for io in &self.io {
            if let ShaderIoKind::RustInstance = io.kind {
                self.glsl_push_field(vm, io, "rustinst_", false, &mut offset, &mut out);
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

        if attribute_packing && attr_format != GlslPackedFormat::Float && (*offset & 3) != 0 {
            *offset += 4 - (*offset & 3);
        }
        out.push(GlslPackedField {
            name: format!("{}{}", prefix, io_name),
            ty: io.ty,
            slots,
            offset: *offset,
            attr_format,
        });
        *offset += slots;
        if attribute_packing && attr_format != GlslPackedFormat::Float && (*offset & 3) != 0 {
            *offset += 4 - (*offset & 3);
        }
    }

    fn glsl_unpack_expr_for_field(&self, vm: &ScriptVm, field: &GlslPackedField, prefix: &str) -> String {
        let scalars = (0..field.slots)
            .map(|slot| Self::glsl_packed_component(prefix, field.offset + slot))
            .collect::<Vec<_>>();
        let mut scalar_index = 0usize;
        self.glsl_reconstruct_from_scalars(
            vm,
            field.ty,
            field.attr_format,
            &scalars,
            &mut scalar_index,
        )
    }

    fn glsl_reconstruct_from_scalars(
        &self,
        vm: &ScriptVm,
        ty: ScriptPodType,
        source_format: GlslPackedFormat,
        scalars: &[String],
        scalar_index: &mut usize,
    ) -> String {
        let pod_ty = vm.bx.heap.pod_type_ref(ty);
        let inline = ScriptPodTypeInline {
            self_ref: ty,
            data: pod_ty.clone(),
        };
        self.glsl_reconstruct_inline(vm, &inline, source_format, scalars, scalar_index)
    }

    fn glsl_reconstruct_inline(
        &self,
        vm: &ScriptVm,
        ty: &ScriptPodTypeInline,
        source_format: GlslPackedFormat,
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
                        source_format,
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
                    let scalar = Self::glsl_take_scalar_or_zero(scalars, scalar_index, source_format);
                    comps.push(Self::glsl_convert_scalar_expr(source_format, &elem_ty, &scalar));
                }
                format!("{}({})", self.glsl_type_name_inline(ty), comps.join(", "))
            }
            ScriptPodTy::Mat(mat_ty) => {
                let mut comps = Vec::new();
                for _ in 0..mat_ty.dim() {
                    let scalar = Self::glsl_take_scalar_or_zero(scalars, scalar_index, source_format);
                    comps.push(Self::glsl_convert_scalar_expr(
                        source_format,
                        &ScriptPodTy::F32,
                        &scalar,
                    ));
                }
                format!("{}({})", self.glsl_type_name_inline(ty), comps.join(", "))
            }
            scalar_ty => {
                let scalar = Self::glsl_take_scalar_or_zero(scalars, scalar_index, source_format);
                Self::glsl_convert_scalar_expr(source_format, scalar_ty, &scalar)
            }
        }
    }

    fn glsl_take_scalar_or_zero(
        scalars: &[String],
        scalar_index: &mut usize,
        source_format: GlslPackedFormat,
    ) -> String {
        let value = scalars
            .get(*scalar_index)
            .cloned()
            .unwrap_or_else(|| match source_format {
                GlslPackedFormat::Float => "0.0".to_string(),
                GlslPackedFormat::UInt => "0u".to_string(),
                GlslPackedFormat::SInt => "0".to_string(),
            });
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

    fn glsl_convert_scalar_expr(source: GlslPackedFormat, target: &ScriptPodTy, expr: &str) -> String {
        match source {
            GlslPackedFormat::Float => match target {
                ScriptPodTy::F32 | ScriptPodTy::F16 => expr.to_string(),
                ScriptPodTy::U32 | ScriptPodTy::AtomicU32 => format!("uint({})", expr),
                ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => format!("int({})", expr),
                ScriptPodTy::Bool => format!("({} != 0.0)", expr),
                _ => format!("float({})", expr),
            },
            GlslPackedFormat::UInt => match target {
                ScriptPodTy::U32 | ScriptPodTy::AtomicU32 => expr.to_string(),
                ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => format!("int({})", expr),
                ScriptPodTy::F32 | ScriptPodTy::F16 => format!("float({})", expr),
                ScriptPodTy::Bool => format!("({} != 0u)", expr),
                _ => format!("float({})", expr),
            },
            GlslPackedFormat::SInt => match target {
                ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => expr.to_string(),
                ScriptPodTy::U32 | ScriptPodTy::AtomicU32 => format!("uint({})", expr),
                ScriptPodTy::F32 | ScriptPodTy::F16 => format!("float({})", expr),
                ScriptPodTy::Bool => format!("({} != 0)", expr),
                _ => format!("float({})", expr),
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

    fn glsl_collect_chunk_formats(fields: &[GlslPackedField]) -> Vec<GlslPackedFormat> {
        let slots = fields
            .last()
            .map(|field| field.offset + field.slots)
            .unwrap_or(0);
        let mut out = vec![GlslPackedFormat::Float; Self::glsl_num_packed_vec4s(slots)];
        for field in fields {
            if field.attr_format == GlslPackedFormat::Float {
                continue;
            }
            for slot in field.offset..field.offset + field.slots {
                out[slot / 4] = field.attr_format;
            }
        }
        out
    }

    fn glsl_vertex_attr_vec_type(format: GlslPackedFormat) -> &'static str {
        match format {
            GlslPackedFormat::Float => "vec4",
            GlslPackedFormat::UInt => "uvec4",
            GlslPackedFormat::SInt => "ivec4",
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

    fn glsl_sampler_type(tex_type: TextureType) -> &'static str {
        match tex_type {
            TextureType::Texture1d => "sampler2D",
            TextureType::Texture1dArray => "sampler2DArray",
            TextureType::Texture2d => "sampler2D",
            TextureType::Texture2dArray => "sampler2DArray",
            TextureType::Texture3d => "sampler3D",
            TextureType::Texture3dArray => "sampler3D",
            TextureType::TextureCube => "samplerCube",
            TextureType::TextureCubeArray => "samplerCubeArray",
            TextureType::TextureDepth => "sampler2D",
            TextureType::TextureDepthArray => "sampler2DArray",
            TextureType::TextureVideo => "samplerExternalOES",
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
        self.backend.pod_type_name_from_ty(&vm.bx.heap, ty, &mut out);
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
