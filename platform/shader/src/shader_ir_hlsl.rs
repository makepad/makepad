//! HLSL (shader model 5) from the shader IR: the one source D3D11 compiles
//! twice, `vertex_main` with `vs_5_0` and `pixel_main` with `ps_5_0`.
//!
//! Registers: uniform buffers at `b<buffer_index>`, the dyn uniforms at b2,
//! the scope uniforms after the uniform buffers, the draw's first instance at
//! b13 (`_mp_inst_base`, zero when unbound); textures at t0.. in io
//! order and the instance word buffer after them; samplers at s0.. in the
//! compile's sampler order. Geometry semantics follow D3D11's input layout
//! (`GEOM<letter><chunk>`).

use crate::shader_ir::*;
use crate::shader_ir_draw::{build_hlsl_entries, HlslDrawInfo};
use crate::shader_ir_text::*;
use crate::shader_output::*;
use std::fmt::Write;

/// The declaration of the buffer a vertex shader reads its instance records
/// from; its register follows.
const HLSL_INSTANCE_BUFFER_DECL: &str = "Buffer<uint> _mp_inst : register(t";

/// The t register of the instance buffer `hlsl` reads its instance records
/// from, or None when the shader has no instance fields.
pub fn hlsl_instance_buffer_register(hlsl: &str) -> Option<u32> {
    let start = hlsl.find(HLSL_INSTANCE_BUFFER_DECL)? + HLSL_INSTANCE_BUFFER_DECL.len();
    let end = start + hlsl[start..].find(')')?;
    hlsl[start..end].parse().ok()
}

/// The HLSL source of a draw shader compiled to IR.
pub fn hlsl_draw_source(output: &ShaderOutput, types: &impl crate::shader_types::ShaderTypes) -> String {
    hlsl_draw(output, types, false).0
}

/// The pick variant (`ShaderOutput::pick`): the same shader writing the 24-bit id of what is drawn
/// at each pixel instead of its colours (the draw's id, or the id a sampled 2D texture's twin
/// holds there), as the Metal and SPIR-V variants do. The draw's id and the twin mask come in
/// `_pk` at b12; twin `k` (the k-th texture) at t(textures + 1 + k), after the instance words.
pub fn hlsl_draw_pick_source(output: &ShaderOutput, types: &impl crate::shader_types::ShaderTypes) -> String {
    hlsl_draw(output, types, true).0
}

pub fn hlsl_draw(output: &ShaderOutput, types: &impl crate::shader_types::ShaderTypes, pick: bool) -> (String, HlslDrawInfo) {
    let mut ir = output.ir.clone();
    let info = build_hlsl_entries(output, &mut ir, types);
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
    let mut cx = TextCx::new(&ir, Dialect::Hlsl, false);

    // The pick twins: the shader's 2D textures among its first 32.
    let ntex = output.io.iter().filter(|io| matches!(io.kind, ShaderIoKind::Texture(_))).count();
    let mut twins = Vec::new();
    if pick {
        let mut k = 0u32;
        for io in &output.io {
            if let ShaderIoKind::Texture(tt) = io.kind {
                if k < 32 && tt == TextureType::Texture2d {
                    if let Some(g) = ir.find_global(IrGlobalKind::Io(IrIo::Texture(tt)), io.name) {
                        cx.pick_twins.push((g, k));
                        twins.push((k, cx.plain_global_name(g)));
                    }
                }
                k += 1;
            }
        }
    }

    // Function bodies first: they tell which helpers the module needs.
    let mut bodies = String::new();
    for f in &funcs {
        if *f == vertex_main || *f == fragment_main {
            continue;
        }
        cx.function(*f, &mut bodies);
    }
    let mut entries = String::new();
    entry(&mut cx, vertex_main, "VertexIn", "VertexOut", "vertex_main", Some(&info.vertex_semantics), &mut entries);
    let fragment_outputs: Vec<(String, u32)> = ir.functions[fragment_main as usize].entry.as_ref().unwrap().outputs.iter()
        .filter_map(|io| match io.binding { IrIoBinding::Location(l) => Some((io.name.clone(), l)), _ => None }).collect();
    let pick_out = pick && !fragment_outputs.is_empty();
    if pick_out {
        let mut pixel = String::new();
        entry(&mut cx, fragment_main, "VertexOut", "PixelOut", "pixel_main", None, &mut pixel);
        // The body's `return out_data;` returns the colours; the pick variant turns them into the
        // id of what is drawn there.
        let mut f = String::from("struct PickOut {\n");
        for (name, l) in &fragment_outputs {
            writeln!(f, "    float4 {} : SV_TARGET{};", name, l).ok();
        }
        f.push_str("};\n");
        let first = ir.functions[fragment_main as usize].entry.as_ref().unwrap().outputs.iter().find(|io| matches!(io.binding, IrIoBinding::Location(_)));
        let cover = match first {
            Some(io) if *ir.get(io.ty) == IrTy::Vec(4, IrScalar::F32) => format!("_mp_pick_cover(c.{})", io.name),
            _ => "1.0".to_string(),
        };
        f.push_str("PickOut _mp_pick_out(PixelOut c) {\n");
        f.push_str("    uint pid = _pick_w > 0.0 ? _pick_id : _pk.x;\n");
        writeln!(f, "    if ({} < 0.1 || pid == 0u) discard;", cover).ok();
        f.push_str("    float4 pc = float4(float(pid & 255u), float((pid >> 8) & 255u), float((pid >> 16) & 255u), 255.0) / 255.0;\n    PickOut o;\n");
        for (name, _) in &fragment_outputs {
            writeln!(f, "    o.{} = pc;", name).ok();
        }
        f.push_str("    return o;\n}\n");
        let pixel = pixel
            .replacen("PixelOut pixel_main(VertexOut input) {", "PickOut pixel_main(VertexOut input) {\n    _pick_mask = _pk.y;", 1)
            .replace("return out_data;", "return _mp_pick_out(out_data);");
        let at = pixel.find("PickOut pixel_main(").unwrap_or(pixel.len());
        entries.push_str(&pixel[..at]);
        entries.push_str(&f);
        entries.push_str(&pixel[at..]);
    } else {
        entry(&mut cx, fragment_main, "VertexOut", "PixelOut", "pixel_main", None, &mut entries);
    }

    let mut out = String::new();
    out.push_str("#pragma pack_matrix(row_major)\n");
    let mut done = vec![false; ir.structs.len()];
    for s in 0..ir.structs.len() {
        struct_def(&cx, s as u32, &mut done, &mut out);
    }
    let mut k = 0;
    while k < cx.hlsl_ctors.len() {
        let s = cx.hlsl_ctors[k];
        let name = cx.struct_name(s);
        let sty = ir.types.iter().position(|t| *t == IrTy::Struct(s)).unwrap_or(0) as TyId;
        let mut params = Vec::new();
        let mut sets = String::new();
        for (i, fd) in ir.structs[s as usize].fields.iter().enumerate() {
            params.push(cx.decl(fd.ty, &format!("p{}", i)));
            write!(sets, " r.{} = p{};", cx.field_name(sty, i as u32), i).ok();
        }
        writeln!(out, "{0} _mp_make_{0}({1}) {{ {0} r;{2} return r; }}", name, params.join(", "), sets).ok();
        k += 1;
    }

    // Constant buffers.
    let mut max_buffer = None;
    for (i, io) in output.io.iter().enumerate() {
        if let (ShaderIoKind::UniformBuffer, Some(b)) = (&io.kind, io.buffer_index) {
            max_buffer = Some(max_buffer.map_or(b, |m: usize| m.max(b)));
            writeln!(out, "cbuffer cb_{0} : register(b{1}) {{ {2} unibuf_{0}; }};", io.name, b, cx.ty_name(io_tys[i])).ok();
        }
    }
    for (kind, block, register, prefix) in [(0, "IoUniform", 2, "uni_"), (1, "IoScopeUniform", max_buffer.map_or(3, |m| m + 1), "su_")] {
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
                writeln!(out, "cbuffer {} : register(b{}) {{", block, register).ok();
                first = false;
            }
            writeln!(out, "    {};", cx.decl(io_tys[i], &format!("{}{}", prefix, io.name))).ok();
        }
        if !first {
            out.push_str("};\n");
        }
    }

    // Textures, the instance buffer, samplers.
    let mut tex = 0;
    for io in &output.io {
        if let ShaderIoKind::Texture(tt) = io.kind {
            writeln!(out, "{} tex_{} : register(t{});", hlsl_texture_type(tt), io.name, tex).ok();
            tex += 1;
        }
    }
    if info.instance_slots > 0 {
        writeln!(out, "{}{});", HLSL_INSTANCE_BUFFER_DECL, tex).ok();
    }
    for (i, s) in output.samplers.iter().enumerate() {
        let t = if s.compare { "SamplerComparisonState" } else { "SamplerState" };
        writeln!(out, "{} _s{} : register(s{});", t, i, i).ok();
    }
    if pick {
        for (k, name) in &twins {
            writeln!(out, "Texture2D {}__pick : register(t{});", name, ntex as u32 + 1 + k).ok();
        }
        out.push_str(HLSL_PICK);
    }

    // Per-invocation globals.
    out.push_str("static uint _mp_iter = 0;\n");
    for (g, glob) in ir.globals.iter().enumerate() {
        if !used.get(g).copied().unwrap_or(false) {
            continue;
        }
        let name = cx.global_name(g as GlobalId);
        match glob.kind {
            IrGlobalKind::Io(IrIo::Geometry | IrIo::DynInstance | IrIo::RustInstance | IrIo::Varying | IrIo::FragmentOutput(_))
            | IrGlobalKind::VertexPosition
            | IrGlobalKind::InstanceIndex => {
                writeln!(out, "static {};", cx.decl(glob.ty, &name)).ok();
            }
            IrGlobalKind::ViewId => out.push_str("static const int VIEW_ID = 0;\n"),
            IrGlobalKind::InstanceBase => out.push_str("cbuffer _mp_draw : register(b13) { uint _mp_inst_base; };\n"),
            _ => {}
        }
    }
    out.push_str(HLSL_HELPERS);
    if cx.needs_inverse {
        out.push_str(HLSL_INVERSE4);
    }
    out.push_str(&bodies);
    out.push_str(&entries);
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

fn semantic(b: IrIoBinding, fragment_output: bool) -> String {
    match b {
        IrIoBinding::Builtin(bi) => match bi {
            IrBuiltinIo::Position => "SV_POSITION",
            IrBuiltinIo::VertexIndex => "SV_VertexID",
            IrBuiltinIo::InstanceIndex => "SV_InstanceID",
            IrBuiltinIo::ViewIndex => "SV_ViewID",
            IrBuiltinIo::FrontFacing => "SV_IsFrontFace",
            IrBuiltinIo::FragDepth => "SV_Depth",
        }
        .to_string(),
        IrIoBinding::Location(l) => {
            if fragment_output {
                format!("SV_TARGET{}", l)
            } else {
                format!("TEXCOORD{}", l)
            }
        }
    }
}

/// An entry point and its output struct (and its input struct when
/// `in_semantics` names them: the vertex stage's; the pixel stage takes the
/// vertex stage's output).
fn entry(cx: &mut TextCx, fi: FuncId, in_struct: &str, out_struct: &str, name: &str, in_semantics: Option<&Vec<String>>, out: &mut String) {
    let m = cx.m;
    let f = &m.functions[fi as usize];
    let e = f.entry.as_ref().unwrap();
    let fragment = e.stage == IrStage::Fragment;
    if let Some(sems) = in_semantics {
        writeln!(out, "struct {} {{", in_struct).ok();
        for (k, io) in e.inputs.iter().enumerate() {
            let sem = match sems.get(k) {
                Some(s) => s.clone(),
                None => semantic(io.binding, false),
            };
            writeln!(out, "    {} : {};", cx.decl(io.ty, &io.name), sem).ok();
        }
        out.push_str("};\n");
    }
    writeln!(out, "struct {} {{", out_struct).ok();
    for io in &e.outputs {
        let interp = if !fragment && m.scalar_of(io.ty).map(|s| !s.is_float()).unwrap_or(false) { "nointerpolation " } else { "" };
        writeln!(out, "    {}{} : {};", interp, cx.decl(io.ty, &io.name), semantic(io.binding, fragment)).ok();
    }
    out.push_str("};\n");
    writeln!(out, "{0} {1}({2} input) {{\n    {0} out_data = ({0})0;", out_struct, name, in_struct).ok();
    cx.stmts(f, &f.body, 1, out);
    out.push_str("}\n");
}

/// The pick variant's state and helpers: the winning twin id and its coverage; `_pk` is
/// (draw id, twin mask).
const HLSL_PICK: &str = "cbuffer _mp_pick : register(b12) { uint4 _pk; };
static uint _pick_mask = 0u;
static uint _pick_id = 0u;
static float _pick_w = 0.0;
float _mp_pick_cover(float4 c) { return max(c.w, max(c.x, max(c.y, c.z))); }
uint _mp_pick_id(float4 e) { return uint(e.x * 255.0 + 0.5) | (uint(e.y * 255.0 + 0.5) << 8) | (uint(e.z * 255.0 + 0.5) << 16); }
float4 _mp_pick_sample(uint b, Texture2D tw, float4 c, float2 uv) {
    float w = _mp_pick_cover(c);
    // straight-line inside one real branch: FXC compiles nested flow per inlined sample slowly
    [branch] if ((_pick_mask & (1u << b)) != 0u && w > _pick_w) {
        uint tw_w, tw_h; tw.GetDimensions(tw_w, tw_h);
        int2 hi = int2(tw_w, tw_h) - 1;
        int2 q = int2(clamp(uv, float2(0.0, 0.0), float2(0.99999, 0.99999)) * float2(tw_w, tw_h));
        // the texel and its four neighbours: the first id found
        uint i0 = _mp_pick_id(tw.Load(int3(clamp(q, int2(0, 0), hi), 0)));
        uint i1 = _mp_pick_id(tw.Load(int3(clamp(q + int2(1, 0), int2(0, 0), hi), 0)));
        uint i2 = _mp_pick_id(tw.Load(int3(clamp(q + int2(-1, 0), int2(0, 0), hi), 0)));
        uint i3 = _mp_pick_id(tw.Load(int3(clamp(q + int2(0, 1), int2(0, 0), hi), 0)));
        uint i4 = _mp_pick_id(tw.Load(int3(clamp(q + int2(0, -1), int2(0, 0), hi), 0)));
        uint id = i0 != 0u ? i0 : (i1 != 0u ? i1 : (i2 != 0u ? i2 : (i3 != 0u ? i3 : i4)));
        _pick_w = id != 0u ? w : _pick_w;
        _pick_id = id != 0u ? id : _pick_id;
    }
    return c;
}
";

const HLSL_HELPERS: &str = "float2 _mp_unpack2f16(float x) { uint u = asuint(x); return float2(f16tof32(u & 0xffff), f16tof32(u >> 16)); }
float4 _mp_unpack4u8(float x) { uint u = asuint(x); return float4(float(u & 0xff), float((u >> 8) & 0xff), float((u >> 16) & 0xff), float((u >> 24) & 0xff)) * (1.0 / 255.0); }
float2 _mp_unpack_half2x16(uint u) { return float2(f16tof32(u & 0xffff), f16tof32(u >> 16)); }
float2 _mp_unpack_unorm2x16(uint u) { return float2(float(u & 0xffff), float(u >> 16)) * (1.0 / 65535.0); }
float2 _mp_unpack_snorm2x16(uint u) { int2 i = int2(int(u << 16), int(u)) >> 16; return clamp(float2(i) / 32767.0, -1.0, 1.0); }
float4 _mp_unpack_unorm4x8(uint u) { return float4(float(u & 0xff), float((u >> 8) & 0xff), float((u >> 16) & 0xff), float((u >> 24) & 0xff)) * (1.0 / 255.0); }
float4 _mp_unpack_snorm4x8(uint u) { int4 i = int4(int(u << 24), int(u << 16), int(u << 8), int(u)) >> 24; return clamp(float4(i) / 127.0, -1.0, 1.0); }
float2 _mpTexSize2D(Texture2D tex) { uint w, h; tex.GetDimensions(w, h); return float2(w, h); }
float2 _mpTexSize2D(Texture2D<float> tex) { uint w, h; tex.GetDimensions(w, h); return float2(w, h); }
#define _mp_asinh(x) log((x) + sqrt((x) * (x) + 1.0))
#define _mp_acosh(x) log((x) + sqrt((x) * (x) - 1.0))
#define _mp_atanh(x) (0.5 * log((1.0 + (x)) / (1.0 - (x))))
";

/// HLSL has no matrix inverse: the cofactor expansion (the same numbers
/// as WGSL's helper: `m[i]` is the IR's column i here too).
const HLSL_INVERSE4: &str = "float4x4 _mp_inverse4(float4x4 m) {
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
    float inv = 1.0 / det;
    return float4x4(
        float4(a11 * b11 - a12 * b10 + a13 * b09, a02 * b10 - a01 * b11 - a03 * b09, a31 * b05 - a32 * b04 + a33 * b03, a22 * b04 - a21 * b05 - a23 * b03) * inv,
        float4(a12 * b08 - a10 * b11 - a13 * b07, a00 * b11 - a02 * b08 + a03 * b07, a32 * b02 - a30 * b05 - a33 * b01, a20 * b05 - a22 * b02 + a23 * b01) * inv,
        float4(a10 * b10 - a11 * b08 + a13 * b06, a01 * b08 - a00 * b10 - a03 * b06, a30 * b04 - a31 * b02 + a33 * b00, a21 * b02 - a20 * b04 - a23 * b00) * inv,
        float4(a11 * b07 - a10 * b09 - a12 * b06, a00 * b09 - a01 * b07 + a02 * b06, a31 * b01 - a30 * b03 - a32 * b00, a20 * b03 - a21 * b01 + a22 * b00) * inv);
}
";

pub fn hlsl_draw_source_into(output: &ShaderOutput, types: &impl crate::shader_types::ShaderTypes, out: &mut String) {
    out.push_str(&hlsl_draw_source(output, types));
}
