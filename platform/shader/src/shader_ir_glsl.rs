//! GLSL ES 3.00 from the shader IR: the vertex and fragment sources the GL
//! ES and WebGL 2 backends link (the host prepends `#version`, precision,
//! `VIEW_ID`, the `sample2d*` / `samplecube*` / `sample2dOES` helpers and
//! `depth_clip`).

use crate::shader_ir::*;
use crate::shader_ir_draw::{build_gl_entries, GlDrawInfo};
use crate::shader_ir_text::*;
use crate::shader_output::*;
use makepad_live_id::LiveId;
use makepad_relax_api::relax_api;
use std::fmt::Write;

pub struct GlslDrawSources {
    pub vertex: String,
    pub fragment: String,
    pub info: GlDrawInfo,
    /// The fragment stage's size once every call is inlined (D3D under
    /// ANGLE inlines all calls): total bytes, and the heaviest functions as
    /// (name, copies, bytes of one copy).
    pub inline_estimate: (usize, Vec<(String, usize, usize)>),
}

// Ops: under makepad run the sources are this (provided) crate's values; JIT code reads them
// only through these.
#[relax_api]
impl GlslDrawSources {
    #[relax_api]
    pub fn vertex(&self) -> &str { &self.vertex }
    #[relax_api]
    pub fn fragment(&self) -> &str { &self.fragment }
    #[relax_api]
    pub fn info(&self) -> GlDrawInfo { self.info.clone() }
    #[relax_api]
    pub fn inline_bytes(&self) -> usize { self.inline_estimate.0 }
    #[relax_api]
    pub fn inline_function_count(&self) -> usize { self.inline_estimate.1.len() }
    #[relax_api]
    pub fn inline_function_name(&self, index: usize) -> &str { &self.inline_estimate.1[index].0 }
    #[relax_api]
    pub fn inline_function_copies(&self, index: usize) -> usize { self.inline_estimate.1[index].1 }
    #[relax_api]
    pub fn inline_function_bytes(&self, index: usize) -> usize { self.inline_estimate.1[index].2 }
}

/// The GL sources of a compiled draw shader; records which sampler each
/// texture is bound with in `output.texture_sampler_bindings`.
pub fn glsl_draw_sources(output: &mut ShaderOutput, types: &impl crate::shader_types::ShaderTypes) -> GlslDrawSources {
    draw_sources(output, types, false, false)
}

/// HAL GLES receives complete compiler output, including its pick variant and prelude.
/// Video frames in this dialect are ordinary HAL 2D textures.
pub fn gles_draw_sources(output: &mut ShaderOutput, types: &impl crate::shader_types::ShaderTypes, pick: bool) -> GlslDrawSources {
    draw_sources(output, types, true, pick)
}

fn draw_sources(output: &mut ShaderOutput, types: &impl crate::shader_types::ShaderTypes, complete: bool, pick: bool) -> GlslDrawSources {
    let mut ir = output.ir.clone();
    let info = build_gl_entries(output, &mut ir, types);
    let mut io_tys = Vec::new();
    for io in &output.io {
        io_tys.push(match io.kind {
            ShaderIoKind::Texture(_) | ShaderIoKind::Sampler(_) => TY_VOID,
            _ => ir.pod_ty(types, io.ty),
        });
    }
    output.texture_sampler_bindings = texture_sampler_bindings(&ir);
    let mut cx = TextCx::new(&ir, Dialect::Glsl, false);
    cx.glsl_oes_video = !complete && cfg!(any(target_os = "android", target_os = "linux")) && !output.use_vulkan;
    if pick {
        for (i, io) in output.io.iter().filter(|io| matches!(io.kind, ShaderIoKind::Texture(_))).enumerate() {
            if i < 32 && matches!(io.kind, ShaderIoKind::Texture(TextureType::Texture2d)) {
                if let Some(g) = ir.find_global(IrGlobalKind::Io(IrIo::Texture(TextureType::Texture2d)), io.name) {
                    cx.pick_twins.push((g, i as u32));
                }
            }
        }
    }

    // Shared definitions: the structs, members before their users.
    let mut shared = if complete { GLES_PRELUDE.to_owned() } else { String::new() };
    if pick {
        shared.push_str(GLES_PICK);
        for (g, _) in &cx.pick_twins {
            writeln!(shared, "uniform sampler2D {}__pick;", cx.plain_global_name(*g)).ok();
        }
    }
    let mut done = vec![false; ir.structs.len()];
    for s in 0..ir.structs.len() {
        struct_def(&cx, s as u32, &mut done, &mut shared);
    }
    shared.push_str("uint _mp_iter = 0u;\n");
    shared.push_str("vec2 _mp_unpack2f16(float x){ return unpackHalf2x16(floatBitsToUint(x)); }\n");
    shared.push_str("vec4 _mp_unpack4u8(float x){ uint u = floatBitsToUint(x); return vec4(float(u & 0xffu), float((u >> 8u) & 0xffu), float((u >> 16u) & 0xffu), float((u >> 24u) & 0xffu)) * (1.0 / 255.0); }\n");
    shared.push_str("vec4 _mp_unpack_unorm4x8(uint u){ return vec4(float(u & 0xffu), float((u >> 8u) & 0xffu), float((u >> 16u) & 0xffu), float((u >> 24u) & 0xffu)) * (1.0 / 255.0); }\n");
    shared.push_str("vec4 _mp_unpack_snorm4x8(uint u){ ivec4 i = ivec4(int(u << 24u), int(u << 16u), int(u << 8u), int(u)) >> 24; return clamp(vec4(i) / 127.0, -1.0, 1.0); }\n");
    uniform_blocks(&cx, output, &io_tys, &mut shared);

    let vertex_main = ir.function_by_name("vertex_main").unwrap();
    let fragment_main = ir.function_by_name("fragment_main").unwrap();

    // Vertex.
    let mut vertex = shared.clone();
    textures(&cx, output, false, &mut vertex);
    vertex.push_str("vec4 vtx_pos;\nuniform float mp_clip_y_flip;\n");
    private_globals(&cx, &ir, true, &mut vertex);
    let entry = ir.functions[vertex_main as usize].entry.clone().unwrap();
    for io in &entry.inputs {
        writeln!(vertex, "in {};", cx.decl(io.ty, &io.name)).ok();
    }
    if info.instance_fetch_from.is_some() {
        vertex.push_str("uniform highp usampler2D mp_inst_data;\nuniform int mp_inst_base;\n");
        writeln!(
            vertex,
            "float _mp_inst_word(int slot) {{ int i = (gl_InstanceID + mp_inst_base) * {} + slot; int w = textureSize(mp_inst_data, 0).x; return uintBitsToFloat(texelFetch(mp_inst_data, ivec2(i % w, i / w), 0).r); }}",
            info.instance_slots
        )
        .ok();
    }
    for io in &entry.outputs {
        if let IrIoBinding::Location(_) = io.binding {
            writeln!(vertex, "out {};", cx.decl(io.ty, &io.name)).ok();
        }
    }
    for f in callees_first(&ir, &[vertex_main]) {
        if f == vertex_main {
            continue;
        }
        cx.function(f, &mut vertex);
    }
    vertex.push_str("void main() {\n");
    let vf = &ir.functions[vertex_main as usize];
    cx.stmts(vf, &vf.body, 1, &mut vertex);
    vertex.push_str("}\n");

    // Fragment.
    let mut fragment = shared;
    textures(&cx, output, true, &mut fragment);
    private_globals(&cx, &ir, false, &mut fragment);
    let entry = ir.functions[fragment_main as usize].entry.clone().unwrap();
    for io in &entry.inputs {
        writeln!(fragment, "in {};", cx.decl(io.ty, &io.name)).ok();
    }
    for io in &entry.outputs {
        if let IrIoBinding::Location(l) = io.binding {
            if pick {
                writeln!(fragment, "{};\nlayout(location = {}) out vec4 mp_id_{};", cx.decl(io.ty, &io.name), l, l).ok();
            } else {
                writeln!(fragment, "layout(location = {}) out {};", l, cx.decl(io.ty, &io.name)).ok();
            }
        }
    }
    let mut sizes = vec![0usize; ir.functions.len()];
    for f in callees_first(&ir, &[fragment_main]) {
        if f == fragment_main {
            continue;
        }
        let start = fragment.len();
        cx.function(f, &mut fragment);
        sizes[f as usize] = fragment.len() - start;
    }
    fragment.push_str("void main() {\n");
    let ff = &ir.functions[fragment_main as usize];
    cx.stmts(ff, &ff.body, 1, &mut fragment);
    if pick {
        let cover = entry.outputs.first().filter(|o| *ir.get(o.ty) == IrTy::Vec(4, IrScalar::F32))
            .map_or_else(|| "1.0".to_owned(), |o| format!("_mp_pick_cover({})", o.name));
        writeln!(fragment, "uint pid = _pick_w > 0.0 ? _pick_id : mp_pick.x;\nif ({} < 0.1 || pid == 0u) discard;", cover).ok();
        fragment.push_str("vec4 pc = vec4(float(pid & 255u), float((pid >> 8u) & 255u), float((pid >> 16u) & 255u), 255.0) / 255.0;\n");
        for io in &entry.outputs {
            if let IrIoBinding::Location(l) = io.binding { writeln!(fragment, "mp_id_{} = pc;", l).ok(); }
        }
    }
    fragment.push_str("}\n");

    let inline_estimate = match ir.function_by_name("io_fragment") {
        Some(root) => inline_estimate(&ir, root, &sizes),
        None => (0, Vec::new()),
    };
    GlslDrawSources { vertex, fragment, info, inline_estimate }
}

const GLES_PRELUDE: &str = "#version 300 es
#define VIEW_ID 0
precision highp float;
precision highp int;
precision highp sampler2D;
precision highp samplerCube;
precision highp sampler2DArray;
precision highp sampler2DShadow;
precision highp sampler2DArrayShadow;
precision highp usampler2D;
vec4 sample2d(sampler2D s, vec2 p) { return texture(s, p); }
vec4 sample2d_lod(sampler2D s, vec2 p, float l) { return textureLod(s, p, l); }
vec4 samplecube(samplerCube s, vec3 p) { return texture(s, p); }
vec4 samplecube_lod(samplerCube s, vec3 p, float l) { return textureLod(s, p, l); }
vec4 depth_clip(vec4 w, vec4 c, float clip) { return c; }
";

const GLES_PICK: &str = "uniform uvec2 mp_pick;
uint _pick_id = 0u;
float _pick_w = 0.0;
float _mp_pick_cover(vec4 c) { return max(c.w, max(c.x, max(c.y, c.z))); }
uint _mp_pick_id(vec4 e) { return uint(e.x * 255.0 + 0.5) | (uint(e.y * 255.0 + 0.5) << 8u) | (uint(e.z * 255.0 + 0.5) << 16u); }
vec4 _mp_pick_sample(uint b, sampler2D tw, vec4 c, vec2 uv) {
    float w = _mp_pick_cover(c);
    if ((mp_pick.y & (1u << b)) != 0u && w > _pick_w) {
        ivec2 hi = textureSize(tw, 0) - 1;
        ivec2 q = ivec2(clamp(uv, vec2(0.0), vec2(0.99999)) * vec2(hi + 1));
        uint id = _mp_pick_id(texelFetch(tw, clamp(q, ivec2(0), hi), 0));
        if (id == 0u) id = _mp_pick_id(texelFetch(tw, clamp(q + ivec2(1, 0), ivec2(0), hi), 0));
        if (id == 0u) id = _mp_pick_id(texelFetch(tw, clamp(q + ivec2(-1, 0), ivec2(0), hi), 0));
        if (id == 0u) id = _mp_pick_id(texelFetch(tw, clamp(q + ivec2(0, 1), ivec2(0), hi), 0));
        if (id == 0u) id = _mp_pick_id(texelFetch(tw, clamp(q + ivec2(0, -1), ivec2(0), hi), 0));
        if (id != 0u) { _pick_w = w; _pick_id = id; }
    }
    return c;
}
";

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
    writeln!(out, "struct {} {{", cx.struct_name(s)).ok();
    for f in &st.fields {
        writeln!(out, "    {};", cx.decl(f.ty, &lid(f.name))).ok();
    }
    out.push_str("};\n");
}

fn uniform_block_name(name: LiveId) -> String {
    let n = lid(name);
    match n.as_str() {
        "draw_pass" => "passUniforms".to_string(),
        "draw_list" => "draw_listUniforms".to_string(),
        "draw_call" => "draw_callUniforms".to_string(),
        _ => format!("{}_Uniforms", n),
    }
}

/// The uniform blocks: one per uniform buffer (the draw pass with its
/// view-dependent matrices as two-view arrays), the dyn uniforms
/// (`userUniforms`) and the scope uniforms (`liveUniforms`), their members
/// read in place.
fn uniform_blocks(cx: &TextCx, output: &ShaderOutput, io_tys: &[TyId], out: &mut String) {
    for (i, io) in output.io.iter().enumerate() {
        if !matches!(io.kind, ShaderIoKind::UniformBuffer) {
            continue;
        }
        let block = uniform_block_name(io.name);
        let n = lid(io.name);
        let t = io_tys[i];
        if n == "draw_pass" {
            if let IrTy::Struct(s) = cx.m.get(t) {
                writeln!(out, "layout(std140) uniform {} {{", block).ok();
                for f in &cx.m.structs[*s as usize].fields {
                    let fname = lid(f.name);
                    match fname.as_str() {
                        "camera_projection" | "camera_view" | "depth_projection" | "depth_view" | "camera_inv" => {
                            writeln!(out, "    {} {}[2];", cx.ty_name(f.ty), fname).ok();
                        }
                        "camera_projection_r" | "camera_view_r" | "depth_projection_r" | "depth_view_r" | "camera_inv_r" => {}
                        _ => {
                            writeln!(out, "    {};", cx.decl(f.ty, &fname)).ok();
                        }
                    }
                }
                writeln!(out, "}} unibuf_{};", n).ok();
                continue;
            }
        }
        writeln!(out, "layout(std140) uniform {} {{", block).ok();
        writeln!(out, "    {} unibuf_{};", cx.ty_name(t), n).ok();
        out.push_str("};\n");
    }
    for (kind, block, prefix) in [(0, "userUniforms", "uni_"), (1, "liveUniforms", "su_")] {
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
                writeln!(out, "layout(std140) uniform {} {{", block).ok();
                first = false;
            }
            writeln!(out, "    {};", cx.decl(io_tys[i], &format!("{}{}", prefix, io.name))).ok();
        }
        if !first {
            out.push_str("};\n");
        }
    }
}

fn textures(cx: &TextCx, output: &ShaderOutput, fragment: bool, out: &mut String) {
    for io in &output.io {
        if let ShaderIoKind::Texture(tt) = io.kind {
            // The vertex stage never samples video (and Adreno fails on a
            // `samplerExternalOES` next to `[int(VIEW_ID)]`).
            if !fragment && matches!(tt, TextureType::TextureVideo) {
                continue;
            }
            writeln!(out, "uniform {} tex_{};", cx.glsl_sampler_type(tt), io.name).ok();
        }
    }
}

/// The IO a stage keeps in plain globals (records, varyings, outputs).
fn private_globals(cx: &TextCx, ir: &IrModule, vertex: bool, out: &mut String) {
    for (g, glob) in ir.globals.iter().enumerate() {
        let keep = match glob.kind {
            IrGlobalKind::Io(IrIo::Geometry) => vertex,
            IrGlobalKind::Io(IrIo::DynInstance) | IrGlobalKind::Io(IrIo::RustInstance) | IrGlobalKind::Io(IrIo::Varying) => true,
            IrGlobalKind::Io(IrIo::FragmentOutput(_)) => !vertex,
            _ => false,
        };
        if keep {
            writeln!(out, "{};", cx.decl(glob.ty, &cx.global_name(g as GlobalId))).ok();
        }
    }
}

/// Which sampler each texture is sampled with, the first sample winning, in
/// the order the compile reached them (the vertex stage's calls first).
pub fn texture_sampler_bindings(ir: &IrModule) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut seen = vec![false; ir.functions.len()];
    for root in ["io_vertex", "io_fragment"] {
        if let Some(f) = ir.function_by_name(root) {
            bindings_in(ir, f, &mut seen, &mut out);
        }
    }
    out
}

fn bindings_in(ir: &IrModule, fi: FuncId, seen: &mut Vec<bool>, out: &mut Vec<(String, usize)>) {
    if seen[fi as usize] {
        return;
    }
    seen[fi as usize] = true;
    let f = &ir.functions[fi as usize];
    let mut order = Vec::new();
    stmt_exprs(&f.body, &mut order);
    let mut stack = Vec::new();
    for e in order {
        stack.push(e);
        while let Some(x) = stack.pop() {
            if x == NO_EXPR {
                continue;
            }
            let kind = &f.exprs[x as usize].kind;
            let mut kids = Vec::new();
            expr_children(kind, &mut kids);
            // Arguments before the expression itself (evaluation order).
            let mut k = kids.len();
            while k > 0 {
                k -= 1;
                stack.push(kids[k]);
            }
            match kind {
                ExprKind::Call(c, _) => bindings_in(ir, *c, seen, out),
                ExprKind::Sample(s) => {
                    if let ExprKind::Global(g) = f.exprs[s.tex as usize].kind {
                        let name = format!("tex_{}", ir.globals[g as usize].name);
                        let mut have = false;
                        for (n, _) in out.iter() {
                            if *n == name {
                                have = true;
                            }
                        }
                        if !have {
                            out.push((name, s.sampler as usize));
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn stmt_exprs(stmts: &[Stmt], out: &mut Vec<ExprId>) {
    for s in stmts {
        match s {
            Stmt::Local(_, e) | Stmt::Expr(e) | Stmt::Return(e) => out.push(*e),
            Stmt::Assign(a, b) | Stmt::CompoundAssign(a, _, b) => {
                out.push(*b);
                out.push(*a);
            }
            Stmt::If(c, a, b) => {
                out.push(*c);
                stmt_exprs(a, out);
                stmt_exprs(b, out);
            }
            Stmt::For(_, a, b, body) => {
                out.push(*a);
                out.push(*b);
                stmt_exprs(body, out);
            }
            Stmt::Loop(body) => stmt_exprs(body, out),
            _ => {}
        }
    }
}

/// Copies of every function the fragment stage inlines into one body, and
/// the bytes that makes.
fn inline_estimate(ir: &IrModule, root: FuncId, sizes: &[usize]) -> (usize, Vec<(String, usize, usize)>) {
    let n = ir.functions.len();
    let order = callees_first(ir, &[root]);
    let mut copies = vec![0usize; n];
    copies[root as usize] = 1;
    let mut i = order.len();
    while i > 0 {
        i -= 1;
        let f = order[i];
        let func = &ir.functions[f as usize];
        let live = live_exprs(func);
        for (k, e) in func.exprs.iter().enumerate() {
            if !live[k] {
                continue;
            }
            if let ExprKind::Call(c, _) = &e.kind {
                copies[*c as usize] = copies[*c as usize].saturating_add(copies[f as usize]);
            }
        }
    }
    let mut total = 0usize;
    let mut top = Vec::new();
    for f in order {
        let c = copies[f as usize];
        if c == 0 {
            continue;
        }
        let size = sizes.get(f as usize).copied().unwrap_or(0);
        total = total.saturating_add(c.saturating_mul(size));
        top.push((ir.functions[f as usize].name.clone(), c, size));
    }
    top.sort_by_key(|(_, c, l)| std::cmp::Reverse(c.saturating_mul(*l)));
    top.truncate(4);
    (total, top)
}
