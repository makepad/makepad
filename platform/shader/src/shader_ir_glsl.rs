//! GLSL ES 3.00 from the shader IR: the vertex and fragment sources the GL
//! ES and WebGL 2 backends link (the host prepends `#version`, precision,
//! `VIEW_ID`, the `sample2d*` / `samplecube*` / `sample2dOES` helpers and
//! `depth_clip`).

use crate::shader_ir::*;
use crate::shader_ir_draw::{build_gl_entries, GlDrawInfo, INSTANCE_WORD_ROW_SHIFT};
use crate::shader_ir_text::*;
use crate::shader_output::*;
use makepad_live_id::LiveId;
use relax_api::relax_api;
use std::collections::HashMap;
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

/// The GL sources of a compiled draw shader and its combined-sampler bindings.
pub fn glsl_draw_sources(output: &mut ShaderOutput, types: &impl crate::shader_types::ShaderTypes) -> GlslDrawSources {
    draw_sources(output, types, false, false)
}

/// HAL GLES receives complete compiler output, including its pick variant and prelude.
/// Video frames in this dialect are ordinary HAL 2D textures.
pub fn gles_draw_sources(output: &mut ShaderOutput, types: &impl crate::shader_types::ShaderTypes, pick: bool) -> GlslDrawSources {
    draw_sources(output, types, true, pick)
}

fn draw_sources(output: &mut ShaderOutput, types: &impl crate::shader_types::ShaderTypes, complete: bool, pick: bool) -> GlslDrawSources {
    if !output.use_vulkan && output.ir.globals.iter().any(|g| matches!(g.kind, IrGlobalKind::Io(IrIo::Storage(_)))) {
        output.push_error("storage buffers in draw shaders need Metal, Vulkan or D3D11".into());
        return GlslDrawSources { vertex: String::new(), fragment: String::new(), info: GlDrawInfo::default(), inline_estimate: (0, Vec::new()) };
    }
    let mut ir = output.ir.clone();
    crate::shader_ir_precise::lower_precise(&mut ir, true);
    let info = build_gl_entries(output, &mut ir, types);
    let mut io_tys = Vec::new();
    for io in &output.io {
        io_tys.push(match io.kind {
            ShaderIoKind::Texture(_) | ShaderIoKind::Sampler(_) => TY_VOID,
            _ => ir.pod_ty(types, io.ty),
        });
    }
    let (bindings, slots) = collect_texture_bindings(output, &ir);
    output.glsl_texture_bindings = bindings;
    let mut cx = TextCx::new(&ir, Dialect::Glsl, false);
    cx.glsl_oes_video = !complete && cfg!(any(target_os = "android", target_os = "linux")) && !output.use_vulkan;
    for b in &output.glsl_texture_bindings {
        let g = slots[b.texture_slot as usize].global.unwrap();
        cx.glsl_sampler_names.push((g, b.sampler_index, b.uniform_name.clone()));
    }

    // Shared definitions: the structs, members before their users.
    let mut shared = if complete { GLES_PRELUDE.to_owned() } else { String::new() };
    if !complete && output.io.iter().any(|io| matches!(io.kind, ShaderIoKind::Texture(TextureType::Texture3d))) {
        // The same packed shader body also runs with the WebGL2 host's ES prelude.
        shared.push_str("#ifdef GL_ES\nprecision highp sampler3D;\n#endif\n");
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
    textures(&cx, &output.glsl_texture_bindings, &slots, GLSL_STAGE_VERTEX, &mut vertex);
    vertex.push_str("vec4 vtx_pos;\nuniform float mp_clip_y_flip;\n");
    private_globals(&cx, &ir, true, &mut vertex);
    let entry = ir.functions[vertex_main as usize].entry.clone().unwrap();
    for io in &entry.inputs {
        writeln!(vertex, "in {};", cx.decl(io.ty, &io.name)).ok();
    }
    if info.instance_fetch_from.is_some() {
        // Word i of the instance buffer is at column i % ROW, row (i / ROW) % ROW and layer
        // i / ROW^2 of a ROW-wide array texture (`INSTANCE_WORD_ROW_SHIFT`): each layer is one
        // window of ROW^2 words, and a buffer of any size takes as many layers as it needs.
        let (shift, mask) = (INSTANCE_WORD_ROW_SHIFT, (1u32 << INSTANCE_WORD_ROW_SHIFT) - 1);
        vertex.push_str("uniform highp usampler2DArray mp_inst_data;\nuniform int mp_inst_base;\n");
        writeln!(
            vertex,
            "float _mp_inst_word(int slot) {{ int i = (gl_InstanceID + mp_inst_base) * {} + slot; return uintBitsToFloat(texelFetch(mp_inst_data, ivec3(i & {mask}, (i >> {shift}) & {mask}, i >> {}), 0).r); }}",
            info.instance_slots, 2 * shift
        )
        .ok();
    }
    for io in &entry.outputs {
        if let IrIoBinding::Location(_) = io.binding {
            writeln!(vertex, "{}out {};", flat(&ir, io), cx.decl(io.ty, &io.name)).ok();
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
    textures(&cx, &output.glsl_texture_bindings, &slots, GLSL_STAGE_FRAGMENT, &mut fragment);
    if pick {
        fragment.push_str(GLES_PICK);
        for (i, slot) in slots.iter().enumerate().take(32) {
            if slot.ty == TextureType::Texture2d && slot.fragment_sample {
                let g = slot.global.unwrap();
                cx.pick_twins.push((g, i as u32));
                writeln!(fragment, "uniform sampler2D {};", pick_twin_name(&cx.plain_global_name(g))).ok();
            }
        }
    }
    private_globals(&cx, &ir, false, &mut fragment);
    if ir.globals.iter().any(|g| g.kind == IrGlobalKind::FragYOrigin) {
        // `fragment_position()`'s row origin (the host sets it with the clip Y flip)
        fragment.push_str("uniform vec2 mp_frag_y;\n");
    }
    let entry = ir.functions[fragment_main as usize].entry.clone().unwrap();
    for io in &entry.inputs {
        // gl_FragCoord (the depth output's default) is built in
        if let IrIoBinding::Location(_) = io.binding {
            writeln!(fragment, "{}in {};", flat(&ir, io), cx.decl(io.ty, &io.name)).ok();
        }
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
precision highp sampler3D;
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

fn textures(cx: &TextCx, bindings: &[GlslTextureBinding], slots: &[TextureSlot], stage: u8, out: &mut String) {
    for b in bindings {
        if b.stages & stage != 0 {
            let ty = slots[b.texture_slot as usize].ty;
            writeln!(out, "uniform {} {};", cx.glsl_sampler_type(ty), b.uniform_name).ok();
        }
    }
}

/// An integer varying's qualifier: GLSL requires it flat.
fn flat(m: &IrModule, io: &IrEntryIo) -> &'static str {
    match m.scalar_of(io.ty) {
        Some(IrScalar::I32 | IrScalar::U32) => "flat ",
        _ => "",
    }
}

/// The IO a stage keeps in plain globals (records, varyings, outputs).
fn private_globals(cx: &TextCx, ir: &IrModule, vertex: bool, out: &mut String) {
    for (g, glob) in ir.globals.iter().enumerate() {
        let keep = match glob.kind {
            IrGlobalKind::Io(IrIo::Geometry) => vertex,
            IrGlobalKind::Io(IrIo::DynInstance) | IrGlobalKind::Io(IrIo::RustInstance) | IrGlobalKind::Io(IrIo::Varying) => true,
            IrGlobalKind::Io(IrIo::FragmentOutput(_) | IrIo::FragmentDepth | IrIo::FragmentPosition) => !vertex,
            _ => false,
        };
        if keep {
            writeln!(out, "{};", cx.decl(glob.ty, &cx.global_name(g as GlobalId))).ok();
        }
    }
}

/// The uniforms the linked stages can use, in first-use order.
pub fn glsl_texture_bindings(output: &mut ShaderOutput, ir: &IrModule) -> Vec<GlslTextureBinding> {
    collect_texture_bindings(output, ir).0
}

struct TextureSlot {
    name: LiveId,
    ty: TextureType,
    global: Option<GlobalId>,
    primary: Option<usize>,
    pair_count: usize,
    size_or_load_stages: u8,
    fragment_sample: bool,
}

fn collect_texture_bindings(output: &mut ShaderOutput, ir: &IrModule) -> (Vec<GlslTextureBinding>, Vec<TextureSlot>) {
    let mut slots = Vec::new();
    let mut slot_by_name = HashMap::new();
    for io in &output.io {
        if let ShaderIoKind::Texture(ty) = io.kind {
            slot_by_name.insert((io.name, ty), slots.len());
            slots.push(TextureSlot {
                name: io.name, ty, global: None, primary: None, pair_count: 0,
                size_or_load_stages: 0, fragment_sample: false,
            });
        }
    }
    let mut global_to_slot = vec![None; ir.globals.len()];
    for (g, global) in ir.globals.iter().enumerate() {
        if let IrGlobalKind::Io(IrIo::Texture(ty)) = global.kind {
            if let Some(&slot) = slot_by_name.get(&(global.name, ty)) {
                slots[slot].global = Some(g as GlobalId);
                global_to_slot[g] = Some(slot);
            }
        }
    }

    let mut bindings: Vec<GlslTextureBinding> = Vec::new();
    let mut pairs = HashMap::<(usize, u32), usize>::new();
    for (root, stage) in [("vertex_main", GLSL_STAGE_VERTEX), ("fragment_main", GLSL_STAGE_FRAGMENT)] {
        let Some(root) = ir.function_by_name(root) else { continue };
        let mut seen = vec![false; ir.functions.len()];
        visit_texture_uses(ir, root, &mut seen, &mut |f, kind| {
            let tex = match kind {
                ExprKind::Sample(s) => s.tex,
                ExprKind::TexSize(t) | ExprKind::TexLoad(t, ..) => *t,
                _ => return,
            };
            let slot_index = match f.exprs[tex as usize].kind {
                ExprKind::Global(g) => global_to_slot[g as usize],
                _ => None,
            };
            let Some(slot_index) = slot_index else {
                if matches!(kind, ExprKind::Sample(_)) {
                    output.push_error("texture sample target must be a texture global".into());
                }
                return;
            };
            let slot = &mut slots[slot_index];
            // Adreno rejects a vertex-stage external sampler beside [int(VIEW_ID)].
            if stage == GLSL_STAGE_VERTEX && slot.ty == TextureType::TextureVideo {
                return;
            }
            let ExprKind::Sample(s) = kind else {
                slot.size_or_load_stages |= stage;
                return;
            };
            slot.fragment_sample |= stage == GLSL_STAGE_FRAGMENT;
            let index = pairs.entry((slot_index, s.sampler)).or_insert_with(|| {
                let index = bindings.len();
                if slot.pair_count == 0 { slot.primary = Some(index); }
                let uniform_name = glsl_texture_uniform_name(slot.name, slot.pair_count);
                slot.pair_count += 1;
                bindings.push(GlslTextureBinding {
                    uniform_name, texture_slot: slot_index as u32, sampler_index: s.sampler, stages: 0,
                });
                index
            });
            bindings[*index].stages |= stage;
        });
    }
    for (i, slot) in slots.iter().enumerate() {
        if slot.size_or_load_stages == 0 {
            continue;
        }
        if let Some(primary) = slot.primary {
            bindings[primary].stages |= slot.size_or_load_stages;
        } else {
            bindings.push(GlslTextureBinding {
                uniform_name: glsl_texture_uniform_name(slot.name, 0),
                texture_slot: i as u32,
                sampler_index: output.get_or_create_sampler(ShaderSampler::default()) as u32,
                stages: slot.size_or_load_stages,
            });
        }
    }
    (bindings, slots)
}

fn visit_texture_uses(
    ir: &IrModule, fi: FuncId, seen: &mut [bool], visit: &mut impl FnMut(&IrFunction, &ExprKind),
) {
    if seen[fi as usize] {
        return;
    }
    seen[fi as usize] = true;
    let f = &ir.functions[fi as usize];
    let mut roots = Vec::new();
    stmt_exprs(&f.body, &mut roots);
    let mut visited = vec![false; f.exprs.len()];
    let mut stack = Vec::new();
    let mut children = Vec::new();
    for root in roots {
        stack.push((root, false));
        while let Some((e, after_children)) = stack.pop() {
            if e == NO_EXPR {
                continue;
            }
            let kind = &f.exprs[e as usize].kind;
            if after_children {
                if let ExprKind::Call(callee, _) = kind {
                    visit_texture_uses(ir, *callee, seen, visit);
                } else {
                    visit(f, kind);
                }
            } else if !visited[e as usize] {
                visited[e as usize] = true;
                stack.push((e, true));
                children.clear();
                expr_children(kind, &mut children);
                stack.extend(children.iter().rev().map(|&child| (child, false)));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shader_ids::ShaderTypeId;
    use crate::shader_schema::ShaderTypeData;
    use crate::shader_types::ShaderTypes;

    struct TextureOnlyTypes;

    impl ShaderTypes for TextureOnlyTypes {
        fn type_data(&self, _: ShaderTypeId) -> &ShaderTypeData {
            unreachable!("texture IO is typed by its IR texture shape")
        }
    }

    fn texture(output: &mut ShaderOutput, name: &str, ty: TextureType) -> (GlobalId, TyId) {
        let name = LiveId::from_str_with_lut(name).unwrap();
        output.io.push(ShaderIo {
            kind: ShaderIoKind::Texture(ty), name, ty: ShaderTypeId::VOID, buffer_index: None, instance_lane: None,
        });
        let texture_ty = output.ir.ty(IrTy::Texture(ty));
        (output.ir.global(IrGlobalKind::Io(IrIo::Texture(ty)), name, texture_ty), texture_ty)
    }

    fn sample_2d(f: &mut IrFunction, tex: ExprId, sampler: u32, uv: ExprId, vec4: TyId) {
        let sample = f.expr(ExprKind::Sample(Box::new(IrSample {
            tex, tex_type: TextureType::Texture2d, sampler, kind: SampleKind::Implicit,
            method: LiveId::from_str("sample"), coord: uv, lod: NO_EXPR, dx: NO_EXPR, dy: NO_EXPR, reference: NO_EXPR,
        })), vec4);
        f.body.push(Stmt::Expr(sample));
    }

    #[test]
    fn multi_sampler_same_texture_emits_distinct_uniforms_and_stage_masks() {
        let mut out = ShaderOutput::default();
        let (tex, d2) = texture(&mut out, "tex", TextureType::Texture2d);
        let (morph, _) = texture(&mut out, "morph_map", TextureType::Texture2d);
        let (volume, d3) = texture(&mut out, "trace_vol", TextureType::Texture3d);
        texture(&mut out, "unused_map", TextureType::Texture2d);
        let repeat = ShaderSampler { address: SamplerAddress::Repeat, ..ShaderSampler::default() };
        let aniso = out.get_or_create_sampler(ShaderSampler { anisotropy: 16, ..repeat }) as u32;
        let linear = out.get_or_create_sampler(repeat) as u32;
        let default = out.get_or_create_sampler(ShaderSampler::default()) as u32;
        let vec2 = out.ir.vec_ty(2, IrScalar::F32);
        let vec4 = out.ir.vec_ty(4, IrScalar::F32);
        let ivec3 = out.ir.vec_ty(3, IrScalar::I32);

        let mut vertex = IrFunction::new("io_vertex".into(), TY_VOID);
        let img = vertex.expr(ExprKind::Global(morph), d2);
        let half = vertex.expr(ExprKind::Lit(IrLit::F32(0.5)), TY_F32);
        let uv = vertex.expr(ExprKind::Construct(vec![half]), vec2);
        sample_2d(&mut vertex, img, default, uv, vec4);
        out.ir.functions.push(vertex);

        let mut fragment = IrFunction::new("io_fragment".into(), TY_VOID);
        let img = fragment.expr(ExprKind::Global(tex), d2);
        let size = fragment.expr(ExprKind::TexSize(img), vec2);
        fragment.body.push(Stmt::Expr(size));
        let half = fragment.expr(ExprKind::Lit(IrLit::F32(0.5)), TY_F32);
        let uv = fragment.expr(ExprKind::Construct(vec![half]), vec2);
        sample_2d(&mut fragment, img, aniso, uv, vec4);
        sample_2d(&mut fragment, img, linear, uv, vec4);
        let vol = fragment.expr(ExprKind::Global(volume), d3);
        let zero = fragment.expr(ExprKind::Lit(IrLit::I32(0)), TY_I32);
        let coord = fragment.expr(ExprKind::Construct(vec![zero]), ivec3);
        let load = fragment.expr(ExprKind::TexLoad(vol, coord, NO_EXPR, zero), vec4);
        fragment.body.push(Stmt::Expr(load));
        out.ir.functions.push(fragment);

        let (wgsl, _) = crate::shader_ir_wgsl::draw_wgsl(&out, &TextureOnlyTypes, false).unwrap();
        assert!(!wgsl.contains("__"), "WGSL contains a reserved identifier");
        for pick in [false, true] {
            let msl = crate::shader_ir_metal::metal_draw_source(&out, &TextureOnlyTypes, pick);
            let (hlsl, _) = crate::shader_ir_hlsl::hlsl_draw(&out, &TextureOnlyTypes, pick);
            assert!(!msl.replace("__attribute__", "").contains("__"), "MSL contains a reserved identifier");
            assert!(!hlsl.contains("__"), "HLSL contains a reserved identifier");
            assert_eq!(msl.contains("mp_pick_tex_tex"), pick);
            assert_eq!(hlsl.contains("mp_pick_tex_tex"), pick);
            let src = gles_draw_sources(&mut out, &TextureOnlyTypes, pick);
            assert!(!src.vertex.contains("__") && !src.fragment.contains("__"), "GLSL reserves double underscores");
            assert!(!out.has_errors(), "{}", out.error_report());
            let actual: Vec<_> = out.glsl_texture_bindings.iter()
                .map(|b| (b.uniform_name.as_str(), b.texture_slot, b.sampler_index, b.stages)).collect();
            assert_eq!(actual, [
                ("tex_morph_map", 1, default, GLSL_STAGE_VERTEX),
                ("tex_tex", 0, aniso, GLSL_STAGE_FRAGMENT),
                ("mp_s1_tex", 0, linear, GLSL_STAGE_FRAGMENT),
                ("tex_trace_vol", 2, default, GLSL_STAGE_FRAGMENT),
            ]);
            let texture_uniforms = |source: &str| source.lines()
                .filter(|line| line.starts_with("uniform sampler")).map(str::to_owned).collect::<Vec<_>>();
            assert_eq!(texture_uniforms(&src.vertex), ["uniform sampler2D tex_morph_map;"]);
            let mut expected = vec!["uniform sampler2D tex_tex;", "uniform sampler2D mp_s1_tex;", "uniform sampler3D tex_trace_vol;"];
            if pick {
                expected.push("uniform sampler2D mp_pick_tex_tex;");
            }
            assert_eq!(texture_uniforms(&src.fragment), expected);
            assert!(src.vertex.contains("sample2d(tex_morph_map,"));
            assert!(src.fragment.contains("sample2d(tex_tex,"));
            assert!(src.fragment.contains("sample2d(mp_s1_tex,"));
            assert!(src.fragment.contains("textureSize(tex_tex, 0)"));
            assert!(src.fragment.contains("texelFetch(tex_trace_vol,"));
            assert_eq!(src.vertex.matches("_mp_pick_sample(").count(), 0);
            assert_eq!(src.fragment.matches("_mp_pick_sample(0u, mp_pick_tex_tex,").count(), if pick { 2 } else { 0 });
        }

        // Reaching the same sampler through the same function in both stages must add both bits.
        let fragment = &mut out.ir.functions[1];
        let call = fragment.expr(ExprKind::Call(0, Vec::new()), TY_VOID);
        fragment.body.push(Stmt::Expr(call));
        let shared = gles_draw_sources(&mut out, &TextureOnlyTypes, true);
        assert_eq!(out.glsl_texture_bindings[0].stages, GLSL_STAGE_VERTEX | GLSL_STAGE_FRAGMENT);
        assert!(shared.fragment.contains("_mp_pick_sample(1u, mp_pick_tex_morph_map,"));
        assert!(!shared.vertex.contains("mp_pick_tex_"));
    }
}
