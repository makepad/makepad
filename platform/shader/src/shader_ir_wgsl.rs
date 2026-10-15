//! WGSL from the shader IR: the module WebGPU runs, with the draw entry
//! points of shader_ir_draw.rs and the same bindings as the Vulkan SPIR-V.

use crate::shader_ir::*;
use crate::shader_ir_draw::{build_draw_entries, IrDrawInfo};
use crate::shader_ir_text::*;
use crate::shader_output::ShaderOutput;

use crate::shader_types::ShaderTypes;
use std::fmt::Write;

/// The binding of the instance-record storage buffer: after every other
/// binding (the XR depth texture is the last of those).
pub fn wgsl_instance_binding(xr_depth_binding: u32) -> u32 {
    xr_depth_binding + 1
}

#[derive(Clone)]
pub struct WgslDrawShaderSource {
    pub wgsl: String,
    pub dyn_uniform_binding: u32,
    pub texture_binding_base: u32,
    pub sampler_binding_base: u32,
    pub xr_depth_binding: u32,
    pub geometry_slots: usize,
    pub instance_slots: usize,
}

pub fn draw_wgsl(output: &ShaderOutput, types: &impl ShaderTypes, xr: bool) -> Result<(String, IrDrawInfo), String> {
    let mut ir = output.ir.clone();
    let info = build_draw_entries(output, &mut ir, types, xr);
    let (Some(v), Some(f)) = (ir.function_by_name("vertex_main"), ir.function_by_name("fragment_main")) else {
        return Err("draw shader has no entry points".to_string());
    };
    Ok((ir_to_wgsl(&ir, &[v, f], xr), info))
}

fn builtin_name(b: IrBuiltinIo) -> &'static str {
    match b {
        IrBuiltinIo::Position => "position",
        IrBuiltinIo::VertexIndex => "vertex_index",
        IrBuiltinIo::InstanceIndex => "instance_index",
        IrBuiltinIo::ViewIndex => "view_index",
        IrBuiltinIo::FrontFacing => "front_facing",
        IrBuiltinIo::FragDepth => "frag_depth",
    }
}

/// The WGSL module of `entries` and everything they reach.
pub fn ir_to_wgsl(m: &IrModule, entries: &[FuncId], xr: bool) -> String {
    let mut cx = TextCx::new(m, Dialect::Wgsl, xr);
    let mut funcs = Vec::new();
    for e in entries {
        for f in m.reachable(*e) {
            if !funcs.contains(&f) {
                funcs.push(f);
            }
        }
    }
    // Function bodies first: they tell which helpers the module needs.
    let mut bodies = String::new();
    let mut i = funcs.len();
    while i > 0 {
        i -= 1;
        let fi = funcs[i];
        if m.functions[fi as usize].entry.is_some() {
            continue;
        }
        cx.function(fi, &mut bodies);
    }
    let mut entry_text = String::new();
    for e in entries {
        let f = &m.functions[*e as usize];
        let Some(entry) = &f.entry else { continue };
        let (stage, base) = match entry.stage {
            IrStage::Vertex => ("vertex", "VertexMain"),
            IrStage::Fragment => ("fragment", "FragmentMain"),
            IrStage::Compute => ("compute", "ComputeMain"),
        };
        writeln!(entry_text, "struct {}In {{", base).ok();
        for io in &entry.inputs {
            writeln!(entry_text, "    {} {}: {},", io_attr(io.binding), io.name, cx.ty_name(io.ty)).ok();
        }
        entry_text.push_str("}\n");
        let has_out = !entry.outputs.is_empty();
        if has_out {
            writeln!(entry_text, "struct {}Out {{", base).ok();
            for io in &entry.outputs {
                writeln!(entry_text, "    {} {}: {},", io_attr(io.binding), io.name, cx.ty_name(io.ty)).ok();
            }
            entry_text.push_str("}\n");
        }
        writeln!(entry_text, "@{}", stage).ok();
        if has_out {
            writeln!(entry_text, "fn {}(in: {}In) -> {}Out {{", f.name, base, base).ok();
            writeln!(entry_text, "    var out_data: {}Out;", base).ok();
        } else {
            writeln!(entry_text, "fn {}(in: {}In) {{", f.name, base).ok();
        }
        cx.stmts(f, &f.body, 1, &mut entry_text);
        entry_text.push_str("}\n");
    }

    let mut decls = String::new();
    for (s, st) in m.structs.iter().enumerate() {
        writeln!(decls, "struct {} {{", cx.struct_name(s as u32)).ok();
        let sty = m.types.iter().position(|t| *t == IrTy::Struct(s as u32)).unwrap_or(0) as TyId;
        for (k, f) in st.fields.iter().enumerate() {
            writeln!(decls, "    {}: {},", cx.field_name(sty, k as u32), cx.ty_name(f.ty)).ok();
        }
        decls.push_str("}\n");
    }
    for (g, glob) in m.globals.iter().enumerate() {
        let name = cx.global_name(g as GlobalId);
        let ty = cx.ty_name(glob.ty);
        match glob.kind {
            IrGlobalKind::Io(IrIo::UniformBuffer) | IrGlobalKind::DynUniformBlock | IrGlobalKind::ScopeUniformBlock => {
                if let Some(b) = glob.binding {
                    writeln!(decls, "@group(0) @binding({}) var<uniform> {}: {};", b, name, ty).ok();
                }
            }
            IrGlobalKind::InstanceBuffer => {
                if let Some(b) = glob.binding {
                    writeln!(decls, "@group(0) @binding({}) var<storage, read> {}: {};", b, name, ty).ok();
                }
            }
            IrGlobalKind::Io(IrIo::Texture(_)) | IrGlobalKind::XrDepth | IrGlobalKind::Sampler(_) => {
                if let Some(b) = glob.binding {
                    writeln!(decls, "@group(0) @binding({}) var {}: {};", b, name, ty).ok();
                }
            }
            _ => {
                writeln!(decls, "var<private> {}: {};", name, ty).ok();
            }
        }
    }
    if cx.needs_pass_helpers {
        for which in [PassMatrix::CameraProjection, PassMatrix::CameraView, PassMatrix::DepthProjection, PassMatrix::DepthView, PassMatrix::CameraInv] {
            let n = pass_matrix_name(which);
            writeln!(
                decls,
                "fn mp_draw_pass_{0}() -> mat4x4f {{\n    if (VIEW_ID != 0) {{\n        return unibuf_draw_pass.{0}_r;\n    }}\n    return unibuf_draw_pass.{0};\n}}",
                n
            )
            .ok();
        }
    }
    if cx.needs_inverse {
        decls.push_str(WGSL_INVERSE4);
    }
    let mut out = String::new();
    if cx.uses_f16.get() {
        out.push_str("enable f16;\n");
    }
    out.push_str(&decls);
    out.push_str(&bodies);
    out.push_str(&entry_text);
    out
}

fn io_attr(b: IrIoBinding) -> String {
    match b {
        IrIoBinding::Builtin(bi) => format!("@builtin({})", builtin_name(bi)),
        IrIoBinding::Location(l) => format!("@location({})", l),
    }
}

/// WGSL has no matrix inverse: the cofactor expansion of a mat4x4f.
const WGSL_INVERSE4: &str = "fn _mp_inverse4(m: mat4x4f) -> mat4x4f {
    let a00 = m[0][0]; let a01 = m[0][1]; let a02 = m[0][2]; let a03 = m[0][3];
    let a10 = m[1][0]; let a11 = m[1][1]; let a12 = m[1][2]; let a13 = m[1][3];
    let a20 = m[2][0]; let a21 = m[2][1]; let a22 = m[2][2]; let a23 = m[2][3];
    let a30 = m[3][0]; let a31 = m[3][1]; let a32 = m[3][2]; let a33 = m[3][3];
    let b00 = a00 * a11 - a01 * a10; let b01 = a00 * a12 - a02 * a10;
    let b02 = a00 * a13 - a03 * a10; let b03 = a01 * a12 - a02 * a11;
    let b04 = a01 * a13 - a03 * a11; let b05 = a02 * a13 - a03 * a12;
    let b06 = a20 * a31 - a21 * a30; let b07 = a20 * a32 - a22 * a30;
    let b08 = a20 * a33 - a23 * a30; let b09 = a21 * a32 - a22 * a31;
    let b10 = a21 * a33 - a23 * a31; let b11 = a22 * a33 - a23 * a32;
    let det = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;
    let inv = 1.0 / det;
    return mat4x4f(
        vec4f(a11 * b11 - a12 * b10 + a13 * b09, a02 * b10 - a01 * b11 - a03 * b09, a31 * b05 - a32 * b04 + a33 * b03, a22 * b04 - a21 * b05 - a23 * b03) * inv,
        vec4f(a12 * b08 - a10 * b11 - a13 * b07, a00 * b11 - a02 * b08 + a03 * b07, a32 * b02 - a30 * b05 - a33 * b01, a20 * b05 - a22 * b02 + a23 * b01) * inv,
        vec4f(a10 * b10 - a11 * b08 + a13 * b06, a01 * b08 - a00 * b10 - a03 * b06, a30 * b04 - a31 * b02 + a33 * b00, a21 * b02 - a20 * b04 - a23 * b00) * inv,
        vec4f(a11 * b07 - a10 * b09 - a12 * b06, a00 * b09 - a01 * b07 + a02 * b06, a31 * b01 - a30 * b03 - a32 * b00, a20 * b03 - a21 * b01 + a22 * b00) * inv);
}
";

impl WgslDrawShaderSource {
    pub fn wgsl(&self) -> &str { &self.wgsl }
}
