//! Metal Shading Language from the shader IR: one library holding
//! `vertex_main` and `fragment_main`, and the pick variant of the same
//! shader (`ShaderOutput::pick`).
//!
//! Bindings: the geometry at `buffer(0)`, the instance records at
//! `buffer(1)`, the dyn uniforms at `buffer(2)`, uniform buffers at their
//! `buffer_index`, the scope uniforms after them; textures at `texture(i)`
//! in io order (the pick twins after them, the pick ids at `buffer(30)`);
//! samplers are `constexpr`.
//!
//! MSL has no mutable program-scope variables, so the per-invocation state
//! the IR keeps in globals (records, varyings, outputs, resources) is the
//! `Mp` struct each entry point fills and every function takes as
//! `thread Mp &_mp`.

use crate::shader_ir::*;
use crate::shader_ir_draw::{build_metal_entries, MetalDrawInfo};
use crate::shader_ir_text::*;
use crate::shader_output::*;
use std::fmt::Write;

/// The library source of a draw shader compiled to IR; `pick` prints its
/// pick variant.
pub fn metal_draw_source(output: &ShaderOutput, types: &impl crate::shader_types::ShaderTypes, pick: bool) -> String {
    metal_draw(output, types, pick).0
}

pub fn metal_draw(output: &ShaderOutput, types: &impl crate::shader_types::ShaderTypes, pick: bool) -> (String, MetalDrawInfo) {
    let mut ir = output.ir.clone();
    let info = build_metal_entries(output, &mut ir, types);
    let mut io_tys = Vec::new();
    for io in &output.io {
        io_tys.push(match io.kind {
            ShaderIoKind::Texture(_) | ShaderIoKind::Sampler(_) => TY_VOID,
            _ => ir.pod_ty(types, io.ty),
        });
    }
    let vertex_main = ir.function_by_name("vertex_main").unwrap();
    let fragment_main = ir.function_by_name("fragment_main").unwrap();
    let funcs = callees_first(&ir, &[vertex_main, fragment_main]);
    let used = ir.globals_used(&funcs);
    let mut cx = TextCx::new(&ir, Dialect::Metal, false);

    // The pick twins: the shader's 2D textures among its first 32.
    let mut twins = Vec::new();
    if pick {
        let mut k = 0u32;
        for io in &output.io {
            if let ShaderIoKind::Texture(tt) = io.kind {
                if k < 32 && tt == TextureType::Texture2d {
                    if let Some(g) = ir.find_global(IrGlobalKind::Io(IrIo::Texture(tt)), io.name) {
                        cx.pick_twins.push((g, k));
                    }
                    twins.push((k, io.name));
                }
                k += 1;
            }
        }
    }

    let mut bodies = String::new();
    for f in &funcs {
        if *f == vertex_main || *f == fragment_main {
            continue;
        }
        cx.function(*f, &mut bodies);
    }

    let mut out = String::new();
    out.push_str("#include <metal_stdlib>\nusing namespace metal;\n");
    let mut done = vec![false; ir.structs.len()];
    for s in 0..ir.structs.len() {
        struct_def(&cx, s as u32, &mut done, &mut out);
    }
    let mut scope_buffer = None;
    for io in &output.io {
        if let (ShaderIoKind::UniformBuffer, Some(b)) = (&io.kind, io.buffer_index) {
            scope_buffer = Some(scope_buffer.map_or(b + 1, |m: usize| m.max(b + 1)));
        }
    }
    let scope_buffer = scope_buffer.unwrap_or(3);
    let mut has = [false, false];
    for (kind, block, prefix) in [(0, "MpUniforms", "uni_"), (1, "MpScopeUniforms", "su_")] {
        let mut first = true;
        for (i, io) in output.io.iter().enumerate() {
            let wanted = match io.kind {
                ShaderIoKind::Uniform => kind == 0,
                ShaderIoKind::ScopeUniform => kind == 1,
                _ => false,
            };
            if !wanted {
                continue;
            }
            if first {
                writeln!(out, "struct {} {{", block).ok();
                first = false;
            }
            writeln!(out, "    {};", cx.decl(io_tys[i], &format!("{}{}", prefix, io.name))).ok();
        }
        if !first {
            out.push_str("};\n");
            has[kind] = true;
        }
    }
    sampler_decls(output, &mut out);

    // The invocation's state.
    out.push_str("struct Mp {\n    uint _mp_iter;\n");
    if has[0] {
        out.push_str("    constant MpUniforms *u;\n");
    }
    if has[1] {
        out.push_str("    constant MpScopeUniforms *su;\n");
    }
    for (i, io) in output.io.iter().enumerate() {
        match io.kind {
            ShaderIoKind::UniformBuffer => {
                writeln!(out, "    constant {} *unibuf_{};", cx.ty_name(io_tys[i]), io.name).ok();
            }
            ShaderIoKind::Texture(tt) => {
                writeln!(out, "    {} tex_{};", metal_texture_type(tt), io.name).ok();
            }
            _ => {}
        }
    }
    for (g, glob) in ir.globals.iter().enumerate() {
        if !used.get(g).copied().unwrap_or(false) {
            continue;
        }
        let name = cx.plain_global_name(g as GlobalId);
        match glob.kind {
            IrGlobalKind::Io(IrIo::Geometry | IrIo::DynInstance | IrIo::RustInstance | IrIo::Varying | IrIo::FragmentOutput(_))
            | IrGlobalKind::VertexPosition
            | IrGlobalKind::InstanceIndex => {
                writeln!(out, "    {};", cx.decl(glob.ty, &name)).ok();
            }
            IrGlobalKind::InstanceBuffer => {
                writeln!(out, "    constant uint *{};", word_buffer_name(glob.name)).ok();
            }
            _ => {}
        }
    }
    for (_, name) in &twins {
        writeln!(out, "    texture2d<float> tex_{}__pick;", name).ok();
    }
    if pick {
        out.push_str("    uint _pick_mask;\n    uint _pick_id;\n    float _pick_w;\n");
    }
    out.push_str("};\n");
    out.push_str(METAL_HELPERS);
    if pick {
        out.push_str(METAL_PICK_HELPERS);
    }
    out.push_str(&bodies);

    // Entry points: the resources both take, and how they fill `_mp`.
    let mut resources = String::new();
    if has[0] {
        resources.push_str(",\n    constant MpUniforms *u [[buffer(2)]]");
    }
    for (i, io) in output.io.iter().enumerate() {
        if let (ShaderIoKind::UniformBuffer, Some(b)) = (&io.kind, io.buffer_index) {
            write!(resources, ",\n    constant {} *unibuf_{} [[buffer({})]]", cx.ty_name(io_tys[i]), io.name, b).ok();
        }
    }
    if has[1] {
        write!(resources, ",\n    constant MpScopeUniforms *su [[buffer({})]]", scope_buffer).ok();
    }
    let mut t = 0;
    for io in &output.io {
        if let ShaderIoKind::Texture(tt) = io.kind {
            write!(resources, ",\n    {} tex_{} [[texture({})]]", metal_texture_type(tt), io.name, t).ok();
            t += 1;
        }
    }
    let mut fill = String::from("    Mp _mp;\n    _mp._mp_iter = 0u;\n");
    if has[0] {
        fill.push_str("    _mp.u = u;\n");
    }
    if has[1] {
        fill.push_str("    _mp.su = su;\n");
    }
    for io in &output.io {
        match io.kind {
            ShaderIoKind::UniformBuffer => {
                writeln!(fill, "    _mp.unibuf_{0} = unibuf_{0};", io.name).ok();
            }
            ShaderIoKind::Texture(_) => {
                writeln!(fill, "    _mp.tex_{0} = tex_{0};", io.name).ok();
            }
            _ => {}
        }
    }
    for (g, glob) in ir.globals.iter().enumerate() {
        if !used.get(g).copied().unwrap_or(false) {
            continue;
        }
        match glob.kind {
            IrGlobalKind::InstanceBuffer => {
                writeln!(fill, "    _mp.{0} = {0};", word_buffer_name(glob.name)).ok();
            }
            // The invocation's state starts zeroed: a varying a vertex
            // function writes on some paths only passes 0 on the others.
            IrGlobalKind::Io(IrIo::Geometry | IrIo::DynInstance | IrIo::RustInstance | IrIo::Varying | IrIo::FragmentOutput(_))
            | IrGlobalKind::VertexPosition
            | IrGlobalKind::InstanceIndex => {
                writeln!(fill, "    _mp.{} = {{}};", cx.plain_global_name(g as GlobalId)).ok();
            }
            _ => {}
        }
    }

    let vf = &ir.functions[vertex_main as usize];
    let ve = vf.entry.as_ref().unwrap();
    out.push_str("struct VertexOut {\n");
    for io in &ve.outputs {
        match io.binding {
            IrIoBinding::Builtin(IrBuiltinIo::Position) => writeln!(out, "    float4 {} [[position]];", io.name).ok(),
            // The instance id: an integer varying is flat.
            _ if io.ty == TY_U32 => writeln!(out, "    {} [[flat]];", cx.decl(io.ty, &io.name)).ok(),
            _ => writeln!(out, "    {};", cx.decl(io.ty, &io.name)).ok(),
        };
    }
    out.push_str("};\n");
    out.push_str("vertex VertexOut vertex_main(\n    constant uint *_mp_geom [[buffer(0)]],\n    constant uint *_mp_inst [[buffer(1)]]");
    out.push_str(&resources);
    out.push_str(",\n    uint vertex_index [[vertex_id]],\n    uint instance_index [[instance_id]]\n) {\n");
    out.push_str(&fill);
    if pick {
        out.push_str("    _mp._pick_mask = 0u;\n    _mp._pick_id = 0u;\n    _mp._pick_w = 0.0;\n");
    }
    out.push_str("    VertexOut out_data = {};\n");
    cx.stmts(vf, &vf.body, 1, &mut out);
    out.push_str("}\n");

    let ff = &ir.functions[fragment_main as usize];
    let fe = ff.entry.as_ref().unwrap();
    out.push_str("struct FragmentOut {\n");
    for io in &fe.outputs {
        if let IrIoBinding::Location(l) = io.binding {
            writeln!(out, "    {} [[color({})]];", cx.decl(io.ty, &io.name), l).ok();
        }
    }
    out.push_str("};\n");
    let pick_out = pick && !fe.outputs.is_empty();
    if pick_out {
        out.push_str("struct PickOut {\n");
        for io in &fe.outputs {
            if let IrIoBinding::Location(l) = io.binding {
                writeln!(out, "    float4 {} [[color({})]];", io.name, l).ok();
            }
        }
        out.push_str("};\n");
    }
    let ret_struct = if pick_out { "PickOut" } else { "FragmentOut" };
    write!(out, "fragment {} fragment_main(\n    VertexOut input [[stage_in]],\n    constant uint *_mp_geom [[buffer(0)]],\n    constant uint *_mp_inst [[buffer(1)]]", ret_struct).ok();
    out.push_str(&resources);
    let ntex = output.io.iter().filter(|io| matches!(io.kind, ShaderIoKind::Texture(_))).count();
    if pick {
        for (k, name) in &twins {
            write!(out, ",\n    texture2d<float> tex_{}__pick [[texture({})]]", name, ntex + *k as usize).ok();
        }
        out.push_str(",\n    constant uint4 *_pk [[buffer(30)]]");
    }
    out.push_str("\n) {\n");
    out.push_str(&fill);
    if pick {
        out.push_str("    _mp._pick_mask = _pk->y;\n    _mp._pick_id = 0u;\n    _mp._pick_w = 0.0;\n");
        for (_, name) in &twins {
            writeln!(out, "    _mp.tex_{0}__pick = tex_{0}__pick;", name).ok();
        }
    }
    out.push_str("    FragmentOut out_data = {};\n");
    if pick_out {
        // The body's `return out_data;` returns the colours; the pick
        // variant turns them into the id of what is drawn there.
        let mut body = String::new();
        cx.stmts(ff, &ff.body, 1, &mut body);
        out.push_str(&body.replace("return out_data;", "return _mp_pick_out(_mp, out_data, _pk->x);"));
    } else {
        cx.stmts(ff, &ff.body, 1, &mut out);
    }
    out.push_str("}\n");
    if pick_out {
        // Defined after use is not allowed: move it before the entry.
        let at = out.find("fragment PickOut fragment_main(").unwrap();
        let mut f = String::new();
        writeln!(f, "PickOut _mp_pick_out(thread Mp &_mp, FragmentOut c, uint draw_id) {{").ok();
        // The coverage: the first colour target's (a data target of one
        // channel covers).
        let first = fe.outputs.iter().find(|io| matches!(io.binding, IrIoBinding::Location(_)));
        let cover = match first {
            Some(io) if *ir.get(io.ty) == IrTy::Vec(4, IrScalar::F32) => format!("_mp_pick_cover(c.{})", io.name),
            _ => "1.0".to_string(),
        };
        writeln!(f, "    uint pid = _mp._pick_w > 0.0 ? _mp._pick_id : draw_id;").ok();
        writeln!(f, "    if ({} < 0.1 || pid == 0u) discard_fragment();", cover).ok();
        writeln!(f, "    float4 pc = float4(float(pid & 255u), float((pid >> 8) & 255u), float((pid >> 16) & 255u), 255.0) / 255.0;").ok();
        writeln!(f, "    PickOut o;").ok();
        for io in &fe.outputs {
            if let IrIoBinding::Location(_) = io.binding {
                writeln!(f, "    o.{} = pc;", io.name).ok();
            }
        }
        writeln!(f, "    return o;\n}}").ok();
        out.insert_str(at, &f);
    }
    (out, info)
}

fn struct_def(cx: &TextCx, s: u32, done: &mut Vec<bool>, out: &mut String) {
    if done[s as usize] {
        return;
    }
    done[s as usize] = true;
    let st = &cx.m.structs[s as usize];
    if st.synth.is_some() {
        return;
    }
    for f in &st.fields {
        let mut t = f.ty;
        while let IrTy::Array(e, _) = cx.m.get(t) {
            t = *e;
        }
        if let IrTy::Struct(inner) = cx.m.get(t) {
            struct_def(cx, *inner, done, out);
        }
    }
    let sty = cx.m.types.iter().position(|t| *t == IrTy::Struct(s)).unwrap_or(0) as TyId;
    writeln!(out, "struct {} {{", cx.struct_name(s)).ok();
    for (k, f) in st.fields.iter().enumerate() {
        writeln!(out, "    {};", cx.decl(f.ty, &cx.field_name(sty, k as u32))).ok();
    }
    out.push_str("};\n");
}

fn sampler_decls(output: &ShaderOutput, out: &mut String) {
    use crate::shader_output::{SamplerAddress, SamplerCoord, SamplerFilter};
    for (idx, sampler) in output.samplers.iter().enumerate() {
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
            if sampler.compare { ", compare_func::less_equal" } else { "" }
        )
        .ok();
    }
}

const METAL_HELPERS: &str = "inline float2 _mp_unpack2f16(float x) { half2 h = as_type<half2>(as_type<uint>(x)); return float2(h.x, h.y); }
inline float4 _mp_unpack4u8(float x) { uint u = as_type<uint>(x); return float4(float(u & 0xffu), float((u >> 8) & 0xffu), float((u >> 16) & 0xffu), float((u >> 24) & 0xffu)) * (1.0 / 255.0); }
inline float4x4 _mp_inverse(float4x4 m) {
    float a00 = m[0][0]; float a01 = m[0][1]; float a02 = m[0][2]; float a03 = m[0][3];
    float a10 = m[1][0]; float a11 = m[1][1]; float a12 = m[1][2]; float a13 = m[1][3];
    float a20 = m[2][0]; float a21 = m[2][1]; float a22 = m[2][2]; float a23 = m[2][3];
    float a30 = m[3][0]; float a31 = m[3][1]; float a32 = m[3][2]; float a33 = m[3][3];
    float b00 = a00 * a11 - a01 * a10; float b01 = a00 * a12 - a02 * a10;
    float b02 = a00 * a13 - a03 * a10; float b03 = a01 * a12 - a02 * a11;
    float b04 = a01 * a13 - a03 * a11; float b05 = a02 * a13 - a03 * a12;
    float b06 = a20 * a31 - a21 * a30; float b07 = a20 * a32 - a22 * a30;
    float b08 = a20 * a33 - a23 * a30; float b09 = a21 * a32 - a22 * a31;
    float b10 = a21 * a33 - a23 * a31; float b11 = a22 * a33 - a23 * a32;
    float det = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;
    if (det == 0.0) {
        return float4x4(float4(1.0, 0.0, 0.0, 0.0), float4(0.0, 1.0, 0.0, 0.0), float4(0.0, 0.0, 1.0, 0.0), float4(0.0, 0.0, 0.0, 1.0));
    }
    float idet = 1.0 / det;
    return float4x4(
        float4((a11 * b11 - a12 * b10 + a13 * b09) * idet, (a02 * b10 - a01 * b11 - a03 * b09) * idet, (a31 * b05 - a32 * b04 + a33 * b03) * idet, (a22 * b04 - a21 * b05 - a23 * b03) * idet),
        float4((a12 * b08 - a10 * b11 - a13 * b07) * idet, (a00 * b11 - a02 * b08 + a03 * b07) * idet, (a32 * b02 - a30 * b05 - a33 * b01) * idet, (a20 * b05 - a22 * b02 + a23 * b01) * idet),
        float4((a10 * b10 - a11 * b08 + a13 * b06) * idet, (a01 * b08 - a00 * b10 - a03 * b06) * idet, (a30 * b04 - a31 * b02 + a33 * b00) * idet, (a21 * b02 - a20 * b04 - a23 * b00) * idet),
        float4((a11 * b07 - a10 * b09 - a12 * b06) * idet, (a00 * b09 - a01 * b07 + a02 * b06) * idet, (a31 * b01 - a30 * b03 - a32 * b00) * idet, (a20 * b03 - a21 * b01 + a22 * b00) * idet));
}
";

/// The pick variant's sample: where the colour shows more than any earlier
/// sample's (its alpha, or its light where it adds), the id in the
/// texture's twin under the place, else one beside it (a filtered sample
/// sees its neighbours too).
const METAL_PICK_HELPERS: &str = "inline float _mp_pick_cover(float4 c) { return max(c.w, max(c.x, max(c.y, c.z))); }
inline float4 _mp_pick_sample(thread Mp &_mp, uint b, texture2d<float> tw, float4 c, float2 uv) {
    float w = _mp_pick_cover(c);
    if ((_mp._pick_mask & (1u << b)) != 0u && w > _mp._pick_w) {
        float2 sz = float2(tw.get_width(), tw.get_height());
        int2 q = int2(clamp(uv, float2(0.0), float2(0.99999)) * sz);
        int2 hi = int2(sz) - 1;
        uint id = 0u;
        for (int k = 0; k < 5 && id == 0u; k++) {
            int2 o = k == 0 ? int2(0) : (k == 1 ? int2(1, 0) : (k == 2 ? int2(-1, 0) : (k == 3 ? int2(0, 1) : int2(0, -1))));
            float4 e = tw.read(uint2(clamp(q + o, int2(0), hi)));
            id = uint(e.x * 255.0 + 0.5) | (uint(e.y * 255.0 + 0.5) << 8) | (uint(e.z * 255.0 + 0.5) << 16);
        }
        if (id != 0u) { _mp._pick_w = w; _mp._pick_id = id; }
    }
    return c;
}
";

pub fn metal_draw_source_into(output: &ShaderOutput, types: &impl crate::shader_types::ShaderTypes, pick: bool, out: &mut String) {
    out.push_str(&metal_draw_source(output, types, pick));
}
