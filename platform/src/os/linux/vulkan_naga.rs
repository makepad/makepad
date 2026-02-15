use {
    crate::{
        makepad_live_id::*,
        makepad_script::{
            pod::{ScriptPodTy, ScriptPodTypeInline, ScriptPodVec},
            shader::{
                ShaderFnCompiler, ShaderIoKind, ShaderMode, ShaderOutput, ShaderType, TextureType,
            },
            shader_backend::ShaderBackend,
            trap::NoTrap,
            value::ScriptObject,
            vm::ScriptVm,
            ScriptPodType,
        },
    },
    std::fmt::Write,
};

#[derive(Clone)]
pub struct CxVulkanShaderBinary {
    pub vertex_spirv: Option<Vec<u32>>,
    pub fragment_spirv: Option<Vec<u32>>,
    pub dyn_uniform_binding: u32,
    pub texture_binding_base: u32,
    pub sampler_binding_base: u32,
    pub geometry_slots: usize,
    pub instance_slots: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WgslPackedFormat {
    Float,
    UInt,
    SInt,
}

#[derive(Clone)]
struct WgslPackedField {
    name: String,
    ty: ScriptPodType,
    slots: usize,
    offset: usize,
    attr_format: WgslPackedFormat,
}

fn wgsl_texture_type(tex_type: TextureType) -> &'static str {
    match tex_type {
        TextureType::Texture1d => "texture_2d<f32>",
        TextureType::Texture1dArray => "texture_2d_array<f32>",
        TextureType::Texture2d => "texture_2d<f32>",
        TextureType::Texture2dArray => "texture_2d_array<f32>",
        TextureType::Texture3d => "texture_3d<f32>",
        TextureType::Texture3dArray => "texture_3d<f32>",
        TextureType::TextureCube => "texture_cube<f32>",
        TextureType::TextureCubeArray => "texture_cube_array<f32>",
        TextureType::TextureDepth => "texture_depth_2d",
        TextureType::TextureDepthArray => "texture_depth_2d_array",
    }
}

fn wgsl_type_name(output: &ShaderOutput, vm: &ScriptVm, ty: ScriptPodType) -> String {
    let mut out = String::new();
    output
        .backend
        .pod_type_name_from_ty(&vm.bx.heap, ty, &mut out);
    out
}

fn wgsl_type_name_inline(output: &ShaderOutput, vm: &ScriptVm, ty: &ScriptPodTypeInline) -> String {
    if matches!(ty.data.ty, ScriptPodTy::Struct { .. }) {
        if let Some(name) = ty.data.name {
            return format!("{}", output.backend.map_pod_name(name));
        }
        return wgsl_type_name(output, vm, ty.self_ref);
    }
    let mut out = String::new();
    output.backend.pod_type_name(ty, &mut out);
    out
}

fn wgsl_attr_format_from_pod_ty(ty: &ScriptPodTy) -> WgslPackedFormat {
    match ty {
        ScriptPodTy::U32 | ScriptPodTy::AtomicU32 | ScriptPodTy::Bool => WgslPackedFormat::UInt,
        ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => WgslPackedFormat::SInt,
        ScriptPodTy::Vec(v) => match v {
            ScriptPodVec::Vec2u
            | ScriptPodVec::Vec3u
            | ScriptPodVec::Vec4u
            | ScriptPodVec::Vec2b
            | ScriptPodVec::Vec3b
            | ScriptPodVec::Vec4b => WgslPackedFormat::UInt,
            ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i => {
                WgslPackedFormat::SInt
            }
            _ => WgslPackedFormat::Float,
        },
        _ => WgslPackedFormat::Float,
    }
}

fn wgsl_num_packed_vec4s(slots: usize) -> usize {
    if slots == 0 {
        0
    } else {
        (slots + 3) / 4
    }
}

fn wgsl_swizzle_component(index: usize) -> &'static str {
    match index {
        0 => "x",
        1 => "y",
        2 => "z",
        _ => "w",
    }
}

fn wgsl_packed_component(prefix: &str, slot: usize) -> String {
    let vec_idx = slot / 4;
    let comp = wgsl_swizzle_component(slot & 3);
    format!("{prefix}{vec_idx}.{comp}")
}

fn wgsl_vertex_attr_vec_type(format: WgslPackedFormat) -> &'static str {
    match format {
        WgslPackedFormat::Float => "vec4f",
        WgslPackedFormat::UInt => "vec4u",
        WgslPackedFormat::SInt => "vec4i",
    }
}

fn wgsl_push_field(
    output: &ShaderOutput,
    vm: &ScriptVm,
    io: &crate::makepad_script::shader::ShaderIo,
    prefix: &str,
    attribute_packing: bool,
    offset: &mut usize,
    out: &mut Vec<WgslPackedField>,
) {
    let io_name = output.backend.map_io_name(io.name);
    let pod_ty = vm.bx.heap.pod_type_ref(io.ty);
    let slots = pod_ty.ty.slots();
    let attr_format = if attribute_packing {
        wgsl_attr_format_from_pod_ty(&pod_ty.ty)
    } else {
        WgslPackedFormat::Float
    };

    if attribute_packing && attr_format != WgslPackedFormat::Float && (*offset & 3) != 0 {
        *offset += 4 - (*offset & 3);
    }

    out.push(WgslPackedField {
        name: format!("{prefix}{io_name}"),
        ty: io.ty,
        slots,
        offset: *offset,
        attr_format,
    });
    *offset += slots;

    if attribute_packing && attr_format != WgslPackedFormat::Float && (*offset & 3) != 0 {
        *offset += 4 - (*offset & 3);
    }
}

fn wgsl_collect_geometry_fields(output: &ShaderOutput, vm: &ScriptVm) -> Vec<WgslPackedField> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for io in &output.io {
        if let ShaderIoKind::VertexBuffer = io.kind {
            wgsl_push_field(output, vm, io, "vb_", true, &mut offset, &mut out);
        }
    }
    out
}

fn wgsl_collect_instance_fields(output: &ShaderOutput, vm: &ScriptVm) -> Vec<WgslPackedField> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for io in &output.io {
        if let ShaderIoKind::DynInstance = io.kind {
            wgsl_push_field(output, vm, io, "dyninst_", true, &mut offset, &mut out);
        }
    }
    for io in &output.io {
        if let ShaderIoKind::RustInstance = io.kind {
            wgsl_push_field(output, vm, io, "rustinst_", true, &mut offset, &mut out);
        }
    }
    out
}

fn wgsl_collect_varying_fields(output: &ShaderOutput, vm: &ScriptVm) -> Vec<WgslPackedField> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for io in &output.io {
        if let ShaderIoKind::DynInstance = io.kind {
            wgsl_push_field(output, vm, io, "dyninst_", false, &mut offset, &mut out);
        }
    }
    for io in &output.io {
        if let ShaderIoKind::RustInstance = io.kind {
            wgsl_push_field(output, vm, io, "rustinst_", false, &mut offset, &mut out);
        }
    }
    for io in &output.io {
        if let ShaderIoKind::Varying = io.kind {
            wgsl_push_field(output, vm, io, "var_", false, &mut offset, &mut out);
        }
    }
    out
}

fn wgsl_collect_chunk_formats(fields: &[WgslPackedField]) -> Vec<WgslPackedFormat> {
    let slots = fields
        .last()
        .map(|field| field.offset + field.slots)
        .unwrap_or(0);
    let mut out = vec![WgslPackedFormat::Float; wgsl_num_packed_vec4s(slots)];
    for field in fields {
        if field.attr_format == WgslPackedFormat::Float {
            continue;
        }
        for slot in field.offset..field.offset + field.slots {
            out[slot / 4] = field.attr_format;
        }
    }
    out
}

fn wgsl_to_float_scalar_expr(ty: &ScriptPodTy, expr: &str) -> String {
    match ty {
        ScriptPodTy::F32 | ScriptPodTy::F16 => expr.to_string(),
        ScriptPodTy::U32
        | ScriptPodTy::I32
        | ScriptPodTy::AtomicU32
        | ScriptPodTy::AtomicI32 => format!("f32({expr})"),
        ScriptPodTy::Bool => format!("select(0.0, 1.0, {expr})"),
        _ => format!("f32({expr})"),
    }
}

fn wgsl_convert_scalar_expr(source: WgslPackedFormat, target: &ScriptPodTy, expr: &str) -> String {
    match source {
        WgslPackedFormat::Float => match target {
            ScriptPodTy::F32 => expr.to_string(),
            ScriptPodTy::F16 => format!("f16({expr})"),
            ScriptPodTy::U32 | ScriptPodTy::AtomicU32 => format!("u32({expr})"),
            ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => format!("i32({expr})"),
            ScriptPodTy::Bool => format!("({expr} != 0.0)"),
            _ => format!("f32({expr})"),
        },
        WgslPackedFormat::UInt => match target {
            ScriptPodTy::U32 | ScriptPodTy::AtomicU32 => expr.to_string(),
            ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => format!("i32({expr})"),
            ScriptPodTy::F32 => format!("f32({expr})"),
            ScriptPodTy::F16 => format!("f16(f32({expr}))"),
            ScriptPodTy::Bool => format!("({expr} != 0u)"),
            _ => format!("f32({expr})"),
        },
        WgslPackedFormat::SInt => match target {
            ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => expr.to_string(),
            ScriptPodTy::U32 | ScriptPodTy::AtomicU32 => format!("u32({expr})"),
            ScriptPodTy::F32 => format!("f32({expr})"),
            ScriptPodTy::F16 => format!("f16(f32({expr}))"),
            ScriptPodTy::Bool => format!("({expr} != 0i)"),
            _ => format!("f32({expr})"),
        },
    }
}

fn wgsl_take_scalar_or_zero(
    scalars: &[String],
    scalar_index: &mut usize,
    source_format: WgslPackedFormat,
) -> String {
    let value = scalars
        .get(*scalar_index)
        .cloned()
        .unwrap_or_else(|| match source_format {
            WgslPackedFormat::Float => "0.0".to_string(),
            WgslPackedFormat::UInt => "0u".to_string(),
            WgslPackedFormat::SInt => "0i".to_string(),
        });
    *scalar_index += 1;
    value
}

fn wgsl_flatten_inline(
    output: &ShaderOutput,
    ty: &ScriptPodTypeInline,
    expr: &str,
    out: &mut Vec<String>,
) {
    match &ty.data.ty {
        ScriptPodTy::Struct { fields, .. } => {
            for field in fields {
                let field_name = output.backend.map_field_name(field.name);
                let field_expr = format!("({expr}).{field_name}");
                wgsl_flatten_inline(output, &field.ty, &field_expr, out);
            }
        }
        ScriptPodTy::Vec(vec_ty) => {
            let elem_ty = vec_ty.elem_ty();
            for comp in 0..vec_ty.dims() {
                let swizzle = wgsl_swizzle_component(comp);
                let comp_expr = format!("({expr}).{swizzle}");
                out.push(wgsl_to_float_scalar_expr(&elem_ty, &comp_expr));
            }
        }
        ScriptPodTy::Mat(mat_ty) => {
            let (cols, rows) = mat_ty.dims();
            for col in 0..cols {
                for row in 0..rows {
                    out.push(format!("({expr})[{col}][{row}]"));
                }
            }
        }
        scalar_ty => {
            out.push(wgsl_to_float_scalar_expr(scalar_ty, expr));
        }
    }
}

fn wgsl_flatten_exprs(
    output: &ShaderOutput,
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
    wgsl_flatten_inline(output, &inline, expr, out);
}

fn wgsl_reconstruct_inline(
    output: &ShaderOutput,
    vm: &ScriptVm,
    ty: &ScriptPodTypeInline,
    source_format: WgslPackedFormat,
    scalars: &[String],
    scalar_index: &mut usize,
) -> String {
    match &ty.data.ty {
        ScriptPodTy::Struct { fields, .. } => {
            let mut field_exprs = Vec::new();
            for field in fields {
                field_exprs.push(wgsl_reconstruct_inline(
                    output,
                    vm,
                    &field.ty,
                    source_format,
                    scalars,
                    scalar_index,
                ));
            }
            format!(
                "{}({})",
                wgsl_type_name_inline(output, vm, ty),
                field_exprs.join(", ")
            )
        }
        ScriptPodTy::Vec(vec_ty) => {
            let dims = vec_ty.dims();
            let mut comps = Vec::with_capacity(dims);
            let elem_ty = vec_ty.elem_ty();
            for _ in 0..dims {
                let scalar = wgsl_take_scalar_or_zero(scalars, scalar_index, source_format);
                comps.push(wgsl_convert_scalar_expr(source_format, &elem_ty, &scalar));
            }
            format!("{}({})", wgsl_type_name_inline(output, vm, ty), comps.join(", "))
        }
        ScriptPodTy::Mat(mat_ty) => {
            let mut comps = Vec::new();
            for _ in 0..mat_ty.dim() {
                let scalar = wgsl_take_scalar_or_zero(scalars, scalar_index, source_format);
                comps.push(wgsl_convert_scalar_expr(
                    source_format,
                    &ScriptPodTy::F32,
                    &scalar,
                ));
            }
            format!("{}({})", wgsl_type_name_inline(output, vm, ty), comps.join(", "))
        }
        scalar_ty => {
            let scalar = wgsl_take_scalar_or_zero(scalars, scalar_index, source_format);
            wgsl_convert_scalar_expr(source_format, scalar_ty, &scalar)
        }
    }
}

fn wgsl_unpack_expr_for_field(
    output: &ShaderOutput,
    vm: &ScriptVm,
    field: &WgslPackedField,
    prefix: &str,
) -> String {
    let scalars = (0..field.slots)
        .map(|slot| wgsl_packed_component(prefix, field.offset + slot))
        .collect::<Vec<_>>();
    let pod_ty = vm.bx.heap.pod_type_ref(field.ty);
    let inline = ScriptPodTypeInline {
        self_ref: field.ty,
        data: pod_ty.clone(),
    };
    let mut scalar_index = 0usize;
    wgsl_reconstruct_inline(
        output,
        vm,
        &inline,
        field.attr_format,
        &scalars,
        &mut scalar_index,
    )
}

fn build_draw_shader_wgsl(vm: &ScriptVm, output: &mut ShaderOutput) -> (String, u32, u32, u32) {
    let mut out = String::new();

    let geometry_fields = wgsl_collect_geometry_fields(output, vm);
    let instance_fields = wgsl_collect_instance_fields(output, vm);
    let varying_fields = wgsl_collect_varying_fields(output, vm);
    let varying_slots = varying_fields
        .last()
        .map(|field| field.offset + field.slots)
        .unwrap_or(0);

    let geometry_formats = wgsl_collect_chunk_formats(&geometry_fields);
    let instance_formats = wgsl_collect_chunk_formats(&instance_fields);

    let has_dyn_uniforms = output
        .io
        .iter()
        .any(|io| matches!(io.kind, ShaderIoKind::Uniform));
    let has_scope_uniforms = output
        .io
        .iter()
        .any(|io| matches!(io.kind, ShaderIoKind::ScopeUniform));

    let dyn_uniform_binding = 2u32;
    let uniform_bindings = output.get_uniform_buffer_bindings(&vm.bx.heap);
    let scope_uniform_binding = uniform_bindings.scope_uniform_buffer_index.map(|v| v as u32);

    let mut max_reserved_binding = dyn_uniform_binding;
    for io in &output.io {
        if let ShaderIoKind::UniformBuffer = io.kind {
            if let Some(idx) = io.buffer_index {
                max_reserved_binding = max_reserved_binding.max(idx as u32);
            }
        }
    }
    if let Some(scope_idx) = scope_uniform_binding {
        max_reserved_binding = max_reserved_binding.max(scope_idx);
    }
    let mut next_binding = max_reserved_binding + 1;
    let mut texture_binding_base: Option<u32> = None;
    let mut sampler_binding_base: Option<u32> = None;

    output.create_struct_defs(vm, &mut out);

    if has_dyn_uniforms {
        writeln!(out, "struct MpDynUniforms {{").ok();
        for io in &output.io {
            if !matches!(io.kind, ShaderIoKind::Uniform) {
                continue;
            }
            let io_name = output.backend.map_io_name(io.name);
            writeln!(
                out,
                "    uni_{}: {},",
                io_name,
                wgsl_type_name(output, vm, io.ty)
            )
            .ok();
        }
        writeln!(out, "}}").ok();
        writeln!(
            out,
            "@group(0) @binding({}) var<uniform> _mp_dyn_uniforms: MpDynUniforms;",
            dyn_uniform_binding
        )
        .ok();
    }

    if has_scope_uniforms {
        let scope_binding = scope_uniform_binding.unwrap_or(max_reserved_binding + 1);
        writeln!(out, "struct MpScopeUniforms {{").ok();
        for io in &output.io {
            if !matches!(io.kind, ShaderIoKind::ScopeUniform) {
                continue;
            }
            let io_name = output.backend.map_io_name(io.name);
            writeln!(
                out,
                "    su_{}: {},",
                io_name,
                wgsl_type_name(output, vm, io.ty)
            )
            .ok();
        }
        writeln!(out, "}}").ok();
        writeln!(
            out,
            "@group(0) @binding({}) var<uniform> _mp_scope_uniforms: MpScopeUniforms;",
            scope_binding
        )
        .ok();
    }

    writeln!(out, "var<private> vtx_pos: vec4f;").ok();

    for io in &output.io {
        let io_name = output.backend.map_io_name(io.name);
        match io.kind {
            ShaderIoKind::VertexBuffer => {
                writeln!(
                    out,
                    "var<private> vb_{}: {};",
                    io_name,
                    wgsl_type_name(output, vm, io.ty)
                )
                .ok();
            }
            ShaderIoKind::DynInstance => {
                writeln!(
                    out,
                    "var<private> dyninst_{}: {};",
                    io_name,
                    wgsl_type_name(output, vm, io.ty)
                )
                .ok();
            }
            ShaderIoKind::RustInstance => {
                writeln!(
                    out,
                    "var<private> rustinst_{}: {};",
                    io_name,
                    wgsl_type_name(output, vm, io.ty)
                )
                .ok();
            }
            ShaderIoKind::Varying => {
                writeln!(
                    out,
                    "var<private> var_{}: {};",
                    io_name,
                    wgsl_type_name(output, vm, io.ty)
                )
                .ok();
            }
            ShaderIoKind::Uniform => {
                writeln!(
                    out,
                    "var<private> uni_{}: {};",
                    io_name,
                    wgsl_type_name(output, vm, io.ty)
                )
                .ok();
            }
            ShaderIoKind::UniformBuffer => {
                let binding = io.buffer_index.unwrap_or(3) as u32;
                writeln!(
                    out,
                    "@group(0) @binding({}) var<uniform> unibuf_{}: {};",
                    binding,
                    io_name,
                    wgsl_type_name(output, vm, io.ty)
                )
                .ok();
            }
            ShaderIoKind::ScopeUniform => {
                writeln!(
                    out,
                    "var<private> su_{}: {};",
                    io_name,
                    wgsl_type_name(output, vm, io.ty)
                )
                .ok();
            }
            ShaderIoKind::FragmentOutput(index) => {
                writeln!(
                    out,
                    "var<private> frag_fb{}: {};",
                    index,
                    wgsl_type_name(output, vm, io.ty)
                )
                .ok();
            }
            ShaderIoKind::StorageBuffer(_) => {
                writeln!(
                    out,
                    "@group(0) @binding({}) var<storage, read_write> sb_{}: {};",
                    next_binding,
                    io_name,
                    wgsl_type_name(output, vm, io.ty)
                )
                .ok();
                next_binding += 1;
            }
            ShaderIoKind::Texture(tex_type) => {
                if texture_binding_base.is_none() {
                    texture_binding_base = Some(next_binding);
                }
                writeln!(
                    out,
                    "@group(0) @binding({}) var tex_{}: {};",
                    next_binding,
                    io_name,
                    wgsl_texture_type(tex_type)
                )
                .ok();
                next_binding += 1;
            }
            ShaderIoKind::Sampler(_) => {
                if sampler_binding_base.is_none() {
                    sampler_binding_base = Some(next_binding);
                }
                writeln!(
                    out,
                    "@group(0) @binding({}) var sampler_{}: sampler;",
                    next_binding, io_name
                )
                .ok();
                next_binding += 1;
            }
            ShaderIoKind::VertexPosition => {}
        }
    }

    for sampler_index in 0..output.samplers.len() {
        let sampler_name = format!("_s{}", sampler_index);
        if sampler_binding_base.is_none() {
            sampler_binding_base = Some(next_binding);
        }
        writeln!(
            out,
            "@group(0) @binding({}) var {}: sampler;",
            next_binding, sampler_name
        )
        .ok();
        next_binding += 1;
    }

    writeln!(out, "struct VertexMainIn {{").ok();
    let mut location = 0u32;
    for (idx, format) in geometry_formats.iter().enumerate() {
        writeln!(
            out,
            "    @location({}) packed_geometry_{}: {},",
            location,
            idx,
            wgsl_vertex_attr_vec_type(*format)
        )
        .ok();
        location += 1;
    }
    for (idx, format) in instance_formats.iter().enumerate() {
        writeln!(
            out,
            "    @location({}) packed_instance_{}: {},",
            location,
            idx,
            wgsl_vertex_attr_vec_type(*format)
        )
        .ok();
        location += 1;
    }
    writeln!(out, "}}").ok();

    writeln!(out, "struct VertexMainOut {{").ok();
    writeln!(out, "    @builtin(position) position: vec4f,").ok();
    for idx in 0..wgsl_num_packed_vec4s(varying_slots) {
        writeln!(out, "    @location({}) packed_varying_{}: vec4f,", idx, idx).ok();
    }
    writeln!(out, "}}").ok();

    let mut fragment_outputs = output
        .io
        .iter()
        .filter_map(|io| {
            if let ShaderIoKind::FragmentOutput(index) = io.kind {
                Some((index, wgsl_type_name(output, vm, io.ty)))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    fragment_outputs.sort_by_key(|(index, _)| *index);
    if !fragment_outputs.is_empty() {
        writeln!(out, "struct FragmentMainOut {{").ok();
        for (index, ty_name) in &fragment_outputs {
            writeln!(out, "    @location({}) fb{}: {},", index, index, ty_name).ok();
        }
        writeln!(out, "}}").ok();
    }

    writeln!(out).ok();
    output.create_functions(&mut out);

    let vertex_fn_name = output.backend.map_function_name("io_vertex");
    let fragment_fn_name = output.backend.map_function_name("io_fragment");
    let vertex_returns_vec4f = output
        .functions
        .iter()
        .find(|f| f.name == id!(vertex))
        .map(|f| f.ret == vm.bx.code.builtins.pod.pod_vec4f)
        .unwrap_or(false);

    writeln!(out, "@vertex").ok();
    writeln!(out, "fn vertex_main(in: VertexMainIn) -> VertexMainOut {{").ok();
    for field in &geometry_fields {
        let value_expr = wgsl_unpack_expr_for_field(output, vm, field, "in.packed_geometry_");
        writeln!(out, "    {} = {};", field.name, value_expr).ok();
    }
    for field in &instance_fields {
        let value_expr = wgsl_unpack_expr_for_field(output, vm, field, "in.packed_instance_");
        writeln!(out, "    {} = {};", field.name, value_expr).ok();
    }
    if has_dyn_uniforms {
        for io in &output.io {
            if !matches!(io.kind, ShaderIoKind::Uniform) {
                continue;
            }
            let io_name = output.backend.map_io_name(io.name);
            writeln!(
                out,
                "    uni_{} = _mp_dyn_uniforms.uni_{};",
                io_name, io_name
            )
            .ok();
        }
    }
    if has_scope_uniforms {
        for io in &output.io {
            if !matches!(io.kind, ShaderIoKind::ScopeUniform) {
                continue;
            }
            let io_name = output.backend.map_io_name(io.name);
            writeln!(
                out,
                "    su_{} = _mp_scope_uniforms.su_{};",
                io_name, io_name
            )
            .ok();
        }
    }
    writeln!(out, "    vtx_pos = vec4f(0.0, 0.0, 0.0, 1.0);").ok();
    if vertex_returns_vec4f {
        writeln!(out, "    vtx_pos = {}();", vertex_fn_name).ok();
    } else {
        writeln!(out, "    {}();", vertex_fn_name).ok();
    }
    writeln!(out, "    var out_data: VertexMainOut;").ok();
    writeln!(out, "    out_data.position = vtx_pos;").ok();
    for field in &varying_fields {
        let mut scalars = Vec::new();
        wgsl_flatten_exprs(output, vm, field.ty, &field.name, &mut scalars);
        for slot in 0..field.slots {
            let src = scalars
                .get(slot)
                .cloned()
                .unwrap_or_else(|| "0.0".to_string());
            let dst = wgsl_packed_component("out_data.packed_varying_", field.offset + slot);
            writeln!(out, "    {} = {};", dst, src).ok();
        }
    }
    writeln!(out, "    return out_data;").ok();
    writeln!(out, "}}").ok();

    if fragment_outputs.is_empty() {
        writeln!(out, "@fragment").ok();
        writeln!(out, "fn fragment_main(in: VertexMainOut) {{").ok();
        for field in &varying_fields {
            let value_expr = wgsl_unpack_expr_for_field(output, vm, field, "in.packed_varying_");
            writeln!(out, "    {} = {};", field.name, value_expr).ok();
        }
        if has_dyn_uniforms {
            for io in &output.io {
                if !matches!(io.kind, ShaderIoKind::Uniform) {
                    continue;
                }
                let io_name = output.backend.map_io_name(io.name);
                writeln!(
                    out,
                    "    uni_{} = _mp_dyn_uniforms.uni_{};",
                    io_name, io_name
                )
                .ok();
            }
        }
        if has_scope_uniforms {
            for io in &output.io {
                if !matches!(io.kind, ShaderIoKind::ScopeUniform) {
                    continue;
                }
                let io_name = output.backend.map_io_name(io.name);
                writeln!(
                    out,
                    "    su_{} = _mp_scope_uniforms.su_{};",
                    io_name, io_name
                )
                .ok();
            }
        }
        writeln!(out, "    {}();", fragment_fn_name).ok();
        writeln!(out, "}}").ok();
    } else {
        writeln!(out, "@fragment").ok();
        writeln!(
            out,
            "fn fragment_main(in: VertexMainOut) -> FragmentMainOut {{"
        )
        .ok();
        for field in &varying_fields {
            let value_expr = wgsl_unpack_expr_for_field(output, vm, field, "in.packed_varying_");
            writeln!(out, "    {} = {};", field.name, value_expr).ok();
        }
        if has_dyn_uniforms {
            for io in &output.io {
                if !matches!(io.kind, ShaderIoKind::Uniform) {
                    continue;
                }
                let io_name = output.backend.map_io_name(io.name);
                writeln!(
                    out,
                    "    uni_{} = _mp_dyn_uniforms.uni_{};",
                    io_name, io_name
                )
                .ok();
            }
        }
        if has_scope_uniforms {
            for io in &output.io {
                if !matches!(io.kind, ShaderIoKind::ScopeUniform) {
                    continue;
                }
                let io_name = output.backend.map_io_name(io.name);
                writeln!(
                    out,
                    "    su_{} = _mp_scope_uniforms.su_{};",
                    io_name, io_name
                )
                .ok();
            }
        }
        writeln!(out, "    {}();", fragment_fn_name).ok();
        writeln!(out, "    var out_data: FragmentMainOut;").ok();
        for (index, ty_name) in &fragment_outputs {
            let expr = match ty_name.as_str() {
                "f32" => "f32(0.0)".to_string(),
                "i32" => "i32(0)".to_string(),
                "u32" => "u32(0)".to_string(),
                "vec2f" => "vec2f(0.0)".to_string(),
                "vec3f" => "vec3f(0.0)".to_string(),
                "vec4f" => "frag_fb0".to_string(),
                "vec2i" => "vec2i(0)".to_string(),
                "vec3i" => "vec3i(0)".to_string(),
                "vec4i" => "vec4i(0)".to_string(),
                "vec2u" => "vec2u(0u)".to_string(),
                "vec3u" => "vec3u(0u)".to_string(),
                "vec4u" => "vec4u(0u)".to_string(),
                _ => format!("{}(0.0)", ty_name),
            };
            writeln!(out, "    out_data.fb{} = {};", index, expr).ok();
        }
        writeln!(out, "    return out_data;").ok();
        writeln!(out, "}}").ok();
    }

    (
        out,
        dyn_uniform_binding,
        texture_binding_base.unwrap_or(0),
        sampler_binding_base.unwrap_or(0),
    )
}

fn compile_wgsl_to_spirv(wgsl: &str) -> Result<(Option<Vec<u32>>, Option<Vec<u32>>), String> {
    use naga::{back::spv, valid};

    fn extract_error_line(details: &str) -> Option<usize> {
        let marker = "wgsl:";
        let start = details.find(marker)? + marker.len();
        let rest = &details[start..];
        let end = rest.find(':')?;
        rest[..end].trim().parse::<usize>().ok()
    }

    fn wgsl_context(wgsl: &str, line: usize, radius: usize) -> String {
        let start = line.saturating_sub(radius).max(1);
        let end = line.saturating_add(radius);
        let mut out = String::new();
        for (i, src_line) in wgsl.lines().enumerate() {
            let ln = i + 1;
            if ln >= start && ln <= end {
                let _ = writeln!(out, "{ln:4} | {src_line}");
            }
        }
        out
    }

    let module = naga::front::wgsl::parse_str(wgsl).map_err(|e| {
        let details = e.emit_to_string(wgsl);
        let context = extract_error_line(&details)
            .map(|line| {
                format!(
                    "\nWGSL context around line {line}:\n{}",
                    wgsl_context(wgsl, line, 4)
                )
            })
            .unwrap_or_default();
        format!("WGSL parse error: {e}\n{details}{context}")
    })?;

    let mut validator =
        valid::Validator::new(valid::ValidationFlags::all(), valid::Capabilities::empty());
    let module_info = validator
        .validate(&module)
        .map_err(|e| format!("WGSL validation error: {e}"))?;

    let options = spv::Options {
        lang_version: (1, 3),
        flags: spv::WriterFlags::empty(),
        fake_missing_bindings: true,
        binding_map: spv::BindingMap::default(),
        capabilities: None,
        bounds_check_policies: naga::proc::BoundsCheckPolicies::default(),
        zero_initialize_workgroup_memory: spv::ZeroInitializeWorkgroupMemoryMode::None,
        force_loop_bounding: false,
        use_storage_input_output_16: false,
        debug_info: None,
    };

    let has_vertex = module
        .entry_points
        .iter()
        .any(|ep| ep.stage == naga::ShaderStage::Vertex && ep.name == "vertex_main");
    let has_fragment = module
        .entry_points
        .iter()
        .any(|ep| ep.stage == naga::ShaderStage::Fragment && ep.name == "fragment_main");

    if !has_vertex && !has_fragment {
        return Err("WGSL module has no entry points".to_string());
    }

    let vertex_spirv = if has_vertex {
        let pipeline = spv::PipelineOptions {
            shader_stage: naga::ShaderStage::Vertex,
            entry_point: "vertex_main".to_string(),
        };
        Some(
            spv::write_vec(&module, &module_info, &options, Some(&pipeline))
                .map_err(|e| format!("SPIR-V write failed for vertex_main: {e}"))?,
        )
    } else {
        None
    };

    let fragment_spirv = if has_fragment {
        let pipeline = spv::PipelineOptions {
            shader_stage: naga::ShaderStage::Fragment,
            entry_point: "fragment_main".to_string(),
        };
        Some(
            spv::write_vec(&module, &module_info, &options, Some(&pipeline))
                .map_err(|e| format!("SPIR-V write failed for fragment_main: {e}"))?,
        )
    } else {
        None
    };

    Ok((vertex_spirv, fragment_spirv))
}

pub(crate) fn compile_raw_wgsl_to_spirv(
    wgsl: &str,
) -> Result<(Option<Vec<u32>>, Option<Vec<u32>>), String> {
    compile_wgsl_to_spirv(wgsl)
}

pub(crate) fn compile_draw_shader_wgsl_to_spirv(
    vm: &mut ScriptVm,
    io_self: ScriptObject,
    layout_source: &ShaderOutput,
) -> Result<CxVulkanShaderBinary, String> {
    let mut output = ShaderOutput::default();
    output.backend = ShaderBackend::Wgsl;
    output.pre_collect_rust_instance_io(vm, io_self);
    output.pre_collect_shader_io(vm, io_self);

    if let Some(fnobj) = vm
        .bx
        .heap
        .object_method(io_self, id!(vertex).into(), vm.thread().trap.pass())
        .as_object()
    {
        output.mode = ShaderMode::Vertex;
        ShaderFnCompiler::compile_shader_def(
            vm,
            &mut output,
            NoTrap,
            id!(vertex),
            fnobj,
            ShaderType::IoSelf(io_self),
            vec![],
        );
    }

    if let Some(fnobj) = vm
        .bx
        .heap
        .object_method(io_self, id!(fragment).into(), vm.thread().trap.pass())
        .as_object()
    {
        output.mode = ShaderMode::Fragment;
        ShaderFnCompiler::compile_shader_def(
            vm,
            &mut output,
            NoTrap,
            id!(fragment),
            fnobj,
            ShaderType::IoSelf(io_self),
            vec![],
        );
    }

    if output.has_errors {
        return Err("WGSL lowering reported shader errors".to_string());
    }

    // Keep Vulkan shader IO layout in lockstep with the draw mapping produced by the
    // primary compiler path (dyn/rust instance packing, uniform buffer bindings, etc.).
    output.io = layout_source
        .io
        .iter()
        .map(|io| crate::makepad_script::shader_output::ShaderIo {
            kind: io.kind.clone(),
            name: io.name,
            ty: io.ty,
            buffer_index: io.buffer_index,
        })
        .collect();
    output.samplers = layout_source.samplers.clone();

    let layout_dyn = layout_source
        .io
        .iter()
        .filter(|io| matches!(io.kind, ShaderIoKind::DynInstance))
        .count();
    let layout_rust = layout_source
        .io
        .iter()
        .filter(|io| matches!(io.kind, ShaderIoKind::RustInstance))
        .count();
    let output_dyn = output
        .io
        .iter()
        .filter(|io| matches!(io.kind, ShaderIoKind::DynInstance))
        .count();
    let output_rust = output
        .io
        .iter()
        .filter(|io| matches!(io.kind, ShaderIoKind::RustInstance))
        .count();
    static IO_SYNC_LOG_ONCE: std::sync::Once = std::sync::Once::new();
    IO_SYNC_LOG_ONCE.call_once(|| {
        crate::log!(
            "Vulkan WGSL IO sync stats: layout_dyn={}, layout_rust={}, output_dyn={}, output_rust={}, output_total={}",
            layout_dyn,
            layout_rust,
            output_dyn,
            output_rust,
            output.io.len()
        );
    });

    output.assign_uniform_buffer_indices(&vm.bx.heap, 3);

    let geometry_slots = wgsl_collect_geometry_fields(&output, vm)
        .last()
        .map(|field| field.offset + field.slots)
        .unwrap_or(0);
    let instance_slots = wgsl_collect_instance_fields(&output, vm)
        .last()
        .map(|field| field.offset + field.slots)
        .unwrap_or(0);

    let (wgsl, dyn_uniform_binding, texture_binding_base, sampler_binding_base) =
        build_draw_shader_wgsl(vm, &mut output);

    if std::env::var_os("MAKEPAD_DUMP_VULKAN_WGSL").is_some() {
        crate::log!("---- Vulkan WGSL ----\n{}", wgsl);
    }
    static WGSL_SAMPLE_ONCE: std::sync::Once = std::sync::Once::new();
    WGSL_SAMPLE_ONCE.call_once(|| {
        crate::log!("---- Vulkan WGSL sample (first shader) ----\n{}", wgsl);
    });
    static WGSL_DYN_SAMPLE_ONCE: std::sync::Once = std::sync::Once::new();
    if layout_dyn > 0 {
        WGSL_DYN_SAMPLE_ONCE.call_once(|| {
            crate::log!("---- Vulkan WGSL sample (dyn shader) ----\n{}", wgsl);
        });
    }

    let (vertex_spirv, fragment_spirv) = compile_wgsl_to_spirv(&wgsl)
        .map_err(|err| format!("{err}\nSet MAKEPAD_DUMP_VULKAN_WGSL=1 to dump generated WGSL."))?;

    Ok(CxVulkanShaderBinary {
        vertex_spirv,
        fragment_spirv,
        dyn_uniform_binding,
        texture_binding_base,
        sampler_binding_base,
        geometry_slots,
        instance_slots,
    })
}
