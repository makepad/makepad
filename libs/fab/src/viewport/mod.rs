//! Lane B owns this directory.
//!
//! `FabViewport` — the realtime 3D viewport. **The renderer is
//! `libs/render`**: its PBR lane, its cascaded shadow maps, its analytic
//! Preetham sky and sun. The CAD modes are overlay passes on top of that lit
//! image, never a second shading pipeline.
//!
//! ## The pass chain (deepest first, which is the order they execute)
//!
//! ```text
//! aux  RGBA32F + D32   our G-buffer: view depth, octahedral view normal,
//!                      signed element id. Also the DEPTH PREPASS the lit
//!                      pass reuses (LessEqual, no writes), which is what
//!                      keeps a 700k-triangle building fragment-cheap. It is
//!                      the authority on visibility, explode and section
//!                      clipping — the lit pass is told the same things
//!                      through the two generic hooks in `libs/render`.
//! ssao RGBA16F ×3      `makepad_render::SsaoPass`, only while the Ambient
//!                      Occlusion overlay is on in Material or Realtime:
//!                      hemisphere taps over the aux view distance, then a
//!                      separable bilateral blur. Material consumes it in
//!                      the composite; Realtime through the renderer's
//!                      ambient-only hook. Off for Rendered and the games.
//! lit  BGRA8 + that D32  `Renderer::draw_scene_full`: textured diffuse for
//!                      CAD modes; PBR materials, sky, sun, CSM shadows and
//!                      available baked lighting for Realtime.
//! comp BGRA8           the composite quad (every shading mode, cavity, SSAO,
//!                      outlines, hover, x-ray, section caps, background),
//!                      then the infinite grid and the contour lines, both
//!                      occluded against `aux` rather than a depth buffer so
//!                      they survive wireframe and ink.
//! window               `draw_bg` blits `comp` into the 2D UI pass.
//! ```
//!
//! ## The two hooks in `libs/render` (§L2.4), both off by default
//!
//! 1. **Per-element lookup** — `elem_map` + `elem_ctl` on the static-mesh
//!    shader. The element index rides the free `ao_uv` vertex lane; one RGBA
//!    texel per element carries `(state, offset.xyz)`. Hide / isolate /
//!    select / explode therefore cost one 706-texel upload per
//!    `SceneState::revision`, and zero geometry re-uploads, ever.
//!    Layout: `viewport::elements`.
//! 2. **Clip planes** — `clip_ctl` + `clip0..clip5`, world-space half-spaces
//!    the pixel stage discards against. Section planes and the section box
//!    both compile down to these.
//!
//! Neither hook changes `MODEL_VERTEX_FLOATS`, adds a vertex format, or
//! touches any existing model's pixels.
//!
//! Input goes out through `api::ViewportInput` to the navigator (lane C) and
//! the tool set (lane E), in that order. Everything drawn *over* the viewport
//! (gizmo, tool overlays, header text) is a separate widget stacked by lane D.

mod bake;
mod dsl;
pub mod elements;
pub mod pack;
mod stage;

use crate::api::*;
use crate::nav::Navigator;
use crate::render::RenderedPreview;
use crate::tools::ToolSet;
use makepad_render::{
    DrawSceneAlpha, DrawSceneCube, DrawSceneShadow, DrawSceneSkinned, DrawSceneSky,
    DrawSceneSkyAnalytic, DrawSceneTerrain, GpuLightmapMode, ModelInstance, Renderer, SceneDraws,
    SsaoParams, SsaoPass, SsaoProjection, DEFAULT_CSM_CONFIG, DEFAULT_SHADOW_BUDGET,
};
use makepad_widgets::*;

use bake::{AoBake, AoBaked};
use elements::ElementLut;

/// Registered before the widget modules that place it (`main.rs` order).
/// `libs/render`'s own shaders have to exist before ours can name them.
pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    makepad_render::script_mod(vm);
    dsl::script_mod(vm)
}

// ===========================================================================
// Vertex formats
// ===========================================================================

/// The aux pass draws the Fab stream verbatim — `fab::model::Vertex`,
/// stride 12, no conversion (`batch.rs`'s promise).
#[repr(C)]
#[derive(Clone, Script, ScriptHook)]
pub struct FabVertex {
    #[live]
    pub pos: Vec3f,
    #[live]
    pub element: f32,
    #[live]
    pub normal: Vec3f,
    #[live]
    pub pad: f32,
    #[live]
    pub uv: Vec2f,
    #[live]
    pub pad2: Vec2f,
}

/// One corner of one contour segment's screen-space quad.
#[repr(C)]
#[derive(Clone, Script, ScriptHook)]
pub struct FabLineVertex {
    #[live]
    pub a: Vec3f,
    /// −1 / +1: which side of the line this corner is pushed to.
    #[live]
    pub side: f32,
    #[live]
    pub b: Vec3f,
    /// 0 = the `a` end, 1 = the `b` end.
    #[live]
    pub end: f32,
    #[live]
    pub element: f32,
    // std140 tail padding: the struct's size must round up to its 16-byte
    // alignment, and a `Vec3f` here would sit at 36 in Rust and 48 in the
    // shader POD.
    #[live]
    pub pad0: f32,
    #[live]
    pub pad1: f32,
    #[live]
    pub pad2: f32,
}

// ---------------------------------------------------------------------------
// std140 vs repr(C): pinned at COMPILE time, because the alternative is a
// panic before the first frame.
//
// `ScriptPod::script_pod` (`platform/script/src/traits.rs:376`) walks both
// layouts side by side and asserts they agree field for field: Rust `repr(C)`
// aligns a `Vec3f` to 4, std140 aligns it to 16, and the struct's std140 size
// rounds up to 16. A `Vec3f` at Rust offset 36 therefore lands at shader
// offset 48 and the app dies at startup with "Rust POD field offset mismatch"
// — which is exactly what happened to `FabLineVertex.pad` once (lane A's
// report §4.7). Nobody should have to launch a GPU binary to find that out.
//
// `pod_layout_end` below is that same walk, in a `const fn`, over the field
// table; `offset_of!` ties the table to the actual struct. Change a field and
// this stops compiling.
// ---------------------------------------------------------------------------

/// `[rust_align, rust_size, std140_align, std140_size]` per field type.
const F_F32: [usize; 4] = [4, 4, 4, 4];
const F_VEC2: [usize; 4] = [4, 8, 8, 8];
const F_VEC3: [usize; 4] = [4, 12, 16, 12];

const fn align_up(v: usize, a: usize) -> usize {
    v.div_ceil(a) * a
}

/// Both layouts, walked together; panics (at compile time, in a `const`
/// context) the moment they disagree. Returns the shared struct size.
const fn pod_layout_end(fields: &[[usize; 4]]) -> usize {
    let mut i = 0;
    let (mut rust_off, mut shader_off, mut shader_align) = (0usize, 0usize, 4usize);
    while i < fields.len() {
        let f = fields[i];
        rust_off = align_up(rust_off, f[0]);
        shader_off = align_up(shader_off, f[2]);
        assert!(
            rust_off == shader_off,
            "fab vertex format: std140 and repr(C) disagree on a field offset — add explicit padding"
        );
        rust_off += f[1];
        shader_off += f[3];
        if f[2] > shader_align {
            shader_align = f[2];
        }
        i += 1;
    }
    assert!(
        align_up(rust_off, 4) == align_up(shader_off, shader_align),
        "fab vertex format: std140 and repr(C) disagree on the struct size — pad the tail"
    );
    align_up(shader_off, shader_align)
}

const _: () = {
    use std::mem::{offset_of, size_of};

    // FabVertex — pos, element, normal, pad, uv, pad2.
    assert!(pod_layout_end(&[F_VEC3, F_F32, F_VEC3, F_F32, F_VEC2, F_VEC2]) == VERTEX_STRIDE * 4);
    assert!(size_of::<FabVertex>() == VERTEX_STRIDE * 4);
    assert!(offset_of!(FabVertex, pos) == 0);
    assert!(offset_of!(FabVertex, element) == 12);
    assert!(offset_of!(FabVertex, normal) == 16);
    assert!(offset_of!(FabVertex, pad) == 28);
    assert!(offset_of!(FabVertex, uv) == 32);
    assert!(offset_of!(FabVertex, pad2) == 40);

    // FabLineVertex — a, side, b, end, element, pad0, pad1, pad2.
    assert!(
        pod_layout_end(&[F_VEC3, F_F32, F_VEC3, F_F32, F_F32, F_F32, F_F32, F_F32])
            == LINE_STRIDE * 4
    );
    assert!(size_of::<FabLineVertex>() == LINE_STRIDE * 4);
    assert!(offset_of!(FabLineVertex, a) == 0);
    assert!(offset_of!(FabLineVertex, side) == 12);
    assert!(offset_of!(FabLineVertex, b) == 16);
    assert!(offset_of!(FabLineVertex, end) == 28);
    assert!(offset_of!(FabLineVertex, element) == 32);
    assert!(offset_of!(FabLineVertex, pad0) == 36);
    assert!(offset_of!(FabLineVertex, pad1) == 40);
    assert!(offset_of!(FabLineVertex, pad2) == 44);
};

pub(crate) fn fab_vertex_pod(vm: &mut ScriptVm) -> ScriptValue {
    let v = FabVertex::script_pod(vm).expect("FabVertex pod");
    vm.bx.heap.pod_type_name_set(v, id_lut!(FabVertex));
    v.into()
}

pub(crate) fn fab_line_vertex_pod(vm: &mut ScriptVm) -> ScriptValue {
    let v = FabLineVertex::script_pod(vm).expect("FabLineVertex pod");
    vm.bx.heap.pod_type_name_set(v, id_lut!(FabLineVertex));
    v.into()
}

/// Only fixes the vertex format for the shader; real geometry is bound per
/// draw call.
pub(crate) fn fab_placeholder_geom(vm: &mut ScriptVm) -> ScriptValue {
    let id = vm.cx_mut().shared_geometry(id!(FabGeom), |cx| {
        let g = Geometry::new(cx);
        let mut v = Vec::new();
        for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            v.extend_from_slice(&[p[0], p[1], p[2], 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        }
        g.update(cx, vec![0, 1, 2], v);
        g
    });
    Geometry::new_borrowed(id).into_script_handle(vm)
}

pub(crate) fn fab_placeholder_line_geom(vm: &mut ScriptVm) -> ScriptValue {
    let id = vm.cx_mut().shared_geometry(id!(FabLineGeom), |cx| {
        let g = Geometry::new(cx);
        let mut v = Vec::new();
        for _ in 0..3 {
            v.extend_from_slice(&[0.0f32; LINE_STRIDE]);
        }
        g.update(cx, vec![0, 1, 2], v);
        g
    });
    Geometry::new_borrowed(id).into_script_handle(vm)
}

const LINE_STRIDE: usize = 12;

/// One occlusion value per merged vertex, shared with the bake worker.
pub type AoValues = std::sync::Arc<Vec<f32>>;
/// The contour-line geometry and how many segments went into it.
type ContourGeometry = (Geometry, usize);

/// Contour segments beyond this are dropped: past it the ink reads as a grey
/// wash anyway and the vertex buffer stops being worth its memory.
const MAX_CONTOUR_SEGMENTS: usize = 400_000;

/// How dark a fully occluded corner goes in the composite. Ambient occlusion
/// is a hint that two surfaces meet, not a second light rig: at 0.45 a corner
/// reads and a room still looks lit. It used to be 0.72, stacked on a 0.55
/// cavity, which is most of why the realtime pane came out nearly black next
/// to the path-traced one.
const AO_STRENGTH: f32 = 0.45;

// ===========================================================================
// Draw structs
// ===========================================================================

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawFabAux {
    #[deref]
    draw_vars: DrawVars,
    #[live]
    transform: Mat4f,
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawFabGrid {
    #[deref]
    draw_vars: DrawVars,
    #[live]
    transform: Mat4f,
    #[live(300.0)]
    fade_far: f32,
    #[live(1.0)]
    opacity: f32,
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawFabLine {
    #[deref]
    draw_vars: DrawVars,
    #[live(vec4(0.1, 0.1, 0.1, 1.0))]
    color: Vec4f,
    #[live(1.4)]
    width: f32,
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawFabComposite {
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawFabSceneTexture {
    #[deref]
    draw_super: DrawQuad,
}

impl DrawFabSceneTexture {
    pub fn set_scene_texture(&mut self, texture: &Texture) {
        self.draw_super.draw_vars.set_texture(0, texture);
    }
}

trait DrawGeometry {
    fn vars(&mut self) -> &mut DrawVars;
    fn draw_geometry(&mut self, cx: &mut CxDraw, geometry_id: GeometryId) {
        let vars = self.vars();
        vars.geometry_id = Some(geometry_id);
        if vars.can_instance() {
            let area = vars.area;
            let new_area = cx.add_instance(vars);
            self.vars().area = cx.update_area_refs(area, new_area);
        }
    }
}

impl DrawGeometry for DrawFabAux {
    fn vars(&mut self) -> &mut DrawVars {
        &mut self.draw_vars
    }
}
impl DrawGeometry for DrawFabGrid {
    fn vars(&mut self) -> &mut DrawVars {
        &mut self.draw_vars
    }
}
impl DrawGeometry for DrawFabLine {
    fn vars(&mut self) -> &mut DrawVars {
        &mut self.draw_vars
    }
}

fn set_pass_camera(cx: &mut Cx, pass: &DrawPass, scene: &SceneState3D) {
    let camera_inv = scene.view.invert();
    let u = &mut cx.passes[pass.draw_pass_id()].pass_uniforms;
    u.camera_projection = scene.projection;
    u.camera_projection_r = scene.projection;
    u.camera_view = scene.view;
    u.camera_view_r = scene.view;
    u.depth_projection = scene.projection;
    u.depth_projection_r = scene.projection;
    u.depth_view = scene.view;
    u.depth_view_r = scene.view;
    u.camera_inv = camera_inv;
    u.camera_inv_r = camera_inv;
}

/// A ground quad big enough to read as infinite; the shader fades it.
fn ground_quad(cx: &mut Cx, half: f32) -> Geometry {
    let g = Geometry::new(cx);
    let mut v = Vec::with_capacity(4 * VERTEX_STRIDE);
    for (x, y) in [(-half, -half), (half, -half), (half, half), (-half, half)] {
        v.extend_from_slice(&[x, y, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    }
    g.update(cx, vec![0, 1, 2, 0, 2, 3], v);
    g
}

/// The scene's authored contour edges as one quad per segment. Built once
/// per `Scene::generation`. Each segment carries its owning element id so
/// the line shader can hide ink with its element, and so Solid mode (which
/// never binds this geometry) cannot accidentally inherit another element's
/// edges.
fn contour_geometry(cx: &mut Cx, scene: &Scene) -> Option<ContourGeometry> {
    let n = (scene.contours.len() / 6).min(MAX_CONTOUR_SEGMENTS);
    if n == 0 {
        return None;
    }
    let mut v: Vec<f32> = Vec::with_capacity(n * 4 * LINE_STRIDE);
    let mut idx: Vec<u32> = Vec::with_capacity(n * 6);
    let mut s = 0usize;
    for e in &scene.elements {
        if s >= n {
            break;
        }
        let segs = scene.element_contours(e.id);
        let element = e.id.0 as f32;
        let count = (segs.len() / 6).min(n - s);
        for i in 0..count {
            let o = i * 6;
            let a = [segs[o], segs[o + 1], segs[o + 2]];
            let b = [segs[o + 3], segs[o + 4], segs[o + 5]];
            for (side, end) in [(-1.0f32, 0.0f32), (1.0, 0.0), (1.0, 1.0), (-1.0, 1.0)] {
                v.extend_from_slice(&[
                    a[0], a[1], a[2], side, b[0], b[1], b[2], end, element, 0.0, 0.0, 0.0,
                ]);
            }
            let base = (s * 4) as u32;
            idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            s += 1;
        }
    }
    if s == 0 {
        return None;
    }
    let g = Geometry::new(cx);
    g.update(cx, idx, v);
    Some((g, s))
}

/// Overlay strengths written into `DrawFabComposite.u_flags`.
///
/// Solid is Fab's clay look: cavity at ~0.25 (ridge/valley) computed at
/// full resolution, SSAO off — the design decision from when an inline ring
/// printed patches on flat walls, kept so Solid stays clean clay. The
/// composite's SSAO flag now means "sample the `SsaoPass` output", so it is
/// on only where that pass runs AND the composite is the consumer: Material.
/// Realtime consumes the same pass through the renderer's ambient hook
/// instead (never here — the lit image must not be double-darkened), and
/// the Rendered pane's path tracer computes true occlusion itself.
fn shading_overlay_flags(vs: &ViewportState) -> [f32; 3] {
    let solid = vs.shading == Shading::Solid;
    let realtime = matches!(vs.shading, Shading::Realtime | Shading::Rendered);
    let outlines = if vs.overlays.outlines { 1.0 } else { 0.0 };
    let cavity = if realtime || !vs.overlays.cavity {
        0.0
    } else if solid {
        0.25
    } else {
        1.0
    };
    let ssao = if vs.overlays.ssao && vs.shading == Shading::Material {
        1.0
    } else {
        0.0
    };
    [outlines, cavity, ssao]
}

/// Whether the screen-space AO pass chain runs at all this frame: the
/// existing "Ambient Occlusion" overlay toggle, in the modes that consume
/// it — Material through the composite, Realtime through the renderer's
/// ambient hook. Off in the Rendered pane (the tracer computes real
/// occlusion) and in Solid/Wireframe/HiddenLine (clay and ink stay clean).
fn wants_ssao_pass(vs: &ViewportState) -> bool {
    vs.overlays.ssao && matches!(vs.shading, Shading::Material | Shading::Realtime)
}

/// Contour-line overlay: file edge lists, drawn only in hidden-line,
/// wireframe, or the explicit wire-on-shaded overlay. Solid keeps them off.
fn wants_contour_lines(vs: &ViewportState) -> bool {
    matches!(vs.shading, Shading::HiddenLine | Shading::Wireframe) || vs.overlays.wire_on_shaded
}

/// Combine the section state into at most six world half-spaces to KEEP,
/// in Fab space. `SectionState` documents the six-plane ceiling.
fn section_planes(section: &SectionState) -> Vec<[f32; 4]> {
    let mut out: Vec<[f32; 4]> = Vec::new();
    if !section.enabled {
        return out;
    }
    for p in &section.planes {
        if !p.enabled || out.len() >= 6 {
            continue;
        }
        out.push([p.plane.a, p.plane.b, p.plane.c, p.plane.d]);
    }
    if let Some(b) = section.boxed {
        // Keep the inside: six inward-facing planes.
        for (n, d) in [
            ([1.0, 0.0, 0.0], -b.min.x),
            ([-1.0, 0.0, 0.0], b.max.x),
            ([0.0, 1.0, 0.0], -b.min.y),
            ([0.0, -1.0, 0.0], b.max.y),
            ([0.0, 0.0, 1.0], -b.min.z),
            ([0.0, 0.0, -1.0], b.max.z),
        ] {
            if out.len() >= 6 {
                break;
            }
            out.push([n[0], n[1], n[2], d]);
        }
    }
    out
}

// ===========================================================================
// The widget
// ===========================================================================

fn render_scene_bounds(bounds: &Aabb) -> Option<(Vec3f, Vec3f)> {
    if aabb_is_empty(bounds) {
        return None;
    }
    // `to_render` negates Fab Y, so transform the opposite endpoints on the
    // renderer Z axis to keep this an ordered AABB.
    Some((
        vec3(bounds.min.x, bounds.min.z, -bounds.max.y),
        vec3(bounds.max.x, bounds.max.z, -bounds.min.y),
    ))
}

fn csm_far_range(bounds: &Aabb, camera: &Camera) -> f32 {
    let Some((min, max)) = render_scene_bounds(bounds) else {
        return DEFAULT_CSM_CONFIG.far_range;
    };
    let eye = pack::to_render(camera.eye);
    let mut far = 0.0f32;
    for bits in 0..8 {
        let corner = vec3(
            if bits & 1 == 0 { min.x } else { max.x },
            if bits & 2 == 0 { min.y } else { max.y },
            if bits & 4 == 0 { min.z } else { max.z },
        );
        far = far.max((corner - eye).length());
    }
    (far * 1.05).max(DEFAULT_CSM_CONFIG.far_range)
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct FabViewport {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// Index into `AppState::views`.
    #[live(0)]
    view: usize,
    #[live]
    draw_bg: DrawFabSceneTexture,
    #[live]
    draw_aux: DrawFabAux,
    #[live]
    draw_comp: DrawFabComposite,
    #[live]
    draw_grid: DrawFabGrid,
    #[live]
    draw_line: DrawFabLine,
    // libs/render's lanes, themed from script like every other draw struct.
    #[live]
    draw_cube: DrawSceneCube,
    #[live]
    draw_alpha: DrawSceneAlpha,
    #[live]
    draw_sky: DrawSceneSky,
    #[live]
    draw_sky_analytic: DrawSceneSkyAnalytic,
    #[live]
    draw_terrain: DrawSceneTerrain,
    #[live]
    draw_shadow: DrawSceneShadow,
    #[live]
    draw_models: DrawSceneSkinned,
    #[live(vec4(0.247, 0.247, 0.247, 1.0))]
    clear_color: Vec4f,
    #[new]
    aux_pass: DrawPass,
    #[new]
    lit_pass: DrawPass,
    #[new]
    comp_pass: DrawPass,
    #[new]
    aux_list: DrawList,
    #[new]
    lit_list: DrawList,
    #[new]
    comp_list: DrawList,
    #[new]
    aux_texture: Texture,
    #[new]
    lit_texture: Texture,
    #[new]
    comp_texture: Texture,
    #[new]
    depth_texture: Texture,
    #[rust]
    area: Area,
    #[rust(false)]
    initialized: bool,
    #[rust]
    renderer: Renderer,
    /// Aux-pass geometry: one per `Scene::batches`, the 48-byte stream as-is.
    #[rust]
    batch_geometries: Vec<Geometry>,
    #[rust]
    uploaded_geometry_generation: u64,
    #[rust]
    uploaded_generation: u64,
    /// `local/fab/cache` key of the model on the GPU.
    #[rust(0u64)]
    model_hash: u64,
    /// Name the merged `StaticModel` is resident under (`load_model_parsed`
    /// early-returns on a repeat id, so a re-pack needs a fresh one).
    #[rust]
    model_id: String,
    #[rust]
    ground_geometry: Option<Geometry>,
    #[rust]
    contour_geometry: Option<ContourGeometry>,
    #[rust]
    elem_lut: ElementLut,
    /// Screen-space AO (libs/render ssao.rs): three passes chained between
    /// `aux` and `lit`, reading the aux target's view distance. Runs only
    /// when `wants_ssao_pass` says so; Material consumes the output in the
    /// composite, Realtime through the renderer's ambient-only hook.
    #[rust]
    ssao: SsaoPass,
    #[rust]
    ao_bake: AoBake,
    #[rust]
    ao: Option<AoValues>,
    #[rust(false)]
    applied_ao: bool,
    #[rust(0u32)]
    model_revision: u32,
    #[rust]
    navigator: Navigator,
    #[rust]
    tools: ToolSet,
    #[rust]
    last_pointer: Option<DVec2>,
    #[rust]
    next_frame: NextFrame,
    /// Lane F's progressive path tracer behind the frozen
    /// `RenderedPreviewApi` seam (api.rs). One per viewport; only used while
    /// `shading == Rendered`. The badge says whether it is still converging.
    #[rust]
    rendered: RenderedPreview,
    /// `ExportPng` target while a capture of the path-traced view is in
    /// flight (Cmd+S on a Rendered pane).
    #[rust]
    pending_export: Option<std::path::PathBuf>,
    #[live]
    draw_badge: DrawText,
    /// The plate under the badge. A caption laid straight on a render is
    /// legible over a shadow and invisible over a sunlit wall; the same
    /// caption on its own dark rounded plate is legible over both.
    #[live]
    draw_badge_bg: DrawQuad,
    #[rust]
    last_frame_time: f64,
    #[rust]
    last_draw_time: f64,
    /// Last accumulation tick (see the 30 Hz redraw law in `handle_event`).
    #[rust]
    last_trace_tick: f64,
    #[rust(false)]
    pointer_locked: bool,
    #[rust(0u32)]
    draw_calls: u32,
    #[rust(0u32)]
    debug_frames: u32,
}

impl FabViewport {
    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.aux_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderRGBAf32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        for t in [&mut self.lit_texture, &mut self.comp_texture] {
            *t = Texture::new_with_format(
                cx,
                TextureFormat::RenderBGRAu8 {
                    size: TextureSize::Auto,
                    initial: true,
                },
            );
        }
        self.depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        for p in [&self.aux_pass, &self.lit_pass, &self.comp_pass] {
            cx.passes[p.draw_pass_id()].keep_camera_matrix = true;
        }
        // Cascaded shadow maps every frame: a BIM model's sun moves whenever
        // the sun-study slider does, and an atlas bake cannot follow that.
        self.renderer
            .set_gpu_lightmap_mode(GpuLightmapMode::Realtime);
        self.renderer.set_shadow_budget(DEFAULT_SHADOW_BUDGET);
        // Fab's materials are a building's: dark stained siding, charcoal
        // roofing, glass. A game's display-space shortcut — sRGB texels times
        // a cosine, written raw — crushes exactly those, so a fully sunlit
        // facade came out a silhouette against the engine's tone-mapped sky.
        // Shade in linear and finish through the same curve the sky and the
        // tracer do; the Realtime and Rendered panes then answer the same
        // question in the same units.
        self.renderer.set_display_transform(true);
        // Night skies deserve the real star map when the cache has one
        // (tools/download_stars.sh); without it the analytic dots stay.
        self.renderer.load_star_map();
    }

    /// Upload once per `Scene::generation` (and once more when the AO bake
    /// lands). Never per frame — see the Metal upload hazard.
    fn ensure_uploaded(&mut self, cx: &mut Cx, state: &AppState) {
        let want_ao = self.ao.is_some();
        if self.uploaded_generation == state.scene.generation && want_ao == self.ao_applied() {
            return;
        }
        // A material-only edit bumps `generation` (repack + re-upload the
        // model: the base colour is baked into the vertex tint) but not
        // `geometry_generation` — the pick-pass geometry, contours, model
        // hash and AO are geometry-derived and stay as they are.
        let fresh_scene = self.uploaded_geometry_generation != state.scene.geometry_generation;
        self.uploaded_generation = state.scene.generation;
        self.uploaded_geometry_generation = state.scene.geometry_generation;

        if fresh_scene {
            self.ao = None;
            self.ao_bake.cancel();
            self.batch_geometries.clear();
            for batch in &state.scene.batches {
                let g = Geometry::new(cx);
                g.update(cx, batch.indices.clone(), batch.vertices.clone());
                self.batch_geometries.push(g);
            }
            self.contour_geometry = contour_geometry(cx, &state.scene);
            self.model_hash = pack::scene_hash(&state.scene);
            // The occlusion bake is OFF by default. On a building it gives
            // coincident faces DIFFERENT values, and a depth fight that was
            // invisible while both faces shared a colour starts flickering
            // between two shades as the camera moves. The sun and its shadow
            // maps carry the look; `FAB_AO=1` brings the bake back.
            let ao_wanted = std::env::var("FAB_AO").map(|v| v != "0").unwrap_or(false);
            if ao_wanted && !state.scene.is_empty() {
                let scene = state.scene.clone();
                let hash = self.model_hash;
                if let Some(ao) = self.ao_bake.start(cx, hash, scene) {
                    self.ao = Some(ao);
                }
            }
        }

        if state.scene.is_empty() {
            self.model_id.clear();
            return;
        }
        let model = pack::pack_scene(&state.scene, self.ao.as_ref().map(|a| a.as_slice()));
        if !self.model_id.is_empty() {
            self.renderer.unload_model(&self.model_id);
        }
        self.model_revision = self.model_revision.wrapping_add(1);
        self.model_id = format!("fab/{:016x}-{}", self.model_hash, self.model_revision);
        match self
            .renderer
            .load_model_parsed(cx, &self.model_id, model, None, None)
        {
            Ok(tris) => {
                // This id is one imported editable scene, not a game level
                // whose enclosed rooms should opt out of casting. Register
                // every resident material layer as static CSM geometry at
                // the upload boundary; no lightmap chart is required.
                self.renderer.set_model_casts_shadow(&self.model_id, true);
                log!(
                    "fab viewport {}: uploaded {} ({} triangles)",
                    self.view,
                    self.model_id,
                    tris
                );
            }
            Err(e) => {
                log!("fab viewport {}: model upload failed: {e}", self.view);
                self.model_id.clear();
            }
        }
        self.applied_ao = self.ao.is_some();
    }

    fn ao_applied(&self) -> bool {
        self.applied_ao
    }

    /// Camera → the two `SceneState3D`s. Both describe the same view of the
    /// same building; they differ only by the fixed Z-up → Y-up turn, so the
    /// composed view-projection over a Fab point is identical in both — the
    /// parity contract with lane F's Rendered viewport.
    fn scene_states(&self, camera: &Camera, rect: Rect, time: f64) -> (SceneState3D, SceneState3D) {
        let aspect = (rect.size.x / rect.size.y.max(1.0)) as f32;
        let projection = camera.projection(aspect);
        let fab = SceneState3D {
            time,
            camera_pos: camera.eye,
            view: camera.view(),
            projection,
            viewport_rect: rect,
        };
        let render = SceneState3D {
            time,
            camera_pos: pack::to_render(camera.eye),
            view: Mat4f::look_at(
                pack::to_render(camera.eye),
                pack::to_render(camera.target),
                pack::to_render(camera.up),
            ),
            projection,
            viewport_rect: rect,
        };
        (fab, render)
    }

    fn draw_aux_pass(&mut self, cx: &mut Cx3d, scene_state: SceneState3D, state: &AppState) {
        cx.begin_scene_3d(scene_state);
        let previous = cx.set_scene_world_transform_3d(Mat4f::identity());
        for (i, _batch) in state.scene.batches.iter().enumerate() {
            let Some(geom) = self.batch_geometries.get(i) else {
                continue;
            };
            let id = geom.geometry_id();
            self.draw_aux.transform = Mat4f::identity();
            self.draw_aux.draw_geometry(cx.cx, id);
            self.draw_calls += 1;
        }
        if let Some(previous) = previous {
            let _ = cx.set_scene_world_transform_3d(previous);
        }
        cx.end_scene_3d();
    }

    fn draw_lit_pass(&mut self, cx: &mut Cx3d, scene_state: SceneState3D, state: &AppState) {
        let realtime = matches!(
            state.view_at(self.view).shading,
            Shading::Realtime | Shading::Rendered
        );
        self.renderer.set_pbr_materials_enabled(realtime);
        let camera = &state.view_at(self.view).camera;
        let world = stage::stage_world(state, camera);
        let csm_bounds = render_scene_bounds(&state.scene.bounds);
        self.renderer.set_csm_scene_bounds(csm_bounds);
        self.renderer.set_csm_config(
            DEFAULT_CSM_CONFIG.tile_resolution,
            csm_far_range(&state.scene.bounds, camera),
        );
        self.renderer.set_csm_focus_distance(Some(camera.distance()));
        self.renderer.set_sky_time(state.sun.time_local);
        let instances = if self.model_id.is_empty() {
            Vec::new()
        } else {
            vec![ModelInstance {
                model: self.model_id.clone(),
                transform: Mat4f::identity(),
                tint: vec4(1.0, 1.0, 1.0, 1.0),
                color_adjust: vec4(0.0, 1.0, 1.0, 0.0),
                // Upload-time registration owns the CSM caster list. Keep
                // architecture static so it never enters the live-mover or
                // analytic-lamp lanes merely to cast a shadow.
                dynamic: false,
                depth_order: 0.0,
                part_poses: Vec::new(),
                custom_material: None,
            }]
        };
        self.renderer.set_models(instances);
        let mut draws = SceneDraws {
            cube: &mut self.draw_cube,
            alpha: &mut self.draw_alpha,
            sky: &mut self.draw_sky,
            sky_analytic: Some(&mut self.draw_sky_analytic),
            terrain: &mut self.draw_terrain,
            shadow: Some(&mut self.draw_shadow),
            shadow_sdf: None,
            firework: None,
            flare: None,
            water: None,
            screen: None,
            screen_instances: &[],
            view_model: None,
        };
        let stats = self.renderer.draw_scene_full(
            cx,
            &mut self.lit_list,
            &mut draws,
            &world,
            scene_state,
            None,
            Some(&mut self.draw_models),
        );
        let _ = stats;
    }

    /// The dark plate behind a viewport badge, sized to the run it will
    /// carry. `DrawText::draw_abs` puts the pen at the BASELINE-ish origin it
    /// was given, so the plate is measured off the run's own ascender and
    /// descender rather than off a guessed line height.
    fn draw_badge_plate(&mut self, cx: &mut Cx2d, pos: DVec2, text: &str) {
        let Some(run) = self.draw_badge.prepare_single_line_run(cx, text) else {
            return;
        };
        let w = run.width_in_lpxs as f64;
        let h = (run.ascender_in_lpxs - run.descender_in_lpxs) as f64;
        let pad = dvec2(6.0, 3.0);
        self.draw_badge_bg.draw_abs(
            cx,
            Rect {
                pos: pos - pad,
                size: dvec2(w, h) + pad * 2.0,
            },
        );
        self.draw_calls += 1;
    }

    fn draw_comp_pass(&mut self, cx: &mut Cx2d, rect: Rect, state: &AppState, ssao_ran: bool) {
        let vs = state.view_at(self.view);
        let dpi = cx.current_dpi_factor() as f32;
        let px = vec2(
            (rect.size.x as f32 * dpi).max(1.0),
            (rect.size.y as f32 * dpi).max(1.0),
        );
        let mode = match vs.shading {
            Shading::Wireframe => 0.0,
            Shading::Solid => 1.0,
            Shading::Material => 2.0,
            Shading::Realtime => 3.0,
            // Until the path tracer produces a frame, Rendered falls back to
            // the same interactive engine view as Realtime.
            Shading::Rendered => 4.0,
            Shading::HiddenLine => 5.0,
        };
        let section_on = !section_planes(&state.scene_state.section).is_empty()
            && state.scene_state.section.caps;
        let hover = vs
            .hover
            .map(|h| h.element.0 as f32 + 1.0)
            .unwrap_or(0.0);
        let radius = if aabb_is_empty(&state.scene.bounds) {
            10.0
        } else {
            aabb_radius(&state.scene.bounds).max(1.0)
        };
        let (lw, lh) = self.elem_lut.size();
        let lut_on = if self.elem_lut.texture().is_some() { 1.0 } else { 0.0 };
        let mut flags = shading_overlay_flags(vs);
        // The composite's AO is a texture sample of the SsaoPass output; if
        // the chain did not record this frame there is nothing to sample.
        if !ssao_ran {
            flags[2] = 0.0;
        }

        let comp = &mut self.draw_comp.draw_super.draw_vars;
        comp.set_texture(0, &self.lit_texture);
        comp.set_texture(1, &self.aux_texture);
        if let Some(t) = self.elem_lut.texture() {
            comp.set_texture(2, t);
        }
        if let Some(t) = self.ssao.output() {
            comp.set_texture(3, t);
        }
        comp.set_uniform(
            cx.cx,
            live_id!(u_mode),
            &[
                mode,
                if vs.xray { 1.0 } else { 0.0 },
                if section_on { 1.0 } else { 0.0 },
                if vs.overlays.wire_on_shaded { 1.0 } else { 0.0 },
            ],
        );
        comp.set_uniform(
            cx.cx,
            live_id!(u_flags),
            &[
                flags[0],
                flags[1],
                flags[2],
                if std::env::var("FAB_SHOW").ok().as_deref() == Some("probe") {
                    1.0
                } else {
                    0.0
                },
            ],
        );
        comp.set_uniform(
            cx.cx,
            live_id!(u_hover),
            &[hover, radius, self.view as f32, 0.0],
        );
        comp.set_uniform(
            cx.cx,
            live_id!(u_texel),
            &[1.0 / px.x, 1.0 / px.y, px.x, px.y],
        );
        // The AO itself — radius, bias, rotation — lives in the SsaoPass
        // (world-space metres, so it never grows when you zoom); the
        // composite only owes the strength it darkens by.
        comp.set_uniform(cx.cx, live_id!(u_ao), &[AO_STRENGTH, 0.0, 0.0, 0.0]);
        comp.set_uniform(cx.cx, live_id!(elem_ctl), &[lw, lh, 0.0, lut_on]);

        let pass_rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: rect.size,
        };
        self.draw_comp.draw_super.draw_abs(cx, pass_rect);
        self.draw_calls += 1;
    }

    fn draw_comp_overlays(&mut self, cx: &mut Cx3d, scene_state: SceneState3D, state: &AppState) {
        let vs = state.view_at(self.view);
        let ink = vs.shading == Shading::HiddenLine;
        let wire = vs.shading == Shading::Wireframe;
        let cad = !matches!(vs.shading, Shading::Realtime | Shading::Rendered);
        let show_grid = cad && vs.overlays.grid;
        // The axis lines are CAD scaffolding too: a green world axis drawn
        // across a sunset is the drawing board showing through the picture.
        let show_axes = cad && vs.overlays.axes;
        let want_grid = show_grid || show_axes;
        let want_lines = wants_contour_lines(vs);
        if !want_grid && !want_lines {
            return;
        }
        let (lw, lh) = self.elem_lut.size();
        let lut_on = if self.elem_lut.texture().is_some() { 1.0 } else { 0.0 };
        let radius = if aabb_is_empty(&state.scene.bounds) {
            50.0
        } else {
            aabb_radius(&state.scene.bounds).max(4.0)
        };
        cx.begin_scene_3d(scene_state);
        let previous = cx.set_scene_world_transform_3d(Mat4f::identity());

        if want_grid {
            let half = (radius * 60.0).clamp(500.0, 20000.0);
            if self.ground_geometry.is_none() {
                self.ground_geometry = Some(ground_quad(cx.cx, half));
            }
            let gid = self.ground_geometry.as_ref().unwrap().geometry_id();
            self.draw_grid.draw_vars.set_texture(0, &self.aux_texture);
            self.draw_grid.draw_vars.set_uniform(
                cx.cx,
                live_id!(u_grid_flags),
                &[
                    if show_axes { 1.0 } else { 0.0 },
                    if show_grid { 1.0 } else { 0.0 },
                    0.0,
                    0.0,
                ],
            );
            self.draw_grid.transform = Mat4f::identity();
            self.draw_grid.fade_far = (radius * 14.0).max(120.0);
            self.draw_grid.opacity = if ink { 0.35 } else { 1.0 };
            self.draw_grid.draw_geometry(cx.cx, gid);
            self.draw_calls += 1;
        }

        if want_lines {
            if let Some((geom, _)) = &self.contour_geometry {
                let gid = geom.geometry_id();
                self.draw_line.draw_vars.set_texture(0, &self.aux_texture);
                if let Some(t) = self.elem_lut.texture() {
                    self.draw_line.draw_vars.set_texture(1, t);
                }
                self.draw_line
                    .draw_vars
                    .set_uniform(cx.cx, live_id!(elem_ctl), &[lw, lh, 0.0, lut_on]);
                let dpi = cx.cx.current_dpi_factor() as f32;
                let size = cx.cx.current_pass_size();
                self.draw_line.draw_vars.set_uniform(
                    cx.cx,
                    live_id!(u_texel),
                    &[
                        1.0 / (size.x as f32 * dpi).max(1.0),
                        1.0 / (size.y as f32 * dpi).max(1.0),
                        (size.x as f32 * dpi).max(1.0),
                        (size.y as f32 * dpi).max(1.0),
                    ],
                );
                // Ink: hidden segments dashed. Wireframe: Fab shows every
                // edge, so hidden lines draw at full strength. Wire-on-shaded:
                // hidden edges are noise, so they are dropped.
                let hidden = if ink {
                    [1.0, 7.0, 0.42, 0.0]
                } else if wire {
                    [1.0, 1.0, 0.55, 0.0]
                } else {
                    [0.0, 7.0, 0.0, 0.0]
                };
                self.draw_line
                    .draw_vars
                    .set_uniform(cx.cx, live_id!(u_hidden), &hidden);
                self.draw_line.color = if ink {
                    vec4(0.102, 0.102, 0.102, 1.0)
                } else if wire {
                    vec4(0.86, 0.86, 0.86, 0.9)
                } else {
                    vec4(0.0, 0.0, 0.0, 0.5)
                };
                // Hidden-line ink is 1 px with a slope-scaled depth bias in
                // the line shader. Wider than that reads as a hatch.
                self.draw_line.width = 1.0;
                self.draw_line.draw_geometry(cx.cx, gid);
                self.draw_calls += 1;
            }
        }

        if let Some(previous) = previous {
            let _ = cx.set_scene_world_transform_3d(previous);
        }
        cx.end_scene_3d();
    }

    /// Both the aux pass and `libs/render`'s static lane learn about hidden
    /// elements, explode offsets and section planes here — the single place
    /// the two can be kept in step.
    fn bind_state_uniforms(&mut self, cx: &mut Cx, state: &AppState) {
        let (lw, lh) = self.elem_lut.size();
        let on = if self.elem_lut.texture().is_some() && lw > 0.0 {
            1.0
        } else {
            0.0
        };
        let planes = section_planes(&state.scene_state.section);
        let count = planes.len() as f32;

        if let Some(t) = self.elem_lut.texture() {
            self.draw_aux.draw_vars.set_texture(0, t);
            self.draw_models.draw_vars.set_texture(6, t);
        }
        self.draw_aux
            .draw_vars
            .set_uniform(cx, live_id!(elem_ctl), &[lw, lh, 0.0, on]);
        self.draw_models
            .draw_vars
            .set_uniform(cx, live_id!(elem_ctl), &[lw, lh, 0.0, on]);

        self.draw_aux
            .draw_vars
            .set_uniform(cx, live_id!(clip_ctl), &[count, 0.0, 0.0, 0.0]);
        self.draw_models
            .draw_vars
            .set_uniform(cx, live_id!(clip_ctl), &[count, 0.0, 0.0, 0.0]);
        const NAMES: [LiveId; 6] = [
            live_id!(clip0),
            live_id!(clip1),
            live_id!(clip2),
            live_id!(clip3),
            live_id!(clip4),
            live_id!(clip5),
        ];
        for i in 0..6 {
            let p = planes.get(i).copied().unwrap_or([0.0, 0.0, 0.0, 0.0]);
            self.draw_aux.draw_vars.set_uniform(cx, NAMES[i], &p);
            // The lit model lives in the Y-up turn of the same world, so the
            // plane takes the same turn.
            let n = pack::to_render(vec3(p[0], p[1], p[2]));
            self.draw_models
                .draw_vars
                .set_uniform(cx, NAMES[i], &[n.x, n.y, n.z, p[3]]);
        }
    }

    fn pick(&self, state: &AppState, rect: Rect, pos: DVec2) -> Option<RayHit> {
        if state.scene.is_empty() {
            return None;
        }
        let proj = ViewProjector::new(state.view_at(self.view).camera, rect);
        let ray = proj.ray(pos);
        state
            .scene
            .bvh
            .raycast(&state.scene.batches, &ray, &|id| state.is_visible(id))
    }

    fn deliver(
        &mut self,
        cx: &mut Cx,
        state: &mut AppState,
        kind: ViewportInputKind,
        pick_at: Option<DVec2>,
    ) {
        let rect = self.area.rect(cx);
        let hit = pick_at.and_then(|p| self.pick(state, rect, p));
        let input = ViewportInput {
            view: self.view,
            rect,
            kind,
            hit,
        };
        let mut resp = self.navigator.handle(cx, &input, state);
        if !resp.consumed {
            let t = self.tools.handle(cx, &input, state);
            resp.consumed = t.consumed;
            resp.redraw |= t.redraw;
            resp.wants_frames |= t.wants_frames;
            if t.cursor.is_some() {
                resp.cursor = t.cursor;
            }
            if t.lock_pointer.is_some() {
                resp.lock_pointer = t.lock_pointer;
            }
        }
        if let Some(cursor) = resp.cursor {
            cx.set_cursor(cursor);
        }
        if let Some(lock) = resp.lock_pointer {
            if lock != self.pointer_locked {
                self.pointer_locked = lock;
                cx.lock_mouse_pointer(lock);
            }
        }
        if resp.wants_frames || self.navigator.is_animating() {
            self.next_frame = cx.new_next_frame();
        }
        if resp.redraw {
            if state.sync_locked_cameras(self.view) {
                cx.redraw_all();
            } else {
                self.area.redraw(cx);
            }
        }
    }

    fn button_of(device: &DigitDevice) -> PointerButton {
        match device.mouse_button() {
            Some(b) if b.is_middle() => PointerButton::Middle,
            Some(b) if b.is_secondary() => PointerButton::Secondary,
            _ => PointerButton::Primary,
        }
    }
}

impl WidgetNode for FabViewport {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for FabViewport {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        match event {
            Event::WindowLostFocus(_) => self.rendered.set_focused(false),
            Event::WindowGotFocus(_) => {
                self.rendered.set_focused(true);
                self.area.redraw(cx);
            }
            _ => {}
        }
        let Some(state) = scope.data.get_mut::<AppState>() else {
            return;
        };

        if let Event::Actions(actions) = event {
            for action in actions {
                if let Some(baked) = action.downcast_ref::<AoBaked>() {
                    if baked.hash == self.model_hash {
                        self.ao = Some(baked.ao.clone());
                        self.area.redraw(cx);
                    }
                }
            }
            for action in shell_actions(actions) {
                match action {
                    ShellAction::SetShading(view, shading) if *view == self.view => {
                        let active = *shading == Shading::Rendered;
                        self.rendered.set_active(active);
                        self.rendered
                            .set_paused(state.view_at(self.view).rendered_paused);
                        if active && !state.view_at(self.view).rendered_paused {
                            self.next_frame = cx.new_next_frame();
                        }
                        self.area.redraw(cx);
                    }
                    ShellAction::SetRenderedPaused(view, paused) if *view == self.view => {
                        self.rendered.set_paused(*paused);
                        if !paused && state.view_at(self.view).shading == Shading::Rendered {
                            self.next_frame = cx.new_next_frame();
                        }
                        self.area.redraw(cx);
                    }
                    ShellAction::SetRenderSettings(_)
                        if state.view_at(self.view).shading == Shading::Rendered =>
                    {
                        if !state.view_at(self.view).rendered_paused {
                            self.next_frame = cx.new_next_frame();
                        }
                        self.area.redraw(cx);
                    }
                    // Cmd+S / "save render": capture the path-traced view of
                    // THIS pane when it is the Rendered one.
                    ShellAction::ExportPng(path)
                        if state.view_at(self.view).shading == Shading::Rendered =>
                    {
                        if let Some(t) = self.rendered.tracer_mut() {
                            t.request_capture(makepad_raytrace::gpu::CaptureKind::View);
                            self.pending_export = Some(path.clone());
                            self.next_frame = cx.new_next_frame();
                            self.area.redraw(cx);
                        } else {
                            cx.action(ShellAction::StatusMessage(
                                "Nothing rendered yet — the path tracer has not started".into(),
                            ));
                        }
                    }
                    ShellAction::FrameAll(v) if *v == self.view => {
                        let b = framing_bounds(&state.scene);
                        self.navigator.frame(cx, state, self.view, b, true);
                        self.area.redraw(cx);
                    }
                    ShellAction::FrameSelected(v) if *v == self.view => {
                        if let Some(b) = state.selection_bounds() {
                            self.navigator.frame(cx, state, self.view, b, true);
                            self.area.redraw(cx);
                        }
                    }
                    ShellAction::FrameSelectedAll => {
                        if let Some(b) = state.selection_bounds() {
                            self.navigator.frame(cx, state, self.view, b, true);
                            self.area.redraw(cx);
                        }
                    }
                    ShellAction::PresetView(v, preset) if *v == self.view => {
                        self.navigator.preset(cx, state, self.view, *preset, true);
                        self.area.redraw(cx);
                    }
                    ShellAction::ToggleOrtho(v) if *v == self.view => {
                        let ortho = !state.view_at(self.view).camera.ortho;
                        self.navigator.set_ortho(cx, state, self.view, ortho);
                        self.area.redraw(cx);
                    }
                    ShellAction::OrbitBy(v, dx, dy) if *v == self.view => {
                        self.navigator.orbit_by(cx, state, self.view, *dx, *dy);
                        self.area.redraw(cx);
                    }
                    ShellAction::Loaded(_) => {
                        self.area.redraw(cx);
                    }
                    // App/UI actions mutate AppState first. Pull that desired
                    // mode into this viewport's Navigator immediately rather
                    // than waiting for a focused pointer/key event.
                    ShellAction::SetNavMode(view, _) if *view == self.view => {
                        self.deliver(
                            cx,
                            state,
                            ViewportInputKind::Frame { dt: 0.0, time: 0.0 },
                            None,
                        );
                    }
                    ShellAction::SetTool(_) if state.active_view == self.view => {
                        self.deliver(
                            cx,
                            state,
                            ViewportInputKind::Frame { dt: 0.0, time: 0.0 },
                            None,
                        );
                    }
                    ShellAction::SetWorkspace(Workspace::Walkthrough) if self.view == 0 => {
                        self.deliver(
                            cx,
                            state,
                            ViewportInputKind::Frame { dt: 0.0, time: 0.0 },
                            None,
                        );
                    }
                    ShellAction::NavKey {
                        view,
                        key,
                        down,
                        mods,
                        repeat,
                    } if *view == self.view => {
                        let kind = if *down {
                            ViewportInputKind::KeyDown {
                                key: *key,
                                mods: *mods,
                                repeat: *repeat,
                            }
                        } else {
                            ViewportInputKind::KeyUp {
                                key: *key,
                                mods: *mods,
                            }
                        };
                        self.deliver(cx, state, kind, None);
                    }
                    ShellAction::NavReleaseCapture => {
                        self.deliver(cx, state, ViewportInputKind::HoverOut, None);
                        // Defensive finalizer: even if navigator state was
                        // reset first, an OS lock can never survive a modal.
                        if self.pointer_locked {
                            self.pointer_locked = false;
                            cx.lock_mouse_pointer(false);
                        }
                        cx.set_cursor(MouseCursor::Default);
                    }
                    _ => {}
                }
                if self.navigator.is_animating() {
                    self.next_frame = cx.new_next_frame();
                }
            }
            if state.sync_locked_cameras(self.view) {
                cx.redraw_all();
            }
        }

        if let Some(ne) = self.next_frame.is_event(event) {
            let dt = if self.last_frame_time > 0.0 {
                (ne.time - self.last_frame_time).clamp(0.0, 0.1) as f32
            } else {
                1.0 / 60.0
            };
            self.last_frame_time = ne.time;
            if state.tour.playing && state.tour.follow_view == self.view {
                if let Some(track) = state.tour.active_track().cloned() {
                    let t = state.tour.time + dt;
                    if t >= track.duration() {
                        state.tour.time = track.duration();
                        state.tour.playing = false;
                    } else {
                        state.tour.time = t;
                    }
                    let tt = state.tour.time;
                    self.navigator.follow_track(cx, state, self.view, &track, tt);
                    state.sync_locked_cameras(self.view);
                    self.next_frame = cx.new_next_frame();
                    cx.redraw_all();
                }
            }
            // A track job pins the camera itself; a navigator Frame tick
            // would mark the view dirty and restart accumulation.
            if !self.rendered.has_track() {
                self.deliver(
                    cx,
                    state,
                    ViewportInputKind::Frame { dt, time: ne.time },
                    None,
                );
            }
            // A requested capture lands a frame or two after the draw that
            // took it: poll here and write the PNG the customer asked for.
            if self.pending_export.is_some() {
                if let Some(t) = self.rendered.tracer_mut() {
                    t.poll_capture(cx);
                    for c in t.take_captures() {
                        if let Some(path) = self.pending_export.take() {
                            let msg = match makepad_raytrace::png::write_bgra8(
                                &path, c.width, c.height, &c.bytes,
                            ) {
                                Ok(()) => format!("Saved {}", path.display()),
                                Err(e) => format!("Save failed: {e}"),
                            };
                            log!("render: {msg}");
                            cx.action(ShellAction::StatusMessage(msg));
                        }
                    }
                }
            }
            // The path tracer accumulates one budgeted slice per DRAW: while
            // it is converging (or a capture is pending), this viewport must
            // keep redrawing, or the badge sits at "0 spp" forever (only
            // camera moves would redraw). A track job drives this
            // unconditionally — `wants_frame` drops between keys and while
            // a capture is in flight, which is the stall after frame 1.
            //
            // THE 30 HZ ACCUMULATION TICK. A redraw repaints the WHOLE
            // window — both viewports' aux/lit/comp raster chains — so a
            // per-frame rearm at 120 Hz spends nearly the whole GPU frame
            // re-rasterizing an unchanged scene and starves the trace it
            // was scheduled for (measured: the trace pass's own command
            // buffers report 5–20 ms wall time for 8 px tiles purely from
            // queue pressure, and the ladder pins at the floor). Ticking
            // the accumulation at 30 Hz leaves the GPU idle between ticks:
            // each tick's trace buffer then gets its full measured budget,
            // and the machine keeps its compositor headroom. Input-driven
            // redraws are untouched — this only paces the self-driven
            // convergence clock.
            let tracking = self.rendered.has_track();
            let converging = state.view_at(self.view).shading == Shading::Rendered
                && (self.pending_export.is_some() || self.rendered.wants_frame());
            if tracking {
                cx.redraw_all();
                self.next_frame = cx.new_next_frame();
            } else if converging {
                const TRACE_TICK_S: f64 = 1.0 / 30.0;
                if ne.time - self.last_trace_tick >= TRACE_TICK_S {
                    self.last_trace_tick = ne.time;
                    self.area.redraw(cx);
                }
                self.next_frame = cx.new_next_frame();
            }
        }
        if let Event::Actions(actions) = event {
            for a in shell_actions(actions) {
                if let ShellAction::TourPlay(true) = a {
                    if state.tour.follow_view == self.view {
                        self.next_frame = cx.new_next_frame();
                    }
                }
            }
        }

        if self.pointer_locked {
            if let Event::MouseMove(me) = event {
                let mods = me.modifiers;
                self.deliver(
                    cx,
                    state,
                    ViewportInputKind::PointerMove {
                        pos: me.abs,
                        delta: dvec2(0.0, 0.0),
                        lock_delta: me.lock_delta,
                        mods,
                        buttons: 0,
                    },
                    None,
                );
                cx.repin_mouse_pointer();
                return;
            }
        }

        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) => {
                cx.set_key_focus(self.area);
                if state.active_view != self.view {
                    state.active_view = self.view;
                }
                self.last_pointer = Some(fe.abs);
                let button = Self::button_of(&fe.device);
                self.deliver(
                    cx,
                    state,
                    ViewportInputKind::PointerDown {
                        button,
                        pos: fe.abs,
                        mods: fe.modifiers,
                        tap_count: fe.tap_count,
                    },
                    Some(fe.abs),
                );
            }
            Hit::FingerMove(fe) => {
                let delta = self.last_pointer.map(|p| fe.abs - p).unwrap_or_default();
                self.last_pointer = Some(fe.abs);
                let buttons = fe
                    .device
                    .mouse_button()
                    .map(|b| {
                        if b.is_middle() {
                            4
                        } else if b.is_secondary() {
                            2
                        } else {
                            1
                        }
                    })
                    .unwrap_or(1);
                self.deliver(
                    cx,
                    state,
                    ViewportInputKind::PointerMove {
                        pos: fe.abs,
                        delta,
                        lock_delta: dvec2(0.0, 0.0),
                        mods: fe.modifiers,
                        buttons,
                    },
                    None,
                );
            }
            Hit::FingerUp(fe) => {
                self.last_pointer = None;
                let button = Self::button_of(&fe.device);
                self.deliver(
                    cx,
                    state,
                    ViewportInputKind::PointerUp {
                        button,
                        pos: fe.abs,
                        mods: fe.modifiers,
                    },
                    Some(fe.abs),
                );
            }
            Hit::FingerHoverIn(fh) | Hit::FingerHoverOver(fh) => {
                let delta = self.last_pointer.map(|p| fh.abs - p).unwrap_or_default();
                self.last_pointer = Some(fh.abs);
                let rect = self.area.rect(cx);
                let hit = self.pick(state, rect, fh.abs);
                let changed =
                    hit.map(|h| h.element) != state.view_at(self.view).hover.map(|h| h.element);
                state.view_at_mut(self.view).hover = hit;
                self.deliver(
                    cx,
                    state,
                    ViewportInputKind::PointerMove {
                        pos: fh.abs,
                        delta,
                        lock_delta: dvec2(0.0, 0.0),
                        mods: fh.modifiers,
                        buttons: 0,
                    },
                    None,
                );
                if changed {
                    // Only the hovered element changing costs a redraw; the
                    // pointer sliding across one wall costs nothing.
                    cx.redraw_all();
                }
            }
            Hit::FingerHoverOut(_) => {
                self.last_pointer = None;
                if state.view_at(self.view).hover.is_some() {
                    state.view_at_mut(self.view).hover = None;
                    cx.redraw_all();
                }
                self.deliver(cx, state, ViewportInputKind::HoverOut, None);
            }
            Hit::FingerScroll(fs) => {
                self.deliver(
                    cx,
                    state,
                    ViewportInputKind::Scroll {
                        delta: fs.scroll,
                        pos: fs.abs,
                        mods: fs.modifiers,
                    },
                    Some(fs.abs),
                );
            }
            Hit::KeyDown(ke) => {
                self.deliver(
                    cx,
                    state,
                    ViewportInputKind::KeyDown {
                        key: ke.key_code,
                        mods: ke.modifiers,
                        repeat: ke.is_repeat,
                    },
                    None,
                );
            }
            Hit::KeyUp(ke) => {
                self.deliver(
                    cx,
                    state,
                    ViewportInputKind::KeyUp {
                        key: ke.key_code,
                        mods: ke.modifiers,
                    },
                    None,
                );
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            if self.rendered.has_track() {
                self.next_frame = cx.new_next_frame();
            }
            return DrawStep::done();
        }
        let Some(state) = scope.data.get_mut::<AppState>() else {
            return DrawStep::done();
        };

        self.ensure_initialized(cx.cx);
        self.ensure_uploaded(cx.cx, state);
        let lut_changed = self.elem_lut.sync(cx.cx, state);
        self.draw_calls = 0;
        self.debug_frames = self.debug_frames.wrapping_add(1);
        if lut_changed || self.debug_frames < 4 {
            // Visibility truth, once per LUT rebuild: what the CPU thinks is
            // on screen. If this says "706 visible" and the pane shows the
            // slab alone, the GPU side of the lookup is the suspect.
            let scene = &state.scene;
            let ss = &state.scene_state;
            let visible = (0..scene.elements.len())
                .filter(|i| ss.is_visible(scene, ElementId::from_index(*i)))
                .count();
            let tris: usize = scene.batches.iter().map(|b| b.indices.len() / 3).sum();
            log!(
                "fab vp{} lut rebuilt: {} elements, {} visible, hidden {}, isolated {:?}, hidden layers {}, hidden stories {}, batches {} ({} tris), lut {:?}",
                self.view,
                scene.elements.len(),
                visible,
                ss.hidden.len(),
                ss.isolated.as_ref().map(|s| s.len()),
                ss.hidden_layers.len(),
                ss.hidden_stories.len(),
                self.batch_geometries.len(),
                tris,
                self.elem_lut.size()
            );
        }
        if self.debug_frames < 4 {
            log!(
                "fab vp{} frame{} rect={:?} dpi={} batches={} model={} lut={:?} passes(aux={:?},lit={:?},comp={:?})",
                self.view, self.debug_frames, rect, cx.current_dpi_factor(),
                self.batch_geometries.len(), self.model_id, self.elem_lut.size(),
                self.aux_pass.draw_pass_id(), self.lit_pass.draw_pass_id(),
                self.comp_pass.draw_pass_id()
            );
            log!(
                "fab vp{} textures aux={:?} lit={:?} comp={:?} depth={:?}",
                self.view,
                self.aux_texture.texture_id(),
                self.lit_texture.texture_id(),
                self.comp_texture.texture_id(),
                self.depth_texture.texture_id()
            );
        }

        let bounds = state.scene.bounds;
        {
            let vs = state.view_at_mut(self.view);
            vs.camera.fit_clip_planes(&bounds);
        }
        let camera = state.view_at(self.view).camera;
        let (fab_state, render_state) = self.scene_states(&camera, rect, cx.time());

        self.bind_state_uniforms(cx.cx, state);

        // The chain is wired parent-first so the pass sort (distance to root,
        // deepest first) runs aux → (ssao chain) → lit → comp → window.
        // `make_child_pass` only knows the pass currently being drawn into,
        // so the inner links are set explicitly, exactly as apps/vj's tween
        // chain does. The SSAO stages parent themselves raw → h → v → lit
        // inside `SsaoPass::run`; the aux pass parents under the chain's
        // deepest stage so its G-buffer exists before the taps read it.
        cx.make_child_pass(&self.comp_pass);
        let comp_id = self.comp_pass.draw_pass_id();
        let lit_id = self.lit_pass.draw_pass_id();
        cx.cx.passes[lit_id].parent = CxDrawPassParent::DrawPass(comp_id);
        let ssao_on = wants_ssao_pass(state.view_at(self.view))
            && !state.scene.is_empty()
            && self.ssao.ensure(cx.cx);
        let aux_parent = if ssao_on {
            self.ssao.first_pass_id().unwrap_or(lit_id)
        } else {
            lit_id
        };
        cx.cx.passes[self.aux_pass.draw_pass_id()].parent = CxDrawPassParent::DrawPass(aux_parent);

        // ---- path tracer (the frozen B↔F seam, api.rs) ---------------------
        // Lane F parents its trace/accumulate/tonemap passes under `comp`,
        // so they render before it and before the window blits the result;
        // F owns the accumulation textures and is the only clearer of
        // `render_dirty`. `None` = no snapshot / not ready: fall back to the
        // realtime composite (never a black pane).
        let is_rendered = state.view_at(self.view).shading == Shading::Rendered;
        self.rendered.set_active(is_rendered);
        self.rendered
            .set_paused(state.view_at(self.view).rendered_paused);
        let rendered_frame = if is_rendered {
            let was_dirty = state.view_at(self.view).render_dirty;
            // Parented under the window pass (the host the tracer is
            // verified in): under `comp` the tonemap never reached its
            // target on Metal — see integration-notes 23:50.
            let frame = self
                .rendered
                .draw_under_current_pass(cx, state, self.view, rect);
            // One line per ~second (and every restart): what the tracer is
            // doing behind the badge. A restart every frame means somebody
            // sets `render_dirty` per frame; 0 spp with frames climbing means
            // the sweep never completes at this size/budget.
            if was_dirty || self.debug_frames % 120 == 0 {
                if let Some(t) = self.rendered.tracer() {
                    let st = &t.stats;
                    log!(
                        "fab vp{} tracer: dirty {} frames {} rung 1/{} spp {:.3} paths {:.0} edge {} tiles/frame {} host {:.1} ms gpu {:.2}/{:.2} ms samples {} done {} size {}x{}",
                        self.view, was_dirty, st.frames, 1u32 << st.rung_shift, st.spp, st.samples_total, st.tile_edge,
                        st.tiles, st.last_frame_ms, st.gpu_time_ms, st.gpu_budget_ms,
                        st.gpu_samples, st.done, st.width, st.height
                    );
                }
            }
            frame
        } else {
            None
        };
        if let Some(frame) = rendered_frame.as_ref() {
            let max_samples = state.render.max_samples;
            let vs = state.view_at_mut(self.view);
            vs.rendered_samples = frame.samples_done;
            vs.rendered_stage = frame.stage_shift;
            // RayTracer marks `done` on the following no-trace present pass;
            // expose completion as soon as this pass reaches the limit so
            // the header is correct on that final scheduled redraw.
            vs.rendered_done = frame.done
                || (max_samples > 0 && frame.samples_done >= max_samples);
        }

        // ---- aux ----------------------------------------------------------
        self.aux_pass.set_size(cx, rect.size);
        self.aux_pass.set_color_texture(
            cx,
            &self.aux_texture,
            // a = 0 is "nothing here"; the composite paints the background.
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
        );
        self.aux_pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.begin_pass(&self.aux_pass, None);
        self.aux_pass.set_size(cx, rect.size);
        set_pass_camera(cx.cx, &self.aux_pass, &fab_state);
        // PASS-LOCAL DRAW LIST + ROOT TURTLE, both load-bearing, in this
        // order (`apps/vj/src/flow_tween.rs`'s stage chain, verbatim):
        // the list because everything drawn before the pass has one lands in
        // the ENCLOSING list — the window's — and gets painted over the UI;
        // the turtle because otherwise every draw inherits the widget's
        // on-screen clip rect, and a viewport that does not start at the
        // window origin has a clip that misses its own offscreen target
        // entirely, so the pass comes out black.
        self.aux_list.begin_always(cx);
        let pass_size = cx.current_pass_size();
        cx.begin_root_turtle(pass_size, Layout::flow_overlay());
        {
            let cx3d = &mut Cx3d::new(cx.cx);
            self.draw_aux_pass(cx3d, fab_state, state);
        }
        cx.end_pass_sized_turtle();
        self.aux_list.end(cx);
        cx.end_pass(&self.aux_pass);

        // ---- ssao ---------------------------------------------------------
        // Hemisphere taps + separable bilateral blur over the aux target's
        // view distance (libs/render ssao.rs — radius and bias are WORLD
        // metres, the rotation a pixel hash, so the shading belongs to the
        // building and never crawls). The FAB-state camera terms below are
        // the ones that produced the aux depth.
        if ssao_on {
            let cam = &state.view_at(self.view).camera;
            let aspect = (rect.size.x / rect.size.y.max(1.0)) as f32;
            let proj = if cam.ortho {
                SsaoProjection {
                    ortho: true,
                    half_x: cam.ortho_height.max(1e-3) * 0.5 * aspect,
                    half_y: cam.ortho_height.max(1e-3) * 0.5,
                }
            } else {
                let ty = (cam.fov_y_deg.to_radians() * 0.5).tan();
                SsaoProjection {
                    ortho: false,
                    half_x: ty * aspect,
                    half_y: ty,
                }
            };
            self.ssao.run(
                cx,
                rect.size,
                &self.aux_texture,
                proj,
                SsaoParams::default(),
                lit_id,
            );
            if self.ssao.gpu_samples > 0 && self.debug_frames % 120 == 0 {
                let [raw, bh, bv] = self.ssao.stage_gpu_ms;
                let [mraw, mbh, mbv] = self.ssao.stage_gpu_min_ms;
                log!(
                    "fab vp{} ssao gpu: {:.3} ms (raw {:.3} + blur {:.3}/{:.3}); floor {:.3} ms ({:.3}+{:.3}+{:.3})",
                    self.view,
                    self.ssao.gpu_ms(),
                    raw,
                    bh,
                    bv,
                    self.ssao.gpu_min_ms(),
                    mraw,
                    mbh,
                    mbv
                );
            }
        }

        // ---- lit ----------------------------------------------------------
        // The depth buffer is INHERITED, not cleared: every fragment the lit
        // pass shades is one the aux pass already proved visible.
        self.lit_pass.set_size(cx, rect.size);
        self.lit_pass.set_color_texture(
            cx,
            &self.lit_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.lit_pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.begin_pass(&self.lit_pass, None);
        self.lit_pass.set_size(cx, rect.size);
        set_pass_camera(cx.cx, &self.lit_pass, &render_state);
        // Realtime consumes the AO through the renderer's ambient-only hook:
        // the factor multiplies the sky fill, never the direct sun or the
        // cascades, so a sunlit facade keeps its brightness while the eaves
        // and reveals darken. Material consumes the same texture in the
        // composite instead; the Rendered pane and every game host stay off.
        let ssao_realtime = ssao_on && state.view_at(self.view).shading == Shading::Realtime;
        self.renderer.set_ssao(match (ssao_realtime, self.ssao.output()) {
            (true, Some(t)) => Some((t.clone(), SsaoParams::default().strength)),
            _ => None,
        });
        let pass_size = cx.current_pass_size();
        cx.begin_root_turtle(pass_size, Layout::flow_overlay());
        {
            let cx3d = &mut Cx3d::new(cx.cx);
            self.draw_lit_pass(cx3d, render_state, state);
        }
        cx.end_pass_sized_turtle();
        cx.end_pass(&self.lit_pass);

        // ---- composite ----------------------------------------------------
        self.comp_pass.set_size(cx, rect.size);
        self.comp_pass.set_color_texture(
            cx,
            &self.comp_texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)),
        );
        cx.begin_pass(&self.comp_pass, None);
        self.comp_pass.set_size(cx, rect.size);
        set_pass_camera(cx.cx, &self.comp_pass, &fab_state);
        // A DRAW LIST, FIRST, ALWAYS. `begin_pass` clears the pass's
        // `main_draw_list_id`; whatever calls `begin_always` first becomes it,
        // and anything drawn before that lands in the *enclosing* list —
        // which belongs to the window pass. That is what painted this
        // viewport's composite quad over the top bar (at pass-local (0,0),
        // i.e. the window origin) and left the real viewport rect showing an
        // empty composite target. Both symptoms, one cause.
        self.comp_list.begin_always(cx);
        let pass_size = cx.current_pass_size();
        cx.begin_root_turtle(pass_size, Layout::flow_overlay());
        self.draw_comp_pass(cx, rect, state, ssao_on);
        {
            let cx3d = &mut Cx3d::new(cx.cx);
            self.draw_comp_overlays(cx3d, fab_state, state);
        }
        cx.end_pass_sized_turtle();
        self.comp_list.end(cx);
        cx.end_pass(&self.comp_pass);

        // ---- back into the 2D pass ----------------------------------------
        let badge_pos = rect.pos + dvec2(12.0, rect.size.y - 26.0);
        match rendered_frame {
            Some(frame) => {
                // The realtime composite is the pane's base layer — same
                // camera, same sun, same tone map — and the traced tiles
                // composite over it as they land: the tracer's tonemap
                // leaves pixels no tile has reached transparent, so a fresh
                // spiral visibly replaces a good raster region by region
                // instead of crawling over a flat placeholder. Once every
                // tile has landed the trace covers the pane completely.
                self.draw_bg.set_scene_texture(&self.comp_texture);
                self.draw_bg.draw_abs(cx, rect);
                if let Some(t) = self.rendered.tracer_mut() {
                    t.draw_view.draw_super.draw_vars.set_texture(0, &frame.texture);
                    t.draw_view.draw_abs(cx, rect);
                }
                let text = state.view_at(self.view).rendered_badge();
                self.draw_badge_plate(cx, badge_pos, &text);
                self.draw_badge.draw_abs(cx, badge_pos, &text);
                if frame.converging || self.rendered.has_track() {
                    self.next_frame = cx.new_next_frame();
                }
            }
            None => {
                match std::env::var("FAB_SHOW").ok().as_deref() {
                    Some("lit") => self.draw_bg.set_scene_texture(&self.lit_texture),
                    Some("aux") => self.draw_bg.set_scene_texture(&self.aux_texture),
                    // The blurred occlusion buffer alone, only where the
                    // chain ran THIS frame — a stale buffer from another
                    // mode would show an old camera and read as a bug.
                    Some("ssao") if ssao_on && self.ssao.output().is_some() => {
                        self.draw_bg.set_scene_texture(self.ssao.output().unwrap())
                    }
                    _ => self.draw_bg.set_scene_texture(&self.comp_texture),
                }
                self.draw_bg.draw_abs(cx, rect);
                if is_rendered {
                    let text = state.view_at(self.view).rendered_badge();
                    self.draw_badge_plate(cx, badge_pos, &text);
                    self.draw_badge.draw_abs(cx, badge_pos, &text);
                }
                if self.rendered.has_track()
                    || (is_rendered && self.rendered.wants_frame())
                {
                    self.next_frame = cx.new_next_frame();
                }
            }
        }
        // The widget's area stays the one `walk_turtle_with_area` gave us.
        // Feeding the composite quad's area back in — and then handing THAT
        // to `set_pass_area` — made the pass rect and the widget rect two
        // different things, and the first viewport's composite ended up
        // painted at the window origin, over the top bar and the toolbar.
        // Nothing here needs a pass area: input is routed by `self.area`.

        // ---- stats --------------------------------------------------------
        let now = cx.time();
        if self.last_draw_time > 0.0 {
            let dt = (now - self.last_draw_time) as f32;
            if dt > 0.0 && dt < 1.0 {
                let fps = 1.0 / dt;
                state.stats.fps = if state.stats.fps > 0.0 {
                    state.stats.fps * 0.9 + fps * 0.1
                } else {
                    fps
                };
                state.stats.frame_ms = dt * 1000.0;
            }
        }
        self.last_draw_time = now;
        state.stats.triangles_drawn = state.scene.stats.triangles as u64;
        state.stats.draw_calls = self.draw_calls;
        state.stats.visible_elements = state.scene.stats.elements_with_geometry;
        state.stats.gpu_bytes = state.scene.stats.geometry_bytes;

        if self.navigator.is_animating() || self.rendered.has_track() {
            self.next_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }
}

/// Lane B's action hook, called from `App::dispatch`.
pub fn apply(_cx: &mut Cx, _state: &mut AppState, _action: &ShellAction) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_section_box_becomes_six_inward_planes() {
        let s = SectionState {
            enabled: true,
            planes: Vec::new(),
            boxed: Some(Aabb {
                min: vec3(-1.0, -2.0, 0.0),
                max: vec3(3.0, 4.0, 5.0),
            }),
            caps: true,
            cap_color: [0.5, 0.5, 0.5, 1.0],
        };
        let p = section_planes(&s);
        assert_eq!(p.len(), 6);
        // The box centre is kept by every plane; a point outside is not.
        let inside = vec3(1.0, 1.0, 2.5);
        for pl in &p {
            let d = pl[0] * inside.x + pl[1] * inside.y + pl[2] * inside.z + pl[3];
            assert!(d >= 0.0, "centre rejected by {pl:?} ({d})");
        }
        let outside = vec3(9.0, 1.0, 2.5);
        let kept = p
            .iter()
            .all(|pl| pl[0] * outside.x + pl[1] * outside.y + pl[2] * outside.z + pl[3] >= 0.0);
        assert!(!kept, "a point past max.x must be cut away");
    }

    #[test]
    fn a_disabled_section_clips_nothing() {
        let s = SectionState {
            enabled: false,
            planes: vec![SectionPlane {
                plane: Plane::from_point_normal(vec3(0.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0)),
                enabled: true,
                source: None,
            }],
            boxed: None,
            caps: true,
            cap_color: [0.5, 0.5, 0.5, 1.0],
        };
        assert!(section_planes(&s).is_empty());
    }

    #[test]
    fn both_cameras_frame_the_same_points() {
        // The parity contract with lane F: the composed view-projection over
        // a Fab point must be identical whichever space it goes through.
        let cam = Camera::default();
        let aspect = 1.6f32;
        let view_fab = cam.view();
        let view_render = Mat4f::look_at(
            pack::to_render(cam.eye),
            pack::to_render(cam.target),
            pack::to_render(cam.up),
        );
        for p in [
            vec3(0.0, 0.0, 0.0),
            vec3(5.0, 3.5, 2.5),
            vec3(-12.0, 7.0, 9.0),
        ] {
            let a = Mat4f::mul(&cam.projection(aspect), &view_fab)
                .transform_vec4(vec4(p.x, p.y, p.z, 1.0));
            let r = pack::to_render(p);
            let b = Mat4f::mul(&cam.projection(aspect), &view_render)
                .transform_vec4(vec4(r.x, r.y, r.z, 1.0));
            for (x, y) in [(a.x, b.x), (a.y, b.y), (a.z, b.z), (a.w, b.w)] {
                assert!((x - y).abs() < 1e-3, "{p:?}: {x} vs {y}");
            }
        }
    }

    #[test]
    fn solid_mode_uses_subtle_cavity_and_no_ssao() {
        let vs = ViewportState::default();
        assert_eq!(vs.shading, Shading::Solid);
        assert!(vs.overlays.cavity && vs.overlays.ssao);
        let f = shading_overlay_flags(&vs);
        assert!((f[1] - 0.25).abs() < 1e-6, "cavity strength {f:?}");
        assert_eq!(f[2], 0.0, "ssao must stay off in solid: {f:?}");
    }

    #[test]
    fn the_ssao_pass_serves_material_and_realtime_and_never_the_traced_pane() {
        let mut vs = ViewportState::default();
        assert!(vs.overlays.ssao, "the overlay defaults on");
        assert!(!wants_ssao_pass(&vs), "solid keeps clean clay — no pass");
        vs.shading = Shading::Material;
        assert!(wants_ssao_pass(&vs));
        let f = shading_overlay_flags(&vs);
        assert_eq!(f[2], 1.0, "material consumes it in the composite: {f:?}");
        vs.shading = Shading::Realtime;
        assert!(wants_ssao_pass(&vs), "realtime consumes it in the renderer");
        assert_eq!(
            shading_overlay_flags(&vs)[2],
            0.0,
            "never ALSO in the composite"
        );
        vs.shading = Shading::Rendered;
        assert!(!wants_ssao_pass(&vs), "the traced pane owns its occlusion");
        vs.shading = Shading::Material;
        vs.overlays.ssao = false;
        assert!(!wants_ssao_pass(&vs), "the menu toggle is the switch");
        assert_eq!(shading_overlay_flags(&vs)[2], 0.0);
    }

    #[test]
    fn realtime_uses_renderer_occlusion_without_cad_double_darkening() {
        let mut vs = ViewportState::default();
        vs.shading = Shading::Realtime;
        let f = shading_overlay_flags(&vs);
        assert_eq!(f[1], 0.0, "CAD cavity must be off in Realtime: {f:?}");
        assert_eq!(f[2], 0.0, "CAD SSAO must be off in Realtime: {f:?}");
    }

    #[test]
    fn csm_bounds_cover_the_rotated_model_and_camera() {
        let bounds = Aabb {
            min: vec3(-10.0, -20.0, -2.0),
            max: vec3(30.0, 40.0, 18.0),
        };
        let (min, max) = render_scene_bounds(&bounds).unwrap();
        assert_eq!(min, vec3(-10.0, -2.0, -40.0));
        assert_eq!(max, vec3(30.0, 18.0, 20.0));
        let range = csm_far_range(&bounds, &Camera::default());
        assert!(range >= DEFAULT_CSM_CONFIG.far_range);
    }

    #[test]
    fn solid_mode_does_not_draw_contour_overlays() {
        let vs = ViewportState::default();
        assert!(!wants_contour_lines(&vs));
        let mut hidden = vs.clone();
        hidden.shading = Shading::HiddenLine;
        assert!(wants_contour_lines(&hidden));
        let mut wire = vs.clone();
        wire.shading = Shading::Wireframe;
        assert!(wants_contour_lines(&wire));
        let mut overlay = vs.clone();
        overlay.overlays.wire_on_shaded = true;
        assert!(wants_contour_lines(&overlay));
    }
}
